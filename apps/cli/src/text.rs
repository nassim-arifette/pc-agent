//! Human-readable terminal output. Machine-readable output is `--json` and is not produced here.
use pcagent_actions::Recovery;
use pcagent_agent::{ActionVerification, Investigation, ToolEvent, VERIFY_SAMPLES};
use pcagent_shared::{format_bytes, Evidence, Metric, Origin, ProcessInstance, Snapshot, Status};
use pcagent_storage::{LargestItemsPage, ScanSummary, StorageOverview};
use serde::Serialize;
use std::{
    io::{self, Write},
    path::Path,
};

/// Untrusted names and paths: control and bidirectional-formatting characters are escaped so a
/// name cannot move the cursor, recolor the terminal or reorder the line; the rest prints as is.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        let bidi = matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}');
        if c.is_control() || bidi {
            out.extend(c.escape_unicode());
        } else {
            out.push(c);
        }
    }
    out
}

/// A local path as the user would type it: without the `\\?\` verbatim prefix, escaped like any
/// untrusted name.
pub fn path(p: &Path) -> String {
    let s = p.display().to_string();
    let s = match s.strip_prefix(r"\\?\UNC\") {
        Some(rest) => format!(r"\\{rest}"),
        None => s
            .strip_prefix(r"\\?\")
            .map_or_else(|| s.clone(), str::to_owned),
    };
    clean(&s)
}

pub fn recovery(r: Recovery) -> &'static str {
    match r {
        Recovery::NotApplicable => "not applicable",
        Recovery::RecycleBinIfOperationSupported => "restore it from the Recycle Bin",
        Recovery::UserCanRestartApplication => "restart the application",
        Recovery::NoneSyntheticRegenerableData => "none needed (synthetic, regenerable data)",
    }
}

/// A snake_case enum value as words: `recycle_bin_available` -> `recycle bin available`.
pub fn words(value: &impl Serialize) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s.replace('_', " "),
        _ => "unknown".into(),
    }
}

fn value_text(e: &Evidence) -> String {
    match e.usable_value() {
        Some(v) => e.metric.format_value(v),
        None if e.status == Status::Ok => "unknown".into(),
        None => format!("not measured ({})", words(&e.status)),
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn seconds(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1_000.0)
}

pub fn plural(n: impl Into<u64>, one: &str, many: &str) -> String {
    let n = n.into();
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

/// Two aligned columns: label, value.
fn rows(out: &mut impl Write, rows: &[(String, String)]) -> io::Result<()> {
    let width = rows
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0);
    for (label, value) in rows {
        writeln!(out, "  {label:<width$}  {value}")?;
    }
    Ok(())
}

fn list(out: &mut impl Write, title: &str, items: &[impl AsRef<str>]) -> io::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    writeln!(out, "{title}")?;
    for item in items {
        writeln!(out, "  - {}", item.as_ref())?;
    }
    Ok(())
}

pub fn process_name(p: &ProcessInstance) -> String {
    let name = p
        .executable_name
        .as_deref()
        .map_or_else(|| "name unavailable".into(), clean);
    format!("{name} (PID {})", p.pid)
}

/// Replaces the opaque process references used in cloud-bound text with local names.
fn localize(text: &str, processes: &[ProcessInstance]) -> String {
    processes.iter().fold(text.to_owned(), |text, p| {
        text.replace(&p.entity_ref, &process_name(p))
    })
}

pub fn snapshot(out: &mut impl Write, s: &Snapshot, processes_only: bool) -> io::Result<()> {
    if s.origin != Origin::Live {
        writeln!(out, "Synthetic data, not a live measurement of this PC.")?;
    }
    if !processes_only {
        writeln!(out, "System")?;
        let system = system_rows(s);
        if system.is_empty() {
            writeln!(out, "  No system measurement is available.")?;
        } else {
            rows(out, &system)?;
        }
        writeln!(out)?;
    }
    processes(out, s)?;
    writeln!(out)?;
    list(out, "Notes", &s.limitations)
}

