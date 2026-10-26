//! Recorded tool calls and their replay, for the evaluation (work package 7). A scenario keeps
//! the initial snapshot and the outputs of real tool calls, so every model and configuration can
//! be run against exactly the same PC. Raw scenarios contain local data (process names, paths)
//! and stay on the machine; only anonymized ones may be versioned.
use crate::tools::{Args, ToolOutput, ToolSpec, Toolbox};
use crate::{FindingAliases, Investigation, Observation, Sharing};
use pcagent_diagnostics::Finding;
use pcagent_shared::{Evidence, ProcessInstance, Snapshot, Status};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, time::Instant};
use thiserror::Error;

pub const SCENARIO_VERSION: u32 = 1;
pub const MAX_RECORDED_CALLS: usize = 4096;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplayError {
    #[error("Unsupported scenario or snapshot schema version")]
    Version,
    #[error("Invalid scenario metadata, tool registry or recorded call")]
    InvalidScenario,
}

/// The tools the agent offers today; replay validates against the same closed schemas.
fn registry() -> Vec<ToolSpec> {
    crate::catalog::inspection_tools()
}
fn default_tools() -> Vec<String> {
    registry().iter().map(|s| s.name.into()).collect()
}

/// A tool output as recorded. Local storage items are not kept: the model sees the same payload,
/// but a replayed report does not list local file paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedOutput {
    pub status: Status,
    pub inspection_complete: bool,
    pub cloud: Value,
    pub evidence: Vec<Evidence>,
    pub findings: Vec<Finding>,
    pub processes: Vec<ProcessInstance>,
    pub limitations: Vec<String>,
}
impl From<&ToolOutput> for RecordedOutput {
    fn from(o: &ToolOutput) -> Self {
        Self {
            status: o.status,
            inspection_complete: o.inspection_complete,
            cloud: o.cloud.clone(),
            evidence: o.evidence.clone(),
            findings: o.findings.clone(),
            processes: o.processes.clone(),
            limitations: o.limitations.clone(),
        }
    }
}
impl RecordedOutput {
    pub fn to_output(&self) -> ToolOutput {
        ToolOutput {
            status: self.status,
            inspection_complete: self.inspection_complete,
            cloud: self.cloud.clone(),
            evidence: self.evidence.clone(),
            findings: self.findings.clone(),
            processes: self.processes.clone(),
            storage_items: Vec::new(),
            limitations: self.limitations.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedCall {
    pub tool: String,
    /// Validated arguments, as a JSON object.
    pub arguments: Value,
    pub output: RecordedOutput,
    #[serde(default)]
    pub recorded_at_unix_ms: Option<u64>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// A local investigation trace. The report includes answer verification, per-request usage,
/// catalog-priced cost and errors. No credentials or raw provider bodies are recorded.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedRun {
    pub model: String,
    pub harness_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<crate::routing::CalibrationRuntime>,
    pub started_at_unix_ms: u64,
    pub duration_ms: u64,
    pub report: Value,
}

/// One situation of a real PC, replayable without the PC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    pub schema_version: u32,
    pub name: String,
    /// The question asked when the situation was captured, if any.
    pub question: Option<String>,
    pub captured_at_unix_ms: u64,
    pub initial: Snapshot,
    pub calls: Vec<RecordedCall>,
    /// What the person observed on the PC at capture time, for the graders.
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub sharing: Sharing,
    /// The captured registry, including tools for which no output was captured.
    #[serde(default = "default_tools")]
    pub tool_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RecordedRun>,
}
impl Scenario {
    pub fn new(name: String, question: Option<String>, initial: Snapshot) -> Self {
        Self {
            schema_version: SCENARIO_VERSION,
            name,
            question,
            captured_at_unix_ms: pcagent_shared::unix_ms(),
            initial,
            calls: Vec::new(),
            notes: Vec::new(),
            sharing: Sharing::Anonymized,
            tool_names: default_tools(),
            run: None,
        }
    }

    /// Check the entire scenario before a model or collector can be called. Errors deliberately
    /// omit input text, which may contain private data in an imported file.
    pub fn validate(&self) -> Result<(), ReplayError> {
        if self.schema_version != SCENARIO_VERSION
            || self.initial.schema_version != pcagent_shared::SCHEMA_VERSION
        {
            return Err(ReplayError::Version);
        }
        if self.name.trim().is_empty()
            || self.name.len() > 256
            || self
                .question
                .as_ref()
                .is_some_and(|q| q.trim().is_empty() || q.len() > crate::MAX_QUESTION_BYTES)
            || self.calls.len() > MAX_RECORDED_CALLS
            || self.tool_names.is_empty()
        {
            return Err(ReplayError::InvalidScenario);
        }
        let registry = registry();
        let mut names = HashSet::new();
        for name in &self.tool_names {
            if !names.insert(name) || !registry.iter().any(|s| s.name == name) {
                return Err(ReplayError::InvalidScenario);
            }
        }
        for call in &self.calls {
            let spec = registry
                .iter()
                .find(|s| s.name == call.tool && names.contains(&call.tool))
                .ok_or(ReplayError::InvalidScenario)?;
            spec.validate(&call.arguments.to_string())
                .map_err(|_| ReplayError::InvalidScenario)?;
            if !call.output.cloud.is_object()
                || call
                    .output
                    .findings
                    .iter()
                    .any(|f| f.schema_version != pcagent_shared::SCHEMA_VERSION)
            {
                return Err(ReplayError::InvalidScenario);
            }
        }
        Ok(())
    }
}

/// Wraps a real toolbox and records every call it answers.
pub struct Recorder<T> {
    inner: T,
    pub calls: Vec<RecordedCall>,
}
impl<T> Recorder<T> {
    pub fn new(inner: T) -> Self {
        Self {
            inner,
            calls: Vec::new(),
        }
    }
    pub fn into_parts(self) -> (T, Vec<RecordedCall>) {
        (self.inner, self.calls)
    }
}
impl<T: Toolbox> Toolbox for Recorder<T> {
    fn specs(&self) -> &[ToolSpec] {
        self.inner.specs()
    }
    fn sharing(&self) -> Sharing {
        self.inner.sharing()
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        let recorded_at_unix_ms = Some(pcagent_shared::unix_ms());
        let started = Instant::now();
        let output = self.inner.run(spec, args);
        self.calls.push(RecordedCall {
            tool: spec.name.into(),
            arguments: args.to_value(),
            output: RecordedOutput::from(&output),
            recorded_at_unix_ms,
            duration_ms: Some(started.elapsed().as_millis() as u64),
        });
        output
    }
    fn definitions(&self) -> Value {
        self.inner.definitions()
    }
}

/// A call the scenario could not answer, for the list of missing captures.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Miss {
    pub tool: String,
    pub arguments: Value,
}

/// Answers tool calls from a scenario. A call is answered only by a capture of the same tool
/// with the same arguments, never by a nearby one; anything else is reported as not captured.
pub struct ReplayTools {
    calls: Vec<RecordedCall>,
    served: Vec<bool>,
    specs: Vec<ToolSpec>,
    sharing: Sharing,
    pub misses: Vec<Miss>,
    /// Matching calls served again after their recorded sequence was exhausted.
    pub reused_calls: usize,
}
impl ReplayTools {
    pub fn new(scenario: &Scenario, sharing: Sharing) -> Result<Self, ReplayError> {
        scenario.validate()?;
        Ok(Self {
            served: vec![false; scenario.calls.len()],
            calls: scenario.calls.clone(),
            specs: registry()
                .into_iter()
                .filter(|s| scenario.tool_names.iter().any(|n| n == s.name))
                .collect(),
            sharing,
            misses: Vec::new(),
            reused_calls: 0,
        })
    }
    pub fn served_count(&self) -> usize {
        self.served.iter().filter(|&&served| served).count()
    }
}
impl Toolbox for ReplayTools {
    fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }
    fn sharing(&self) -> Sharing {
        self.sharing
    }
    fn definitions(&self) -> Value {
        let privacy = crate::PrivacyContext::local();
        json!(self.specs.iter().map(|spec| {
            let mut definition = spec.definition();
            let mut captured: Vec<Value> = self.calls.iter()
                .filter(|call| call.tool == spec.name)
                .map(|call| call.arguments.clone()).collect();
            captured.sort_by_key(Value::to_string);
            captured.dedup();
            let mut captured = json!(captured);
            privacy.sanitize_value(&mut captured);
            definition["function"]["description"] = json!(format!(
                "{} Frozen replay: only these exact argument objects were captured: {}. Omitted defaults differ from explicit defaults; other calls are unavailable and never approximated.",
                spec.description, captured));
            definition
        }).collect::<Vec<_>>())
    }
    fn run(&mut self, spec: &ToolSpec, args: &Args) -> ToolOutput {
        let arguments = args.to_value();
        let same = |c: &RecordedCall| c.tool == spec.name && c.arguments == arguments;
        // A repeated call gets the next capture of the same call, then the last one again.
        let pick = (0..self.calls.len())
            .find(|&i| !self.served[i] && same(&self.calls[i]))
            .or_else(|| (0..self.calls.len()).rev().find(|&i| same(&self.calls[i])));
        if let Some(i) = pick {
            if self.served[i] {
                self.reused_calls += 1;
            }
            self.served[i] = true;
            // Imported captures are local collector output. ModelView/the controller apply
            // the final C5 projection, including names embedded in findings and prose.
            return self.calls[i].output.to_output();
        }
        self.misses.push(Miss {
            tool: spec.name.into(),
            arguments: arguments.clone(),
        });
        let captured: Vec<&Value> = self
            .calls
            .iter()
            .filter(|c| c.tool == spec.name)
            .map(|c| &c.arguments)
            .collect();
        ToolOutput {
            status: Status::Unsupported,
            inspection_complete: false,
            cloud: json!({"status": "unsupported",
                "reason": "Not measured: this replayed situation has no capture of this call.",
                "captured_arguments": captured}),
            evidence: Vec::new(),
            findings: Vec::new(),
            processes: Vec::new(),
            storage_items: Vec::new(),
            limitations: vec!["Replay: this call was not captured.".into()],
        }
    }
}

