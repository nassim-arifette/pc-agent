//! Evaluation entry point. No execution broker and no provider request without explicit opt-in.
#![forbid(unsafe_code)]
mod calibration;
mod capture;
mod files;
mod mcp;
mod mcp_trace;
#[cfg(test)]
mod tests;

use clap::{Parser, Subcommand};
use files::OutputFile;
use pcagent_agent::{
    capabilities::Probes,
    credentials::Credentials,
    replay::{ModelView, RecordedRun, Recorder, ReplayTools, Scenario, SCENARIO_VERSION},
    routing::{CalibrationRuntime, REPLAY_HARNESS_VERSION},
    Config, Model, Nebius, Sharing, Toolbox, Workstation, MAX_QUESTION_BYTES,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{unix_ms, Snapshot};
use pcagent_storage::StorageService;
use pcagent_telemetry::Collector;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
    sync::Arc,
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Parser)]
#[command(
    version,
    about = "Capture, replay and compare PC Agent investigations. Offline by default."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate human-reviewed, paired real Nemotron replay results locally. Makes no provider request.
    CalibrateNemotron {
        /// Closed annotation manifest binding the exact captures and trace hashes reviewed by a human.
        #[arg(long)]
        annotations: PathBuf,
        /// New routing Calibration JSON outside Git. Automatic routing is not enabled by this command.
        #[arg(long)]
        out: PathBuf,
        /// New local proof of checked identities, usage and hashes, without raw questions or answers.
        #[arg(long)]
        provenance_out: PathBuf,
    },
    /// Measure full and compact tool-context bytes offline; this does not measure model quality.
    ContextBenchmark {
        scenario: PathBuf,
        /// Keep executable names opaque (recommended for export).
        #[arg(long)]
        anonymize: bool,
        /// New local result file outside Git; otherwise emit only byte measurements to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Record local tool outputs; with --allow-cloud, record a real Nemotron investigation.
    Capture {
        /// New JSON file outside any Git repository. Parent directory must already exist.
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        question: Option<String>,
        /// Ground truth for graders. Kept locally; never included in model messages.
        #[arg(long = "note")]
        notes: Vec<String>,
        /// Authorize a folder for metadata scanning (repeatable).
        #[arg(long = "scope")]
        scopes: Vec<PathBuf>,
        /// Consent to recording raw local measurements, process names and any answer.
        #[arg(long, required = true)]
        allow_recording: bool,
        /// Send the question and minimized measurements to Nebius; may incur cost.
        #[arg(long, requires = "question", conflicts_with = "calls")]
        allow_cloud: bool,
        #[arg(long, requires = "allow_cloud")]
        model: Option<String>,
        /// Share opaque process references instead of executable basenames.
        #[arg(long)]
        anonymize: bool,
        /// JSON array of {tool, arguments}; otherwise sweep the implemented tools.
        #[arg(long)]
        calls: Option<PathBuf>,
        /// Additional exact variants after the offline sweep; use a new capture for every campaign.
        #[arg(long, conflicts_with_all = ["calls", "allow_cloud"])]
        extra_calls: Option<PathBuf>,
        /// Local performance history to read (default: PC Agent's pcagent.sqlite beside its
        /// settings). The sampler keeps recording into it while the capture runs.
        #[arg(long)]
        history: Option<PathBuf>,
    },
    /// Read a scenario without collectors. With --allow-cloud, run Nemotron against it.
    Replay {
        scenario: PathBuf,
        #[arg(long)]
        question: Option<String>,
        /// Call a chosen sequence; without this or --allow-cloud, replay the recorded sequence.
        #[arg(long, conflicts_with_all = ["allow_cloud", "preview_cloud"])]
        calls: Option<PathBuf>,
        #[arg(long, conflicts_with = "preview_cloud")]
        allow_cloud: bool,
        #[arg(long, requires = "allow_cloud")]
        model: Option<String>,
        #[arg(long)]
        anonymize: bool,
        /// Show the initial message and all recorded model-facing outputs. Sends nothing.
        #[arg(long)]
        preview_cloud: bool,
        /// Write a new local result file outside Git; otherwise emit JSON to stdout.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// MCP over newline-delimited JSON-RPC on stdio. No network listener or action execution.
    Mcp {
        #[arg(long, required_unless_present = "live", conflicts_with = "live")]
        scenario: Option<PathBuf>,
        /// Use the real Windows toolbox instead of a recorded scenario.
        #[arg(long)]
        live: bool,
        /// Consent to exposing minimized results to the MCP client and its model.
        #[arg(long, required = true)]
        allow_cloud: bool,
        #[arg(long)]
        anonymize: bool,
        #[arg(long)]
        question: Option<String>,
        #[arg(long = "scope", requires = "live")]
        scopes: Vec<PathBuf>,
        /// Record the replay MCP tool session locally; no final client answer or bill is inferred.
        #[arg(long, requires = "allow_recording", conflicts_with = "live")]
        out: Option<PathBuf>,
        /// Consent to retaining raw local tool outputs in --out, outside any Git repository.
        #[arg(long, requires = "out")]
        allow_recording: bool,
    },
}

fn question(value: Option<String>, fallback: Option<&str>) -> Result<String> {
    let text = value
        .or_else(|| fallback.map(str::to_owned))
        .unwrap_or_else(|| "Inspect this recorded PC situation.".into());
    if text.trim().is_empty() || text.len() > MAX_QUESTION_BYTES {
        return Err("Question must contain 1 to 4096 bytes".into());
    }
    Ok(text)
}

fn model(id: Option<String>) -> Result<(Nebius, String)> {
    pcagent_inspect::elevation::ensure_unelevated_host()?;
    let credentials = Credentials::load();
    let config = Config::from_lookup(|key| {
        if key == "NEBIUS_MODEL_ID" {
            id.clone().or_else(|| credentials.get(key))
        } else {
            credentials.get(key)
        }
    })?;
    let id = id
        .or_else(|| credentials.get("NEBIUS_MODEL_ID"))
        .expect("validated model ID");
    Ok((Nebius::new(config)?, id))
}

fn investigate(
    model: &mut impl Model,
    id: String,
    question: &str,
    initial: &Snapshot,
    tools: &mut impl Toolbox,
) -> (RecordedRun, bool) {
    let runtime = CalibrationRuntime::current();
    let started_at_unix_ms = unix_ms();
    let started = Instant::now();
    let report = pcagent_agent::investigate(model, question, initial, tools, |event| {
        eprintln!(
            "{}: {:?} ({} ms)",
            event.tool, event.status, event.duration_ms
        );
    });
    let success = report.error.is_none()
        && report
            .answer
            .as_ref()
            .is_some_and(|a| a.unverified.is_empty());
    (
        RecordedRun {
            model: id,
            harness_version: REPLAY_HARNESS_VERSION.into(),
            runtime: Some(runtime),
            started_at_unix_ms,
            duration_ms: started.elapsed().as_millis() as u64,
            report: serde_json::to_value(report).expect("serializable report"),
        },
        success,
    )
}

/// The persisted history the history tools read: the path given, or PC Agent's own database.
fn attach_history(workstation: &mut Workstation<'_>, path: Option<PathBuf>) -> Result<()> {
    let path = path
        .or_else(|| {
            pcagent_agent::credentials::file_path()
                .and_then(|p| p.parent().map(|dir| dir.join("pcagent.sqlite")))
        })
        .ok_or("The local history location is unknown; use --history.")?;
    let sampler = pcagent_store::sampler::Sampler::start(&path)?;
    workstation.set_history(sampler, Arc::new(std::sync::atomic::AtomicBool::new(false)));
    Ok(())
}

fn storage(scopes: &[PathBuf]) -> Result<Arc<StorageService>> {
    let storage = Arc::new(StorageService::new());
    for scope in scopes {
        storage.authorize(scope)?;
    }
    Ok(storage)
}

fn load_scenario(path: &std::path::Path) -> Result<Scenario> {
    let scenario: Scenario = files::read_json(path)?;
    scenario.validate()?;
    Ok(scenario)
}

fn emit(value: &Value, output: Option<OutputFile>) -> Result<()> {
    if let Some(output) = output {
        output.save(value)
    } else {
        let mut stdout = io::stdout().lock();
        serde_json::to_writer_pretty(&mut stdout, value)?;
        writeln!(stdout)?;
        Ok(())
    }
}

fn fingerprint(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}

fn replay_identity(scenario: &Scenario, tools: &impl Toolbox) -> Result<Value> {
    Ok(json!({
        "scenario_sha256": fingerprint(scenario)?,
        "tool_registry_sha256": fingerprint(&tools.definitions())?,
        "scenario_origin": scenario.initial.origin,
        "captured_at_unix_ms": scenario.captured_at_unix_ms,
        "harness_version": REPLAY_HARNESS_VERSION,
        "runtime": CalibrationRuntime::current(),
    }))
}

fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Command::CalibrateNemotron {
            annotations,
            out,
            provenance_out,
        } => {
            let (calibration, provenance) = calibration::compile(&annotations)?;
            let output = OutputFile::reserve(&out)?;
            let proof_output = OutputFile::reserve(&provenance_out)?;
            proof_output.save(&provenance)?;
            output.save(&calibration)?;
            eprintln!("Saved validated local Nemotron calibration and provenance. No provider request was made; routing settings are unchanged.");
            Ok(0)
        }
        Command::ContextBenchmark {
            scenario: path,
            anonymize,
            out,
        } => {
            let scenario = load_scenario(&path)?;
            let sharing = if anonymize {
                Sharing::Anonymized
            } else {
                scenario.sharing
            };
            let replay = ReplayTools::new(&scenario, sharing)?;
            let mut result = replay_identity(&scenario, &replay)?;
            result["measurement_kind"] = json!("offline_context_bytes");
            result["metrics"] =
                serde_json::to_value(pcagent_agent::replay::measure_context(&scenario, sharing)?)?;
            result["limitations"] = json!(["Byte counts exclude system messages and tool definitions. They do not measure tokens, model answers, quality or billed costs."]);
            let output = out.as_deref().map(OutputFile::reserve).transpose()?;
            emit(&result, output)?;
            Ok(0)
        }
        Command::Capture {
            out,
            name,
            question: asked,
            notes,
            scopes,
            allow_recording: _,
            allow_cloud,
            model: id,
            anonymize,
            calls,
            extra_calls,
            history,
        } => {
            if name.trim().is_empty() || name.len() > 256 {
                return Err("Name must contain 1 to 256 bytes".into());
            }
            let asked = asked.map(|q| question(Some(q), None)).transpose()?;
            let calls: Option<Vec<capture::Call>> =
                calls.as_deref().map(files::read_json).transpose()?;
            if let Some(calls) = &calls {
                capture::validate_calls(calls, &Workstation::tool_specs())?;
            }
            let extra_calls: Option<Vec<capture::Call>> =
                extra_calls.as_deref().map(files::read_json).transpose()?;
            if let Some(calls) = &extra_calls {
                capture::validate_calls(calls, &Workstation::tool_specs())?;
            }
            if !cfg!(windows) {
                return Err(
                    "Live capture requires Windows; use replay for portable evaluation".into(),
                );
            }
            let output = OutputFile::reserve(&out)?;
            let mut model = if allow_cloud { Some(model(id)?) } else { None };
            let storage = storage(&scopes)?;
            let (mut collector, policy, prefs) =
                (Collector::new(), Policy::from_env(), Preferences::default());
            let initial = collector.snapshot(true, 10);
            let mut scenario = Scenario::new(name, asked.clone(), initial);
            scenario.notes = notes;
            scenario.sharing = if anonymize {
                Sharing::Anonymized
            } else {
                Sharing::Complete
            };
            let mut workstation = Workstation::new(&mut collector, storage, &policy, &prefs);
            workstation.set_sharing(scenario.sharing);
            attach_history(&mut workstation, history)?;
            if model.is_some() {
                // The model is offered what works on this PC, as in the CLI; the scenario keeps
                // that list for its replays. Sweeps and call lists still reach every tool.
                workstation.offer_available(Probes::without_helper(&scenario.initial));
                scenario.tool_names = workstation
                    .specs()
                    .iter()
                    .map(|spec| spec.name.into())
                    .collect();
            }
            let mut recorder = Recorder::new(workstation);
            let mut success = true;
            if let Some((model, id)) = &mut model {
                let (run, ok) = investigate(
                    model,
                    id.clone(),
                    asked.as_deref().expect("required question"),
                    &scenario.initial,
                    &mut recorder,
                );
                scenario.run = Some(run);
                success = ok;
            } else if let Some(calls) = calls {
                for call in calls {
                    capture::run(&mut recorder, &call.tool, call.arguments)?;
                }
            } else {
                capture::sweep(&mut recorder, &scenario.initial)?;
                recorder = capture::finish_scans(recorder)?;
                for call in extra_calls.into_iter().flatten() {
                    capture::run(&mut recorder, &call.tool, call.arguments)?;
                }
            }
            let (_, calls) = recorder.into_parts();
            scenario.calls = calls;
            scenario.validate()?;
            output.save(&scenario)?;
            eprintln!(
                "Saved local scenario: {} calls. No file was uploaded.",
                scenario.calls.len()
            );
            Ok(if success { 0 } else { 2 })
        }
        Command::Replay {
            scenario: path,
            question: asked,
            calls,
            allow_cloud,
            model: id,
            anonymize,
            preview_cloud,
            out,
        } => {
            let scenario = load_scenario(&path)?;
            let asked = question(asked, scenario.question.as_deref())?;
            let sharing = if anonymize {
                Sharing::Anonymized
            } else {
                scenario.sharing
            };
            let mut replay = ReplayTools::new(&scenario, sharing)?;
            let calls: Vec<capture::Call> = if let Some(path) = calls {
                let calls: Vec<capture::Call> = files::read_json(&path)?;
                capture::validate_calls(&calls, replay.specs())?;
                calls
            } else {
                scenario
                    .calls
                    .iter()
                    .map(|c| capture::Call {
                        tool: c.tool.clone(),
                        arguments: c.arguments.clone(),
                    })
                    .collect()
            };
            if !calls.is_empty() {
                capture::validate_calls(&calls, replay.specs())?;
            }
            let output = out.as_deref().map(OutputFile::reserve).transpose()?;
            let started = Instant::now();
            let mut success = true;
            let mut result = replay_identity(&scenario, &replay)?;
            result["schema_version"] = json!(SCENARIO_VERSION);
            result["scenario"] = json!(scenario.name);
            result["question"] = json!(asked);
            result["sharing"] = json!(sharing);
            result["mode"] = json!(if preview_cloud {
                "preview"
            } else if allow_cloud {
                "nebius_replay"
            } else {
                "offline_replay"
            });
            if allow_cloud {
                let (mut model, id) = model(id)?;
                let mut recorder = Recorder::new(replay);
                let (run, ok) =
                    investigate(&mut model, id, &asked, &scenario.initial, &mut recorder);
                result["run"] = serde_json::to_value(run)?;
                let (tools, actual_calls) = recorder.into_parts();
                replay = tools;
                result["calls"] = serde_json::to_value(actual_calls)?;
                success = ok;
            } else {
                let mut view = ModelView::new(&asked, &scenario.initial, sharing);
                result["initial_message"] = view.initial().clone();
                result["tools"] = replay.definitions();
                let mut outputs = Vec::new();
                for call in calls {
                    let spec = *replay
                        .specs()
                        .iter()
                        .find(|s| s.name == call.tool)
                        .expect("validated call");
                    let args = spec.validate(&call.arguments.to_string())?;
                    let out = replay.run(&spec, &args);
                    outputs.push(json!({"tool": call.tool, "arguments": call.arguments, "output": view.output(&spec, out)}));
                }
                result["calls"] = json!(outputs);
            }
            result["replay_misses"] = serde_json::to_value(&replay.misses)?;
            result["served_captures"] = json!(replay.served_count());
            result["reused_calls"] = json!(replay.reused_calls);
            result["unused_captures"] = json!(scenario.calls.len() - replay.served_count());
            result["duration_ms"] = json!(started.elapsed().as_millis() as u64);
            result["replay_complete"] = json!(replay.misses.is_empty());
            let code = if success && replay.misses.is_empty() {
                0
            } else {
                2
            };
            emit(&result, output)?;
            Ok(code)
        }
        Command::Mcp {
            scenario,
            live: _,
            allow_cloud: _,
            anonymize,
            question: asked,
            scopes,
            out,
            allow_recording: _,
        } => {
            if let Some(path) = scenario {
                let scenario = load_scenario(&path)?;
                let asked = question(asked, scenario.question.as_deref())?;
                let sharing = if anonymize {
                    Sharing::Anonymized
                } else {
                    scenario.sharing
                };
                let mut replay = ReplayTools::new(&scenario, sharing)?;
                let output = out.as_deref().map(OutputFile::reserve).transpose()?;
                let Some(output) = output else {
                    mcp::serve(
                        &mut replay,
                        &scenario.initial,
                        &asked,
                        io::stdin().lock(),
                        io::stdout().lock(),
                    )?;
                    if !replay.misses.is_empty() {
                        eprintln!("Replay had {} unrecorded calls.", replay.misses.len());
                    }
                    return Ok(0);
                };
                let mut trace = replay_identity(&scenario, &replay)?;
                trace["schema_version"] = json!(1);
                trace["mode"] = json!("mcp_replay_trace");
                trace["question"] = json!(asked);
                trace["sharing"] = json!(sharing);
                trace["started_at_unix_ms"] = json!(unix_ms());
                let started = Instant::now();
                let mut tools = mcp_trace::TraceTools::new(replay);
                let served = mcp::serve(
                    &mut tools,
                    &scenario.initial,
                    &asked,
                    io::stdin().lock(),
                    io::stdout().lock(),
                );
                let (replay, calls, recording_complete) = tools.into_parts();
                trace["calls"] = serde_json::to_value(calls)?;
                trace["duration_ms"] = json!(started.elapsed().as_millis() as u64);
                trace["replay_misses"] = serde_json::to_value(&replay.misses)?;
                trace["served_captures"] = json!(replay.served_count());
                trace["unused_captures"] = json!(scenario.calls.len() - replay.served_count());
                trace["reused_calls"] = json!(replay.reused_calls);
                trace["replay_complete"] = json!(replay.misses.is_empty());
                trace["transport_complete"] = json!(served.is_ok());
                trace["recording_complete"] = json!(recording_complete);
                // The tool server cannot observe the client's final answer or provider usage.
                trace["answer"] = Value::Null;
                trace["usage"] = Value::Null;
                trace["cost_usd"] = Value::Null;
                output.save(&trace)?;
                served?;
                if !replay.misses.is_empty() {
                    eprintln!("Replay had {} unrecorded calls.", replay.misses.len());
                }
            } else {
                if !cfg!(windows) {
                    return Err("Live MCP requires Windows; use --scenario elsewhere".into());
                }
                let asked = question(asked, Some("Inspect this PC."))?;
                let storage = storage(&scopes)?;
                let (mut collector, policy, prefs) =
                    (Collector::new(), Policy::from_env(), Preferences::default());
                let initial = collector.snapshot(true, 10);
                let mut tools = Workstation::new(&mut collector, storage, &policy, &prefs);
                tools.set_sharing(if anonymize {
                    Sharing::Anonymized
                } else {
                    Sharing::Complete
                });
                attach_history(&mut tools, None)?;
                // The client is offered what works on this PC, like PC Agent's own model.
                tools.offer_available(Probes::without_helper(&initial));
                mcp::serve(
                    &mut tools,
                    &initial,
                    &asked,
                    io::stdin().lock(),
                    io::stdout().lock(),
                )?;
            }
            Ok(0)
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("pcagent-eval: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod runtime_identity_tests {
    use super::*;
    use pcagent_agent::{AgentError, Reply, ToolChoice};
    use pcagent_shared::{Evidence, Metric, Origin, Window};
    use std::time::Duration;

    struct FailingFixtureModel {
        requests: usize,
    }
    impl Model for FailingFixtureModel {
        fn complete(
            &mut self,
            _: &[Value],
            _: &Value,
            _: ToolChoice,
            _: Duration,
        ) -> std::result::Result<Reply, AgentError> {
            self.requests += 1;
            // Exercise the trace producer without a transport or a calibration-quality claim.
            Err(AgentError::Network)
        }
    }

    #[test]
    fn new_runs_stamp_current_identity_and_old_runs_are_never_relabelled() {
        let mut initial = Snapshot::empty("synthetic-runtime-fixture");
        initial.origin = Origin::SyntheticFixture;
        let mut evidence = Evidence::measured(
            Metric::CpuBusyBasisPoints,
            "system",
            500,
            "synthetic unit fixture",
            Window::instant(),
        );
        evidence.origin = Origin::SyntheticFixture;
        initial.evidence.push(evidence);
        let mut scenario = Scenario::new("synthetic-runtime-fixture".into(), None, initial);
        scenario.tool_names = vec!["get_system_snapshot".into()];
        let mut tools = ReplayTools::new(&scenario, Sharing::Anonymized).unwrap();
        let identity = replay_identity(&scenario, &tools).unwrap();
        let current = CalibrationRuntime::current();
        assert_eq!(identity["runtime"], serde_json::to_value(&current).unwrap());
        assert_eq!(identity["harness_version"], REPLAY_HARNESS_VERSION);
        // The frozen replay's subset and descriptions retain their separate fingerprint.
        assert_ne!(
            identity["tool_registry_sha256"],
            current.registered_tool_registry_sha256
        );
        let mut model = FailingFixtureModel { requests: 0 };
        let (run, success) = investigate(
            &mut model,
            "synthetic-runtime-model".into(),
            "Synthetic fixture question",
            &scenario.initial,
            &mut tools,
        );
        assert_eq!(model.requests, 1);
        assert!(!success);
        assert_eq!(run.runtime.as_ref(), Some(&current));
        assert_eq!(run.harness_version, REPLAY_HARNESS_VERSION);

        let mut legacy = serde_json::to_value(&run).unwrap();
        legacy.as_object_mut().unwrap().remove("runtime");
        let decoded: RecordedRun = serde_json::from_value(legacy).unwrap();
        assert!(decoded.runtime.is_none());
        assert!(serde_json::to_value(decoded)
            .unwrap()
            .get("runtime")
            .is_none());
    }
}
