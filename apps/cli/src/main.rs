#![forbid(unsafe_code)]
mod storage_view;
mod text;
use clap::{Parser, Subcommand};
use pcagent_actions::{
    approve_locally, recover, ActionCancellation, ActionKind, Journal, LocalSession,
    ResolvedTarget, SyntheticDemoCache, MAX_APPROVAL_TTL,
};
use pcagent_agent::credentials::{Credentials, Source};
use pcagent_agent::{
    converse_with_privacy, initial_message_with_privacy, local_report, verify_action, ActionDesk,
    AgentError, Config, ExecutionStatus, ExtensionRequest, Investigation, Nebius, PastTurn,
    PendingPlan, PolicyProtection, Pricing, PrivacyContext, Sharing, StorageContext, Toolbox,
    Workstation, MAX_DURATION, MAX_QUESTION_BYTES, MAX_ROUNDS, MAX_TOOLS, PREPARE_ACTION_PLAN,
    VERIFY_SAMPLES,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{format_bytes, Snapshot, Status};
use pcagent_storage::{volume_space, ContentOptions, ItemKind, ScanOptions, StorageService};
use pcagent_telemetry::Collector;
use serde::Serialize;
use serde_json::json;
use std::{
    error::Error,
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const EXAMPLES: &str = r#"Examples:
  pcagent snapshot
  pcagent diagnose "Why is my PC slow?"
  pcagent diagnose "Why is my PC slow?" --allow-cloud --verbose
  pcagent chat --allow-cloud
  pcagent storage "$env:USERPROFILE\Downloads" --limit 10
  pcagent doctor --check-cloud

Nothing is sent to the cloud without --allow-cloud, and nothing on your PC is changed unless you
approve a proposed action (diagnose --allow-cloud --propose-actions)."#;
#[derive(Parser)]
#[command(
    name = "pcagent",
    version,
    about = "Windows investigation prototype: local diagnosis and explicitly approved actions.",
    after_help = EXAMPLES
)]
struct Args {
    /// Print machine-readable JSON instead of text (byte counts are 64-bit integers).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Review restoring one startup entry previously disabled by a completed local action.
    RestoreStartup {
        source_plan_id: String,
        #[arg(long, default_value_t = 0)]
        item_index: usize,
        #[arg(long)]
        journal_dir: Option<PathBuf>,
    },
    /// Prepare one fixed read-only elevated inspection; execution requires local approval.
    ElevatedInspect {
        #[arg(value_parser = ["system-caches", "mft"])]
        task: String,
        /// One uppercase local drive letter for the future MFT inventory.
        #[arg(long, default_value = "C")]
        volume: String,
        #[arg(long, default_value_t = 120, value_parser = clap::value_parser!(u64).range(1..=120))]
        timeout_seconds: u64,
        /// Explicit local approval after reading the task; then Windows asks for UAC consent.
        #[arg(long)]
        approve_local: bool,
    },
    /// List the NVIDIA Nemotron models actually available to the saved account.
    Models {
        /// Authorize a catalog request with the local key; no measurements are sent.
        #[arg(long)]
        allow_cloud: bool,
    },
    /// Run a closed-schema read-only tool locally, without a provider request.
    Inspect {
        tool: String,
        #[arg(long, default_value = "{}")]
        arguments: String,
        #[arg(long)]
        anonymize: bool,
        /// Local history database; defaults to the desktop app's database.
        #[arg(long)]
        database: Option<PathBuf>,
    },
    /// Read persisted measurements, including windows before the last app restart.
    History {
        #[arg(long, default_value = "cpu_busy_basis_points")]
        metric: String,
        #[arg(long, default_value = "system")]
        entity_ref: String,
        #[arg(long, default_value_t = 3600, value_parser=clap::value_parser!(u64).range(1..=604_800))]
        window_seconds: u64,
        #[arg(long)]
        end_unix_ms: Option<u64>,
        #[arg(long)]
        database: Option<PathBuf>,
    },
    /// Record a problem at one-second cadence, then print its local measurements.
    Record {
        #[arg(long, default_value_t = 30, value_parser=clap::value_parser!(u64).range(30..=120))]
        seconds: u64,
        #[arg(long)]
        database: Option<PathBuf>,
    },
    /// Collect at five-second cadence and report the sampler's measured footprint.
    Sample {
        #[arg(long, default_value_t = 60, value_parser=clap::value_parser!(u64).range(10..=3600))]
        seconds: u64,
        #[arg(long)]
        database: Option<PathBuf>,
    },
    /// Measure memory, CPU, disk and GPU now and list the biggest processes.
    Snapshot {
        /// Number of processes to show (1-50).
        #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u16).range(1..=50))]
        limit: u16,
    },
    /// List readable processes ranked by a selected resource. Never terminates a process.
    Processes {
        /// Number of processes to show (1-50).
        #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u16).range(1..=50))]
        limit: u16,
        #[arg(long, default_value="working_set", value_parser=["working_set", "private_commit", "cpu", "io_read", "io_write"])]
        sort_by: String,
    },
    /// Find the largest files or folders (metadata only unless --compare-content; no links).
    Storage {
        /// Folder to scan.
        root: PathBuf,
        /// Number of items to show (1-100).
        #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Rank directories instead of files.
        #[arg(long)]
        dirs: bool,
        /// Read file contents locally to find same-content copies (bounded; no deletion).
        #[arg(long)]
        compare_content: bool,
    },
    /// Check this PC's support and the cloud configuration (offline unless --check-cloud).
    Doctor {
        /// Authorize a catalog request to Nebius using your key; sends no telemetry.
        #[arg(long)]
        check_cloud: bool,
    },
    /// Answer a question such as "Why is my PC slow?". Local analysis unless --allow-cloud.
    Diagnose {
        /// Your question, in quotes.
        question: String,
        /// Explicitly keep this investigation offline (also the default).
        #[arg(long, conflicts_with = "allow_cloud")]
        offline: bool,
        /// Send the sanitized question and minimized observations to Nebius for this run.
        #[arg(long)]
        allow_cloud: bool,
        /// Inspect the initial cloud payload locally, without sending it. Later samples may differ.
        #[arg(long,conflicts_with_all=["allow_cloud","offline"])]
        preview_cloud: bool,
        /// Also show each completed tool call, evidence IDs and collection notes (never model
        /// reasoning or secrets).
        #[arg(long)]
        verbose: bool,
        /// Authorize a folder the model may scan (metadata only), as an opaque scope. Repeatable.
        #[arg(long)]
        scan_root: Vec<PathBuf>,
        /// Read contents locally in --scan-root folders to compare copies (256 MiB total).
        #[arg(long, requires = "scan_root")]
        compare_content: bool,
        /// Allow selected basenames to be sent; use --preview-cloud to review them offline.
        #[arg(long, requires = "scan_root", conflicts_with_all = ["offline", "anonymize"])]
        share_file_names: bool,
        /// Send opaque process references instead of executable names (for example chrome.exe).
        /// Paths, PIDs and your user name are never sent either way.
        #[arg(long, conflicts_with = "offline")]
        anonymize: bool,
        /// JSON preferences file (exclusions, preserved apps, permissions).
        #[arg(long)]
        preferences: Option<PathBuf>,
        /// Let the model propose action plans; each one is shown here for explicit approval.
        #[arg(long)]
        propose_actions: bool,
        /// Directory for the local action journal (default: %LOCALAPPDATA%\pcagent\journal).
        #[arg(long)]
        journal_dir: Option<PathBuf>,
    },
    /// Talk with the model about this PC: each question can follow up on the previous answers.
    /// Type an empty line or `exit` to stop. Conversation text stays in this session;
    /// local performance samples use the same persistent history as the desktop app.
    Chat {
        /// Send each sanitized question and minimized observations to Nebius (required).
        #[arg(long)]
        allow_cloud: bool,
        /// Send opaque process references instead of executable names (for example chrome.exe).
        #[arg(long)]
        anonymize: bool,
        /// Also show each completed tool call, evidence IDs and collection notes.
        #[arg(long)]
        verbose: bool,
    },
    /// Create the clearly labelled synthetic cache used to demonstrate the cleanup adapter.
    DemoCache {
        /// Size of the synthetic cache in MiB (1-256).
        #[arg(long,default_value_t=64,value_parser=clap::value_parser!(u16).range(1..=256))]
        mb: u16,
    },
}
fn local_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pcagent")
}