/// The model-facing view of a tool session for MCP and offline previews. It uses the same
/// initial envelope, finding aliases, observation citations and name projection as the agent.
/// Local PIDs, boot/session identifiers and storage paths are never serialized here.
pub struct ModelView {
    report: Investigation,
    aliases: FindingAliases,
    sharing: Sharing,
    privacy: crate::PrivacyContext,
    initial: Value,
}

/// Offline byte measurements on real or explicitly labelled synthetic captures. These are not
/// provider token counts, prices or answer-quality scores; collection and evidence are unchanged.
#[derive(Debug, Clone, Serialize)]
pub struct ContextMeasurement {
    pub tool_calls: usize,
    pub identical_results: usize,
    pub full_projection_bytes: u64,
    pub compact_projection_bytes: u64,
    pub cumulative_full_context_bytes: u64,
    pub cumulative_compact_context_bytes: u64,
}

pub fn measure_context(
    scenario: &Scenario,
    sharing: Sharing,
) -> Result<ContextMeasurement, ReplayError> {
    let mut replay = ReplayTools::new(scenario, sharing)?;
    let mut view = ModelView::new(
        scenario
            .question
            .as_deref()
            .unwrap_or("Inspect this recorded PC situation."),
        &scenario.initial,
        sharing,
    );
    let mut context = crate::tool_context::ToolContext::default();
    let mut messages = Vec::new();
    let mut measured = ContextMeasurement {
        tool_calls: 0,
        identical_results: 0,
        full_projection_bytes: 0,
        compact_projection_bytes: 0,
        cumulative_full_context_bytes: 0,
        cumulative_compact_context_bytes: 0,
    };
    for call in &scenario.calls {
        let spec = *replay
            .specs()
            .iter()
            .find(|s| s.name == call.tool)
            .ok_or(ReplayError::InvalidScenario)?;
        let args = spec
            .validate(&call.arguments.to_string())
            .map_err(|_| ReplayError::InvalidScenario)?;
        let output = replay.run(&spec, &args);
        let full = view.output(&spec, output);
        let compact = context.message(&full);
        measured.tool_calls += 1;
        measured.identical_results += usize::from(compact.get("same_result_as").is_some());
        measured.full_projection_bytes += full.to_string().len() as u64;
        measured.cumulative_full_context_bytes += measured.full_projection_bytes;
        messages.push(json!({"role":"tool", "content":compact.to_string()}));
        context.compact(&mut messages);
        measured.compact_projection_bytes = messages
            .iter()
            .filter_map(|m| m["content"].as_str())
            .map(|s| s.len() as u64)
            .sum();
        measured.cumulative_compact_context_bytes += measured.compact_projection_bytes;
    }
    Ok(measured)
}
impl ModelView {
    pub fn new(question: &str, initial: &Snapshot, sharing: Sharing) -> Self {
        let report = crate::local_report(initial);
        let mut aliases = FindingAliases::default();
        // The same identity and secret removal as the agent's hosts apply before a cloud request.
        let mut privacy = crate::PrivacyContext::local();
        if let Some(secret) = crate::credentials::Credentials::load().get("NEBIUS_API_KEY") {
            privacy.protect_secret(&secret);
        }
        let initial = crate::first_message(
            question,
            initial,
            &report.findings,
            &mut aliases,
            sharing,
            &privacy,
        );
        Self {
            report,
            aliases,
            sharing,
            privacy,
            initial,
        }
    }
    pub fn initial(&self) -> &Value {
        &self.initial
    }
    pub fn output(&mut self, spec: &ToolSpec, mut output: ToolOutput) -> Value {
        self.report.coverage.record(spec, &output);
        let citation = self
            .report
            .push_observation(Observation::record(spec, &mut output));
        let mut cloud = output.cloud.clone();
        let fresh = output.findings.clone();
        let status = output.status;
        let inspection_complete = output.inspection_complete;
        self.report.append(output);
        cloud["observation_id"] = json!(citation);
        cloud["status"] = json!(status);
        cloud["inspection_complete"] = json!(inspection_complete);
        if !fresh.is_empty() {
            cloud["findings"] = json!(fresh
                .iter()
                .map(|f| self.aliases.model_finding(f))
                .collect::<Vec<_>>());
        }
        cloud["current_finding_ids"] = json!(self
            .report
            .findings
            .iter()
            .map(|f| self.aliases.alias(&f.id))
            .collect::<Vec<_>>());
        cloud["citable_observation_ids"] = json!(self
            .report
            .observations
            .iter()
            .filter(|o| o.citable)
            .map(|o| &o.citation)
            .collect::<Vec<_>>());
        crate::privacy::project_tool_names(
            &mut cloud,
            self.sharing,
            &self.report.local_processes,
            &self.privacy,
            &mut self.report.conversation_names,
        );
        crate::tool_context::add_readable_values(&mut cloud);
        cloud
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::SnapshotTools;
    use pcagent_shared::{Metric, Window};

    fn snapshot(cpu: u64) -> Snapshot {
        let mut s = Snapshot::empty("fixture");
        s.evidence.push(Evidence::measured(
            Metric::CpuBusyBasisPoints,
            "system",
            cpu,
            "fixture",
            Window::instant(),
        ));
        s
    }
    fn spec(name: &str) -> &'static ToolSpec {
        crate::WORKSTATION_TOOLS
            .iter()
            .find(|s| s.name == name)
            .unwrap()
    }

