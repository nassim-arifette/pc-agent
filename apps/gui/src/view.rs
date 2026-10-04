//! Plain data for the desktop interface, built from the core's own types. Every number keeps its
//! raw value next to the text the core formats; a missing measurement carries its status and
//! reason and is never shown as zero. This is a local window, so local names (process names,
//! full paths) may appear here; nothing in this module is sent to a provider.
use pcagent_agent::{
    credentials::{Credentials, Source},
    AgentError, Answer, Category, CitationKind, Confidence, Config, Cost, ExecutionStatus,
    ExtensionRequest, Investigation, IssueKind, Pricing, SensorCoverage, Stats,
};
use pcagent_diagnostics::{Classification, Finding, Support};
use pcagent_shared::{
    format_bytes, gpu_slowdown_reasons, unix_ms, Evidence, HardwareInventory, Metric,
    ProcessCoverage, ProcessInstance, Snapshot, Status, VolumeKind,
};
use pcagent_storage::{
    Coverage, ItemKind, ItemView, LargestItemsPage, ScanProgress, ScanSummary, StorageOverview,
    Totals,
};
use serde::Serialize;
use serde_json::Value;

/// Models the settings screen offers, with what the 2026-09-27 comparison measured
/// (docs/archive/model-comparison.md); Ultra was added to the catalog later and is not evaluated
/// yet. Any other `nvidia/` Nemotron ID can still be typed.
pub const MODELS: [(&str, &str); 4] = [
    ("nvidia/nemotron-3-super-120b-a12b", "recommended"),
    ("nvidia/Nemotron-3-Ultra-550b-a55b", "most_capable"),
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
    /// Fixed volumes with their free space; filled from the hardware inventory by the caller.
    pub volumes: Vec<VolumeView>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VolumeView {
    pub root: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub free_ratio: f64,
}

/// Fixed and RAM volumes whose size and free space were both read; others are left out rather
/// than shown as empty.
pub fn volumes(inventory: &HardwareInventory) -> Vec<VolumeView> {
    let Some(list) = inventory.volumes.value.as_ref() else {
        return Vec::new();
    };
    list.iter()
        .filter(|v| matches!(v.kind, VolumeKind::Fixed | VolumeKind::RamDisk))
        .filter_map(|v| {
            let (total, free) = (v.total_bytes?, v.free_bytes?);
            (total > 0).then(|| VolumeView {
                root: v.root.clone(),
                total_bytes: total,
                free_bytes: free.min(total),
                free_ratio: free.min(total) as f64 / total as f64,
            })
        })
        .collect()
}

/// One target of a proposed action, as the person reviews it. Local paths may appear: this is
/// the local window, and the person must see exactly what would be touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetView {
    pub kind: String,
    pub label: String,
    pub files: Option<u64>,
    pub bytes: Option<u64>,
}
/// A proposal awaiting a decision in the window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanView {
    pub plan_id: String,
    pub kind: String,
    pub targets: Vec<TargetView>,
    pub recovery: String,
    pub checks: Vec<String>,
    /// `None` until the person declines or runs it; then `declined`, `running`, the outcome or
    /// `refused`.
    pub decision: Option<String>,
}
/// What an approved action did, with the measured before/after comparison.
#[derive(Debug, Clone, Serialize)]
pub struct ActionOutcome {
    pub plan_id: String,
    pub overall: String,
    pub items: Value,
    pub verification: Value,
}

