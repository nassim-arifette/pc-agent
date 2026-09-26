#![forbid(unsafe_code)]
use clap::{Parser, Subcommand};
use pcagent_actions::{
    approve_locally, Journal, LocalSession, ResolvedTarget, SyntheticDemoCache, MAX_APPROVAL_TTL,
};
use pcagent_agent::{
    cloud_observation, investigate, local_report, ActionDesk, Config, Investigation, Nebius,
    PendingPlan, PolicyProtection, Workstation, MAX_QUESTION_BYTES, PREPARE_ACTION_PLAN,
    WORKSTATION_TOOLS,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{Evidence, Metric, Snapshot, Status};
use pcagent_storage::{ItemKind, ScanOptions, StorageService};
use pcagent_telemetry::Collector;
use serde::Serialize;
use serde_json::json;
use std::{
    error::Error,
    fs,
    io::{self, BufRead, IsTerminal, Write},
    path::PathBuf,
    process::ExitCode,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
#[derive(Parser)]
#[command(
    name = "pcagent",
    version,
    about = "Read-only Windows investigation prototype. No changes to your PC."
)]
struct Args {
    /// Machine-readable output. Byte values are 64-bit integers; do not parse them as JS Number.
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Collect system memory, one-second aggregate CPU and readable process contributors.
    Snapshot {
        #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u16).range(1..=50))]
        limit: u16,
    },
    /// List readable processes ranked by working set. Never terminates a process.
    Processes {
        #[arg(long,default_value_t=10,value_parser=clap::value_parser!(u16).range(1..=50))]
        limit: u16,
    },
    /// Scan one explicitly authorized folder read-only (metadata only; links are never followed).
    Storage {
        root: PathBuf,
        #[arg(long,default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        /// Rank directories instead of files.
        #[arg(long)]
        dirs: bool,
    },
    /// Check local capabilities and configuration, offline unless explicitly requested.
    Doctor {
        /// Authorize a catalog request to Nebius using your key; sends no telemetry.
        #[arg(long)]
        check_cloud: bool,
    },
    /// Investigate a question. Local rules are the default; cloud requires --allow-cloud.
    Diagnose {
        question: String,
        /// Explicitly keep this investigation offline (also the default).
        #[arg(long, conflicts_with = "allow_cloud")]
        offline: bool,
        /// Send the question verbatim and minimized observations to Nebius for this run.
        #[arg(long)]
        allow_cloud: bool,
        /// Inspect the initial cloud payload locally, without sending it. Later samples may differ.
        #[arg(long,conflicts_with_all=["allow_cloud","offline"])]
        preview_cloud: bool,
        /// Print only completed local tool calls; never print model reasoning or secrets.
        #[arg(long)]
        verbose: bool,
        /// Authorize a folder the model may scan (metadata only), as an opaque scope. Repeatable.
        #[arg(long)]
        scan_root: Vec<PathBuf>,
        /// JSON preferences file (exclusions, preserved apps, permissions).
        #[arg(long)]
        preferences: Option<PathBuf>,
        /// Let the model propose action plans; each one is shown here for explicit approval.
        #[arg(long, requires = "allow_cloud")]
        propose_actions: bool,
        /// Directory for the local action journal (default: %LOCALAPPDATA%\pcagent\journal).
        #[arg(long, requires = "propose_actions")]
        journal_dir: Option<PathBuf>,
    },
    /// Create the clearly labelled synthetic cache used to demonstrate the cleanup adapter.
    DemoCache {
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
/// Trusted local review: every pending plan needs a typed "yes" on an interactive terminal.
fn review_actions(
    desk: &ActionDesk,
    pending: Vec<PendingPlan>,
    protection: &PolicyProtection<'_>,
    json_out: bool,
) -> Result<Vec<serde_json::Value>> {
    let mut results = Vec::new();
    if pending.is_empty() {
        return Ok(results);
    }
    let interactive = io::stdin().is_terminal() && io::stderr().is_terminal();
    let session = LocalSession::new();
    for mut p in pending {
        let plan_id = p.plan.plan_id().to_string();
        let mut err = io::stderr().lock();
        writeln!(
            err,
            "\nProposed action {plan_id}: {:?}",
            p.plan.action_kind()
        )?;
        for item in p.plan.items() {
            let target = match &item.target {
                ResolvedTarget::File(f) => f.path.display().to_string(),
                ResolvedTarget::SettingsPage { page } => page.uri().into(),
                ResolvedTarget::CleanupAdapter(c) => format!(
                    "{} ({} files, {:?} bytes)",
                    c.root.display(),
                    c.files,
                    c.bytes
                ),
                ResolvedTarget::Process(t) => format!("process {}", t.identity.pid),
            };
            writeln!(err, "  target: {target}")?;
        }
        writeln!(
            err,
            "  recovery: {:?}; preconditions: {:?}",
            p.plan.recovery(),
            p.plan.preconditions()
        )?;
        if !interactive || json_out {
            writeln!(
                err,
                "  Not executed: approval requires an interactive terminal without --json."
            )?;
            results.push(json!({"plan_id": plan_id, "decision": "not_reviewed"}));
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
            results.push(json!({"plan_id": plan_id, "decision": "declined"}));
            continue;
        }
        let token = approve_locally(&mut p.plan, &session, MAX_APPROVAL_TTL)?;
        let mut plan = p.plan.clone();
        let outcome = desk.broker(&p, protection).execute(
            &mut plan,
            &token,
            &session,
            &AtomicBool::new(false),
            None,
        );
        match outcome {
            Ok(result) => {
                eprintln!("  Result: {:?}", result.overall);
                results.push(json!({"plan_id": plan_id, "decision": "approved", "result": result}));
            }
            Err(e) => {
                eprintln!("  Refused: {e}");
                results.push(
                    json!({"plan_id": plan_id, "decision": "approved", "error": e.to_string()}),
                );
            }
        }
    }
    Ok(results)
}
fn write_json(value: &impl Serialize) -> Result<()> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out)?;
    Ok(())
}
fn print_evidence(e: &Evidence, out: &mut impl Write) -> io::Result<()> {
    let value = match e.value {
        Some(value) if e.metric == Metric::CpuBusyBasisPoints => {
            format!("{:.2}% of total CPU", value as f64 / 100.0)
        }
        Some(value) => format!("{value} bytes ({:.2} GiB)", value as f64 / 1_073_741_824.0),
        None => "unknown".into(),
    };
    writeln!(
        out,
        "  {:?}: {} [{:?}] entity={} source={} time={} UTC(ms), window={}ms, evidence={}",
        e.metric,
        value,
        e.status,
        e.entity_ref,
        e.source,
        e.window.finished_at_unix_ms,
        e.window.duration_ms,
        e.id
    )
}
fn print_snapshot(s: &Snapshot, processes_only: bool) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "Snapshot {} | origin {:?}", s.id, s.origin)?;
    if !processes_only {
        for e in s.evidence.iter().filter(|e| e.entity_ref == "system") {
            print_evidence(e, &mut out)?;
        }
    }
    writeln!(
        out,
        "Processes: {:?}; {} returned / {} readable / {} enumerated; {} denied, {} unavailable",
        s.process_coverage.status,
        s.process_coverage.returned,
        s.process_coverage.readable,
        s.process_coverage.enumerated,
        s.process_coverage.permission_denied,
        s.process_coverage.unavailable
    )?;
    for p in &s.processes {
        // Debug formatting escapes control characters in untrusted executable names.
        writeln!(
            out,
            "PID {} | {:?} | {}",
            p.pid,
            p.executable_name.as_deref().unwrap_or("name unavailable"),
            p.entity_ref
        )?;
        for e in s.evidence.iter().filter(|e| e.entity_ref == p.entity_ref) {
            print_evidence(e, &mut out)?;
        }
    }
    for limit in &s.limitations {
        writeln!(out, "Limit: {limit}")?;
    }
    Ok(())
}
fn print_report(report: &Investigation) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "Mode: {} | status: {:?} | support: {:?}",
        report.mode, report.status, report.selected.support
    )?;
    let f = &report.selected;
    writeln!(out, "{}", f.summary)?;
    writeln!(out, "Support: {:?} ({})", f.support, f.support_reason)?;
    for fact in &f.facts {
        writeln!(
            out,
            "Fact: {} [{}]",
            fact.text,
            fact.evidence_refs.join(", ")
        )?;
    }
    if let Some(h) = &f.hypothesis {
        writeln!(out, "Hypothesis: {h}")?;
    }
    for c in &f.counter_evidence {
        writeln!(out, "Counter-evidence: {c}")?;
    }
    if !f.missing.is_empty() {
        writeln!(out, "Missing: {}", f.missing.join("; "))?;
    }
    for alt in &report.alternatives {
        writeln!(
            out,
            "Alternative: {:?} - {}",
            alt.classification, alt.summary
        )?;
    }
    writeln!(out, "Next: {}", report.selected.next_step)?;
    for limit in report
        .selected
        .limitations
        .iter()
        .chain(&report.limitations)
    {
        writeln!(out, "Limit: {limit}")?;
    }
    if let Some(error) = &report.error {
        writeln!(out, "Incomplete investigation: {error}")?;
    }
    writeln!(
        out,
        "Executed: {} inference requests, {} tools; tokens: {:?}",
        report.stats.inference_calls, report.stats.tool_calls, report.stats.total_tokens
    )?;
    Ok(())
}
fn configured(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}
fn run(args: Args) -> Result<u8> {
    match args.command {
        Command::Snapshot { limit } | Command::Processes { limit } => {
            let processes_only = matches!(args.command, Command::Processes { .. });
            let sample = Collector::new().snapshot(true, usize::from(limit));
            if args.json {
                write_json(&sample)?;
            } else {
                print_snapshot(&sample, processes_only)?;
            }
            Ok(if sample.has_observations() { 0 } else { 3 })
        }
        Command::Storage { root, limit, dirs } => {
            let storage = StorageService::new();
            let scope = storage.authorize(&root)?.scope_id;
            let summary = storage.scan_blocking(&scope, &ScanOptions::default(), &mut |_| {})?;
            let kind = if dirs {
                ItemKind::Dirs
            } else {
                ItemKind::Files
            };
            let overview = storage.get_storage_overview(&scope)?;
            let largest = storage.get_largest_items(&scope, kind, 0, usize::from(limit))?;
            if args.json {
                write_json(&json!({"overview":overview,"largest":largest}))?;
            } else {
                let mut out = io::stdout().lock();
                writeln!(
                    out,
                    "Scan {:?} in {} ms | {:?}",
                    summary.status, summary.duration_ms, summary.coverage
                )?;
                for item in &largest.items {
                    // Debug formatting escapes control characters in untrusted file names.
                    writeln!(
                        out,
                        "{:>12} logical, {:>12} allocated | {:?}",
                        item.logical_bytes,
                        item.allocated_bytes
                            .map_or("unknown".into(), |b| b.to_string()),
                        item.path.as_deref().unwrap_or(&item.redacted_path)
                    )?;
                }
                for limit in &largest.limitations {
                    writeln!(out, "Limit: {limit}")?;
                }
            }
            Ok(if summary.status == Status::Ok { 0 } else { 3 })
        }
        Command::Doctor { check_cloud } => {
            let cloud_result = if check_cloud {
                Some(
                    Config::from_env()
                        .and_then(Nebius::new)
                        .and_then(|mut client| client.check_catalog(Duration::from_secs(10))),
                )
            } else {
                None
            };
            let cloud_failed = cloud_result.as_ref().is_some_and(|r| r.is_err());
            let report = json!({"schema_version":1,"native_collector_available":cfg!(windows),
                "compiled_os":std::env::consts::OS,"compiled_arch":std::env::consts::ARCH,
                "product_target":"Windows 11 x64","windows_version_check":"not_implemented",
                "read_only":true,"persistent_history":false,"actions_available":false,
                "nebius_key_configured":configured("NEBIUS_API_KEY"),"nebius_model_configured":configured("NEBIUS_MODEL_ID"),
                "cloud_check":match cloud_result { None=>"not_requested".to_string(),Some(Ok(()))=>"catalog_ok".to_string(),Some(Err(e))=>e.to_string() },
                "limits":["Doctor does not yet validate Windows build, storage capabilities or a local database.",
                    "A catalog check does not validate tool calling, diagnostic quality or latency."]});
            write_json(&report)?;
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
            write_json(
                &json!({"created": true, "root": root, "files": files.len(), "bytes": total,
                "notice": "Synthetic filler only. Authorize its parent with --scan-root to let the model propose the cleanup adapter."}),
            )?;
            Ok(0)
        }
        Command::Diagnose {
            question,
            offline: _,
            allow_cloud,
            preview_cloud,
            verbose,
            scan_root,
            preferences,
            propose_actions,
            journal_dir,
        } => {
            let prefs = match &preferences {
                Some(path) => Preferences::from_json(&fs::read_to_string(path)?)?,
                None => Preferences::default(),
            };
            if allow_cloud && preferences.is_some() && !prefs.permissions.cloud_allowed {
                return Err("The preferences file does not allow cloud requests (permissions.cloud_allowed is false).".into());
            }
            let storage = Arc::new(StorageService::new());
            for root in &scan_root {
                storage.authorize(root)?;
            }
            if question.trim().is_empty() || question.len() > MAX_QUESTION_BYTES {
                return Err(pcagent_agent::AgentError::InvalidQuestion.into());
            }
            if allow_cloud && !cfg!(windows) {
                return Err("Cloud diagnostics require the native Windows collector; no synthetic or other-platform telemetry is substituted.".into());
            }
            let mut model = if allow_cloud {
                Some(Nebius::new(Config::from_env()?)?)
            } else {
                None
            };
            let mut collector = Collector::new();
            let initial = collector.snapshot(true, 10);
            if preview_cloud {
                write_json(
                    &json!({"sent":false,"notice":"Question is sent verbatim only with --allow-cloud. Later tool samples may differ; the same allowlist applies.",
                    "question":question,"initial_observation":cloud_observation(&initial),
                    "tools":WORKSTATION_TOOLS.iter().chain(propose_actions.then_some(&PREPARE_ACTION_PLAN)).map(|t| t.name).collect::<Vec<_>>(),
                    "authorized_scopes":storage.scopes().len()}),
                )?;
                return Ok(0);
            }
            let policy = Policy::from_env();
            let mut workstation =
                Workstation::new(&mut collector, storage.clone(), &policy, &prefs);
            if propose_actions {
                let dir = journal_dir.unwrap_or_else(|| local_data_dir().join("journal"));
                workstation.enable_action_proposals(ActionDesk::native(Journal::open(dir)?));
            }
            let report = match &mut model {
                Some(model) => investigate(model, &question, &initial, &mut workstation, |event| {
                    if verbose {
                        eprintln!(
                            "Completed {} [{:?}] in {} ms",
                            event.tool, event.status, event.duration_ms
                        );
                    }
                }),
                None => local_report(&initial),
            };
            for job in workstation.take_jobs() {
                job.cancel();
            }
            let actions = workstation.take_actions();
            drop(workstation);
            if args.json {
                write_json(&report)?;
            } else {
                print_report(&report)?;
            }
            if let Some((desk, pending)) = actions {
                let protection = PolicyProtection {
                    policy: &policy,
                    prefs: &prefs,
                };
                let results = review_actions(&desk, pending, &protection, args.json)?;
                if args.json && !results.is_empty() {
                    write_json(&json!({ "actions": results }))?;
                }
            }
            Ok(if report.error.is_none() && report.status == Status::Ok {
                0
            } else {
                3
            })
        }
    }
}
fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
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