    #[test]
    fn a_recorded_call_replays_identically_and_survives_json() {
        let mut cpu = 500;
        let mut recorder = Recorder::new(SnapshotTools::new(|_, _| {
            cpu += 100;
            snapshot(cpu)
        }));
        let snap = spec("get_system_snapshot");
        let args = snap.validate("{}").unwrap();
        let first = recorder.run(snap, &args);
        recorder.run(snap, &args);
        let (_, calls) = recorder.into_parts();
        let mut scenario = Scenario::new("fixture".into(), None, snapshot(1));
        scenario.calls = calls;
        let json = serde_json::to_string(&scenario).unwrap();
        let scenario: Scenario = serde_json::from_str(&json).unwrap();
        let mut replay = ReplayTools::new(&scenario, Sharing::Anonymized).unwrap();
        let a = replay.run(snap, &args);
        let b = replay.run(snap, &args);
        let c = replay.run(snap, &args);
        assert_eq!(a.cloud, first.cloud);
        // Captures are served in order, then the last one repeats.
        assert_ne!(a.cloud, b.cloud);
        assert_eq!(b.cloud, c.cloud);
        assert_eq!(a.findings.len(), first.findings.len());
        assert_eq!(
            serde_json::to_value(&a.evidence).unwrap(),
            serde_json::to_value(&first.evidence).unwrap()
        );
        assert_eq!(
            serde_json::to_value(&a.findings).unwrap(),
            serde_json::to_value(&first.findings).unwrap()
        );
        assert_eq!(replay.served_count(), 2);
        assert_eq!(replay.reused_calls, 1);
        assert!(replay.misses.is_empty());
    }