/// Display order of system metrics: CPU, memory, disk, GPU.
const SYSTEM_ORDER: [Metric; 12] = [
    Metric::CpuBusyBasisPoints,
    Metric::CpuBusiestCoreBasisPoints,
    Metric::PhysicalTotalBytes,
    Metric::PhysicalAvailableBytes,
    Metric::SystemCommitBytes,
    Metric::SystemCommitLimitBytes,
    Metric::HardFaultPagesPerSecondX100,
    Metric::DiskReadLatencyMicroseconds,
    Metric::DiskWriteLatencyMicroseconds,
    Metric::DiskIdleBasisPoints,
    Metric::DiskQueueLength,
    Metric::GpuBusiestEngineBasisPoints,
];

fn system_rows(s: &Snapshot) -> Vec<(String, String)> {
    let share = |part: Metric, whole: Metric, of: &str| {
        let part = s.system_value(part)?.1;
        let whole = s.system_value(whole)?.1;
        (whole > 0).then(|| format!(" ({:.0}% of {of})", part as f64 * 100.0 / whole as f64))
    };
    let mut system: Vec<&Evidence> = s
        .evidence
        .iter()
        .filter(|e| e.entity_ref == "system")
        .collect();
    system.sort_by_key(|e| {
        SYSTEM_ORDER
            .iter()
            .position(|m| *m == e.metric)
            .unwrap_or(SYSTEM_ORDER.len())
    });
    let mut rows: Vec<(String, String)> = system
        .into_iter()
        .map(|e| {
            let extra = match e.metric {
                Metric::PhysicalAvailableBytes => share(
                    Metric::PhysicalAvailableBytes,
                    Metric::PhysicalTotalBytes,
                    "physical memory",
                ),
                Metric::SystemCommitBytes => share(
                    Metric::SystemCommitBytes,
                    Metric::SystemCommitLimitBytes,
                    "the limit",
                ),
                _ => None,
            };
            (
                e.metric.label().to_owned(),
                value_text(e) + &extra.unwrap_or_default(),
            )
        })
        .collect();
    // One line for all GPU engine types: the busy ones by name, the rest counted.
    let engines: Vec<(&str, &Evidence)> = s
        .evidence
        .iter()
        .filter_map(|e| Some((e.entity_ref.strip_prefix("gpu-engine-")?, e)))
        .collect();
    if !engines.is_empty() {
        let mut busy: Vec<(&str, u64)> = engines
            .iter()
            .filter_map(|(kind, e)| e.usable_value().filter(|v| *v > 0).map(|v| (*kind, v)))
            .collect();
        busy.sort_by(|a, b| b.1.cmp(&a.1));
        let idle = engines
            .iter()
            .filter(|(_, e)| e.usable_value() == Some(0))
            .count();
        let unmeasured = engines.len() - busy.len() - idle;
        let mut parts: Vec<String> = busy
            .iter()
            .map(|(kind, v)| {
                format!(
                    "{} {}",
                    clean(kind),
                    Metric::GpuEngineBasisPoints.format_value(*v)
                )
            })
            .collect();
        if idle > 0 {
            parts.push(format!("{idle} idle"));
        }
        if unmeasured > 0 {
            parts.push(format!("{unmeasured} not measured"));
        }
        rows.push(("GPU engines".to_owned(), parts.join(", ")));
    }
    rows
}

