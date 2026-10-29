//! Human-readable terminal output. Machine-readable output is `--json` and is not produced here.
use pcagent_actions::Recovery;
use pcagent_agent::{
    ActionVerification, Answer, CitationKind, Cost, Investigation, IssueKind, ModelRequestUsage,
    Pricing, RequestUsage, Stats, ToolEvent, VERIFY_SAMPLES,
};
use pcagent_shared::{
    format_utc_minute, presentation::Language, Evidence, Metric, Origin, Phrase, ProcessInstance,
    Quantity, Snapshot, Status,
};
use pcagent_storage::{LargestItemsPage, ScanSummary, StorageOverview};
use serde::Serialize;
use std::{
    io::{self, Write},
    path::Path,
    sync::OnceLock,
};

static LANGUAGE: OnceLock<Language> = OnceLock::new();

pub fn set_language(language: Language) {
    let _ = LANGUAGE.set(language);
}

pub fn language() -> Language {
    LANGUAGE.get().copied().unwrap_or_default()
}

pub fn label(key: &str) -> &str {
    language().label(key).unwrap_or(key)
}

fn format_bytes(value: u64) -> String {
    if language() == Language::En {
        pcagent_shared::format_bytes(value)
    } else {
        language().bytes(value)
    }
}

macro_rules! say {
    ($key:literal $(, $name:ident = $value:expr)* $(,)?) => {
        language().fill(
            $key,
            &std::collections::BTreeMap::from([
                $((stringify!($name), $value.to_string())),*
            ]),
        ).unwrap_or_else(|| format!("[{}]", $key))
    };
}

/// Only core-owned diagnostic fields use this lookup. Model/user prose never enters it.
fn diagnostic(text: &str, processes: &[ProcessInstance]) -> String {
    presented(pcagent_diagnostics::phrase(text).as_ref(), text, processes)
}

fn presented(phrase: Option<&Phrase>, fallback: &str, processes: &[ProcessInstance]) -> String {
    let rendered = phrase.and_then(|phrase| {
        let mut phrase = phrase.clone();
        for value in phrase.values.values_mut() {
            if let Quantity::Entity(reference) = value {
                if let Some(process) = processes.iter().find(|p| p.entity_ref == *reference) {
                    *value = Quantity::Process {
                        name: process.executable_name.clone(),
                        pid: process.pid,
                    };
                }
            }
        }
        language().render(&phrase)
    });
    clean(&rendered.unwrap_or_else(|| localize(fallback, processes)))
}

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
        Ok(serde_json::Value::String(s)) => language()
            .label(&format!("word.{s}"))
            .map(str::to_owned)
            .unwrap_or_else(|| s.replace('_', " ")),
        _ => label("cli.unknown").into(),
    }
}

fn metric_label(metric: Metric) -> &'static str {
    if language() == Language::En {
        metric.label()
    } else {
        language().metric(metric).unwrap_or_else(|| metric.label())
    }
}

fn metric_value(metric: Metric, value: u64) -> String {
    if language() == Language::En {
        metric.format_value(value)
    } else {
        language()
            .metric_value(metric, value)
            .unwrap_or_else(|| label("cli.unknown").into())
    }
}

fn classification(value: pcagent_diagnostics::Classification) -> String {
    let code = serde_json::to_value(value).expect("classification");
    language()
        .label(&format!(
            "classification.{}",
            code.as_str().expect("classification code")
        ))
        .unwrap_or_else(|| value.label())
        .to_owned()
}

fn value_text(e: &Evidence) -> String {
    match e.usable_value() {
        Some(v) => metric_value(e.metric, v),
        None if e.status == Status::Ok => label("cli.unknown").into(),
        None => say!("cli.not_measured", reason = words(&e.status)),
    }
}

fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(if language() == Language::Fr {
                '\u{202f}'
            } else {
                ','
            });
        }
        out.push(c);
    }
    out
}

fn seconds(ms: u64) -> String {
    let text = format!("{:.1} s", ms as f64 / 1_000.0);
    if language() == Language::Fr {
        text.replace('.', ",")
    } else {
        text
    }
}