fn history_path(path: Option<PathBuf>) -> Result<PathBuf> {
    path.or_else(|| {
        pcagent_agent::credentials::file_path()
            .and_then(|path| path.parent().map(|dir| dir.join("pcagent.sqlite")))
    })
    .ok_or_else(|| "The local history location is unknown; use --database.".into())
}
fn cli_sampler(path: Option<PathBuf>) -> Result<pcagent_store::sampler::Sampler> {
    Ok(pcagent_store::sampler::Sampler::start(&history_path(
        path,
    )?)?)
}

/// No reading stdin or emitting an extra report in machine-readable/batch mode.
fn review_extension(
    report: &Investigation,
    request: &ExtensionRequest,
    json_mode: bool,
    interactive: bool,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> io::Result<bool> {
    if json_mode || !interactive || !request.available {
        return Ok(false);
    }
    text::report(output, report, false)?;
    write!(output, "Continue with up to {} additional model requests, {} tools and {} seconds? This can consume more provider tokens. Type 'yes' to continue [no]: ",
        request.additional_rounds, request.additional_tool_calls, request.additional_seconds)?;
    output.flush()?;
    let mut answer = String::new();
    input.read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}
/// Trusted local review: every pending plan needs a typed "yes" on an interactive terminal.
fn review_actions(
    desk: &ActionDesk,
    pending: Vec<PendingPlan>,
    protection: &PolicyProtection<'_>,
    collector: &mut Collector,
    json_out: bool,
) -> Result<Vec<serde_json::Value>> {
    let mut results = Vec::new();
    if pending.is_empty() {
        return Ok(results);
    }
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let session = LocalSession::new();
    let mut cancelled = false;
    for mut p in pending {
        let plan_id = p.plan.plan_id().to_string();
        if cancelled {
            results.push(json!({"plan_id": plan_id, "plan": p.plan, "decision": "cancelled"}));
            continue;
        }
        let mut err = io::stderr().lock();
        writeln!(
            err,
            "\nProposed action: {} (plan {plan_id})",
            text::words(&p.plan.action_kind())
        )?;
        for item in p.plan.items() {
            let target = match &item.target {
                ResolvedTarget::File(f) => text::path(&f.path),
                ResolvedTarget::SettingsPage { page } => page.uri().into(),
                ResolvedTarget::CleanupAdapter(c) => format!(
                    "{} ({}, {})",
                    text::path(&c.root),
                    text::plural(c.files, "file", "files"),
                    c.bytes.map_or_else(|| "size unknown".into(), format_bytes)
                ),
                ResolvedTarget::Process(t) => format!("process {}", t.identity.pid),
                ResolvedTarget::UserStartup(t) => format!("user startup entry {}", t.value_name()),
                ResolvedTarget::RecycleCleanup(c) => format!(
                    "{} ({}, {})",
                    text::path(&c.root().path),
                    text::plural(c.files().len() as u64, "file", "files"),
                    c.bytes()
                        .map_or_else(|| "size unknown".into(), format_bytes)
                ),
            };
            writeln!(err, "  Target: {target}")?;
        }
        let checks: Vec<String> = p.plan.preconditions().iter().map(text::words).collect();
        writeln!(
            err,
            "  Recovery: {}\n  Checked again just before running: {}",
            text::recovery(p.plan.recovery()),
            checks.join(", ")
        )?;
        if !interactive || json_out {
            writeln!(
                err,
                "  Not executed: approval requires an interactive terminal without --json."
            )?;
            results.push(json!({"plan_id": plan_id, "plan": p.plan, "decision": "not_reviewed"}));
            continue;
        }
        write!(
            err,
            "  Type 'yes' to run this action now, anything else to skip: "
        )?;
        err.flush()?;
        drop(err);
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        if answer.trim() != "yes" {
            eprintln!("  Skipped.");
            results.push(json!({"plan_id": plan_id, "plan": p.plan, "decision": "declined"}));
            continue;
        }
        let volume_root = p.plan.items().iter().find_map(|i| match &i.target {
            ResolvedTarget::CleanupAdapter(c) => Some(c.root.clone()),
            ResolvedTarget::RecycleCleanup(c) => Some(c.root().path.clone()),
            _ => None,
        });
        let cancellation = ActionCancellation::begin()?;
        let measure_system = p.plan.action_kind() == ActionKind::RequestNormalClose;
        let before = if measure_system {
            sample_system(collector, cancellation.flag())
        } else {
            Vec::new()
        };
        let free_before = volume_root.as_deref().and_then(free_bytes);
        let token = approve_locally(&mut p.plan, &session, MAX_APPROVAL_TTL)?;
        let mut plan = p.plan.clone();
        let outcome = desk.broker(&p, protection).execute(
            &mut plan,
            &token,
            &session,
            cancellation.flag(),
            before.last().map(|s| s.id.clone()),
        );
        match outcome {
            Ok(mut result) => {
                eprintln!("  Result: {}", text::words(&result.overall));
                if measure_system && !cancellation.flag().load(Ordering::SeqCst) {
                    eprintln!("  Measuring after the action...");
                    std::thread::sleep(SETTLE_AFTER_ACTION);
                }
                let after = if measure_system {
                    sample_system(collector, cancellation.flag())
                } else {
                    Vec::new()
                };
                let free_after = volume_root.as_deref().and_then(free_bytes);
                if let Some(last) = after.last() {
                    result.record_after_measurement(last.id.clone());
                }
                let verification = verify_action(
                    &result,
                    &before,
                    &after,
                    volume_root.map(|_| (free_before, free_after)),
                );
                text::verification(&mut io::stderr().lock(), &verification)?;
                results.push(json!({"plan_id": plan_id, "plan": p.plan, "decision": "approved", "result": result, "verification": verification}));
            }
            Err(e) => {
                eprintln!("  Refused: {e}");
                results.push(
                    json!({"plan_id": plan_id, "plan": p.plan, "decision": "approved", "error": e.to_string()}),
                );
            }
        }
        cancelled = cancellation.flag().load(Ordering::SeqCst);
    }
    Ok(results)
}
const SETTLE_AFTER_ACTION: Duration = Duration::from_secs(2);
fn sample_system(collector: &mut Collector, cancel: &AtomicBool) -> Vec<Snapshot> {
    (0..VERIFY_SAMPLES)
        .take_while(|_| !cancel.load(Ordering::SeqCst))
        .map(|_| collector.snapshot(false, 1))
        .collect()
}
fn free_bytes(root: &Path) -> Option<u64> {
    volume_space(root).free_bytes
}
fn write_json(value: &impl Serialize) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}
/// Authorizes a folder for scanning, with messages that name the folder the user typed.
fn authorize_folder(storage: &StorageService, root: &Path) -> Result<pcagent_storage::ScopeInfo> {
    let shown = text::path(root);
    match fs::metadata(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!("Folder not found: {shown}").into())
        }
        Ok(m) if !m.is_dir() => return Err(format!("Not a folder: {shown}").into()),
        _ => {}
    }
    storage
        .authorize(root)
        .map_err(|e| format!("Cannot scan {shown}: {e}").into())
}
fn credentials_file_text(creds: &Credentials) -> String {
    creds
        .file()
        .map_or_else(|| "%APPDATA%\\pcagent\\.env".to_owned(), text::path)
}
fn cloud_setup_error(e: AgentError, creds: &Credentials) -> Box<dyn Error> {
    match e {
        AgentError::MissingKey | AgentError::MissingModel | AgentError::InvalidModel => {
            let mut message = format!(
                "{e}.\nPut NEBIUS_API_KEY and NEBIUS_MODEL_ID in {} or set them as environment variables (see \"Opt-in cloud investigation\" in the README), or run without --allow-cloud for a local analysis.",
                credentials_file_text(creds)
            );
            if let Some(problem) = &creds.problem {
                message.push_str(&format!("\nThe credentials file was not used: {problem}."));
            }
            message.into()
        }
        other => other.into(),
    }
}
fn print_doctor(
    creds: &Credentials,
    cloud: Option<&std::result::Result<Option<Pricing>, AgentError>>,
    limits: &[&str],
) -> Result<()> {
    let origin = |name: &str| match creds.source(name) {
        Some(Source::Environment) => " (environment variable)",
        Some(Source::File) => " (credentials file)",
        None => "",
    };
    let key = match creds.get("NEBIUS_API_KEY") {
        Some(_) => format!("set{}", origin("NEBIUS_API_KEY")),
        None => "not set".to_owned(),
    };
    let model = match creds.get("NEBIUS_MODEL_ID") {
        Some(m) => format!("{}{}", text::clean(&m), origin("NEBIUS_MODEL_ID")),
        None => "not set".to_owned(),
    };
    let file = match (&creds.problem, creds.file_found()) {
        (Some(problem), _) => format!("unusable: {problem}"),
        (None, true) => credentials_file_text(creds),
        (None, false) => format!("{} (not found)", credentials_file_text(creds)),
    };
    let rows = [
        ("This PC", String::new()),
        (
            "  Native collector",
            if cfg!(windows) {
                "available".to_owned()
            } else {
                "not available on this platform (Windows only)".to_owned()
            },
        ),
        (
            "  Build",
            format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        ),
        ("  Windows version check", "not implemented yet".to_owned()),
        ("Cloud (Nebius)", String::new()),
        ("  Credentials file", file),
        ("  NEBIUS_API_KEY", key),
        ("  NEBIUS_MODEL_ID", model),
        (
            "  Catalog check",
            match cloud {
                None => "not requested (add --check-cloud)".to_owned(),
                Some(Ok(Some(pricing))) => format!(
                    "the model is in your account catalog; {}",
                    text::price_text(pricing)
                ),
                Some(Ok(None)) => {
                    "the model is in your account catalog; no verifiable price published".to_owned()
                }
                Some(Err(e)) => format!("failed: {e}"),
            },
        ),
        ("Safety", String::new()),
        (
            "  Default mode",
            "read-only by default; cloud and actions require explicit consent".to_owned(),
        ),
        (
            "  Actions",
            "opt-in with --propose-actions; each one needs a typed 'yes'".to_owned(),
        ),
        ("  History", "not stored".to_owned()),
    ];
    let width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let mut out = io::stdout().lock();
    for (label, value) in rows {
        if value.is_empty() {
            writeln!(out, "{label}")?;
        } else {
            writeln!(out, "{label:<width$}  {value}")?;
        }
    }
    writeln!(out, "Notes")?;
    for l in limits {
        writeln!(out, "  - {l}")?;
    }
    Ok(())
}
fn run(args: Args) -> Result<u8> {
    match args.command {
        Command::RestoreStartup {
            source_plan_id,
            item_index,
            journal_dir,
        } => {
            pcagent_inspect::elevation::ensure_unelevated_host()?;
            let desk = ActionDesk::native(Journal::open(
                journal_dir.unwrap_or_else(|| local_data_dir().join("journal")),
            )?);
            let (policy, prefs) = (Policy::from_env(), Preferences::default());
            let protection = PolicyProtection {
                policy: &policy,
                prefs: &prefs,
            };
            let pending = desk.prepare_startup_restore(&source_plan_id, item_index, &protection)?;
            let results = review_actions(
                &desk,
                vec![pending],
                &protection,
                &mut Collector::new(),
                args.json,
            )?;
            write_json(&json!({"restoration":results}))?;
            Ok(0)
        }
        Command::ElevatedInspect {
            task,
            volume,
            timeout_seconds,
            approve_local,
        } => {
            use pcagent_inspect::elevation::{DriveLetter, ElevatedTask, PreparedElevation};
            let task = match task.as_str() {
                "system-caches" => ElevatedTask::SystemCaches,
                "mft" => ElevatedTask::MftInventory {
                    volume: DriveLetter::parse(&volume)?,
                },
                _ => return Err("Unsupported elevated task.".into()),
            };
            if matches!(task, ElevatedTask::MftInventory { .. }) {
                write_json(
                    &json!({"task":task,"status":"unsupported","reason":"MFT allocation accuracy has not been validated on a controlled volume."}),
                )?;
                return Ok(3);
            }
            let mut prepared =
                PreparedElevation::prepare(task, Duration::from_secs(timeout_seconds))?;
            if !approve_local {
                write_json(
                    &json!({"task":task,"budget_seconds":timeout_seconds,"state":"awaiting_local_approval",
                    "note":"Reads system-cache metadata only. It never deletes files. Re-run with --approve-local after reviewing this fixed task."}),
                )?;
                return Ok(0);
            }
            let approval = prepared.approve_locally()?;
            let response =
                prepared.run_locally(approval, &pcagent_storage::CancelToken::default())?;
            let inspection = pcagent_inspect::elevation::to_inspection(&response);
            write_json(&inspection)?;
            Ok(
                if matches!(inspection.status, Status::Ok | Status::Partial) {
                    0
                } else {
                    3
                },
            )
        }
        Command::Models { allow_cloud } => {
            if !allow_cloud {
                return Err("Listing account models requires --allow-cloud; the request sends only the saved credential, never telemetry.".into());
            }
            let credentials = Credentials::load();
            let catalog =
                Nebius::account_catalog(|name| credentials.get(name), Duration::from_secs(10))?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&catalog)?);
            } else {
                println!(
                    "NVIDIA Nemotron account catalog ({}):",
                    pcagent_shared::format_utc_minute(catalog.read_at_unix_ms)
                );
                for model in catalog.models {
                    let prices = model
                        .pricing
                        .as_ref()
                        .and_then(Pricing::per_million_usd)
                        .map(|(input, output)| {
                            format!("${input} / ${output} per million input / output tokens")
                        })
                        .unwrap_or_else(|| "prices unknown".into());
                    let context = model
                        .context_tokens
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "unknown".into());
                    println!("  {}: {prices}; context {context}", model.id);
                }
            }
            Ok(0)
        }
        Command::Inspect {
            tool,
            arguments,
            anonymize,
            database,
        } => {
            let mut collector = Collector::new();
            let (policy, prefs) = (Policy::from_env(), Preferences::default());
            let mut workstation = Workstation::new(
                &mut collector,
                Arc::new(StorageService::new()),
                &policy,
                &prefs,
            );
            workstation.set_sharing(if anonymize {
                Sharing::Anonymized
            } else {
                Sharing::Complete
            });
            let spec = *workstation
                .specs()
                .iter()
                .find(|spec| spec.name == tool)
                .ok_or("Unknown read-only tool.")?;
            let arguments = spec.validate(&arguments)?;
            let sampler = if pcagent_agent::history_tools::TOOLS
                .iter()
                .any(|spec| spec.name == tool)
            {
                Some(cli_sampler(database)?)
            } else {
                None
            };
            if let Some(sampler) = &sampler {
                workstation.set_history(sampler.clone(), Arc::new(AtomicBool::new(false)));
            }
            let output = workstation.run(&spec, &arguments);
            write_json(&output.cloud)?;
            Ok(if matches!(output.status, Status::Ok | Status::Partial) {
                0
            } else {
                3
            })
        }
        Command::History {
            metric,
            entity_ref,
            window_seconds,
            end_unix_ms,
            database,
        } => {
            let metric = serde_json::from_value::<pcagent_shared::Metric>(json!(metric))?;
            let end = end_unix_ms.unwrap_or_else(pcagent_shared::unix_ms);
            if end > pcagent_shared::unix_ms() {
                return Err("History cannot read a future window.".into());
            }
            let store = pcagent_store::Store::open(&history_path(database)?)?;
            let result = store.metric_history(&pcagent_store::HistoryQuery {
                metric,
                entity_ref,
                from_unix_ms: end.saturating_sub(window_seconds * 1000),
                to_unix_ms: end,
            })?;
            write_json(&result)?;
            Ok(0)
        }
        Command::Record { seconds, database } => {
            let sampler = cli_sampler(database)?;
            let recording = sampler.record(seconds, &AtomicBool::new(false))?;
            let end = recording
                .ended_at_unix_ms
                .unwrap_or_else(pcagent_shared::unix_ms);
            let history = sampler.history(&pcagent_store::HistoryQuery {
                metric: pcagent_shared::Metric::CpuBusyBasisPoints,
                entity_ref: "system".into(),
                from_unix_ms: recording.started_at_unix_ms,
                to_unix_ms: end.max(recording.started_at_unix_ms + 1),
            })?;
            let process_activity = sampler.process_peaks(
                recording.started_at_unix_ms,
                end.max(recording.started_at_unix_ms + 1),
                10,
            )?;
            let battery = sampler.battery_recording(&recording.id)?;
            let resource_activity = sampler.process_resource_peaks(
                recording.started_at_unix_ms,
                end.max(recording.started_at_unix_ms + 1),
                10,
            )?;
            write_json(&json!({"recording":recording,"history":history,
                    "process_activity":process_activity,"battery":battery,
                    "resource_activity":resource_activity,
                    "sampler":sampler.status()}))?;
            Ok(0)
        }
        Command::Sample { seconds, database } => {
            let sampler = cli_sampler(database)?;
            std::thread::sleep(Duration::from_secs(seconds));
            sampler.stop();
            write_json(&sampler.status())?;
            Ok(0)
        }
        Command::Snapshot { limit } | Command::Processes { limit, .. } => {
            let processes_only = matches!(args.command, Command::Processes { .. });
            let sort = match &args.command {
                Command::Processes { sort_by, .. } => {
                    pcagent_telemetry::ProcessSort::parse(sort_by).unwrap_or_default()
                }
                _ => pcagent_telemetry::ProcessSort::WorkingSet,
            };
            // One snapshot suffices: its one-second window already gives CPU and I/O rates.
            let mut collector = Collector::new();
            let sample = collector.snapshot_sorted(true, usize::from(limit), sort);
            if args.json {
                write_json(&sample)?;
            } else {
                text::snapshot(
                    &mut io::stdout().lock(),
                    &sample,
                    processes_only,
                    sort.label(),
                )?;
            }
            Ok(if sample.has_observations() { 0 } else { 3 })
        }
        Command::Storage {
            root,
            limit,
            dirs,
            compare_content,
        } => {
            let storage = StorageService::new();
            let info = authorize_folder(&storage, &root)?;
            let scope = info.scope_id;
            let summary = storage.scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})?;
            let kind = if dirs {
                ItemKind::Dirs
            } else {
                ItemKind::Files
            };
            let overview = storage.get_storage_overview(&scope)?;
            let largest = storage.get_largest_items(&scope, kind, 0, usize::from(limit))?;
            let mut context = StorageContext::default();
            if compare_content {
                context.add_comparison(
                    storage.compare_content(&scope, &ContentOptions::default())?,
                    storage.index(&scope)?.as_ref(),
                );
            }
            let items =
                context.local_items(&largest.items, &Policy::from_env(), &Preferences::default());
            if args.json {
                write_json(
                    &json!({"overview":overview,"largest":largest,"items":items,"content_comparisons":context.comparisons()}),
                )?;
            } else {
                text::storage(
                    &mut io::stdout().lock(),
                    &info.root,
                    &summary,
                    &overview,
                    &largest,
                    dirs,
                )?;
                text::storage_context(&mut io::stdout().lock(), &items)?;
                text::content_comparisons(&mut io::stdout().lock(), context.comparisons())?;
            }
            Ok(if summary.status == Status::Ok { 0 } else { 3 })
        }
        Command::Doctor { check_cloud } => {
            let creds = Credentials::load();
            let cloud_result = if check_cloud {
                Some(
                    Config::from_lookup(|name| creds.get(name))
                        .and_then(Nebius::new)
                        .and_then(|mut client| {
                            client
                                .check_catalog(Duration::from_secs(10))
                                .map(|()| client.catalog_pricing().cloned())
                        }),
                )
            } else {
                None
            };
            let cloud_failed = cloud_result.as_ref().is_some_and(|r| r.is_err());
            let limits = [
                "Doctor does not yet validate Windows build, storage capabilities or a local database.",
                "A catalog check does not validate tool calling, diagnostic quality or latency.",
            ];
            if args.json {
                let report = json!({"schema_version":1,"native_collector_available":cfg!(windows),
                    "compiled_os":std::env::consts::OS,"compiled_arch":std::env::consts::ARCH,
                    "product_target":"Windows 11 x64","windows_version_check":"not_implemented",
                    "read_only":true,"persistent_history":false,"actions_available":cfg!(windows),
                    "nebius_key_configured":creds.get("NEBIUS_API_KEY").is_some(),"nebius_model_configured":creds.get("NEBIUS_MODEL_ID").is_some(),
                    "credentials_file_found":creds.file_found(),
                    "cloud_check":match &cloud_result { None=>"not_requested".to_string(),Some(Ok(_))=>"catalog_ok".to_string(),Some(Err(e))=>e.to_string() },
                    "pricing":cloud_result.as_ref().and_then(|r| r.as_ref().ok()).cloned().flatten(),
                    "limits":limits});
                write_json(&report)?;
            } else {
                print_doctor(&creds, cloud_result.as_ref(), &limits)?;
            }
            Ok(if cfg!(windows) && !cloud_failed { 0 } else { 3 })
        }
        Command::DemoCache { mb } => {
            let root = local_data_dir().join("synthetic-demo-cache");
            let blob = 8 * 1024 * 1024_u64;
            let total = u64::from(mb) * 1024 * 1024;
            let names: Vec<String> = (0..total.div_ceil(blob))
                .map(|i| format!("blob-{i}.bin"))
                .collect();
            let files: Vec<(&str, u64)> = names
                .iter()
                .enumerate()
                .map(|(i, n)| (n.as_str(), blob.min(total - i as u64 * blob)))
                .collect();
            SyntheticDemoCache::create(&root, &files)?;
            let notice = "Synthetic filler only. Authorize its parent with --scan-root to let the model propose the cleanup adapter.";
            if args.json {
                write_json(
                    &json!({"created": true, "root": root, "files": files.len(), "bytes": total, "notice": notice}),
                )?;
            } else {
                println!(
                    "Created {} of synthetic data in {} under {}",
                    format_bytes(total),
                    text::plural(files.len() as u64, "file", "files"),
                    text::path(&root)
                );
                println!("{notice}");
            }
            Ok(0)
        }
        Command::Diagnose {
            question,
            offline: _,
            allow_cloud,
            preview_cloud,
            verbose,
            scan_root,
            compare_content,
            share_file_names,
            anonymize,
            preferences,
            propose_actions,
            journal_dir,
        } => {
            let sharing = if anonymize {
                Sharing::Anonymized
            } else {
                Sharing::Complete
            };
            if share_file_names && !allow_cloud && !preview_cloud {
                return Err("--share-file-names requires --preview-cloud or --allow-cloud.".into());
            }
            let prefs = match &preferences {
                Some(path) => fs::read_to_string(path)
                    .map_err(|e| {
                        format!("Cannot read the preferences file {}: {e}", text::path(path))
                    })
                    .and_then(|json| {
                        Preferences::from_json(&json).map_err(|e| {
                            format!("Invalid preferences file {}: {e}", text::path(path))
                        })
                    })?,
                None => Preferences::default(),
            };
            if propose_actions && !allow_cloud {
                return Err(
                    "--propose-actions requires --allow-cloud: only the model proposes plans."
                        .into(),
                );
            }
            if journal_dir.is_some() && !propose_actions {
                return Err("--journal-dir requires --propose-actions.".into());
            }
            if allow_cloud && preferences.is_some() && !prefs.permissions.cloud_allowed {
                return Err("The preferences file does not allow cloud requests (permissions.cloud_allowed is false).".into());
            }
            let storage = Arc::new(StorageService::new());
            for root in &scan_root {
                authorize_folder(&storage, root)?;
            }
            if question.trim().is_empty() {
                return Err(
                    "The question is empty; ask something like \"Why is my PC slow?\"".into(),
                );
            }
            if question.len() > MAX_QUESTION_BYTES {
                return Err(format!(
                    "The question is longer than {MAX_QUESTION_BYTES} bytes; shorten it."
                )
                .into());
            }
            if allow_cloud && !cfg!(windows) {
                return Err("Cloud diagnostics require the native Windows collector; no synthetic or other-platform telemetry is substituted.".into());
            }
            let creds = Credentials::load();
            let mut privacy = PrivacyContext::local();
            if let Some(secret) = creds.get("NEBIUS_API_KEY") {
                privacy.protect_secret(&secret);
            }
            let mut model = if allow_cloud {
                let config = Config::from_lookup(|name| creds.get(name))
                    .map_err(|e| cloud_setup_error(e, &creds))?;
                Some(Nebius::new(config)?)
            } else {
                None
            };
            let mut collector = Collector::new();
            let initial = collector.snapshot(true, 10);
            let policy = Policy::from_env();
            let prepared = storage_view::prepare_with_privacy(
                &storage,
                &policy,
                &prefs,
                share_file_names,
                compare_content,
                &privacy,
            )?;
            if preview_cloud {
                write_json(
                    &json!({"sent":false,"notice":"The sanitized question is sent only with --allow-cloud. Later tool samples may differ; the same allowlist applies.",
                    "message":initial_message_with_privacy(&question, &initial, sharing, &privacy),
                    "tools":Workstation::tool_specs().iter().chain(propose_actions.then_some(&PREPARE_ACTION_PLAN)).map(|t| t.name).collect::<Vec<_>>(),
                    "authorized_scopes":storage.scopes().len(),
                    "file_name_sharing":prepared.context.sharing_preview(),
                    "storage_tool_previews":prepared.preview}),
                )?;
                return Ok(0);
            }
            if share_file_names {
                // The flag is the explicit permission. Show this run's exact finite allowlist
                // before the first request, including in batch mode (stderr preserves JSON).
                eprintln!(
                    "File names authorized for this run (before any cloud request):\n{}",
                    serde_json::to_string_pretty(&prepared.context.sharing_preview())?
                );
            }
            let mut workstation =
                Workstation::new(&mut collector, storage.clone(), &policy, &prefs);
            workstation.set_storage_context(prepared.context.clone());
            workstation.set_sharing(sharing);
            if allow_cloud {
                workstation.set_history(cli_sampler(None)?, Arc::new(AtomicBool::new(false)));
            }
            let mut recovery = None;
            if propose_actions {
                let dir = journal_dir.unwrap_or_else(|| local_data_dir().join("journal"));
                let journal = Journal::open(dir)?;
                let recovered = recover(&journal)?;
                if !recovered.needs_inspection.is_empty()
                    || recovered.journals_unreadable > 0
                    || recovered.malformed_lines > 0
                    || recovered.truncated
                {
                    eprintln!("Action journal: {} interrupted items need manual inspection; {} unreadable journals, {} malformed lines. No action was replayed.",
                        recovered.needs_inspection.len(), recovered.journals_unreadable, recovered.malformed_lines);
                }
                recovery = Some(recovered);
                workstation.enable_action_proposals(ActionDesk::native(journal));
            }
            let mut report = match &mut model {
                Some(model) => {
                    if io::stderr().is_terminal() {
                        eprintln!(
                            "Investigating with Nebius; initial budget: {MAX_ROUNDS} requests, {MAX_TOOLS} tools, {} seconds. An extension requires your confirmation.",
                            MAX_DURATION.as_secs()
                        );
                    }
                    converse_with_privacy(
                        model,
                        &[],
                        &question,
                        &initial,
                        &mut workstation,
                        &privacy,
                        |event| {
                            if verbose {
                                eprintln!("{}", text::tool_event(event));
                            }
                        },
                        |partial, request| {
                            review_extension(
                                partial,
                                request,
                                args.json,
                                io::stdin().is_terminal() && io::stderr().is_terminal(),
                                &mut io::stdin().lock(),
                                &mut io::stderr().lock(),
                            )
                            .unwrap_or(false)
                        },
                    )
                }
                None => local_report(&initial),
            };
            prepared.record_uninspected_scopes(&mut report);
            let jobs = workstation.take_jobs();
            for job in &jobs {
                job.cancel();
            }
            for job in jobs {
                job.wait()?;
            }
            let actions = workstation.take_actions();
            drop(workstation);
            if !args.json {
                text::report(&mut io::stdout().lock(), &report, verbose)?;
                if !prepared.local_items.is_empty() {
                    println!("Authorized folder observations (prepared locally):");
                    text::storage_context(&mut io::stdout().lock(), &prepared.local_items)?;
                }
                text::content_comparisons(
                    &mut io::stdout().lock(),
                    prepared.context.comparisons(),
                )?;
            }
            let mut results = Vec::new();
            if let Some((desk, pending)) = actions {
                let protection = PolicyProtection {
                    policy: &policy,
                    prefs: &prefs,
                };
                results = review_actions(&desk, pending, &protection, &mut collector, args.json)?;
            }
            if args.json {
                let mut json = diagnosis_json(&report, &results, recovery.as_ref());
                json["storage_preparation"] = prepared.local_report();
                write_json(&json)?;
            }
            let action_failed = results.iter().any(|r| {
                r.get("error").is_some()
                    || r.get("result").is_some_and(|v| v["overall"] != "completed")
            });
            Ok(
                if report.execution_status == ExecutionStatus::Completed && !action_failed {
                    0
                } else {
                    3
                },
            )
        }
        Command::Chat {
            allow_cloud,
            anonymize,
            verbose,
        } => {
            if !allow_cloud {
                return Err("chat talks with Nebius: add --allow-cloud to send your questions and minimized measurements.".into());
            }
            if args.json {
                return Err(
                    "chat is interactive and has no --json mode; use diagnose --json.".into(),
                );
            }
            if !cfg!(windows) {
                return Err("Cloud diagnostics require the native Windows collector; no synthetic or other-platform telemetry is substituted.".into());
            }
            let creds = Credentials::load();
            let config = Config::from_lookup(|name| creds.get(name))
                .map_err(|e| cloud_setup_error(e, &creds))?;
            let mut model = Nebius::new(config)?;
            let mut privacy = PrivacyContext::local();
            if let Some(secret) = creds.get("NEBIUS_API_KEY") {
                privacy.protect_secret(&secret);
            }
            let sampler = cli_sampler(None)?;
            let sharing = if anonymize {
                Sharing::Anonymized
            } else {
                Sharing::Complete
            };
            let (policy, prefs) = (Policy::from_env(), Preferences::default());
            let storage = Arc::new(StorageService::new());
            let mut collector = Collector::new();
            let mut past: Vec<PastTurn> = Vec::new();
            println!("Ask about this PC; an empty line or `exit` ends the conversation.");
            loop {
                print!("\n> ");
                io::stdout().flush()?;
                let mut line = String::new();
                if io::stdin().lock().read_line(&mut line)? == 0 {
                    break;
                }
                let question = line.trim();
                if question.is_empty() || ["exit", "quit"].contains(&question) {
                    break;
                }
                if question.len() > MAX_QUESTION_BYTES {
                    eprintln!(
                        "The question is longer than {MAX_QUESTION_BYTES} bytes; shorten it."
                    );
                    continue;
                }
                let initial = collector.snapshot(true, 10);
                let mut workstation =
                    Workstation::new(&mut collector, storage.clone(), &policy, &prefs);
                workstation.set_sharing(sharing);
                workstation.set_history(sampler.clone(), Arc::new(AtomicBool::new(false)));
                let report = converse_with_privacy(
                    &mut model,
                    &past,
                    question,
                    &initial,
                    &mut workstation,
                    &privacy,
                    |event| {
                        if verbose {
                            eprintln!("{}", text::tool_event(event));
                        }
                    },
                    |partial, request| {
                        review_extension(
                            partial,
                            request,
                            false,
                            io::stdin().is_terminal() && io::stderr().is_terminal(),
                            &mut io::stdin().lock(),
                            &mut io::stderr().lock(),
                        )
                        .unwrap_or(false)
                    },
                );
                let jobs = workstation.take_jobs();
                for job in &jobs {
                    job.cancel();
                }
                for job in jobs {
                    job.wait()?;
                }
                drop(workstation);
                println!();
                text::report(&mut io::stdout().lock(), &report, verbose)?;
                // Only answered turns are read again; a failed turn adds nothing to follow up on.
                if report.answer.is_some() {
                    past.push(PastTurn::from_report(question, &report));
                }
            }
            Ok(0)
        }
    }
}
fn main() -> ExitCode {
    let args = Args::parse();
    if pcagent_inspect::elevation::ensure_unelevated_host().is_err() {
        eprintln!("Start PC Agent from a standard, unelevated session.");
        return ExitCode::from(2);
    }
    match run(args) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Error: {}", text::clean(&error.to_string()));
            ExitCode::from(2)
        }
    }
}