fn processes(out: &mut impl Write, s: &Snapshot) -> io::Result<()> {
    let c = &s.process_coverage;
    let mut coverage = vec![format!("{} of {} readable", c.readable, c.enumerated)];
    if c.permission_denied > 0 {
        coverage.push(format!("{} access denied", c.permission_denied));
    }
    if c.unavailable > 0 {
        coverage.push(format!("{} unavailable", c.unavailable));
    }
    if c.enumeration_truncated {
        coverage.push("list truncated".into());
    }
    if s.processes.is_empty() {
        return writeln!(
            out,
            "Processes: none readable ({}; {})",
            words(&c.status),
            coverage.join(", ")
        );
    }
    writeln!(
        out,
        "Processes: top {} by working set ({})",
        s.processes.len(),
        coverage.join(", ")
    )?;
    let cell = |p: &ProcessInstance, m: Metric| {
        s.evidence
            .iter()
            .find(|e| e.entity_ref == p.entity_ref && e.metric == m)
            .and_then(Evidence::usable_value)
            .map(|v| m.format_value(v))
    };
    // Rate columns only appear when at least one instance was observed twice.
    let optional = [
        Metric::ProcessCpuBasisPoints,
        Metric::ProcessIoReadBytes,
        Metric::ProcessIoWriteBytes,
    ];
    let columns: Vec<Metric> = [
        Metric::ProcessWorkingSetBytes,
        Metric::ProcessPrivateCommitBytes,
    ]
    .into_iter()
    .chain(
        optional
            .into_iter()
            .filter(|m| s.processes.iter().any(|p| cell(p, *m).is_some())),
    )
    .collect();
    write!(out, "  {:>7}", "PID")?;
    for m in &columns {
        write!(out, "  {:>14}", m.label())?;
    }
    writeln!(out, "  Name")?;
    for p in &s.processes {
        write!(out, "  {:>7}", p.pid)?;
        for m in &columns {
            write!(out, "  {:>14}", cell(p, *m).unwrap_or_else(|| "-".into()))?;
        }
        let name = p
            .executable_name
            .as_deref()
            .map_or_else(|| "(name unavailable)".into(), clean);
        writeln!(out, "  {name}")?;
    }
    Ok(())
}

pub fn storage(
    out: &mut impl Write,
    root: &str,
    summary: &ScanSummary,
    overview: &StorageOverview,
    largest: &LargestItemsPage,
    dirs: bool,
) -> io::Result<()> {
    let c = &summary.coverage;
    writeln!(
        out,
        "Scanned {} in {}: {} in {} ({})",
        clean(root),
        seconds(summary.duration_ms),
        plural(c.files, "file", "files"),
        plural(c.dirs, "folder", "folders"),
        if summary.status == Status::Ok {
            "complete".to_owned()
        } else {
            words(&summary.status)
        }
    )?;
    let mut gaps: Vec<String> = [
        (c.denied_dirs, "folders not readable"),
        (c.denied_files, "files not readable"),
        (c.reparse_skipped, "links or junctions not followed"),
        (c.placeholders, "cloud placeholders not downloaded"),
        (c.other_volume_skipped, "items on other volumes skipped"),
        (c.special_skipped, "special files skipped"),
        (c.vanished, "items vanished during the scan"),
        (c.changed_during_scan, "items changed during the scan"),
        (c.errors, "errors"),
        (c.hard_link_aliases, "extra hard links counted once"),
        (
            c.allocation_unknown_files,
            "files with unknown size on disk",
        ),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, what)| format!("{} {what}", thousands(n)))
    .collect();
    if let Some(reason) = &c.truncated {
        gaps.push(format!("scan stopped early ({})", words(reason)));
    }
    if c.cancelled {
        gaps.push("scan cancelled".into());
    }
    let mut facts = Vec::new();
    if let Some(t) = &overview.scanned {
        facts.push((
            "Total size".to_owned(),
            format!(
                "{} ({} on disk)",
                format_bytes(t.logical_bytes),
                format_bytes(t.allocated_bytes)
            ),
        ));
    }
    if let (Some(free), Some(total)) = (overview.volume.free_bytes, overview.volume.total_bytes) {
        facts.push((
            "Volume".to_owned(),
            format!("{} free of {}", format_bytes(free), format_bytes(total)),
        ));
    }
    if !gaps.is_empty() {
        facts.push(("Coverage".to_owned(), gaps.join(", ")));
    }
    rows(out, &facts)?;
    writeln!(out)?;
    let what = if dirs { "folders" } else { "files" };
    if largest.items.is_empty() {
        writeln!(out, "No {what} found.")?;
    } else {
        writeln!(out, "Largest {what}")?;
        writeln!(out, "  {:>11}  {:>11}  Path", "Size", "On disk")?;
        let prefix = format!("{}\\", root.trim_end_matches('\\'));
        for item in &largest.items {
            let path = item.path.as_deref().unwrap_or(&item.redacted_path);
            let shown = path.strip_prefix(&prefix).unwrap_or(path);
            writeln!(
                out,
                "  {:>11}  {:>11}  {}",
                format_bytes(item.logical_bytes),
                item.allocated_bytes
                    .map_or_else(|| "unknown".into(), format_bytes),
                clean(shown)
            )?;
        }
        if largest.total > largest.items.len() {
            writeln!(
                out,
                "  ... {} more; use --limit to show up to 100",
                thousands((largest.total - largest.items.len()) as u64)
            )?;
        }
    }
    writeln!(out)?;
    list(out, "Notes", &largest.limitations)
}