pub fn plural(n: impl Into<u64>, one: &str, many: &str) -> String {
    let n = n.into();
    let form = if n == 1 || (n == 0 && language() == Language::Fr) {
        "one"
    } else {
        "many"
    };
    let key = format!("noun.{}.{}", one.replace(' ', "_"), form);
    let noun = language()
        .label(&key)
        .unwrap_or(if n == 1 { one } else { many });
    format!("{} {noun}", thousands(n))
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
        .map_or_else(|| label("cli.name_unavailable").into(), clean);
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
        writeln!(out, "{}", label("cli.synthetic"))?;
    }
    if !processes_only {
        writeln!(out, "{}", label("cli.system"))?;
        let system = system_rows(s);
        if system.is_empty() {
            writeln!(out, "{}", label("cli.no_measurements"))?;
        } else {
            rows(out, &system)?;
        }
        writeln!(out)?;
    }
    processes(out, s, ranked_by)?;
    writeln!(out)?;
    list(out, label("cli.notes"), &s.limitations)
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
        (whole > 0).then(|| {
            say!(
                "cli.share",
                share = if language() == Language::En {
                    format!("{:.0}%", part as f64 * 100.0 / whole as f64)
                } else {
                    language()
                        .quantity(&Quantity::BasisPoints(
                            pcagent_shared::ratio_basis_points(part, whole).unwrap_or(0),
                        ))
                        .expect("basis points")
                },
                total = of
            )
        })
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
                    label("cli.physical_memory"),
                ),
                Metric::SystemCommitBytes => share(
                    Metric::SystemCommitBytes,
                    Metric::SystemCommitLimitBytes,
                    label("cli.commit_limit"),
                ),
                _ => None,
            };
            (
                metric_label(e.metric).to_owned(),
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
                    metric_value(Metric::GpuEngineBasisPoints, *v)
                )
            })
            .collect();
        if idle > 0 {
            parts.push(say!("cli.idle", count = thousands(idle as u64)));
        }
        if unmeasured > 0 {
            parts.push(say!("cli.unmeasured", count = thousands(unmeasured as u64)));
        }
        rows.push((label("cli.gpu_engines").to_owned(), parts.join(", ")));
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
            parts.push(metric_value(Metric::GpuTemperatureCelsius, v));
        }
        if let (Some(used), Some(total)) = (
            value(Metric::GpuMemoryUsedBytes),
            value(Metric::GpuMemoryTotalBytes),
        ) {
            parts.push(say!(
                "cli.gpu_memory",
                used = format_bytes(used),
                total = format_bytes(total)
            ));
        }
        if let Some(v) = value(Metric::GpuUtilizationBasisPoints) {
            parts.push(say!(
                "cli.busy",
                share = metric_value(Metric::GpuUtilizationBasisPoints, v)
            ));
        }
        if let Some(v) = value(Metric::GpuSmClockMegahertz) {
            parts.push(metric_value(Metric::GpuSmClockMegahertz, v));
        }
        if let Some(v) = value(Metric::GpuPowerMilliwatts) {
            parts.push(metric_value(Metric::GpuPowerMilliwatts, v));
        }
        match value(Metric::GpuSlowdownReasons) {
            Some(0) => parts.push(label("cli.no_slowdown").to_owned()),
            Some(bits) => parts.push(say!(
                "cli.slowdown",
                reasons = metric_value(Metric::GpuSlowdownReasons, bits)
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
            label("cli.no_readings").to_owned()
        } else {
            parts.join(", ")
        };
        rows.push((name, text));
    }
    rows
}

