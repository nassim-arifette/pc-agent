//! Local action review and execution only after explicit terminal approval.
use crate::local::{local_data_dir, write_json};
use crate::{args as options, text, Result};
use pcagent_actions::{
    approve_locally, ActionCancellation, ActionKind, Journal, LocalSession, ResolvedTarget,
    SyntheticDemoCache, MAX_APPROVAL_TTL,
};
use pcagent_agent::{verify_action, VERIFY_SAMPLES};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::{format_bytes, Snapshot};
use pcagent_storage::volume_space;
use pcagent_telemetry::Collector;
use pcagent_tools::{ActionDesk, PendingPlan, PolicyProtection};
use serde_json::json;
use std::io;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Trusted local review: every pending plan needs a typed "yes" on an interactive terminal.
pub(super) fn review_actions(
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
            "\n{}",
            text::fill(
                "cli.action.proposed",
                &[
                    ("action", text::words(&p.plan.action_kind())),
                    ("plan", plan_id.to_string())
                ]
            )
        )?;
        for item in p.plan.items() {
            let target = match &item.target {
                ResolvedTarget::File(f) => text::path(&f.path),
                ResolvedTarget::SettingsPage { page } => page.uri().into(),
                ResolvedTarget::CleanupAdapter(c) => format!(
                    "{} ({}, {})",
                    text::path(&c.root),
                    text::plural(c.files, "file", "files"),
                    c.bytes.map_or_else(
                        || text::label("cli.action.size_unknown").into(),
                        format_bytes
                    )
                ),
                ResolvedTarget::Process(t) => {
                    text::fill("cli.action.process", &[("pid", t.identity.pid.to_string())])
                }
                ResolvedTarget::BrowserManager(t) => text::fill(
                    "cli.action.browser_manager",
                    &[
                        ("destination", format!("{:?}", t.destination)),
                        ("pid", t.process.identity.pid.to_string()),
                    ],
                ),
                ResolvedTarget::UserStartup(t) => text::fill(
                    "cli.action.startup",
                    &[("name", t.value_name().to_string())],
                ),
                ResolvedTarget::RecycleCleanup(c) => format!(
                    "{} ({}, {})",
                    text::path(&c.root().path),
                    text::plural(c.files().len() as u64, "file", "files"),
                    c.bytes().map_or_else(
                        || text::label("cli.action.size_unknown").into(),
                        format_bytes
                    )
                ),
            };
            writeln!(
                err,
                "  {}",
                text::fill("cli.action.target", &[("target", target)])
            )?;
        }
        let checks: Vec<String> = p.plan.preconditions().iter().map(text::words).collect();
        writeln!(
            err,
            "  {}\n  {}",
            text::fill(
                "cli.action.recovery",
                &[("recovery", text::recovery(p.plan.recovery()).to_string())]
            ),
            text::fill("cli.action.checks", &[("checks", checks.join(", "))])
        )?;
        if !interactive || json_out {
            writeln!(err, "  {}", text::label("cli.action.not_executed"))?;
            results.push(json!({"plan_id": plan_id, "plan": p.plan, "decision": "not_reviewed"}));
            continue;
        }
        write!(err, "  {}", text::label("cli.action.prompt"))?;
        err.flush()?;
        drop(err);
        let mut answer = String::new();
        io::stdin().lock().read_line(&mut answer)?;
        if !text::is_yes(&answer) {
            eprintln!("  {}", text::label("cli.action.skipped"));
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
                eprintln!(
                    "  {}",
                    text::fill(
                        "cli.action.result",
                        &[("result", text::words(&result.overall))]
                    )
                );
                if measure_system && !cancellation.flag().load(Ordering::SeqCst) {
                    eprintln!("  {}", text::label("cli.action.measuring"));
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
                eprintln!(
                    "  {}",
                    text::fill("cli.action.refused", &[("error", e.to_string())])
                );
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
pub(super) fn sample_system(collector: &mut Collector, cancel: &AtomicBool) -> Vec<Snapshot> {
    (0..VERIFY_SAMPLES)
        .take_while(|_| !cancel.load(Ordering::SeqCst))
        .map(|_| collector.snapshot(false, 1))
        .collect()
}
pub(super) fn free_bytes(root: &Path) -> Option<u64> {
    volume_space(root).free_bytes
}

pub(super) fn restore_startup(options: options::RestoreStartup, json: bool) -> Result<u8> {
    let options::RestoreStartup {
        source_plan_id,
        item_index,
        journal_dir,
    } = options;
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
        json,
    )?;
    write_json(&json!({"restoration":results}))?;
    Ok(0)
}

pub(super) fn demo_cache(options: options::DemoCache, json: bool) -> Result<u8> {
    let options::DemoCache { mb } = options;
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
    if json {
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