    #[test]
    fn other_arguments_are_never_answered_by_a_nearby_capture() {
        let mut recorder = Recorder::new(SnapshotTools::new(|_, _| snapshot(500)));
        let procs = spec("get_processes");
        recorder.run(procs, &procs.validate("{\"limit\":5}").unwrap());
        let (_, calls) = recorder.into_parts();
        let mut scenario = Scenario::new("fixture".into(), None, snapshot(1));
        scenario.calls = calls;
        let mut replay = ReplayTools::new(&scenario, Sharing::Anonymized).unwrap();
        let out = replay.run(procs, &procs.validate("{\"limit\":3}").unwrap());
        assert_eq!(out.status, Status::Unsupported);
        assert!(!out.inspection_complete);
        assert_eq!(out.cloud["captured_arguments"][0]["limit"], 5);
        assert_eq!(replay.misses.len(), 1);
        assert_eq!(replay.misses[0].arguments["limit"], 3);
    }

    #[test]
    fn schemas_and_every_recorded_call_are_checked_before_replay() {
        let mut scenario = Scenario::new("fixture".into(), None, snapshot(1));
        scenario.schema_version += 1;
        assert_eq!(scenario.validate(), Err(ReplayError::Version));
        scenario.schema_version = SCENARIO_VERSION;
        scenario.tool_names.push("run_shell".into());
        assert_eq!(scenario.validate(), Err(ReplayError::InvalidScenario));
        scenario.tool_names.pop();
        scenario.calls.push(RecordedCall {
            tool: "get_processes".into(),
            arguments: json!({"limit": 999}),
            output: RecordedOutput::from(&ToolOutput::from_snapshot(&snapshot(10))),
            recorded_at_unix_ms: None,
            duration_ms: None,
        });
        assert_eq!(scenario.validate(), Err(ReplayError::InvalidScenario));
        scenario.calls[0].arguments = json!({"limit": 5});
        scenario.validate().unwrap();
        scenario.tool_names.retain(|name| name != "get_processes");
        assert_eq!(scenario.validate(), Err(ReplayError::InvalidScenario));
    }

