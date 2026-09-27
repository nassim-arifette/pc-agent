//! Plain data for the desktop interface, built from the core's own types. Every number keeps its
//! raw value next to the text the core formats; a missing measurement carries its status and
//! reason and is never shown as zero. This is a local window, so local names (process names,
//! full paths) may appear here; nothing in this module is sent to a provider.
use pcagent_agent::{
    credentials::{Credentials, Source},
    AgentError, Config, Cost, ExecutionStatus, ExtensionRequest, Investigation, Pricing,
    SensorCoverage, Stats,
};
use pcagent_diagnostics::{Classification, Finding, Support};
use pcagent_shared::{
    format_bytes, gpu_slowdown_reasons, unix_ms, Evidence, Metric, ProcessCoverage,
    ProcessInstance, Snapshot, Status,
};
use pcagent_storage::{
    Coverage, ItemKind, ItemView, LargestItemsPage, ScanProgress, ScanSummary, StorageOverview,
    Totals,
};
use serde::Serialize;
use serde_json::Value;

/// Models the settings screen offers, with what the 2026-09-27 comparison measured
/// (docs/model-comparison.md). Any other `nvidia/` Nemotron ID can still be typed.
pub const MODELS: [(&str, &str); 3] = [
    ("nvidia/nemotron-3-super-120b-a12b", "recommended"),
    ("nvidia/NVIDIA-Nemotron-3-Nano-30B-A3B", "cheaper_slower"),
    ("nvidia/Nemotron-3_5-Lightning", "not_recommended"),
];

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measure {
    /// Raw value in the metric's unit; `None` unless the measurement is usable.
    pub value: Option<u64>,
    pub text: Option<String>,
    /// `None` when the metric was not collected at all.
    pub status: Option<Status>,
    /// Why the value is missing, when it is.
    pub note: Option<String>,
}
impl Measure {
    fn of(evidence: Option<&Evidence>) -> Self {
        let Some(e) = evidence else {
            return Self {
                value: None,
                text: None,
                status: None,
                note: None,
            };
        };
        match e.usable_value() {
            Some(v) => Self {
                value: Some(v),
                text: Some(e.metric.format_value(v)),
                status: Some(e.status),
                note: None,
            },
            None => Self {
                value: None,
                text: None,
                status: Some(e.status),
                note: e.limitations.first().cloned(),
            },
        }
    }
}
fn find<'a>(s: &'a Snapshot, entity: &str, metric: Metric) -> Option<&'a Evidence> {
    s.evidence
        .iter()
        .find(|e| e.entity_ref == entity && e.metric == metric)
}
fn measure(s: &Snapshot, entity: &str, metric: Metric) -> Measure {
    Measure::of(find(s, entity, metric))
}
/// `part / whole` in 0..=1 when both are usable and `whole` is positive.
fn ratio(part: Option<u64>, whole: Option<u64>) -> Option<f64> {
    match (part, whole) {
        (Some(p), Some(w)) if w > 0 => Some((p as f64 / w as f64).clamp(0.0, 1.0)),
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Memory {
    pub total: Measure,
    pub available: Measure,
    pub commit: Measure,
    pub commit_limit: Measure,
    /// Share of physical memory not available, when both values are measured.
    pub used_ratio: Option<f64>,
    pub used_text: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Cpu {
    pub busy: Measure,
    pub busiest_core: Measure,
}
#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    pub read_latency: Measure,
    pub write_latency: Measure,
    pub idle: Measure,
    pub queue: Measure,
    pub hard_faults: Measure,
    /// Share of the window during which the disks were busy (the complement of idle time).
    pub busy_ratio: Option<f64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Gpu {
    pub name: String,
    pub temperature: Measure,
    pub memory_used: Measure,
    pub memory_total: Measure,
    pub memory_ratio: Option<f64>,
    pub utilization: Measure,
    pub sm_clock: Measure,
    pub power: Measure,
    pub slowdown: Measure,
    pub slowdown_reasons: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Engine {
    pub kind: String,
    pub usage: Measure,
}
#[derive(Debug, Clone, Serialize)]
pub struct ProcessRow {
    pub name: String,
    pub pid: u32,
    pub working_set: Measure,
    pub private_commit: Measure,
    pub cpu: Measure,
}
#[derive(Debug, Clone, Serialize)]
pub struct Dashboard {
    pub collected_at_unix_ms: u64,
    pub memory: Memory,
    pub cpu: Cpu,
    pub disk: Disk,
    pub gpus: Vec<Gpu>,
    pub gpu_engines: Vec<Engine>,
    pub gpu_busiest_engine: Measure,
    pub processes: Vec<ProcessRow>,
    pub process_coverage: ProcessCoverage,
    pub limitations: Vec<String>,
}

pub fn process_name(p: &ProcessInstance) -> String {
    p.executable_name
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("Unnamed process")
        .to_owned()
}

pub fn dashboard(s: &Snapshot) -> Dashboard {
    let sys = |metric| measure(s, "system", metric);
    let (total, available) = (
        sys(Metric::PhysicalTotalBytes),
        sys(Metric::PhysicalAvailableBytes),
    );
    let used = total
        .value
        .zip(available.value)
        .and_then(|(t, a)| t.checked_sub(a));
    let idle = sys(Metric::DiskIdleBasisPoints);
    let gpus = s
        .gpus
        .iter()
        .map(|g| {
            let m = |metric| measure(s, &g.entity_ref, metric);
            let (used, total, slowdown) = (
                m(Metric::GpuMemoryUsedBytes),
                m(Metric::GpuMemoryTotalBytes),
                m(Metric::GpuSlowdownReasons),
            );
            Gpu {
                name: g.name.clone().unwrap_or_else(|| {
                    format!(
                        "NVIDIA GPU {}",
                        g.entity_ref.trim_start_matches("gpu-nvidia-")
                    )
                }),
                memory_ratio: ratio(used.value, total.value),
                slowdown_reasons: slowdown
                    .value
                    .map(|bits| {
                        gpu_slowdown_reasons(bits)
                            .into_iter()
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                temperature: m(Metric::GpuTemperatureCelsius),
                utilization: m(Metric::GpuUtilizationBasisPoints),
                sm_clock: m(Metric::GpuSmClockMegahertz),
                power: m(Metric::GpuPowerMilliwatts),
                memory_used: used,
                memory_total: total,
                slowdown,
            }
        })
        .collect();
    let mut gpu_engines: Vec<Engine> = s
        .evidence
        .iter()
        .filter(|e| e.metric == Metric::GpuEngineBasisPoints)
        .filter_map(|e| {
            Some(Engine {
                kind: e.entity_ref.strip_prefix("gpu-engine-")?.to_owned(),
                usage: Measure::of(Some(e)),
            })
        })
        .collect();
    gpu_engines.sort_by(|a, b| b.usage.value.cmp(&a.usage.value));
    let processes = s
        .processes
        .iter()
        .map(|p| {
            let m = |metric| measure(s, &p.entity_ref, metric);
            ProcessRow {
                name: process_name(p),
                pid: p.pid,
                working_set: m(Metric::ProcessWorkingSetBytes),
                private_commit: m(Metric::ProcessPrivateCommitBytes),
                cpu: m(Metric::ProcessCpuBasisPoints),
            }
        })
        .collect();
    Dashboard {
        collected_at_unix_ms: unix_ms(),
        memory: Memory {
            used_ratio: ratio(used, total.value),
            used_text: used.map(format_bytes),
            total,
            available,
            commit: sys(Metric::SystemCommitBytes),
            commit_limit: sys(Metric::SystemCommitLimitBytes),
        },
        cpu: Cpu {
            busy: sys(Metric::CpuBusyBasisPoints),
            busiest_core: sys(Metric::CpuBusiestCoreBasisPoints),
        },
        disk: Disk {
            busy_ratio: idle
                .value
                .map(|v| (10_000_u64.saturating_sub(v)) as f64 / 10_000.0),
            read_latency: sys(Metric::DiskReadLatencyMicroseconds),
            write_latency: sys(Metric::DiskWriteLatencyMicroseconds),
            idle,
            queue: sys(Metric::DiskQueueLength),
            hard_faults: sys(Metric::HardFaultPagesPerSecondX100),
        },
        gpus,
        gpu_engines,
        gpu_busiest_engine: sys(Metric::GpuBusiestEngineBasisPoints),
        processes,
        process_coverage: s.process_coverage.clone(),
        limitations: s.limitations.clone(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FindingView {
    pub classification: Classification,
    pub label: &'static str,
    pub support: Support,
    pub support_reason: String,
    pub summary: String,
    pub facts: Vec<String>,
    pub hypothesis: Option<String>,
    pub counter_evidence: Vec<String>,
    pub missing: Vec<String>,
    pub next_step: String,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ObservationView {
    pub tool: String,
    pub status: Status,
    pub facts: Vec<String>,
    pub limitations: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct PriceView {
    pub model_id: String,
    pub input_per_million: Option<String>,
    pub output_per_million: Option<String>,
    pub read_at_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct CostView {
    /// Rounded to four decimals, or exact when that would show zero.
    pub usd: String,
    pub exact_usd: String,
    pub price: PriceView,
}
#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub requests: usize,
    pub tool_calls: usize,
    pub extensions: usize,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    pub cost: Option<CostView>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ErrorView {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub question: String,
    pub mode: String,
    pub execution: ExecutionStatus,
    pub coverage: SensorCoverage,
    pub support: Support,
    pub error: Option<ErrorView>,
    pub selected: FindingView,
    pub alternatives: Vec<FindingView>,
    /// Observations the model cited, or every usable one when the investigation stopped early.
    pub observations: Vec<ObservationView>,
    pub limitations: Vec<String>,
    /// Absent for a local analysis, which makes no provider request.
    pub usage: Option<Usage>,
    pub continuation: Option<ExtensionRequest>,
}

/// Replaces opaque process references (`proc-1a2b3c4d`) with local names for this window.
fn localize(text: &str, processes: &[ProcessInstance]) -> String {
    processes.iter().fold(text.to_owned(), |text, p| {
        text.replace(
            &p.entity_ref,
            &format!("{} (PID {})", process_name(p), p.pid),
        )
    })
}
pub fn finding(f: &Finding, processes: &[ProcessInstance]) -> FindingView {
    let local = |t: &str| localize(t, processes);
    let all = |items: &[String]| items.iter().map(|t| local(t)).collect();
    FindingView {
        classification: f.classification,
        label: f.classification.label(),
        support: f.support,
        support_reason: local(&f.support_reason),
        summary: local(&f.summary),
        facts: f.facts.iter().map(|fact| local(&fact.text)).collect(),
        hypothesis: f.hypothesis.as_deref().map(local),
        counter_evidence: all(&f.counter_evidence),
        missing: all(&f.missing),
        next_step: local(&f.next_step),
        limitations: all(&f.limitations),
    }
}
pub fn error_code(e: &AgentError) -> String {
    match serde_json::to_value(e) {
        Ok(Value::String(code)) => code,
        Ok(Value::Object(map)) => map.keys().next().cloned().unwrap_or_default(),
        _ => "error".to_owned(),
    }
}
pub fn price(p: &Pricing) -> PriceView {
    let rates = p.per_million_usd();
    PriceView {
        model_id: p.model_id.clone(),
        input_per_million: rates.as_ref().map(|(input, _)| input.clone()),
        output_per_million: rates.map(|(_, output)| output),
        read_at_unix_ms: p.read_at_unix_ms,
    }
}
fn cost(c: &Cost) -> CostView {
    let rounded = c.rounded_usd(4);
    CostView {
        usd: if rounded == "0.0000" && c.usd != "0" {
            c.usd.clone()
        } else {
            rounded
        },
        exact_usd: c.usd.clone(),
        price: price(&c.pricing),
    }
}
pub fn usage(s: &Stats) -> Usage {
    Usage {
        requests: s.inference_calls,
        tool_calls: s.tool_calls,
        extensions: s.extensions,
        prompt_tokens: s.prompt_tokens,
        completion_tokens: s.completion_tokens,
        reasoning_tokens: s.reasoning_tokens,
        cost: s.cost.as_ref().map(cost),
    }
}
pub fn report(r: &Investigation, question: &str) -> Report {
    let processes = &r.local_processes;
    let observations = r
        .observations
        .iter()
        .filter(|o| {
            r.observation_refs.contains(&o.observation_id) || (r.error.is_some() && o.citable)
        })
        .map(|o| ObservationView {
            tool: o.tool.clone(),
            status: o.status,
            facts: o.facts.iter().map(|f| localize(f, processes)).collect(),
            limitations: o.limitations.clone(),
        })
        .collect();
    Report {
        question: question.to_owned(),
        mode: r.mode.clone(),
        execution: r.execution_status,
        coverage: r.coverage.clone(),
        support: r.diagnostic_support,
        error: r.error.as_ref().map(|e| ErrorView {
            code: error_code(e),
            message: e.to_string(),
        }),
        selected: finding(&r.selected, processes),
        alternatives: r
            .alternatives
            .iter()
            .map(|f| finding(f, processes))
            .collect(),
        observations,
        limitations: r.limitations.clone(),
        usage: (r.mode != "local_rules").then(|| usage(&r.stats)),
        continuation: r.continuation.clone(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub item_ref: String,
    pub name: String,
    /// Folder that contains the item, relative to the scanned folder ("" at its top level).
    pub parent: String,
    pub path: String,
    pub logical_bytes: u64,
    pub allocated_bytes: Option<u64>,
    /// Size on disk when known, otherwise the logical size; this is how items are ranked.
    pub size: u64,
    pub size_text: String,
    pub files: Option<u64>,
    pub modified_unix_ms: Option<u64>,
    pub status: Status,
}
#[derive(Debug, Clone, Serialize)]
pub struct Page {
    pub kind: ItemKind,
    pub items: Vec<Item>,
    pub total: usize,
    pub next_offset: Option<usize>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub total_text: Option<String>,
    pub free_text: Option<String>,
    pub filesystem: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct StorageView {
    pub scope_id: String,
    pub root: String,
    pub status: Status,
    pub duration_ms: u64,
    pub scanned: Option<Totals>,
    pub scanned_text: Option<String>,
    pub volume: Volume,
    pub coverage: Coverage,
    pub denied: Vec<String>,
    pub dirs: Page,
    pub files: Page,
    pub limitations: Vec<String>,
}
fn item(i: &ItemView, root: &str) -> Item {
    let path = i.path.clone().unwrap_or_default();
    let trimmed = path.trim_end_matches('\\');
    let (folder, name) = trimmed.rsplit_once('\\').unwrap_or(("", trimmed));
    let parent = folder
        .strip_prefix(root.trim_end_matches('\\'))
        .map(|rest| rest.trim_start_matches('\\').to_owned())
        .unwrap_or_else(|| folder.to_owned());
    let size = i.allocated_bytes.unwrap_or(i.logical_bytes);
    Item {
        item_ref: i.item_ref.clone(),
        name: if name.is_empty() {
            path.clone()
        } else {
            name.to_owned()
        },
        parent,
        path: path.clone(),
        logical_bytes: i.logical_bytes,
        allocated_bytes: i.allocated_bytes,
        size,
        size_text: format_bytes(size),
        files: i.files,
        modified_unix_ms: i.modified_unix_ms,
        status: i.status,
    }
}
pub fn page(p: &LargestItemsPage, root: &str) -> Page {
    Page {
        kind: p.kind,
        items: p.items.iter().map(|i| item(i, root)).collect(),
        total: p.total,
        next_offset: p.next_offset,
    }
}
pub fn storage(
    root: &str,
    summary: &ScanSummary,
    overview: &StorageOverview,
    dirs: &LargestItemsPage,
    files: &LargestItemsPage,
) -> StorageView {
    let v = &overview.volume;
    StorageView {
        scope_id: summary.scope_id.clone(),
        root: root.to_owned(),
        status: summary.status,
        duration_ms: summary.duration_ms,
        scanned_text: overview
            .scanned
            .as_ref()
            .map(|t| format_bytes(t.allocated_bytes.max(t.logical_bytes))),
        scanned: overview.scanned.clone(),
        volume: Volume {
            total_bytes: v.total_bytes,
            free_bytes: v.free_bytes,
            total_text: v.total_bytes.map(format_bytes),
            free_text: v.free_bytes.map(format_bytes),
            filesystem: v.filesystem.clone(),
        },
        coverage: summary.coverage.clone(),
        denied: overview
            .denied
            .iter()
            .filter_map(|d| d.path.clone())
            .collect(),
        dirs: page(dirs, root),
        files: page(files, root),
        limitations: overview.limitations.clone(),
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub scope_id: String,
    pub entries: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub bytes_text: String,
    pub elapsed_ms: u64,
}
pub fn progress(scope_id: &str, p: &ScanProgress) -> Progress {
    Progress {
        scope_id: scope_id.to_owned(),
        entries: p.entries_indexed,
        dirs: p.dirs_enumerated,
        bytes: p.logical_bytes_seen,
        bytes_text: format_bytes(p.logical_bytes_seen),
        elapsed_ms: p.elapsed_ms,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud {
    pub key_configured: bool,
    /// The last four characters, so a person can tell keys apart; never the key itself.
    pub key_hint: Option<String>,
    pub key_source: Option<&'static str>,
    pub model_id: Option<String>,
    pub model_source: Option<&'static str>,
    /// Both settings are present and the model ID is an accepted Nemotron ID.
    pub ready: bool,
    pub problem: Option<String>,
    pub file: Option<String>,
    pub file_problem: Option<String>,
}
fn source_name(s: Option<Source>) -> Option<&'static str> {
    s.map(|s| match s {
        Source::Environment => "environment",
        Source::File => "file",
    })
}
pub fn cloud(creds: &Credentials) -> Cloud {
    let key = creds.get("NEBIUS_API_KEY");
    let checked = Config::from_lookup(|name| creds.get(name));
    Cloud {
        key_configured: key.is_some(),
        key_hint: key
            .filter(|k| k.chars().count() >= 12)
            .map(|k| k.chars().skip(k.chars().count() - 4).collect()),
        key_source: source_name(creds.source("NEBIUS_API_KEY")),
        model_id: creds.get("NEBIUS_MODEL_ID"),
        model_source: source_name(creds.source("NEBIUS_MODEL_ID")),
        ready: checked.is_ok(),
        problem: checked.err().map(|e| e.to_string()),
        file: creds.file().map(|p| p.display().to_string()),
        file_problem: creds.problem.clone(),
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct ModelChoice {
    pub id: &'static str,
    pub note: &'static str,
}
#[derive(Debug, Clone, Serialize)]
pub struct AppInfo {
    pub version: &'static str,
    pub cloud: Cloud,
    pub models: Vec<ModelChoice>,
}
pub fn app_info(creds: &Credentials) -> AppInfo {
    AppInfo {
        version: env!("CARGO_PKG_VERSION"),
        cloud: cloud(creds),
        models: MODELS
            .iter()
            .map(|&(id, note)| ModelChoice { id, note })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{GpuDevice, Window};

    fn fixture() -> Snapshot {
        let mut s = Snapshot::empty("fixture");
        let e = |metric, entity: &str, value| {
            Evidence::measured(metric, entity, value, "fixture", Window::instant())
        };
        s.evidence.extend([
            e(Metric::PhysicalTotalBytes, "system", 16 << 30),
            e(Metric::PhysicalAvailableBytes, "system", 4 << 30),
            e(Metric::DiskIdleBasisPoints, "system", 7_500),
            e(Metric::GpuEngineBasisPoints, "gpu-engine-3d", 1_200),
            e(Metric::GpuEngineBasisPoints, "gpu-engine-copy", 30),
            e(Metric::GpuMemoryUsedBytes, "gpu-nvidia-0", 512 << 20),
            e(Metric::GpuMemoryTotalBytes, "gpu-nvidia-0", 2 << 30),
            e(Metric::GpuSlowdownReasons, "gpu-nvidia-0", 0x20),
            e(Metric::ProcessWorkingSetBytes, "proc-00000001", 3 << 30),
        ]);
        s.evidence.push(Evidence::missing(
            Metric::CpuBusyBasisPoints,
            "system",
            "fixture",
            Status::Partial,
            "First observation; a rate needs two samples.",
        ));
        s.gpus.push(GpuDevice {
            entity_ref: "gpu-nvidia-0".into(),
            name: Some("NVIDIA Fixture".into()),
            source: "NVML".into(),
        });
        s.processes.push(ProcessInstance {
            entity_ref: "proc-00000001".into(),
            pid: 4242,
            creation_time_filetime: 1,
            boot_id: None,
            executable_name: Some("editor.exe".into()),
        });
        s
    }

    #[test]
    fn dashboard_keeps_raw_values_and_never_turns_a_missing_value_into_zero() {
        let d = dashboard(&fixture());
        assert_eq!(d.memory.used_ratio, Some(0.75));
        assert_eq!(d.memory.used_text.as_deref(), Some("12.00 GiB"));
        assert_eq!(d.memory.total.text.as_deref(), Some("16.00 GiB"));
        // CPU was observed once: its status and reason are kept, not a zero.
        assert_eq!(d.cpu.busy.value, None);
        assert_eq!(d.cpu.busy.status, Some(Status::Partial));
        assert!(d.cpu.busy.note.as_deref().unwrap().contains("two samples"));
        // Never collected at all: no status.
        assert_eq!(d.cpu.busiest_core.status, None);
        assert_eq!(d.disk.busy_ratio, Some(0.25));
        assert_eq!(d.gpus[0].name, "NVIDIA Fixture");
        assert_eq!(d.gpus[0].memory_ratio, Some(0.25));
        assert_eq!(d.gpus[0].slowdown_reasons, ["thermal (driver)"]);
        assert_eq!(d.gpus[0].temperature.status, None);
        assert_eq!(d.gpu_engines[0].kind, "3d");
        assert_eq!(d.processes[0].name, "editor.exe");
        assert_eq!(d.processes[0].working_set.value, Some(3 << 30));
    }

    #[test]
    fn a_local_report_localizes_process_references_and_has_no_usage() {
        let mut s = fixture();
        s.evidence.push(Evidence::measured(
            Metric::ProcessPrivateCommitBytes,
            "proc-00000001",
            5 << 30,
            "fixture",
            Window::instant(),
        ));
        let inv = pcagent_agent::local_report(&s);
        let r = report(&inv, "Why is my PC slow?");
        assert_eq!(r.mode, "local_rules");
        assert!(r.usage.is_none());
        assert_eq!(r.question, "Why is my PC slow?");
        let contributor = inv
            .findings
            .iter()
            .find(|f| f.classification == Classification::LargestMemoryContributor)
            .expect("the fixture has a largest memory contributor");
        let text = serde_json::to_string(&finding(contributor, &inv.local_processes)).unwrap();
        assert!(!text.contains("proc-00000001"), "{text}");
        assert!(text.contains("editor.exe (PID 4242)"), "{text}");
    }

    #[test]
    fn storage_items_show_their_folder_relative_to_the_scanned_root() {
        let view = ItemView {
            item_ref: "scan-1:7".into(),
            kind: ItemKind::Files,
            path: Some(r"C:\Users\me\Downloads\iso\disk.img".into()),
            redacted_path: "<scope>/*/*.img".into(),
            logical_bytes: 1 << 30,
            allocated_bytes: None,
            allocation_unknown_files: 1,
            estimated_reclaim_bytes: None,
            files: None,
            link_count: Some(1),
            sparse: false,
            compressed: false,
            modified_unix_ms: Some(1),
            last_used: pcagent_storage::LastUse::Unknown,
            status: Status::Partial,
        };
        let i = item(&view, r"C:\Users\me\Downloads");
        assert_eq!((i.name.as_str(), i.parent.as_str()), ("disk.img", "iso"));
        // Unknown allocation: the logical size ranks and is shown.
        assert_eq!((i.size, i.size_text.as_str()), (1 << 30, "1.00 GiB"));
        let top = item(
            &ItemView {
                path: Some(r"C:\Users\me\Downloads\a.zip".into()),
                allocated_bytes: Some(4096),
                ..view
            },
            r"C:\Users\me\Downloads\",
        );
        assert_eq!((top.name.as_str(), top.parent.as_str()), ("a.zip", ""));
        assert_eq!(top.size, 4096);
    }

    #[test]
    fn cloud_state_shows_a_key_hint_but_never_the_key() {
        let dir = std::env::temp_dir().join(format!("pcagent-gui-{}", std::process::id()));
        let file = dir.join(".env");
        pcagent_agent::credentials::save(
            &file,
            &[
                ("NEBIUS_API_KEY", "v1.secret-key-abcd"),
                ("NEBIUS_MODEL_ID", "nvidia/nemotron-3-super-120b-a12b"),
            ],
        )
        .unwrap();
        let creds = Credentials::load_file(&file);
        let c = cloud(&creds);
        assert!(c.ready && c.key_configured);
        assert_eq!(c.key_hint.as_deref(), Some("abcd"));
        assert_eq!(c.key_source, Some("file"));
        assert!(!serde_json::to_string(&c).unwrap().contains("secret"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