fn snake(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

pub fn plan(plan: &pcagent_actions::ActionPlan, decision: Option<&str>) -> PlanView {
    use pcagent_actions::ResolvedTarget;
    let targets = plan
        .items()
        .iter()
        .map(|item| match &item.target {
            ResolvedTarget::File(f) => TargetView {
                kind: "file".into(),
                label: f.path.display().to_string(),
                files: Some(1),
                bytes: None,
            },
            ResolvedTarget::SettingsPage { page } => TargetView {
                kind: "settings_page".into(),
                label: page.uri().into(),
                files: None,
                bytes: None,
            },
            ResolvedTarget::CleanupAdapter(c) => TargetView {
                kind: "cleanup".into(),
                label: c.root.display().to_string(),
                files: Some(c.files),
                bytes: c.bytes,
            },
            ResolvedTarget::Process(t) => TargetView {
                kind: "process".into(),
                label: format!(
                    "{} (PID {})",
                    t.executable_name.as_deref().unwrap_or("process"),
                    t.identity.pid
                ),
                files: None,
                bytes: None,
            },
            ResolvedTarget::UserStartup(t) => TargetView {
                kind: "startup".into(),
                label: t.value_name().into(),
                files: None,
                bytes: None,
            },
            ResolvedTarget::RecycleCleanup(c) => TargetView {
                kind: "cleanup".into(),
                label: c.root().path.display().to_string(),
                files: Some(c.files().len() as u64),
                bytes: c.bytes(),
            },
        })
        .collect();
    PlanView {
        plan_id: plan.plan_id().to_owned(),
        kind: snake(&plan.action_kind()),
        targets,
        recovery: snake(&plan.recovery()),
        checks: plan.preconditions().iter().map(snake).collect(),
        decision: decision.map(str::to_owned),
    }
}

pub fn outcome(
    result: &pcagent_actions::ActionResult,
    verification: &pcagent_agent::ActionVerification,
) -> ActionOutcome {
    ActionOutcome {
        plan_id: result.plan_id.clone(),
        overall: snake(&result.overall),
        items: serde_json::to_value(&result.items).unwrap_or_default(),
        verification: serde_json::to_value(verification).unwrap_or_default(),
    }
}

/// A Windows service hosted by one of an application's processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceView {
    pub name: String,
    pub display_name: String,
    pub state: String,
}
/// One process of an application group, so a group is never mistaken for a single process.
#[derive(Debug, Clone, Serialize)]
pub struct MemberView {
    pub entity_ref: String,
    pub name: String,
    pub pid: u32,
    pub cpu: Measure,
    pub working_set: Measure,
    pub services: Vec<ServiceView>,
}
/// An application group: its measurements are sums over its members during the window.
#[derive(Debug, Clone, Serialize)]
pub struct ApplicationView {
    pub entity_ref: String,
    pub name: String,
    pub process_count: usize,
    pub members_truncated: bool,
    pub cpu: Measure,
    pub working_set: Measure,
    pub private_commit: Measure,
    pub working_set_increase: Measure,
    pub io_read: Measure,
    pub io_write: Measure,
    pub gpu: Measure,
    pub services: Vec<ServiceView>,
    /// Members among the returned processes; others were ranked lower and are counted only.
    pub members: Vec<MemberView>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ActivityView {
    pub collected_at_unix_ms: u64,
    pub window_ms: u64,
    pub status: Status,
    pub applications: Vec<ApplicationView>,
    pub applications_total: usize,
    pub coverage: ProcessCoverage,
    pub services_status: Status,
    pub gpu_status: Status,
    pub limitations: Vec<String>,
}

fn evidence_measure(evidence: &[Evidence], entity: &str, metric: Metric) -> Measure {
    Measure::of(
        evidence
            .iter()
            .find(|e| e.entity_ref == entity && e.metric == metric),
    )
}

fn services(list: &[pcagent_telemetry::ServiceProcess]) -> Vec<ServiceView> {
    list.iter()
        .map(|s| ServiceView {
            name: s.service_name.clone(),
            display_name: s.display_name.clone(),
            state: s.state.clone(),
        })
        .collect()
}

/// Application groups with their members and services, for the local Live PC view.
pub fn activity(a: &pcagent_telemetry::ProcessActivity) -> ActivityView {
    let m = |entity: &str, metric| evidence_measure(&a.evidence, entity, metric);
    let applications = a
        .applications
        .iter()
        .map(|app| {
            let members: Vec<MemberView> = a
                .processes
                .iter()
                .filter(|p| p.application_ref == app.entity_ref)
                .map(|p| MemberView {
                    entity_ref: p.instance.entity_ref.clone(),
                    name: process_name(&p.instance),
                    pid: p.instance.pid,
                    cpu: m(&p.instance.entity_ref, Metric::ProcessCpuBasisPoints),
                    working_set: m(&p.instance.entity_ref, Metric::ProcessWorkingSetBytes),
                    services: services(&p.services),
                })
                .collect();
            let mut all_services: Vec<ServiceView> =
                members.iter().flat_map(|p| p.services.clone()).collect();
            all_services.sort_by(|x, y| x.name.cmp(&y.name));
            all_services.dedup_by(|x, y| x.name == y.name);
            ApplicationView {
                entity_ref: app.entity_ref.clone(),
                name: app
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or("Unnamed application")
                    .to_owned(),
                process_count: app.process_count,
                members_truncated: app.members_truncated,
                cpu: m(&app.entity_ref, Metric::ProcessCpuBasisPoints),
                working_set: m(&app.entity_ref, Metric::ProcessWorkingSetBytes),
                private_commit: m(&app.entity_ref, Metric::ProcessPrivateCommitBytes),
                working_set_increase: m(&app.entity_ref, Metric::ProcessWorkingSetIncreaseBytes),
                io_read: m(&app.entity_ref, Metric::ProcessIoReadBytesPerSecond),
                io_write: m(&app.entity_ref, Metric::ProcessIoWriteBytesPerSecond),
                gpu: m(&app.entity_ref, Metric::ProcessGpuBasisPoints),
                services: all_services,
                members,
            }
        })
        .collect();
    ActivityView {
        collected_at_unix_ms: a.window.finished_at_unix_ms,
        window_ms: a.window.duration_ms,
        status: a.status,
        applications,
        applications_total: a.coverage.applications_total,
        coverage: a.coverage.processes.clone(),
        services_status: a.coverage.services_status,
        gpu_status: a.coverage.gpu_status,
        limitations: a.limitations.clone(),
    }
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
        volumes: Vec::new(),
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
pub struct CitationView {
    /// As written in the answer: `O2`, `F1`.
    pub reference: String,
    pub kind: CitationKind,
    /// The tool that made the observation, or the finding's label.
    pub title: String,
    pub facts: Vec<String>,
    /// A finding a fresher measurement has since replaced: it describes an earlier moment.
    pub earlier: bool,
    pub measured_at_unix_ms: Option<u64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct IssueView {
    pub kind: IssueKind,
    pub excerpt: String,
}
/// The model's answer. `text` is untrusted: the page renders it as text, never as markup.
#[derive(Debug, Clone, Serialize)]
pub struct AnswerView {
    pub text: String,
    pub confidence: Confidence,
    pub category: Category,
    pub citations: Vec<CitationView>,
    pub follow_ups: Vec<String>,
    pub action_refs: Vec<String>,
    pub unverified: Vec<IssueView>,
    pub repairs: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    /// Original model context and measured sources, before local display substitutions.
    pub context: pcagent_agent::PastTurn,
    pub question: String,
    pub mode: String,
    pub execution: ExecutionStatus,
    pub coverage: SensorCoverage,
    pub support: Support,
    pub error: Option<ErrorView>,
    /// Absent for a local analysis or an investigation that ended without an answer.
    pub answer: Option<AnswerView>,
    pub selected: FindingView,
    pub alternatives: Vec<FindingView>,
    /// Observations the model cited, or every usable one when the investigation stopped early.
    pub observations: Vec<ObservationView>,
    pub tool_events: Vec<pcagent_agent::ToolEvent>,
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
fn answer(r: &Investigation, a: &Answer) -> AnswerView {
    let processes = &r.local_processes;
    let local = |text: &str| localize(text, processes);
    let citations = a
        .citations
        .iter()
        .filter_map(|c| {
            if let Some(old) = r
                .historical_observations
                .iter()
                .find(|o| o.reference == c.target)
            {
                return Some(CitationView {
                    reference: c.reference.clone(),
                    kind: c.kind,
                    title: old.tool.clone(),
                    facts: old.facts.clone(),
                    earlier: true,
                    measured_at_unix_ms: (old.measured_at_unix_ms > 0)
                        .then_some(old.measured_at_unix_ms),
                });
            }
            let (title, facts, earlier) = match c.kind {
                CitationKind::Observation => {
                    let o = r
                        .observations
                        .iter()
                        .chain(std::iter::once(&r.initial_observation))
                        .find(|o| o.observation_id == c.target)?;
                    (
                        o.tool.clone(),
                        o.facts.iter().map(|f| local(f)).collect(),
                        false,
                    )
                }
                CitationKind::Finding => {
                    let earlier = r.historical_findings.iter().any(|f| f.id == c.target);
                    let f = r
                        .findings
                        .iter()
                        .chain(&r.historical_findings)
                        .find(|f| f.id == c.target)?;
                    let facts = std::iter::once(local(&f.summary))
                        .chain(f.facts.iter().map(|fact| local(&fact.text)))
                        .collect();
                    (f.classification.label().to_owned(), facts, earlier)
                }
            };
            Some(CitationView {
                reference: c.reference.clone(),
                kind: c.kind,
                title,
                facts,
                earlier,
                measured_at_unix_ms: r
                    .observations
                    .iter()
                    .chain(std::iter::once(&r.initial_observation))
                    .find(|o| o.observation_id == c.target)
                    .map(|o| o.measured_at_unix_ms)
                    .filter(|at| *at > 0),
            })
        })
        .collect();
    AnswerView {
        text: local(&a.text),
        confidence: a.confidence,
        category: a.category,
        citations,
        follow_ups: a.follow_ups.clone(),
        action_refs: a.action_refs.clone(),
        unverified: a
            .unverified
            .iter()
            .map(|i| IssueView {
                kind: i.kind,
                excerpt: local(&i.excerpt),
            })
            .collect(),
        repairs: a.repairs,
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
        context: pcagent_agent::PastTurn::from_report(question, r),
        question: question.to_owned(),
        mode: r.mode.clone(),
        execution: r.execution_status,
        coverage: r.coverage.clone(),
        support: r.diagnostic_support,
        error: r.error.as_ref().map(|e| ErrorView {
            code: error_code(e),
            message: e.to_string(),
        }),
        answer: r.answer.as_ref().map(|a| answer(r, a)),
        selected: finding(&r.selected, processes),
        alternatives: r
            .alternatives
            .iter()
            .map(|f| finding(f, processes))
            .collect(),
        observations,
        tool_events: r.tool_events.clone(),
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
    fn applications_keep_their_own_sums_members_and_services() {
        let window = serde_json::json!({"started_at_unix_ms":1000,"finished_at_unix_ms":3000,"duration_ms":2000});
        let ev = |metric: &str, entity: &str, value: Option<u64>| {
            serde_json::json!({"id":format!("{entity}-{metric}"),
            "metric":metric,"entity_ref":entity,"value":value,"unit":"","source":"fixture","window":window,
            "status": if value.is_some() {"ok"} else {"partial"},"origin":"synthetic_fixture","limitations":["not measured"]})
        };
        let process = |r: &str, pid: u32, app: &str, services: serde_json::Value| {
            serde_json::json!({
            "instance":{"entity_ref":r,"pid":pid,"creation_time_filetime":1,"boot_id":null,"executable_name":"browser.exe"},
            "parent_ref":null,"application_ref":app,"role":"application","handles":1,"threads":1,"session":1,"services":services})
        };
        let raw: pcagent_telemetry::ProcessActivity = serde_json::from_value(serde_json::json!({
            "schema_version":1,"id":"a","session_id":"s","origin":"synthetic_fixture","status":"ok","window":window,
            "processes":[process("proc-1",10,"app-1",serde_json::json!([])),
                process("proc-2",11,"app-1",serde_json::json!([{"pid":11,"service_name":"Spooler","display_name":"Print Spooler","state":"running"}]))],
            "applications":[{"entity_ref":"app-1","name":"browser.exe","process_refs":["proc-1","proc-2"],"process_count":12,"members_truncated":false}],
            "evidence":[ev("process_cpu_basis_points","app-1",Some(940)),ev("process_working_set_bytes","app-1",Some(3 << 30)),
                ev("process_cpu_basis_points","proc-1",Some(700)),ev("process_gpu_basis_points","app-1",None)],
            "coverage":{"processes":{"status":"ok","enumerated":2,"readable":2,"permission_denied":0,"unavailable":0,"returned":2,"enumeration_truncated":false},
                "stable_instances":2,"new_instances":0,"exited_instances":0,"pseudo_processes":[],"services_status":"ok",
                "services_attributed":1,"services_unattributed":0,"gpu_status":"unsupported","gpu_processes":0,
                "gpu_instances_rejected":0,"gpu_instances_unattributed":0,"applications_total":5,"applications_returned":1},
            "limitations":[]
        })).unwrap();
        let view = activity(&raw);
        let app = &view.applications[0];
        assert_eq!((app.name.as_str(), app.process_count), ("browser.exe", 12));
        // The group's own sum, not its first member's value.
        assert_eq!(app.cpu.value, Some(940));
        assert_eq!(app.members.len(), 2);
        assert_eq!(app.members[0].cpu.value, Some(700));
        assert_eq!(app.services[0].display_name, "Print Spooler");
        // Unknown GPU stays unknown, with its reason.
        assert_eq!(app.gpu.value, None);
        assert_eq!(app.gpu.note.as_deref(), Some("not measured"));
        assert_eq!(view.applications_total, 5);
    }

    #[test]
    fn volumes_keep_fixed_drives_with_both_sizes() {
        use pcagent_shared::{new_id, Observed, Origin, VolumeInfo, SCHEMA_VERSION};
        let volume = |root: &str, kind, total, free| VolumeInfo {
            root: root.into(),
            kind,
            total_bytes: total,
            free_bytes: free,
            available_to_caller_bytes: free,
            source: "fixture".into(),
            status: Status::Ok,
            limitations: Vec::new(),
        };
        fn missing<T>() -> Observed<T> {
            Observed::missing("fixture", Status::Unsupported)
        }
        let inventory = HardwareInventory {
            schema_version: SCHEMA_VERSION,
            id: new_id("inventory"),
            origin: Origin::Live,
            status: Status::Ok,
            collected_at_unix_ms: 0,
            os_version: missing(),
            architecture: missing(),
            logical_processors: missing(),
            installed_memory_bytes: missing(),
            volumes: Observed::ok(
                vec![
                    volume("C:\\", VolumeKind::Fixed, Some(1000), Some(250)),
                    volume("D:\\", VolumeKind::Fixed, Some(1000), None),
                    volume("E:\\", VolumeKind::Removable, Some(1000), Some(900)),
                ],
                "fixture",
            ),
            gpu_adapters: missing(),
            limitations: Vec::new(),
        };
        let list = volumes(&inventory);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].root, "C:\\");
        assert_eq!(list[0].free_ratio, 0.25);
        // No inventory at all: nothing, never an empty drive.
        let unknown = HardwareInventory {
            volumes: missing(),
            ..inventory
        };
        assert!(volumes(&unknown).is_empty());
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
            alias: "file-7.img".into(),
            alias_path: "<scope>/folder-2/file-7.img".into(),
            parent_ref: Some("scan-1:2".into()),
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
