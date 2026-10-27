//! Human-readable terminal output. Machine-readable output is `--json` and is not produced here.
use pcagent_actions::Recovery;
use pcagent_agent::{
    ActionVerification, Answer, CitationKind, Cost, Investigation, IssueKind, ModelRequestUsage,
    Pricing, RequestUsage, Stats, ToolEvent, VERIFY_SAMPLES,
};
use pcagent_shared::{
    format_bytes, format_utc_minute, Evidence, Metric, Origin, ProcessInstance, Snapshot, Status,
};
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
        Recovery::RestorePreviousStartupApproval => {
            "prepare a locally approved restoration of the previous startup state"
        }
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

/// `ranked_by` names the resource the process list is sorted by.
pub fn snapshot(
    out: &mut impl Write,
    s: &Snapshot,
    processes_only: bool,
    ranked_by: &str,
) -> io::Result<()> {
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
    processes(out, s, ranked_by)?;
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
    // One line per NVIDIA GPU read through NVML.
    for gpu in &s.gpus {
        let value = |metric: Metric| {
            s.evidence
                .iter()
                .find(|e| e.entity_ref == gpu.entity_ref && e.metric == metric)
                .and_then(Evidence::usable_value)
        };
        let mut parts = Vec::new();
        if let Some(v) = value(Metric::GpuTemperatureCelsius) {
            parts.push(Metric::GpuTemperatureCelsius.format_value(v));
        }
        if let (Some(used), Some(total)) = (
            value(Metric::GpuMemoryUsedBytes),
            value(Metric::GpuMemoryTotalBytes),
        ) {
            parts.push(format!(
                "{} of {} memory",
                format_bytes(used),
                format_bytes(total)
            ));
        }
        if let Some(v) = value(Metric::GpuUtilizationBasisPoints) {
            parts.push(format!(
                "{} busy",
                Metric::GpuUtilizationBasisPoints.format_value(v)
            ));
        }
        if let Some(v) = value(Metric::GpuSmClockMegahertz) {
            parts.push(Metric::GpuSmClockMegahertz.format_value(v));
        }
        if let Some(v) = value(Metric::GpuPowerMilliwatts) {
            parts.push(Metric::GpuPowerMilliwatts.format_value(v));
        }
        match value(Metric::GpuSlowdownReasons) {
            Some(0) => parts.push("no slowdown".to_owned()),
            Some(bits) => parts.push(format!(
                "slowing down: {}",
                Metric::GpuSlowdownReasons.format_value(bits)
            )),
            None => {}
        }
        let name = gpu.name.as_deref().map_or_else(
            || {
                format!(
                    "NVIDIA GPU {}",
                    gpu.entity_ref.trim_start_matches("gpu-nvidia-")
                )
            },
            clean,
        );
        let text = if parts.is_empty() {
            "no readings (see notes)".to_owned()
        } else {
            parts.join(", ")
        };
        rows.push((name, text));
    }
    rows
}