    #[test]
    fn validated_arguments_keep_strings_integers_and_omitted_defaults_distinct() {
        let processes = spec("get_processes");
        let args = processes
            .validate(r#"{"sort_by":"cpu","limit":5}"#)
            .unwrap();
        assert_eq!(args.to_value(), json!({"limit":5,"sort_by":"cpu"}));
        assert_eq!(
            processes.validate(&args.to_value().to_string()).unwrap(),
            args
        );
        assert_ne!(
            processes.validate("{}").unwrap().to_value(),
            json!({"limit":10})
        );
    }

    #[test]
    fn model_view_projects_names_and_never_serializes_local_process_identifiers() {
        let mut initial = snapshot(1);
        initial.processes.push(ProcessInstance {
            entity_ref: "proc-fixture".into(),
            pid: 424242,
            creation_time_filetime: 987654321,
            boot_id: Some("PRIVATE_BOOT".into()),
            executable_name: Some(r"C:\Users\PRIVATE_PROFILE\browser.exe".into()),
        });
        initial.evidence.push(Evidence::measured(
            Metric::ProcessWorkingSetBytes,
            "proc-fixture",
            100,
            "fixture",
            Window::instant(),
        ));
        for sharing in [Sharing::Complete, Sharing::Anonymized] {
            let mut view = ModelView::new("Which program?", &initial, sharing);
            let mut out = ToolOutput::from_snapshot(&initial);
            out.cloud["process_names"] = json!({"proc-fixture": "UNTRUSTED_NAME.exe"});
            out.cloud["items"] = json!([{"shared_name": "PRIVATE_FILE", "shared_ancestors": [{"name":"PRIVATE_FOLDER"}]}]);
            let model = view.output(spec("get_processes"), out);
            let encoded = json!([view.initial(), model]).to_string();
            for forbidden in [
                "PRIVATE_BOOT",
                "PRIVATE_PROFILE",
                "424242",
                "987654321",
                "UNTRUSTED_NAME",
                "executable_name",
            ] {
                assert!(!encoded.contains(forbidden), "leaked {forbidden}");
            }
            assert_eq!(
                encoded.contains("browser.exe"),
                sharing == Sharing::Complete
            );
            if sharing == Sharing::Anonymized {
                assert!(!encoded.contains("PRIVATE_FILE"));
                assert!(!encoded.contains("PRIVATE_FOLDER"));
            }
        }
    }

    #[test]
    fn anonymized_replay_strips_every_name_dictionary() {
        let mut cloud = json!({
            "process_names": {"proc-a1b2c3d4": "browser.exe"},
            "shared_names": {"app-b1c2d3e4": "browser.exe"},
            "service_names": {"svc-a1b2c3d4": "FixtureService"},
            "application_names": {"app-a1b2c3d4": "FixtureApp"},
            "startup_names": {"startup-a1b2c3d4": "FixtureStartup"},
            "hardware_names": {"hardware-a1b2c3d4": "FixtureHardware"},
            "note": "browser.exe failed; FixtureService, FixtureApp, FixtureStartup and FixtureHardware.",
            "items": [{"shared_name": "Browser", "nested": {"shared_names": {"x": "y"}}}],
            "kept": 1
        });
        crate::privacy::project_tool_names(
            &mut cloud,
            Sharing::Anonymized,
            &[],
            &crate::PrivacyContext::default(),
            &mut Default::default(),
        );
        let text = cloud.to_string();
        assert!(!text.contains("browser") && !text.contains("Browser"));
        for forbidden in [
            "FixtureService",
            "FixtureApp",
            "FixtureStartup",
            "FixtureHardware",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        assert_eq!(cloud["kept"], 1);
    }

    #[test]
    fn context_byte_measurements_preserve_origin_and_are_not_quality_scores() {
        let mut scenario = Scenario::new("fixture".into(), None, snapshot(10));
        let mut output = RecordedOutput::from(&ToolOutput::from_snapshot(&scenario.initial));
        let row = json!({"measured_at_unix_ms":100,"window":{"duration_ms":3000},"origin":"synthetic_fixture",
            "metric":"temperature","unit":"celsius","value":52,"status":"ok","source":"fixture"});
        output.cloud["observations"] = json!(vec![row; 300]);
        let call = RecordedCall {
            tool: "get_system_snapshot".into(),
            arguments: json!({}),
            output,
            recorded_at_unix_ms: None,
            duration_ms: None,
        };
        let mut changed = call.clone();
        changed.output.cloud["observations"][0]["value"] = json!(53);
        scenario.calls = vec![call.clone(), call, changed];
        let original = serde_json::to_value(&scenario).unwrap();
        let result = measure_context(&scenario, Sharing::Anonymized).unwrap();
        assert_eq!(result.tool_calls, 3);
        assert_eq!(result.identical_results, 1);
        assert!(result.full_projection_bytes > result.compact_projection_bytes);
        assert!(result.cumulative_full_context_bytes > result.cumulative_compact_context_bytes);
        assert_eq!(serde_json::to_value(&scenario).unwrap(), original);
    }

    #[test]
    fn imported_tool_text_passes_the_same_final_privacy_filter_as_the_controller() {
        let mut view = ModelView::new("fixture", &snapshot(10), Sharing::Complete);
        view.privacy = crate::PrivacyContext::from_identifiers(
            r"C:\Users\FixturePerson",
            "FixturePerson",
            "FIXTURE-HOST",
            &["fixture-secret-token"],
        );
        let mut output = RecordedOutput {
            status: Status::Ok,
            inspection_complete: true,
            cloud: json!({"note": "FixturePerson on FIXTURE-HOST; fixture-secret-token; test@example.invalid",
                "path": r"C:\Users\FixturePerson\Documents\file.txt", "value": 10}),
            evidence: vec![], findings: vec![], processes: vec![], limitations: vec![],
        }.to_output();
        output.cloud["process_names"] = json!({});
        let cloud = view.output(spec("get_system_snapshot"), output);
        let text = cloud.to_string();
        for forbidden in [
            "FixturePerson",
            "FIXTURE-HOST",
            "fixture-secret-token",
            "test@example.invalid",
        ] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        assert_eq!(cloud["value"], 10);
    }
}