fn processes(out: &mut impl Write, s: &Snapshot, ranked_by: &str) -> io::Result<()> {
    let c = &s.process_coverage;
    let mut coverage = vec![say!(
        "cli.readable",
        readable = thousands(c.readable as u64),
        total = thousands(c.enumerated as u64)
    )];
    if c.permission_denied > 0 {
        coverage.push(say!(
            "cli.denied",
            count = thousands(c.permission_denied as u64)
        ));
    }
    if c.unavailable > 0 {
        coverage.push(say!(
            "cli.unavailable",
            count = thousands(c.unavailable as u64)
        ));
    }
    if c.enumeration_truncated {
        coverage.push(label("cli.truncated").into());
    }
    if s.processes.is_empty() {
        return writeln!(
            out,
            "{}",
            say!(
                "cli.no_processes",
                status = words(&c.status),
                coverage = coverage.join(", ")
            )
        );
    }
    writeln!(
        out,
        "{}",
        say!(
            "cli.processes",
            count = thousands(s.processes.len() as u64),
            resource = ranked_by,
            coverage = coverage.join(", ")
        )
    )?;
    let cell = |p: &ProcessInstance, m: Metric| {
        s.evidence
            .iter()
            .find(|e| e.entity_ref == p.entity_ref && e.metric == m)
            .and_then(Evidence::usable_value)
            .map(|v| metric_value(m, v))
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
        write!(out, "  {:>14}", metric_label(*m))?;
    }
    writeln!(out, "  {}", label("cli.name"))?;
    for p in &s.processes {
        write!(out, "  {:>7}", p.pid)?;
        for m in &columns {
            write!(out, "  {:>14}", cell(p, *m).unwrap_or_else(|| "-".into()))?;
        }
        let name = p
            .executable_name
            .as_deref()
            .map_or_else(|| format!("({})", label("cli.name_unavailable")), clean);
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
            "    {}",
            say!(
                "cli.storage.role",
                role = words(&item.context.role),
                folder = words(&item.context.folder_role),
                basis = words(&item.context.basis)
            )
        )?;
        if let Some(app) = &item.context.application {
            writeln!(
                out,
                "    {}",
                say!("cli.storage.application", name = clean(app.name))
            )?;
        }
        writeln!(
            out,
            "    {}",
            say!("cli.storage.content", status = words(&item.content.status))
        )?;
        for copy in &item.other_copy_paths {
            writeln!(
                out,
                "      {}",
                say!("cli.storage.copy", path = clean(copy))
            )?;
        }
        if item.content.other_copy_count > item.other_copy_paths.len() {
            writeln!(
                out,
                "      {}",
                say!(
                    "cli.storage.copies_shown",
                    shown = item.other_copy_paths.len(),
                    total = item.content.other_copy_count
                )
            )?;
        }
        if let Some(regeneration) = item.context.recovery.regeneration {
            writeln!(
                out,
                "    {}",
                say!(
                    "cli.storage.regeneration",
                    text = clean(&core(regeneration))
                )
            )?;
        }
        if let Some(method) = item.context.recovery.adapter_method {
            writeln!(
                out,
                "    {}",
                say!("cli.storage.adapter", method = words(&method))
            )?;
        }
        writeln!(
            out,
            "    {}",
            say!(
                "cli.storage.recovery",
                text = clean(&core(item.content.recovery_note)).trim_end_matches('.')
            )
        )?;
        for condition in &item.context.conditions {
            writeln!(
                out,
                "    {}",
                say!("cli.storage.condition", text = clean(&core(condition)))
            )?;
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
            "{}",
            say!(
                "cli.storage.comparison",
                groups = comparison.groups.len(),
                files = comparison.checked_refs.len(),
                bytes = format_bytes(comparison.bytes_read),
                status = words(&comparison.status)
            )
        )?;
        for limitation in &comparison.limitations {
            writeln!(out, "  {}", clean(&core(limitation)))?;
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
        "{}",
        say!(
            "cli.storage.scanned",
            root = clean(root),
            duration = seconds(summary.duration_ms),
            files = plural(c.files, "file", "files"),
            folders = plural(c.dirs, "folder", "folders"),
            status = if summary.status == Status::Ok {
                label("cli.storage.complete").to_owned()
            } else {
                words(&summary.status)
            }
        )
    )?;
    let mut gaps: Vec<String> = [
        (c.denied_dirs, "cli.storage.gap.denied_dirs"),
        (c.denied_files, "cli.storage.gap.denied_files"),
        (c.reparse_skipped, "cli.storage.gap.reparse_skipped"),
        (c.placeholders, "cli.storage.gap.placeholders"),
        (
            c.other_volume_skipped,
            "cli.storage.gap.other_volume_skipped",
        ),
        (c.special_skipped, "cli.storage.gap.special_skipped"),
        (c.vanished, "cli.storage.gap.vanished"),
        (c.changed_during_scan, "cli.storage.gap.changed_during_scan"),
        (c.errors, "cli.storage.gap.errors"),
        (c.hard_link_aliases, "cli.storage.gap.hard_link_aliases"),
        (
            c.allocation_unknown_files,
            "cli.storage.gap.allocation_unknown_files",
        ),
    ]
    .into_iter()
    .filter(|(n, _)| *n > 0)
    .map(|(n, key)| format!("{} {}", thousands(n), label(key)))
    .collect();
    if let Some(reason) = &c.truncated {
        gaps.push(say!("cli.storage.truncated", reason = words(reason)));
    }
    if c.cancelled {
        gaps.push(label("cli.storage.cancelled").to_owned());
    }
    let mut facts = Vec::new();
    if let Some(t) = &overview.scanned {
        facts.push((
            label("cli.storage.total").to_owned(),
            say!(
                "cli.storage.total_value",
                logical = format_bytes(t.logical_bytes),
                allocated = format_bytes(t.allocated_bytes)
            ),
        ));
    }
    if let (Some(free), Some(total)) = (overview.volume.free_bytes, overview.volume.total_bytes) {
        facts.push((
            label("cli.storage.volume").to_owned(),
            say!(
                "cli.storage.volume_value",
                free = format_bytes(free),
                total = format_bytes(total)
            ),
        ));
    }
    if !gaps.is_empty() {
        facts.push((label("cli.storage.coverage").to_owned(), gaps.join(", ")));
    }
    rows(out, &facts)?;
    writeln!(out)?;
    if largest.items.is_empty() {
        writeln!(
            out,
            "{}",
            label(if dirs {
                "cli.storage.none_dirs"
            } else {
                "cli.storage.none_files"
            })
        )?;
    } else {
        writeln!(
            out,
            "{}",
            label(if dirs {
                "cli.storage.largest_dirs"
            } else {
                "cli.storage.largest_files"
            })
        )?;
        writeln!(
            out,
            "  {:>11}  {:>11}  {}",
            label("cli.storage.size"),
            label("cli.storage.on_disk"),
            label("cli.storage.path")
        )?;
        let prefix = format!("{}\\", root.trim_end_matches('\\'));
        for item in &largest.items {
            let path = item.path.as_deref().unwrap_or(&item.redacted_path);
            let shown = path.strip_prefix(&prefix).unwrap_or(path);
            writeln!(
                out,
                "  {:>11}  {:>11}  {}",
                format_bytes(item.logical_bytes),
                item.allocated_bytes
                    .map_or_else(|| label("cli.unknown").to_owned(), format_bytes),
                clean(shown)
            )?;
        }
        if largest.total > largest.items.len() {
            writeln!(
                out,
                "  {}",
                say!(
                    "cli.storage.more",
                    count = thousands((largest.total - largest.items.len()) as u64)
                )
            )?;
        }
    }
    writeln!(out)?;
    let notes: Vec<String> = largest.limitations.iter().map(|l| core(l)).collect();
    list(out, label("cli.notes"), &notes)
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
    if language() == Language::Fr {
        if let Ok(serde_json::Value::String(code)) = serde_json::to_value(kind) {
            if let Some(text) = language().label(&format!("issue.{code}")) {
                return text.to_owned();
            }
        }
    }
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

pub fn agent_error(error: &pcagent_agent::AgentError) -> String {
    if language() == Language::En {
        return error.to_string();
    }
    match serde_json::to_value(error) {
        Ok(serde_json::Value::String(code)) => language()
            .label(&format!("error.{code}"))
            .unwrap_or_else(|| label("failure.unknown"))
            .to_owned(),
        Ok(serde_json::Value::Object(value)) if value.contains_key("http") => {
            format!("{} (HTTP {})", label("error.http"), value["http"])
        }
        _ => label("failure.unknown").to_owned(),
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
        writeln!(out, "{}", label("cli.unverified"))?;
        for issue in &a.unverified {
            writeln!(
                out,
                "{}",
                say!(
                    "cli.quoted_issue",
                    excerpt = clean(&issue.excerpt),
                    reason = issue_text(issue.kind)
                )
            )?;
        }
    }
    if !a.citations.is_empty() {
        writeln!(out, "{}", label("cli.sources"))?;
    }
    for citation in &a.citations {
        if let Some(old) = r
            .historical_observations
            .iter()
            .find(|o| o.reference == citation.target)
        {
            writeln!(
                out,
                "{}",
                say!(
                    "cli.source_time",
                    reference = citation.reference,
                    tool = clean(&old.tool),
                    time = old.measured_at_unix_ms
                )
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
                        classification(f.classification),
                        diagnostic(&f.summary, &r.local_processes),
                        if earlier.is_some() {
                            label("cli.earlier")
                        } else {
                            ""
                        }
                    )?;
                }
            }
        }
    }
    let follow_ups: Vec<String> = a.follow_ups.iter().map(|f| clean(f)).collect();
    list(out, label("cli.followups"), &follow_ups)
}