pub fn tool_event(e: &ToolEvent) -> String {
    format!(
        "  -> {} ({}, {})",
        e.tool,
        words(&e.status),
        seconds(e.duration_ms)
    )
}

pub fn report(out: &mut impl Write, r: &Investigation, verbose: bool) -> io::Result<()> {
    let local = |text: &str| localize(text, &r.local_processes);
    match r.mode.as_str() {
        "local_rules" => writeln!(out, "Local analysis (nothing was sent to the cloud)")?,
        "nebius" => writeln!(out, "Cloud-assisted investigation (Nebius)")?,
        other => writeln!(out, "Mode: {}", clean(other))?,
    }
    writeln!(
        out,
        "Execution: {} | Sensor coverage: {} | Diagnostic support: {}",
        words(&r.execution_status),
        words(&r.coverage.status),
        r.diagnostic_support.label()
    )?;
    for domain in &r.coverage.domains {
        let target = domain
            .target
            .as_ref()
            .map(|t| format!(" ({})", clean(t)))
            .unwrap_or_default();
        writeln!(
            out,
            "  {}{target}: {}",
            words(&domain.domain),
            words(&domain.status)
        )?;
        if verbose {
            list(out, "    Missing coverage", &domain.missing)?;
        }
    }
    if let Some(error) = &r.error {
        writeln!(
            out,
            "Warning: the investigation did not complete ({error}). Collected observations are retained below."
        )?;
    }
    writeln!(out)?;
    let f = &r.selected;
    writeln!(out, "{}", f.classification.label())?;
    writeln!(out, "{}", f.summary)?;
    writeln!(out, "Support: {} ({})", f.support.label(), f.support_reason)?;
    writeln!(out)?;
    let facts: Vec<String> = f
        .facts
        .iter()
        .map(|fact| {
            let text = local(&fact.text);
            if verbose && !fact.evidence_refs.is_empty() {
                format!("{text} [{}]", fact.evidence_refs.join(", "))
            } else {
                text
            }
        })
        .collect();
    list(out, "Evidence", &facts)?;
    if let Some(h) = &f.hypothesis {
        writeln!(out, "Hypothesis: {}", local(h))?;
    }
    let counter: Vec<String> = f.counter_evidence.iter().map(|c| local(c)).collect();
    list(out, "Counter-evidence", &counter)?;
    if !f.missing.is_empty() {
        writeln!(out, "Not measured: {}", f.missing.join(", "))?;
    }
    let alternatives: Vec<String> = r
        .alternatives
        .iter()
        .map(|a| format!("{}: {}", a.classification.label(), a.summary))
        .collect();
    list(out, "Other possibilities", &alternatives)?;
    for observation in r
        .observations
        .iter()
        .filter(|o| {
            r.observation_refs.contains(&o.observation_id) || (r.error.is_some() && o.citable)
        })
        .take(pcagent_agent::MAX_CITED_OBSERVATIONS)
    {
        writeln!(
            out,
            "Observation: {} [{}] ({})",
            clean(&observation.tool),
            clean(&observation.observation_id),
            words(&observation.status)
        )?;
        let facts: Vec<_> = observation.facts.iter().map(|f| clean(f)).collect();
        list(out, "  Facts", &facts)?;
        let limits: Vec<_> = observation.limitations.iter().map(|l| clean(l)).collect();
        list(out, "  Limits", &limits)?;
    }
    if let Some(request) = &r.continuation {
        writeln!(
            out,
            "Budget reached: {}. {}",
            words(&request.reason),
            if request.available {
                "Further investigation requires explicit consent in this interactive session."
            } else {
                "The maximum extension or per-response tool limit was reached."
            }
        )?;
    }
    writeln!(out, "Next step: {}", f.next_step)?;
    writeln!(out)?;
    list(out, "Limitations", &f.limitations)?;
    let notes: Vec<&String> = r
        .limitations
        .iter()
        .filter(|l| !f.limitations.contains(l))
        .collect();
    if verbose {
        list(out, "Collection notes", &notes)?;
    } else if !notes.is_empty() {
        writeln!(
            out,
            "({} not shown; add --verbose to see them.)",
            plural(notes.len() as u64, "collection note", "collection notes")
        )?;
    }
    if r.mode == "local_rules" {
        writeln!(
            out,
            "For a model-guided investigation, add --allow-cloud (sends your question and minimized measurements to Nebius)."
        )?;
    } else {
        let tokens = r
            .stats
            .total_tokens
            .map_or_else(|| "unknown".into(), thousands);
        writeln!(
            out,
            "Model usage: {}, {}; tokens: {tokens}",
            plural(r.stats.inference_calls as u64, "request", "requests"),
            plural(r.stats.tool_calls as u64, "tool call", "tool calls"),
        )?;
    }
    Ok(())
}