fn processes(out: &mut impl Write, s: &Snapshot, ranked_by: &str) -> io::Result<()> {
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
        "Processes: top {} by {ranked_by} ({})",
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

/// Alias resolution uses local index entries, never string substitution in model output.
pub fn storage_context(
    out: &mut impl Write,
    items: &[pcagent_agent::LocalStorageItem],
) -> io::Result<()> {
    for item in items {
        writeln!(
            out,
            "  {} -> {} ({})",
            clean(&item.alias_path),
            clean(&item.path),
            format_bytes(item.logical_bytes)
        )?;
        writeln!(
            out,
            "    Role: {}; folder: {} ({})",
            words(&item.context.role),
            words(&item.context.folder_role),
            item.context.basis.replace('_', " ")
        )?;
        if let Some(app) = &item.context.application {
            writeln!(
                out,
                "    Associated application: {} (path pattern; installation/ownership unverified)",
                clean(app.name)
            )?;
        }
        writeln!(
            out,
            "    Content comparison: {}",
            item.content.status.replace('_', " ")
        )?;
        for copy in &item.other_copy_paths {
            writeln!(out, "      Observed copy: {}", clean(copy))?;
        }
        if item.content.other_copy_count > item.other_copy_paths.len() {
            writeln!(
                out,
                "      Showing {} of {} other observed copies.",
                item.other_copy_paths.len(),
                item.content.other_copy_count
            )?;
        }
        if let Some(regeneration) = item.context.recovery.regeneration {
            writeln!(out, "    Possible regeneration: {}", clean(regeneration))?;
        }
        if let Some(method) = item.context.recovery.adapter_method {
            writeln!(
                out,
                "    Adapter recovery: {} (conditional; no action authorized)",
                words(&method)
            )?;
        }
        writeln!(
            out,
            "    Recovery: {}; backup and Recycle Bin not verified.",
            clean(item.content.recovery_note).trim_end_matches('.')
        )?;
        for condition in &item.context.conditions {
            writeln!(out, "    Condition: {}", clean(condition))?;
        }
    }
    Ok(())
}

pub fn content_comparisons(
    out: &mut impl Write,
    comparisons: &[pcagent_storage::ContentComparison],
) -> io::Result<()> {
    for comparison in comparisons {
        writeln!(
            out,
            "Content comparison: {} groups, {} files hashed, {} read ({})",
            comparison.groups.len(),
            comparison.checked_refs.len(),
            format_bytes(comparison.bytes_read),
            words(&comparison.status)
        )?;
        for limitation in &comparison.limitations {
            writeln!(out, "  {}", clean(limitation))?;
        }
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

/// What an unverified part of an answer means, for the reader.
fn issue_text(kind: IssueKind) -> String {
    match kind {
        IssueKind::UnsupportedNumber => "number not found in the measurements".into(),
        IssueKind::MissingCitation => "numbers without a cited source".into(),
        IssueKind::UncitedNumber => "measured numbers without their source cited".into(),
        IssueKind::UnknownCitation => "cites a source that does not exist".into(),
        IssueKind::UnseenName => "name not returned by any tool".into(),
        IssueKind::Command => "contains a command; do not run it without checking".into(),
        IssueKind::Link => "contains a link".into(),
        IssueKind::Html => "contains markup".into(),
        IssueKind::UnknownAction => "refers to an action that was not prepared".into(),
        IssueKind::TooLong => "cut to the maximum length".into(),
        other => words(&other),
    }
}

/// The model's answer, its checked sources and what could not be verified.
fn answer(out: &mut impl Write, r: &Investigation, a: &Answer) -> io::Result<()> {
    let local = |text: &str| localize(text, &r.local_processes);
    for line in local(&a.text).lines() {
        writeln!(out, "{}", clean(line))?;
    }
    writeln!(out)?;
    if !a.unverified.is_empty() {
        writeln!(out, "Not verified by PC Agent")?;
        for issue in &a.unverified {
            writeln!(
                out,
                "  - \"{}\": {}",
                clean(&issue.excerpt),
                issue_text(issue.kind)
            )?;
        }
    }
    if !a.citations.is_empty() {
        writeln!(out, "Sources")?;
    }
    for citation in &a.citations {
        if let Some(old) = r
            .historical_observations
            .iter()
            .find(|o| o.reference == citation.target)
        {
            writeln!(
                out,
                "  [{}] {} (earlier measurement, Unix ms {})",
                citation.reference,
                clean(&old.tool),
                old.measured_at_unix_ms
            )?;
            for fact in &old.facts {
                writeln!(out, "      {}", clean(fact))?;
            }
            continue;
        }
        match citation.kind {
            CitationKind::Web => {
                // A web source: its title, site and address, from the lookup that found it.
                let Some((id, source)) = citation.target.split_once('#') else {
                    continue;
                };
                let Some(found) = r
                    .observations
                    .iter()
                    .find(|o| o.observation_id == id)
                    .and_then(|o| o.observation["sources"].as_array())
                    .and_then(|sources| sources.iter().find(|s| s["source_id"] == source))
                else {
                    continue;
                };
                let text = |key: &str| clean(found[key].as_str().unwrap_or_default());
                writeln!(
                    out,
                    "  [{}] {} ({}, web)",
                    citation.reference,
                    text("title"),
                    text("domain")
                )?;
                writeln!(out, "      {}", text("url"))?;
            }
            CitationKind::Observation => {
                let Some(o) = r
                    .observations
                    .iter()
                    .chain(std::iter::once(&r.initial_observation))
                    .find(|o| o.observation_id == citation.target)
                else {
                    continue;
                };
                writeln!(
                    out,
                    "  [{}] {} ({})",
                    citation.reference,
                    clean(&o.tool),
                    words(&o.status)
                )?;
                for fact in &o.facts {
                    writeln!(out, "      {}", clean(&local(fact)))?;
                }
                storage_context(out, &o.storage_items)?;
            }
            CitationKind::Finding => {
                let earlier = r
                    .historical_findings
                    .iter()
                    .find(|f| f.id == citation.target);
                if let Some(f) = r
                    .findings
                    .iter()
                    .find(|f| f.id == citation.target)
                    .or(earlier)
                {
                    writeln!(
                        out,
                        "  [{}] {}: {}{}",
                        citation.reference,
                        f.classification.label(),
                        local(&f.summary),
                        if earlier.is_some() {
                            " (earlier measurement)"
                        } else {
                            ""
                        }
                    )?;
                }
            }
        }
    }
    let follow_ups: Vec<String> = a.follow_ups.iter().map(|f| clean(f)).collect();
    list(out, "You could ask next", &follow_ups)
}

/// Observations listed when a report without an answer falls back on everything usable.
const MAX_SHOWN_OBSERVATIONS: usize = 6;

/// A local analysis, or an investigation that ended without an answer: the selected local
/// finding with its evidence, hypothesis and limits.
fn findings(out: &mut impl Write, r: &Investigation, verbose: bool) -> io::Result<()> {
    let local = |text: &str| localize(text, &r.local_processes);
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
        .take(MAX_SHOWN_OBSERVATIONS)
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
        storage_context(out, &observation.storage_items)?;
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
    Ok(())
}

pub fn report(out: &mut impl Write, r: &Investigation, verbose: bool) -> io::Result<()> {
    match r.mode.as_str() {
        "local_rules" => writeln!(out, "Local analysis (nothing was sent to the cloud)")?,
        "nebius" => writeln!(out, "Cloud-assisted investigation (Nebius)")?,
        other => writeln!(out, "Mode: {}", clean(other))?,
    }
    let support = match &r.answer {
        Some(a) => format!("Confidence: {}", words(&a.confidence)),
        None => format!("Diagnostic support: {}", r.diagnostic_support.label()),
    };
    writeln!(
        out,
        "Execution: {} | Sensor coverage: {} | {support}",
        words(&r.execution_status),
        words(&r.coverage.status),
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
    match &r.answer {
        Some(a) => answer(out, r, a)?,
        None => findings(out, r, verbose)?,
    }
    if r.mode == "local_rules" {
        writeln!(
            out,
            "For a model-guided investigation, add --allow-cloud (sends your question and minimized measurements to Nebius)."
        )?;
    } else {
        let s = &r.stats;
        let tokens = match (s.total_tokens, s.prompt_tokens, s.completion_tokens) {
            (Some(total), Some(input), Some(output)) => format!(
                "{} tokens ({} in, {} out)",
                thousands(total),
                thousands(input),
                thousands(output)
            ),
            (Some(total), _, _) => format!("{} tokens", thousands(total)),
            _ if s.unknown_requests > 0
                && s.known_prompt_tokens + s.known_completion_tokens > 0 =>
            {
                format!(
                    "at least {} tokens ({} in, {} out); {} reported no usage",
                    thousands(s.known_prompt_tokens + s.known_completion_tokens),
                    thousands(s.known_prompt_tokens),
                    thousands(s.known_completion_tokens),
                    plural(s.unknown_requests as u64, "request", "requests")
                )
            }
            _ => "tokens unknown".to_owned(),
        };
        writeln!(
            out,
            "Model usage: {}, {}; {tokens}",
            plural(s.inference_calls as u64, "request", "requests"),
            plural(s.tool_calls as u64, "tool call", "tool calls"),
        )?;
        if let Some(change) = model_change(r) {
            writeln!(out, "{change}")?;
        }
        let models = by_model(s);
        if models.len() > 1 {
            // Each model at its own price: never both models' tokens at the last one's.
            for (model_id, requests) in &models {
                writeln!(out, "  {}", model_line(*model_id, requests))?;
            }
            writeln!(out, "{}", total_cost_line(s))?;
        } else {
            match (s.cost.as_ref(), s.cost_lower_bound.as_ref()) {
                (None, Some(lower)) => writeln!(
                    out,
                    "{}",
                    cost_line(Some(lower)).replacen(
                        "Estimated cost: $",
                        "Estimated cost: at least $",
                        1
                    )
                )?,
                (cost, _) => writeln!(out, "{}", cost_line(cost))?,
            }
        }
    }
    Ok(())
}

/// The model change of a routed turn (chantier 14), and whose answer is printed above when the
/// stronger model did not write one.
fn model_change(r: &Investigation) -> Option<String> {
    let routing = r.stats.routing.as_ref()?;
    let change = routing.change.as_ref()?;
    let (from, to) = (clean(&change.from_model_id), clean(&change.to_model_id));
    let after = plural(change.after_requests as u64, "request", "requests");
    let reason = words(&change.reason);
    // Requests numbered after the change were sent to the new model.
    let taken = r
        .stats
        .model_requests
        .iter()
        .filter(|m| m.request > change.after_requests)
        .count();
    let shown = |text: &str| {
        if r.answer.is_some() {
            format!(" {text}")
        } else {
            String::new()
        }
    };
    let from_answer = shown(&format!("The answer above was written by {from}."));
    Some(match (taken, routing.last_request_error.as_ref()) {
        (0, _) => format!(
            "Model change: {to} was to take over from {from} after {after} ({reason}), but the investigation ended before it answered.{from_answer}"
        ),
        (1, Some(error)) => format!(
            "Model change: {to} was asked to take over from {from} after {after} ({reason}), but its request failed ({error}).{from_answer}"
        ),
        (_, Some(error)) => format!(
            "Model change: {to} took over from {from} after {after} ({reason}), but its last request failed ({error}).{}",
            shown("The answer above is the best one written before the failure.")
        ),
        (_, None) => {
            format!("Model change: {to} took over from {from} after {after} ({reason}).")
        }
    })
}

/// Requests grouped by model, in the order the models were first used.
fn by_model(s: &Stats) -> Vec<(Option<&str>, Vec<&ModelRequestUsage>)> {
    let mut groups: Vec<(Option<&str>, Vec<&ModelRequestUsage>)> = Vec::new();
    for request in &s.model_requests {
        let id = request.model_id.as_deref();
        match groups.iter_mut().find(|(model, _)| *model == id) {
            Some((_, group)) => group.push(request),
            None => groups.push((id, vec![request])),
        }
    }
    groups
}

/// One model's requests, tokens and cost; a sum only when all of it is known.
fn model_line(model_id: Option<&str>, requests: &[&ModelRequestUsage]) -> String {
    let tokens = |count: fn(&RequestUsage) -> Option<u64>| {
        requests
            .iter()
            .try_fold(0u64, |sum, r| sum.checked_add(count(&r.usage)?))
    };
    let usage = match (tokens(|u| u.prompt_tokens), tokens(|u| u.completion_tokens)) {
        (Some(input), Some(output)) => format!(
            "{} tokens ({} in, {} out)",
            thousands(input.saturating_add(output)),
            thousands(input),
            thousands(output)
        ),
        _ => "tokens unknown".to_owned(),
    };
    let priced: Vec<&Cost> = requests.iter().filter_map(|r| r.cost.as_ref()).collect();
    let price = priced
        .first()
        .filter(|first| priced.iter().all(|c| c.pricing == first.pricing))
        .map(|c| format!(" at {}", price_text(&c.pricing)))
        .unwrap_or_default();
    let cost = match (priced.len() == requests.len())
        .then(|| sum_pico(&priced))
        .flatten()
    {
        Some(pico) => format!("estimated cost ${}{price}", usd_text(pico)),
        None => format!(
            "estimated cost not shown ({} without usage or verified price)",
            plural(
                (requests.len() - priced.len()) as u64,
                "request",
                "requests"
            )
        ),
    };
    format!(
        "{}: {}; {usage}; {cost}",
        model_id.map_or_else(|| "unknown model".to_owned(), clean),
        plural(requests.len() as u64, "request", "requests")
    )
}

/// The total of a turn served by several models, only when every request is priced.
fn total_cost_line(s: &Stats) -> String {
    if let Some(total) = s.total_cost_usd.as_deref().and_then(pico) {
        return format!(
            "Estimated cost: ${} in total, each model at its own catalog price",
            usd_text(total)
        );
    }
    let priced: Vec<&Cost> = s
        .model_requests
        .iter()
        .filter_map(|r| r.cost.as_ref())
        .collect();
    match sum_pico(&priced).filter(|_| !priced.is_empty()) {
        Some(lower) => format!(
            "Estimated cost: at least ${} (requests without usage or verified price are not counted)",
            usd_text(lower)
        ),
        None => cost_line(None),
    }
}

const PICO_PER_USD: u128 = 1_000_000_000_000;
/// Picodollars of an exact amount from the core ("0.0000432"); `None` for anything else.
fn pico(usd: &str) -> Option<u128> {
    let (whole, fraction) = usd.split_once('.').unwrap_or((usd, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (usd.contains('.') && !digits(fraction)) || fraction.len() > 12 {
        return None;
    }
    let fraction = if fraction.is_empty() {
        0
    } else {
        format!("{fraction:0<12}").parse::<u128>().ok()?
    };
    whole
        .parse::<u128>()
        .ok()?
        .checked_mul(PICO_PER_USD)?
        .checked_add(fraction)
}
fn sum_pico(costs: &[&Cost]) -> Option<u128> {
    costs
        .iter()
        .try_fold(0u128, |sum, c| sum.checked_add(pico(&c.usd)?))
}
/// As [`cost_line`] shows an amount: four decimals rounded half-up, or exact when that would
/// show zero.
fn usd_text(pico: u128) -> String {
    const STEP: u128 = 100_000_000;
    let rounded = pico / STEP + u128::from(pico % STEP >= STEP / 2);
    let shown = format!("{}.{:04}", rounded / 10_000, rounded % 10_000);
    if shown != "0.0000" || pico == 0 {
        return shown;
    }
    let digits = format!("{:012}", pico % PICO_PER_USD);
    format!("{}.{}", pico / PICO_PER_USD, digits.trim_end_matches('0'))
}

/// Spec 13: a cost is shown only from a verified, dated price; otherwise usage alone.
pub fn cost_line(cost: Option<&Cost>) -> String {
    let Some(cost) = cost else {
        return "Estimated cost: not shown (no verified, dated catalog price, or incomplete usage)"
            .to_owned();
    };
    let rounded = cost.rounded_usd(4);
    let amount = if rounded == "0.0000" && cost.usd != "0" {
        cost.usd.clone()
    } else {
        rounded
    };
    format!("Estimated cost: ${amount} at {}", price_text(&cost.pricing))
}

pub fn price_text(pricing: &Pricing) -> String {
    let rates = pricing.per_million_usd().map_or_else(
        || "unparsed prices".to_owned(),
        |(input, output)| format!("${input} in / ${output} out per million tokens"),
    );
    format!(
        "Nebius catalog prices ({rates}, read {})",
        format_utc_minute(pricing.read_at_unix_ms)
    )
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

    /// A Super → Ultra turn as the engine reports it, with synthetic prices (Ultra ten times
    /// Super) and no provider: Super's two requests, then Ultra's, each priced unless it failed.
    mod routed {
        use super::*;
        use pcagent_agent::routing::{EscalationReason, ModelChange, RoutingMode, RoutingState};
        use pcagent_agent::{estimate_cost, local_report, AgentError, Category, Confidence};

        const SUPER: &str = "nvidia/nemotron-super-fixture";
        const ULTRA: &str = "nvidia/nemotron-ultra-fixture";

        /// `ultra` lists Ultra's requests: their tokens, or `None` for one that failed.
        fn turn(ultra: &[Option<(u64, u64)>], answered: bool) -> Investigation {
            let pricing = |model_id: &str, prompt: &str, completion: &str| Pricing {
                model_id: model_id.into(),
                prompt_usd_per_token: prompt.into(),
                completion_usd_per_token: completion.into(),
                source: "synthetic fixture".into(),
                read_at_unix_ms: 0,
            };
            let prices = [
                pricing(SUPER, "0.0000003", "0.0000009"),
                pricing(ULTRA, "0.000003", "0.000009"),
            ];
            let sent: Vec<_> = [(SUPER, Some((4_700, 80))), (SUPER, Some((5_000, 20)))]
                .into_iter()
                .chain(ultra.iter().map(|&tokens| (ULTRA, tokens)))
                .collect();
            let model_requests: Vec<ModelRequestUsage> = sent
                .iter()
                .enumerate()
                .map(|(i, &(id, tokens))| ModelRequestUsage {
                    request: i + 1,
                    model_id: Some(id.into()),
                    usage: RequestUsage {
                        prompt_tokens: tokens.map(|t| t.0),
                        completion_tokens: tokens.map(|t| t.1),
                        reasoning_tokens: None,
                    },
                    cost: tokens
                        .and_then(|(p, c)| estimate_cost(p, c, &prices[usize::from(id == ULTRA)])),
                    duration_ms: 0,
                })
                .collect();
            let known = |count: fn((u64, u64)) -> u64| sent.iter().filter_map(|s| s.1).map(count);
            let complete = sent.iter().all(|s| s.1.is_some());
            let mut r = local_report(&Snapshot::empty("fixture"));
            r.mode = "nebius".into();
            let s = &mut r.stats;
            s.inference_calls = sent.len();
            s.tool_calls = 1;
            s.requests = model_requests.iter().map(|m| m.usage.clone()).collect();
            (s.known_prompt_tokens, s.known_completion_tokens) =
                (known(|t| t.0).sum(), known(|t| t.1).sum());
            s.prompt_tokens = complete.then_some(s.known_prompt_tokens);
            s.completion_tokens = complete.then_some(s.known_completion_tokens);
            s.total_tokens = complete.then_some(s.known_prompt_tokens + s.known_completion_tokens);
            s.unknown_requests = sent.iter().filter(|s| s.1.is_none()).count();
            // As the core sums them: exact, and only when every request is priced.
            s.total_cost_usd = model_requests
                .iter()
                .map(|m| pico(&m.cost.as_ref()?.usd))
                .sum::<Option<u128>>()
                .map(|total| {
                    let fraction = format!("{:012}", total % PICO_PER_USD);
                    format!(
                        "{}.{}",
                        total / PICO_PER_USD,
                        fraction.trim_end_matches('0')
                    )
                });
            s.model_requests = model_requests;
            s.routing = Some(RoutingState {
                mode: RoutingMode::Automatic,
                current_model_id: Some(ULTRA.into()),
                calibration_reviewed_at_unix_ms: Some(1),
                change: Some(ModelChange {
                    from_model_id: SUPER.into(),
                    to_model_id: ULTRA.into(),
                    reason: EscalationReason::LowConfidence,
                    after_requests: 2,
                }),
                last_request_error: ultra
                    .last()
                    .is_some_and(Option::is_none)
                    .then_some(AgentError::Timeout),
            });
            r.answer = answered.then(|| Answer {
                text: "See the measurements.".into(),
                confidence: Confidence::Low,
                category: Category::Other,
                citations: Vec::new(),
                action_refs: Vec::new(),
                follow_ups: Vec::new(),
                unverified: Vec::new(),
                repairs: 0,
            });
            r
        }
        fn routed(ultra: &[Option<(u64, u64)>], answered: bool) -> String {
            let mut out = Vec::new();
            report(&mut out, &turn(ultra, answered), false).unwrap();
            String::from_utf8(out).unwrap()
        }

        #[test]
        fn the_model_change_and_each_models_cost_are_printed_with_their_total() {
            let text = routed(&[Some((6_000, 50))], true);
            assert!(text.contains(&format!(
                "Model change: {ULTRA} took over from {SUPER} after 2 requests (low confidence).\n"
            )));
            assert!(text.contains(&format!(
                "  {SUPER}: 2 requests; 9,800 tokens (9,700 in, 100 out); estimated cost $0.0030 at Nebius catalog prices ($0.30 in / $0.90 out per million tokens, read "
            )));
            assert!(text.contains(&format!(
                "  {ULTRA}: 1 request; 6,050 tokens (6,000 in, 50 out); estimated cost $0.0185 at Nebius catalog prices ($3.00 in / $9.00 out per million tokens, read "
            )));
            assert!(text.contains(
                "Estimated cost: $0.0215 in total, each model at its own catalog price\n"
            ));
            assert!(!text.contains("not shown"), "{text}");
        }

        #[test]
        fn a_failed_ultra_request_says_whose_answer_is_shown_and_gives_no_total() {
            let text = routed(&[None], true);
            assert!(text.contains(&format!(
                "Model change: {ULTRA} was asked to take over from {SUPER} after 2 requests (low confidence), but its request failed ({}). The answer above was written by {SUPER}.\n",
                AgentError::Timeout
            )));
            assert!(text.contains(&format!(
                "  {ULTRA}: 1 request; tokens unknown; estimated cost not shown (1 request without usage or verified price)\n"
            )));
            assert!(text.contains("Estimated cost: at least $0.0030 (requests without usage or verified price are not counted)\n"));
            assert!(!text.contains("in total"), "{text}");
            // Without an answer, nothing is said about whose answer is shown.
            let text = routed(&[None], false);
            assert!(text.contains("but its request failed"));
            assert!(!text.contains("The answer above"), "{text}");
        }

        #[test]
        fn a_change_without_an_ultra_answer_never_credits_ultra() {
            // The investigation ended before Ultra's first request: Super's costs only.
            let text = routed(&[], true);
            assert!(text.contains(&format!(
                "Model change: {ULTRA} was to take over from {SUPER} after 2 requests (low confidence), but the investigation ended before it answered. The answer above was written by {SUPER}.\n"
            )));
            assert!(!text.contains(&format!("  {ULTRA}:")), "{text}");
            // Ultra answered once, then its last request failed: whose answer is kept is unknown.
            let text = routed(&[Some((6_000, 50)), None], true);
            assert!(text.contains(&format!(
                "Model change: {ULTRA} took over from {SUPER} after 2 requests (low confidence), but its last request failed ({}). The answer above is the best one written before the failure.\n",
                AgentError::Timeout
            )));
            assert!(text.contains(&format!(
                "  {ULTRA}: 2 requests; tokens unknown; estimated cost not shown (1 request without usage or verified price)\n"
            )));
            // Super's $0.0030 and Ultra's priced $0.01845, each at its own price.
            assert!(
                text.contains("Estimated cost: at least $0.0215 (requests"),
                "{text}"
            );
        }

        #[test]
        fn exact_amounts_are_shown_like_a_single_cost() {
            assert_eq!(pico("0.0223698").map(usd_text).as_deref(), Some("0.0224"));
            assert_eq!(
                pico("0.0000432").map(usd_text).as_deref(),
                Some("0.0000432")
            );
            assert_eq!(pico("0").map(usd_text).as_deref(), Some("0.0000"));
            for bad in ["", ".3", "3.", "-1", "1e-7", "0.0000000000001"] {
                assert_eq!(pico(bad), None, "{bad:?}");
            }
        }
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