/// Observations listed when a report without an answer falls back on everything usable.
const MAX_SHOWN_OBSERVATIONS: usize = 6;

/// A local analysis, or an investigation that ended without an answer: the selected local
/// finding with its evidence, hypothesis and limits.
fn findings(out: &mut impl Write, r: &Investigation, verbose: bool) -> io::Result<()> {
    let f = &r.selected;
    writeln!(out, "{}", classification(f.classification))?;
    writeln!(out, "{}", diagnostic(&f.summary, &r.local_processes))?;
    writeln!(
        out,
        "{}",
        say!(
            "cli.support",
            level = words(&f.support),
            reason = diagnostic(&f.support_reason, &r.local_processes)
        )
    )?;
    writeln!(out)?;
    let facts: Vec<String> = f
        .facts
        .iter()
        .map(|fact| {
            let text = presented(fact.phrase.as_ref(), &fact.text, &r.local_processes);
            if verbose && !fact.evidence_refs.is_empty() {
                format!("{text} [{}]", fact.evidence_refs.join(", "))
            } else {
                text
            }
        })
        .collect();
    list(out, label("cli.evidence"), &facts)?;
    if let Some(h) = &f.hypothesis {
        writeln!(
            out,
            "{}",
            say!("cli.hypothesis", text = diagnostic(h, &r.local_processes))
        )?;
    }
    let counter: Vec<String> = f
        .counter_evidence
        .iter()
        .enumerate()
        .map(|(i, c)| presented(f.counter_phrases.get(i), c, &r.local_processes))
        .collect();
    list(out, label("cli.counter"), &counter)?;
    if !f.missing.is_empty() {
        writeln!(
            out,
            "{}",
            say!(
                "cli.missing",
                text = f
                    .missing
                    .iter()
                    .map(|text| diagnostic(text, &r.local_processes))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        )?;
    }
    let alternatives: Vec<String> = r
        .alternatives
        .iter()
        .map(|a| {
            format!(
                "{}: {}",
                classification(a.classification),
                diagnostic(&a.summary, &r.local_processes)
            )
        })
        .collect();
    list(out, label("cli.alternatives"), &alternatives)?;
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
            "{}",
            say!(
                "cli.observation",
                tool = clean(&observation.tool),
                id = clean(&observation.observation_id),
                status = words(&observation.status)
            )
        )?;
        let facts: Vec<_> = observation.facts.iter().map(|f| clean(f)).collect();
        list(out, label("cli.facts"), &facts)?;
        storage_context(out, &observation.storage_items)?;
        let limits: Vec<_> = observation.limitations.iter().map(|l| clean(l)).collect();
        list(out, label("cli.limits"), &limits)?;
    }
    if let Some(request) = &r.continuation {
        writeln!(
            out,
            "{}",
            say!(
                "cli.budget",
                reason = words(&request.reason),
                next = if request.available {
                    label("cli.budget_continue")
                } else {
                    label("cli.budget_stop")
                }
            )
        )?;
    }
    writeln!(
        out,
        "{}",
        say!(
            "cli.next",
            text = diagnostic(&f.next_step, &r.local_processes)
        )
    )?;
    writeln!(out)?;
    list(
        out,
        label("cli.limitations"),
        &f.limitations
            .iter()
            .map(|text| diagnostic(text, &r.local_processes))
            .collect::<Vec<_>>(),
    )?;
    let notes: Vec<&String> = r
        .limitations
        .iter()
        .filter(|l| !f.limitations.contains(l))
        .collect();
    if verbose {
        list(out, label("cli.collection"), &notes)?;
    } else if !notes.is_empty() {
        writeln!(
            out,
            "{}",
            say!(
                "cli.collection_hidden",
                count = thousands(notes.len() as u64)
            )
        )?;
    }
    Ok(())
}