fn diagnosis_json(
    report: &pcagent_agent::Investigation,
    actions: &[serde_json::Value],
    recovery: Option<&pcagent_actions::RecoveryReport>,
) -> serde_json::Value {
    let mut value = serde_json::to_value(report).expect("serializable report");
    value["actions"] = json!(actions);
    value["recovery"] = json!(recovery);
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    #[test]
    fn extension_prompt_requires_interactive_explicit_yes_and_never_reads_json_or_piped_input() {
        let report = local_report(&Snapshot::empty("fixture"));
        let request = ExtensionRequest {
            reason: pcagent_agent::BudgetLimit::Rounds,
            available: true,
            additional_rounds: 6,
            additional_tool_calls: 12,
            additional_seconds: 90,
        };
        for (json_mode, interactive) in [(true, true), (false, false)] {
            let mut input = io::Cursor::new(b"yes\n");
            let mut output = Vec::new();
            assert!(!review_extension(
                &report,
                &request,
                json_mode,
                interactive,
                &mut input,
                &mut output
            )
            .unwrap());
            assert_eq!(input.position(), 0);
            assert!(output.is_empty());
        }
        for (answer, expected) in [
            ("yes\n", true),
            ("YES\n", true),
            ("no\n", false),
            ("\n", false),
            ("", false),
        ] {
            let mut output = Vec::new();
            assert_eq!(
                review_extension(
                    &report,
                    &request,
                    false,
                    true,
                    &mut io::Cursor::new(answer.as_bytes()),
                    &mut output
                )
                .unwrap(),
                expected
            );
            let shown = String::from_utf8(output).unwrap();
            assert!(shown.contains("Execution:") && shown.contains("provider tokens"));
        }
    }
    #[cfg(windows)]
    #[test]
    fn json_report_retains_pending_manifest_without_approving_or_executing() {
        use pcagent_actions::{ActionIntent, Broker, NativeEffects, NativeGuard, DEFAULT_PLAN_TTL};
        let dir = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-cli-plan-test"));
        let journal = Journal::open(&dir).unwrap();
        let policy = Policy::default();
        let prefs = Preferences::default();
        let protection = PolicyProtection {
            policy: &policy,
            prefs: &prefs,
        };
        let targets = pcagent_agent::FixedTargets::default();
        let effects = NativeEffects::default();
        let plan = Broker {
            resolver: &targets,
            guard: &NativeGuard,
            protection: &protection,
            effects: &effects,
            journal: &journal,
            synthetic_cache: None,
            plan_ttl: DEFAULT_PLAN_TTL,
        }
        .prepare_plan(&ActionIntent {
            action_kind: ActionKind::OpenSettingsPage,
            target_refs: vec!["storagesense".into()],
            evidence_refs: vec!["ev-fixture".into()],
        })
        .unwrap();
        let desk = ActionDesk::native(journal);
        let actions = review_actions(
            &desk,
            vec![PendingPlan {
                plan,
                targets,
                synthetic_cache: None,
            }],
            &protection,
            &mut Collector::new(),
            true,
        )
        .unwrap();
        let report = local_report(&Snapshot::empty("fixture"));
        let text = serde_json::to_string(&diagnosis_json(&report, &actions, None)).unwrap();
        let document: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(document["actions"][0]["decision"], "not_reviewed");
        assert_eq!(document["actions"][0]["plan"]["state"], "awaiting_approval");
        assert_eq!(
            document["actions"][0]["plan"]["items"][0]["target_ref"],
            "storagesense"
        );
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            0,
            "no action journal without approval"
        );
        fs::remove_dir(&dir).unwrap();
    }
    #[test]
    fn cli_schema_is_consistent() {
        Args::command().debug_assert();
    }
    #[test]
    fn cloud_requires_an_explicit_flag() {
        let args = Args::try_parse_from(["pcagent", "diagnose", "Why is my PC slow?"]).unwrap();
        assert!(matches!(
            args.command,
            Command::Diagnose {
                allow_cloud: false,
                ..
            }
        ));
    }
    #[test]
    fn conflicting_permissions_and_unbounded_limits_are_rejected() {
        assert!(Args::try_parse_from([
            "pcagent",
            "diagnose",
            "test",
            "--offline",
            "--allow-cloud"
        ])
        .is_err());
        assert!(Args::try_parse_from([
            "pcagent",
            "diagnose",
            "test",
            "--preview-cloud",
            "--allow-cloud"
        ])
        .is_err());
        assert!(Args::try_parse_from(["pcagent", "processes", "--limit", "0"]).is_err());
        assert!(Args::try_parse_from(["pcagent", "snapshot", "--limit", "51"]).is_err());
    }
}