pub fn verification(out: &mut impl Write, v: &ActionVerification) -> io::Result<()> {
    writeln!(out, "  Before/after ({VERIFY_SAMPLES} samples each side):")?;
    for c in &v.comparisons {
        let median = |w: &Option<pcagent_diagnostics::WindowSummary>| {
            w.as_ref()
                .map_or_else(|| "unknown".into(), |w| c.metric.format_value(w.median))
        };
        writeln!(
            out,
            "    {}: {} (median {} -> {})",
            c.metric.label(),
            words(&c.outcome),
            median(&c.before),
            median(&c.after)
        )?;
    }
    if let Some(vol) = &v.volume {
        let show = |b: Option<u64>| b.map_or_else(|| "unknown".into(), format_bytes);
        // `gained_bytes` is absent both when a side is unknown and when free space did not grow.
        let gained = match (
            vol.gained_bytes,
            vol.free_before_bytes,
            vol.free_after_bytes,
        ) {
            (Some(g), _, _) => format_bytes(g),
            (None, Some(_), Some(_)) => "none".into(),
            _ => "unknown".into(),
        };
        writeln!(
            out,
            "    Free space: {} -> {} (gained {gained}; the adapter reports {} removed)",
            show(vol.free_before_bytes),
            show(vol.free_after_bytes),
            show(vol.reported_removed_bytes)
        )?;
    }
    for l in &v.limitations {
        writeln!(out, "    Limitation: {l}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn untrusted_text_cannot_drive_the_terminal() {
        assert_eq!(clean(r"C:\Users\me\été.txt"), r"C:\Users\me\été.txt");
        assert_eq!(clean("a\u{1b}[31mb"), "a\\u{1b}[31mb");
        assert_eq!(clean("x\u{202e}gpj.exe"), "x\\u{202e}gpj.exe");
        assert_eq!(clean("line\nbreak\u{85}"), "line\\u{a}break\\u{85}");
    }
    #[test]
    fn paths_drop_the_verbatim_prefix() {
        assert_eq!(path(Path::new(r"\\?\C:\Users\me\x")), r"C:\Users\me\x");
        assert_eq!(path(Path::new(r"\\?\UNC\srv\share\x")), r"\\srv\share\x");
        assert_eq!(path(Path::new(r"C:\plain")), r"C:\plain");
    }
    #[test]
    fn numbers_and_enums_read_as_text() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(46_480), "46,480");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(words(&Status::PermissionDenied), "permission denied");
        assert_eq!(plural(1_u64, "file", "files"), "1 file");
        assert_eq!(plural(2_u64, "file", "files"), "2 files");
    }
    #[test]
    fn opaque_process_references_become_local_names() {
        let p = ProcessInstance {
            entity_ref: "proc-1".into(),
            pid: 42,
            creation_time_filetime: 1,
            boot_id: None,
            executable_name: Some("app\u{1b}.exe".into()),
        };
        assert_eq!(
            localize("Process proc-1 had 1.00 GiB private commit.", &[p]),
            "Process app\\u{1b}.exe (PID 42) had 1.00 GiB private commit."
        );
    }
}