pub fn report(out: &mut impl Write, r: &Investigation, verbose: bool) -> io::Result<()> {
    match r.mode.as_str() {
        "local_rules" => writeln!(out, "{}", label("cli.local"))?,
        "nebius" => writeln!(out, "{}", label("cli.cloud"))?,
        other => writeln!(out, "{}", say!("cli.mode", mode = clean(other)))?,
    }
    let support = match &r.answer {
        Some(a) => say!("cli.confidence", value = words(&a.confidence)),
        None => say!(
            "cli.diagnostic_support",
            value = words(&r.diagnostic_support)
        ),
    };
    writeln!(
        out,
        "{}",
        say!(
            "cli.execution",
            execution = words(&r.execution_status),
            coverage = words(&r.coverage.status),
            support = support
        ),
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
            list(out, label("cli.coverage_missing"), &domain.missing)?;
        }
    }
    if let Some(error) = &r.error {
        writeln!(out, "{}", say!("cli.failed", error = agent_error(error)))?;
    }
    writeln!(out)?;
    match &r.answer {
        Some(a) => answer(out, r, a)?,
        None => findings(out, r, verbose)?,
    }
    if r.mode == "local_rules" {
        writeln!(out, "{}", label("cli.cloud_hint"))?;
    } else {
        let s = &r.stats;
        let tokens = match (s.total_tokens, s.prompt_tokens, s.completion_tokens) {
            (Some(total), Some(input), Some(output)) => say!(
                "cli.tokens",
                total = thousands(total),
                input = thousands(input),
                output = thousands(output)
            ),
            (Some(total), _, _) => say!("cli.tokens_total", total = thousands(total)),
            _ if s.unknown_requests > 0
                && s.known_prompt_tokens + s.known_completion_tokens > 0 =>
            {
                say!(
                    "cli.tokens_lower",
                    total = thousands(s.known_prompt_tokens + s.known_completion_tokens),
                    input = thousands(s.known_prompt_tokens),
                    output = thousands(s.known_completion_tokens),
                    requests = plural(s.unknown_requests as u64, "request", "requests")
                )
            }
            _ => label("cli.tokens_unknown").to_owned(),
        };
        writeln!(
            out,
            "{}",
            say!(
                "cli.model_usage",
                requests = plural(s.inference_calls as u64, "request", "requests"),
                tools = plural(s.tool_calls as u64, "tool call", "tool calls"),
                tokens = tokens
            ),
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
                        label("cli.cost_prefix"),
                        label("cli.cost_lower_prefix"),
                        1
                    )
                )?,
                (cost, _) => writeln!(out, "{}", cost_line(cost))?,
            }
        }
        if let Some(web_cost) = web_cost_line(s) {
            writeln!(out, "{web_cost}")?;
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
    let from_answer = shown(&say!("cli.answer_author", from = from));
    Some(match (taken, routing.last_request_error.as_ref()) {
        (0, _) => say!(
            "cli.model_change_pending",
            to = to,
            from = from,
            after = after,
            reason = reason,
            answer = from_answer
        ),
        (1, Some(error)) => say!(
            "cli.model_change_failed",
            to = to,
            from = from,
            after = after,
            reason = reason,
            error = agent_error(error),
            answer = from_answer
        ),
        (_, Some(error)) => say!(
            "cli.model_change_last_failed",
            to = to,
            from = from,
            after = after,
            reason = reason,
            error = agent_error(error),
            answer = shown(label("cli.answer_before_failure"))
        ),
        (_, None) => say!(
            "cli.model_change",
            to = to,
            from = from,
            after = after,
            reason = reason
        ),
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
        (Some(input), Some(output)) => say!(
            "cli.tokens",
            total = thousands(input.saturating_add(output)),
            input = thousands(input),
            output = thousands(output)
        ),
        _ => label("cli.tokens_unknown").to_owned(),
    };
    let priced: Vec<&Cost> = requests.iter().filter_map(|r| r.cost.as_ref()).collect();
    let price = priced
        .first()
        .filter(|first| priced.iter().all(|c| c.pricing == first.pricing))
        .map(|c| say!("cli.price_at", price = price_text(&c.pricing)))
        .unwrap_or_default();
    let cost = match (priced.len() == requests.len())
        .then(|| sum_pico(&priced))
        .flatten()
    {
        Some(pico) => say!("cli.model_cost", amount = usd_text(pico), price = price),
        None => say!(
            "cli.model_cost_unknown",
            requests = plural(
                (requests.len() - priced.len()) as u64,
                "request",
                "requests"
            )
        ),
    };
    format!(
        "{}: {}; {usage}; {cost}",
        model_id.map_or_else(|| label("cli.unknown_model").to_owned(), clean),
        plural(requests.len() as u64, "request", "requests")
    )
}

/// The total of a turn served by several models, only when every request is priced.
fn web_cost_line(s: &Stats) -> Option<String> {
    if s.web_usage.lookups > 0 {
        let web = &s.web_usage;
        let cost = web
            .estimated_usd()
            .map_or_else(|| label("cli.unknown").to_owned(), |usd| format!("${usd}"));
        let total = web
            .with_model_usd(s.total_cost_usd.as_deref())
            .map_or_else(|| label("cli.unknown").to_owned(), |usd| format!("${usd}"));
        return Some(say!(
            "cli.cost.web",
            total = total,
            credits = web.known_credits,
            hits = web.cache_hits,
            unknown = web.unknown_requests,
            cost = cost
        ));
    }
    None
}

fn total_cost_line(s: &Stats) -> String {
    if let Some(total) = s.total_cost_usd.as_deref().and_then(pico) {
        return say!("cli.cost_total", amount = usd_text(total));
    }
    let priced: Vec<&Cost> = s
        .model_requests
        .iter()
        .filter_map(|r| r.cost.as_ref())
        .collect();
    match sum_pico(&priced).filter(|_| !priced.is_empty()) {
        Some(lower) => say!("cli.cost_lower", amount = usd_text(lower)),
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
        return label("cli.cost_unknown").to_owned();
    };
    let rounded = cost.rounded_usd(4);
    let amount = if rounded == "0.0000" && cost.usd != "0" {
        cost.usd.clone()
    } else {
        rounded
    };
    say!(
        "cli.cost",
        amount = amount,
        price = price_text(&cost.pricing)
    )
}

pub fn price_text(pricing: &Pricing) -> String {
    let rates = pricing.per_million_usd().map_or_else(
        || label("cli.rates_unknown").to_owned(),
        |(input, output)| say!("cli.rates", input = input, output = output),
    );
    say!(
        "cli.catalog_prices",
        rates = rates,
        date = format_utc_minute(pricing.read_at_unix_ms)
    )
}

pub fn verification(out: &mut impl Write, v: &ActionVerification) -> io::Result<()> {
    writeln!(
        out,
        "{}",
        say!("cli.verification", samples = VERIFY_SAMPLES)
    )?;
    for c in &v.comparisons {
        let median = |w: &Option<pcagent_diagnostics::WindowSummary>| {
            w.as_ref().map_or_else(
                || label("cli.unknown").into(),
                |w| metric_value(c.metric, w.median),
            )
        };
        writeln!(
            out,
            "{}",
            say!(
                "cli.comparison",
                metric = metric_label(c.metric),
                outcome = words(&c.outcome),
                before = median(&c.before),
                after = median(&c.after)
            )
        )?;
    }
    if let Some(vol) = &v.volume {
        let show = |b: Option<u64>| b.map_or_else(|| label("cli.unknown").into(), format_bytes);
        // `gained_bytes` is absent both when a side is unknown and when free space did not grow.
        let gained = match (
            vol.gained_bytes,
            vol.free_before_bytes,
            vol.free_after_bytes,
        ) {
            (Some(g), _, _) => format_bytes(g),
            (None, Some(_), Some(_)) => label("cli.none").into(),
            _ => label("cli.unknown").into(),
        };
        writeln!(
            out,
            "{}",
            say!(
                "cli.space",
                before = show(vol.free_before_bytes),
                after = show(vol.free_after_bytes),
                gained = gained,
                removed = show(vol.reported_removed_bytes)
            )
        )?;
    }
    for l in &v.limitations {
        writeln!(out, "{}", say!("cli.limitation", text = l))?;
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

/// Human prompts and notices of the interactive commands, in the chosen language.
pub fn extension_prompt(
    rounds: impl ToString,
    tools: impl ToString,
    seconds: impl ToString,
) -> String {
    say!(
        "cli.extension.prompt",
        rounds = rounds.to_string(),
        tools = tools.to_string(),
        seconds = seconds.to_string()
    )
}

/// `yes` in English, and also `oui` in French; anything else declines.
pub fn is_yes(answer: &str) -> bool {
    let answer = answer.trim();
    answer.eq_ignore_ascii_case("yes")
        || (language() == Language::Fr && answer.eq_ignore_ascii_case("oui"))
}

pub fn question_too_long(limit: usize) -> String {
    say!("cli.question.too_long", limit = limit)
}

pub fn question_empty() -> String {
    label("cli.question.empty").to_owned()
}

/// A catalogue sentence with named values, for modules outside this one (which cannot use
/// `say!`). An unknown key shows itself rather than inventing text.
pub fn fill(key: &str, values: &[(&str, String)]) -> String {
    language()
        .fill(key, &values.iter().cloned().collect())
        .unwrap_or_else(|| format!("[{key}]"))
}

/// Closed English sentences of the storage, policy and storage-context cores, rendered in the
/// chosen language by exact match. Any other text is shown unchanged, never guessed.
const CORE_TEXT: [&str; 45] = [
    "core.text.00",
    "core.text.01",
    "core.text.02",
    "core.text.03",
    "core.text.04",
    "core.text.05",
    "core.text.06",
    "core.text.07",
    "core.text.08",
    "core.text.09",
    "core.text.10",
    "core.text.11",
    "core.text.12",
    "core.text.13",
    "core.text.14",
    "core.text.15",
    "core.text.16",
    "core.text.17",
    "core.text.18",
    "core.text.19",
    "core.text.20",
    "core.text.21",
    "core.text.22",
    "core.text.23",
    "core.text.24",
    "core.text.25",
    "core.text.26",
    "core.text.27",
    "core.text.28",
    "core.text.29",
    "core.text.30",
    "core.text.31",
    "core.text.32",
    "core.text.33",
    "core.text.34",
    "core.text.35",
    "core.text.36",
    "core.text.37",
    "core.text.38",
    "core.text.39",
    "core.text.40",
    "core.text.41",
    "core.text.42",
    "core.text.43",
    "core.text.44",
];

pub fn core(text: &str) -> String {
    if language() == Language::En {
        return text.to_owned();
    }
    CORE_TEXT
        .iter()
        .find(|key| Language::En.label(key) == Some(text))
        .and_then(|key| language().label(key))
        .map_or_else(|| text.to_owned(), str::to_owned)
}

#[cfg(test)]
mod core_text_tests {
    use super::*;

    #[test]
    fn every_closed_core_sentence_has_an_english_source_and_a_french_rendering() {
        for key in CORE_TEXT {
            let english = Language::En.label(key).expect(key);
            let french = Language::Fr.label(key).expect(key);
            assert_ne!(english, french, "{key}");
            assert!(!french.is_empty());
        }
    }

    #[test]
    fn closed_status_words_have_both_languages() {
        for code in [
            "unclassified",
            "not_checked",
            "not_applicable_to_folders",
            "same_content_sha256",
            "no_match_in_checked_set",
            "registered_path_pattern",
            "local_path_rule_not_content_validation",
            "temporary",
            "personal_data",
        ] {
            let key = format!("word.{code}");
            assert!(Language::En.label(&key).is_some(), "{key}");
            assert!(Language::Fr.label(&key).is_some(), "{key}");
        }
    }

    #[test]
    fn unknown_text_is_never_rewritten() {
        assert_eq!(core("A sentence from a model"), "A sentence from a model");
    }
}
