//! Plain data for the desktop interface, built from the core's own types. Every number keeps its
//! raw value next to the text the core formats; a missing measurement carries its status and
//! reason and is never shown as zero. This is a local window, so local names (process names,
//! full paths) may appear here; nothing in this module is sent to a provider.
use pcagent_agent::selection::is_battery_ref;
use pcagent_agent::{
    credentials::{Credentials, Source},
    AgentError, Answer, Category, CitationKind, Confidence, Config, Cost, ExecutionStatus,
    ExtensionRequest, Investigation, IssueKind, Pricing, SensorCoverage, Stats,
};
use pcagent_diagnostics::{Classification, Finding, Support};
use pcagent_inspect::{CoverageStatus, Inspection, Observation};
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
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

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
    pub entity_ref: String,
    pub name: String,
    pub pid: u32,
    pub working_set: Measure,
    pub private_commit: Measure,
    pub cpu: Measure,
}
#[derive(Debug, Clone, Serialize)]
pub struct Dashboard {
    pub selection_source_id: Option<String>,
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
    /// System batteries of the latest live battery read, each with its own values (never
    /// summed); filled by the caller from `LiveBattery`. Empty on a desktop, on other systems,
    /// before the first read, and once the latest values are older than 30 seconds.
    pub batteries: Vec<BatteryView>,
    /// Selection source of that read (resource `battery`), reused until the next read. `None`
    /// whenever `batteries` is empty.
    pub battery_source_id: Option<String>,
    /// Windows lists a battery the window cannot currently show: it is enumerated but could not
    /// be read, or reads stopped finishing (a stalled driver). Always false while `batteries` is
    /// non-empty, on a desktop and on other systems.
    pub battery_unavailable: bool,
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

/// One system battery of a live battery read, as the window shows it. Every value is this
/// battery's own reading; batteries are never summed. A value Windows did not report, or reported
/// with contradicting flags, is `None`, never zero.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BatteryView {
    /// Opaque key (`battery-<uuid>`) of this battery for this run of the app; never displayed.
    pub battery_ref: String,
    /// `system`, or `None` when Windows did not report the kind.
    pub kind: Option<String>,
    /// `milliwatt_hours`, or `relative` for a battery that reports a percentage only.
    pub capacity_units: Option<String>,
    /// Newest native measurement time among the values shown.
    pub measured_at_unix_ms: u64,
    /// Remaining / full-charge capacity × 100 (0 to 100).
    pub charge_percent: Option<f64>,
    /// `charging`, `discharging` or `idle`.
    pub charge_state: Option<String>,
    pub on_ac: Option<bool>,
    /// Windows' own hardware critical flag; no threshold is invented.
    pub critical: Option<bool>,
    /// Signed milliwatts: positive while charging, negative while discharging.
    pub power_rate_mw: Option<i64>,
    /// The battery driver's own estimate, only while discharging.
    pub estimated_runtime_windows_seconds: Option<u64>,
    /// Remaining energy / discharge power × 3600, assuming the present drain continues.
    pub estimated_runtime_at_current_rate_seconds: Option<f64>,
    pub remaining_capacity_mwh: Option<u64>,
    pub full_charge_capacity_mwh: Option<u64>,
    pub design_capacity_mwh: Option<u64>,
    /// Full-charge / design capacity × 100.
    pub capacity_retention_percent: Option<f64>,
    /// 100 − retention: an estimate from reported capacities, not a health score.
    pub estimated_wear_percent: Option<f64>,
    pub cycle_count: Option<u64>,
    /// Windows reported contradicting values for this battery; those values are `None` above.
    pub inconsistent: bool,
}

/// The battery metrics the window displays, with the two consistency flags behind its
/// "contradicting values" note. The selection source keeps only these metrics (C5), including a
/// partial or contradicting value the window marks as not reported; no voltage, critical bias,
/// technology or derived magnitudes.
const BATTERY_METRICS: [&str; 17] = [
    "battery_kind",
    "battery_capacity_units",
    "battery_charge_percent",
    "battery_charge_state",
    "battery_on_ac",
    "battery_critical",
    "battery_power_rate_mw",
    "battery_estimated_runtime_windows_seconds",
    "battery_estimated_runtime_at_current_rate_seconds",
    "battery_remaining_capacity_mwh",
    "battery_full_charge_capacity_mwh",
    "battery_design_capacity_mwh",
    "battery_capacity_retention_percent",
    "battery_estimated_wear_percent",
    "battery_cycle_count",
    "battery_state_consistent",
    "battery_capacities_consistent",
];

/// One battery's observations, read field by field.
struct BatteryValues<'a> {
    observations: Vec<&'a Observation>,
    measured_at: u64,
    inconsistent: bool,
}
impl BatteryValues<'_> {
    /// A usable value: status ok, not null and of the expected type. A partial (contradicting)
    /// value is never shown; it marks the battery inconsistent instead.
    fn take<T>(&mut self, metric: &str, convert: impl Fn(&Value) -> Option<T>) -> Option<T> {
        let observation = self.observations.iter().find(|o| o.metric == metric)?;
        if observation.value.is_null() {
            return None;
        }
        if observation.status == CoverageStatus::Partial {
            self.inconsistent = true;
        }
        if observation.status != CoverageStatus::Ok {
            return None;
        }
        let value = convert(&observation.value)?;
        self.measured_at = self.measured_at.max(observation.measured_at_unix_ms);
        Some(value)
    }
    fn number(&mut self, metric: &str) -> Option<f64> {
        self.take(metric, |v| v.as_f64().filter(|v| v.is_finite()))
    }
    fn count(&mut self, metric: &str) -> Option<u64> {
        self.take(metric, Value::as_u64)
    }
    fn flag(&mut self, metric: &str) -> Option<bool> {
        self.take(metric, Value::as_bool)
    }
    fn category(&mut self, metric: &str, known: &[&str]) -> Option<String> {
        self.take(metric, |v| {
            v.as_str()
                .filter(|text| known.contains(text))
                .map(str::to_owned)
        })
    }
}

/// Why a battery of the read is not shown.
enum NotShown {
    /// A UPS (`short_term`) or a non-system battery: not the PC's battery.
    Other,
    /// A system battery without any charge, state or AC value.
    Unread,
}

fn battery(reference: &str, read: &Inspection) -> Result<BatteryView, NotShown> {
    let mut values = BatteryValues {
        observations: read
            .observations
            .iter()
            .filter(|o| o.entity_ref == reference)
            .collect(),
        measured_at: 0,
        inconsistent: false,
    };
    let kind = values.category("battery_kind", &["system", "short_term", "non_system"]);
    if kind.as_deref().is_some_and(|kind| kind != "system") {
        return Err(NotShown::Other);
    }
    let capacity_units =
        values.category("battery_capacity_units", &["milliwatt_hours", "relative"]);
    // Relative capacities have no physical unit (docs/battery.md): percent and state only, so
    // energy, power and the values derived from them are never read for such a battery.
    let absolute = capacity_units.as_deref() != Some("relative");
    let charge_percent = values.number("battery_charge_percent");
    let charge_state =
        values.category("battery_charge_state", &["charging", "discharging", "idle"]);
    let on_ac = values.flag("battery_on_ac");
    let critical = values.flag("battery_critical");
    let estimated_runtime_windows_seconds =
        values.count("battery_estimated_runtime_windows_seconds");
    let cycle_count = values.count("battery_cycle_count");
    let contradicting = values.flag("battery_state_consistent") == Some(false)
        || values.flag("battery_capacities_consistent") == Some(false);
    if charge_percent.is_none() && charge_state.is_none() && on_ac.is_none() {
        return Err(NotShown::Unread);
    }
    let mut energy = |metric: &str| absolute.then(|| values.count(metric)).flatten();
    let remaining_capacity_mwh = energy("battery_remaining_capacity_mwh");
    let full_charge_capacity_mwh = energy("battery_full_charge_capacity_mwh");
    let design_capacity_mwh = energy("battery_design_capacity_mwh");
    let mut derived = |metric: &str| absolute.then(|| values.number(metric)).flatten();
    let estimated_runtime_at_current_rate_seconds =
        derived("battery_estimated_runtime_at_current_rate_seconds");
    let capacity_retention_percent = derived("battery_capacity_retention_percent");
    let estimated_wear_percent = derived("battery_estimated_wear_percent");
    let power_rate_mw = absolute
        .then(|| values.take("battery_power_rate_mw", Value::as_i64))
        .flatten();
    Ok(BatteryView {
        battery_ref: reference.to_owned(),
        kind,
        capacity_units,
        measured_at_unix_ms: values.measured_at,
        charge_percent,
        charge_state,
        on_ac,
        critical,
        power_rate_mw,
        estimated_runtime_windows_seconds,
        estimated_runtime_at_current_rate_seconds,
        remaining_capacity_mwh,
        full_charge_capacity_mwh,
        design_capacity_mwh,
        capacity_retention_percent,
        estimated_wear_percent,
        cycle_count,
        inconsistent: values.inconsistent || contradicting,
    })
}

/// One live battery read, prepared for the window and the selection registry.
#[derive(Debug)]
pub struct BatteryRead {
    /// The read was refused or timed out (`collector` coverage): the caller keeps its previous
    /// values.
    pub busy: bool,
    /// Windows lists a battery that this read could not show: enumeration or a device read was
    /// incomplete, or a system battery reported no charge, state or AC value.
    pub unavailable: bool,
    /// System batteries (kind `system` or unreported) in the collector's order. UPS and
    /// non-system batteries are left out.
    pub batteries: Vec<BatteryView>,
    /// Selection source for resource `battery`: only the shown batteries and the metrics the
    /// window displays, with the read's own window and coverage. `None` when nothing is shown.
    pub source: Option<Inspection>,
}

pub fn battery_read(mut read: Inspection) -> BatteryRead {
    if read.coverage.iter().any(|c| c.scope == "collector") {
        return BatteryRead {
            busy: true,
            unavailable: false,
            batteries: Vec::new(),
            source: None,
        };
    }
    let mut references: Vec<&str> = Vec::new();
    for observation in &read.observations {
        let reference = observation.entity_ref.as_str();
        if is_battery_ref(reference) && !references.contains(&reference) {
            references.push(reference);
        }
    }
    let mut batteries = Vec::new();
    let mut unread = false;
    for reference in references {
        match battery(reference, &read) {
            Ok(view) => batteries.push(view),
            Err(NotShown::Other) => {}
            Err(NotShown::Unread) => unread = true,
        }
    }
    // Unsupported means Windows lists no battery (a desktop): that is not "unavailable".
    let incomplete = read.coverage.iter().any(|c| {
        c.scope == "battery_devices"
            && !matches!(c.status, CoverageStatus::Ok | CoverageStatus::Unsupported)
    });
    let source = if batteries.is_empty() {
        None
    } else {
        let shown: BTreeSet<&str> = batteries.iter().map(|b| b.battery_ref.as_str()).collect();
        read.observations.retain(|o| {
            shown.contains(o.entity_ref.as_str()) && BATTERY_METRICS.contains(&o.metric.as_str())
        });
        read.names.clear();
        Some(read)
    };
    BatteryRead {
        busy: false,
        unavailable: batteries.is_empty() && (incomplete || unread),
        batteries,
        source,
    }
}

/// A battery the window shows is read again about this often (every second 5-second tick).
const BATTERY_MAX_AGE: Duration = Duration::from_secs(10);
/// After a complete read that shows no system battery (a desktop, or only a UPS).
const BATTERY_ABSENT_RECHECK: Duration = Duration::from_secs(300);
/// After a busy read, while a previous read is still blocked in the driver.
const BATTERY_BUSY_RETRY: Duration = Duration::from_secs(5);
/// Values older than this are no longer shown as current, and their selection source (well
/// inside its five-minute lifetime) is no longer offered.
const BATTERY_STALE_AFTER: Duration = Duration::from_secs(30);

/// The latest live battery read, shared by the measurement thread, which decides when a read is
/// due and never waits for one, and the battery thread, which records each finished read.
#[derive(Debug, Default)]
pub struct LiveBattery {
    next_due: Option<Instant>,
    /// When the latest read that was not busy finished.
    read_at: Option<Instant>,
    batteries: Vec<BatteryView>,
    source_id: Option<String>,
    unavailable: bool,
    /// The latest attempt was busy: a previous read is still blocked in the driver.
    stalled: bool,
}

impl LiveBattery {
    /// Whether the measurement thread should request a read now.
    pub fn due(&self, now: Instant) -> bool {
        self.next_due.is_none_or(|due| now >= due)
    }

    /// Stamped when the request is sent, so the cadence does not drift with the read's duration.
    pub fn requested(&mut self, now: Instant) {
        self.next_due = Some(now + BATTERY_MAX_AGE);
    }

    /// Record a finished read. The caller has already published `read.source` and passes the
    /// resulting selection source ID.
    pub fn record(&mut self, read: BatteryRead, source_id: Option<String>, now: Instant) {
        if read.busy {
            // Keep the previous values while they are recent; retry soon.
            self.stalled = true;
            self.next_due = Some(now + BATTERY_BUSY_RETRY);
            return;
        }
        if read.batteries.is_empty() && !read.unavailable {
            // A desktop (or only a UPS): look again rarely, in case a battery is connected.
            self.next_due = Some(now + BATTERY_ABSENT_RECHECK);
        }
        self.stalled = false;
        self.read_at = Some(now);
        self.unavailable = read.unavailable;
        self.batteries = read.batteries;
        self.source_id = source_id;
    }

    /// Copy what is current into the dashboard; old values are never shown as current.
    pub fn show(&self, dashboard: &mut Dashboard, now: Instant) {
        let current = self
            .read_at
            .is_some_and(|at| now.saturating_duration_since(at) <= BATTERY_STALE_AFTER);
        if current {
            dashboard.batteries = self.batteries.clone();
            dashboard.battery_source_id = self.source_id.clone();
            dashboard.battery_unavailable = self.unavailable;
        } else {
            dashboard.batteries = Vec::new();
            dashboard.battery_source_id = None;
            // Reads stopped finishing while a battery was known: say so instead of hiding it.
            // Values that are merely old (the window was hidden) are not a failure.
            dashboard.battery_unavailable =
                self.stalled && (self.unavailable || !self.batteries.is_empty());
        }
    }
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
    pub selection_source_id: Option<String>,
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
        selection_source_id: None,
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
                entity_ref: p.entity_ref.clone(),
                name: process_name(p),
                pid: p.pid,
                working_set: m(Metric::ProcessWorkingSetBytes),
                private_commit: m(Metric::ProcessPrivateCommitBytes),
                cpu: m(Metric::ProcessCpuBasisPoints),
            }
        })
        .collect();
    Dashboard {
        selection_source_id: None,
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
        batteries: Vec::new(),
        battery_source_id: None,
        battery_unavailable: false,
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
    pub model_requests: Vec<pcagent_agent::ModelRequestUsage>,
    pub total_cost_usd: Option<String>,
    pub routing: Option<pcagent_agent::routing::RoutingState>,
    /// Lower bounds when a request reported no usage (see `Stats`).
    pub known_prompt_tokens: u64,
    pub known_completion_tokens: u64,
    pub unknown_requests: usize,
    pub cost_lower_bound: Option<CostView>,
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
    /// A web source (C6): where it comes from and when it was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub web: Option<WebSourceView>,
}
#[derive(Debug, Clone, Serialize)]
pub struct WebSourceView {
    pub domain: String,
    /// Opened only by the `open_web_source` command, which reads it from the stored turn.
    pub url: String,
    pub published_date: Option<String>,
    pub searched_at_unix_ms: Option<u64>,
    /// The exact query that found it.
    pub query: Option<String>,
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
    /// Deeper checks (closed `ElevatedTask` kinds) whose reading this turn lacked, in display
    /// order. A local rule: the model has no authority here, and nothing runs without the
    /// person's click and the Windows prompt.
    pub admin_hints: Vec<String>,
    /// Absent for a local analysis, which makes no provider request.
    pub usage: Option<Usage>,
    pub continuation: Option<ExtensionRequest>,
}

/// Replaces opaque process references (`proc-1a2b3c4d`) with local names for this window. A model
/// that names a process and adds its reference ("firefox.exe (proc-1a2b3c4d)", needed when several
/// processes share a name) gets "firefox.exe (PID 42)", not the name twice.
fn localize(text: &str, processes: &[ProcessInstance]) -> String {
    processes.iter().fold(text.to_owned(), |text, p| {
        let name = process_name(p);
        let local = format!("{name} (PID {})", p.pid);
        text.replace(&format!("{name} ({})", p.entity_ref), &local)
            .replace(&p.entity_ref, &local)
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
        model_requests: s.model_requests.clone(),
        total_cost_usd: s.total_cost_usd.clone(),
        routing: s.routing.clone(),
        known_prompt_tokens: s.known_prompt_tokens,
        known_completion_tokens: s.known_completion_tokens,
        unknown_requests: s.unknown_requests,
        cost_lower_bound: s.cost_lower_bound.as_ref().map(cost),
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
                    web: None,
                });
            }
            if c.kind == CitationKind::Web {
                let (observation_id, id) = c.target.split_once('#')?;
                let o = r
                    .observations
                    .iter()
                    .find(|o| o.observation_id == observation_id)?;
                let source = o.observation["sources"]
                    .as_array()?
                    .iter()
                    .find(|s| s["source_id"] == id)?;
                let text = |key: &str| source[key].as_str().map(str::to_owned);
                return Some(CitationView {
                    reference: c.reference.clone(),
                    kind: c.kind,
                    title: text("title").unwrap_or_default(),
                    facts: text("snippet").into_iter().collect(),
                    earlier: false,
                    measured_at_unix_ms: None,
                    web: Some(WebSourceView {
                        domain: text("domain").unwrap_or_default(),
                        url: text("url")?,
                        published_date: text("published_date"),
                        searched_at_unix_ms: o.observation["searched_at_unix_ms"].as_u64(),
                        query: o.observation["query"].as_str().map(str::to_owned),
                    }),
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
                CitationKind::Web => return None,
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
                web: None,
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
/// The deeper checks the chat offers under an answer; the Storage view lists the same ones.
const OFFERED_CHECKS: [&str; 4] = [
    "system_caches",
    "shadow_storage",
    "disk_reliability",
    "battery_energy",
];
/// Cleanup categories the helper's `system_caches` task measures when the user cannot.
const SYSTEM_CACHE_CATEGORIES: [&str; 3] = [
    "previous_windows",
    "windows_update_cache",
    "delivery_optimization",
];
/// Which offered checks a turn lacked: the model asked for an approved helper result that did
/// not exist, or a local inspection reported `needs_admin` on a scope one of the checks covers.
fn admin_hints(r: &Investigation) -> Vec<String> {
    let mut hints: Vec<&str> = Vec::new();
    for o in &r.observations {
        let Some(coverage) = o.observation["coverage"].as_array() else {
            continue;
        };
        let denied = |scope: fn(&str) -> bool| {
            coverage
                .iter()
                .any(|c| c["status"] == "needs_admin" && c["scope"].as_str().is_none_or(scope))
        };
        let hint = match o.tool.as_str() {
            "get_approved_elevated_inspection" if denied(|_| true) => {
                o.observation["task"].as_str()
            }
            "get_cleanup_opportunities" if denied(|s| SYSTEM_CACHE_CATEGORIES.contains(&s)) => {
                Some("system_caches")
            }
            _ => None,
        };
        if let Some(hint) = hint.filter(|h| OFFERED_CHECKS.contains(h)) {
            if !hints.contains(&hint) {
                hints.push(hint);
            }
        }
    }
    hints.sort_by_key(|h| OFFERED_CHECKS.iter().position(|o| o == h));
    hints.into_iter().map(str::to_owned).collect()
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
        admin_hints: admin_hints(r),
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
    pub selection_source_id: Option<String>,
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
        selection_source_id: None,
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
    /// How much the model reasons: on (default), low or off.
    pub reasoning: &'static str,
    pub problem: Option<String>,
    pub file: Option<String>,
    pub file_problem: Option<String>,
    /// A Tavily key is saved for web lookups (C6); the key itself never reaches the page.
    pub search_key_configured: bool,
    pub search_key_hint: Option<String>,
    /// `environment` when a variable overrides the saved key, as for Nebius.
    pub search_key_source: Option<&'static str>,
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
        reasoning: creds
            .get("NEBIUS_REASONING")
            .and_then(|text| pcagent_agent::Reasoning::parse(&text))
            .unwrap_or_default()
            .as_str(),
        problem: checked.err().map(|e| e.to_string()),
        file: creds.file().map(|p| p.display().to_string()),
        file_problem: creds.problem.clone(),
        search_key_configured: creds.get("TAVILY_API_KEY").is_some(),
        search_key_hint: creds
            .get("TAVILY_API_KEY")
            .filter(|k| k.chars().count() >= 12)
            .map(|k| k.chars().skip(k.chars().count() - 4).collect()),
        search_key_source: source_name(creds.source("TAVILY_API_KEY")),
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
        // The battery comes from its own read, filled by the caller: none until then.
        assert!(d.batteries.is_empty());
        assert!(d.battery_source_id.is_none());
        assert!(!d.battery_unavailable);
    }

    const B1: &str = "battery-00000000-0000-4000-8000-00000000b001";
    const B2: &str = "battery-00000000-0000-4000-8000-00000000b002";
    const UPS: &str = "battery-00000000-0000-4000-8000-00000000c001";

    /// Synthetic observations of one battery: `(metric, value)`, ok unless null; no device is
    /// opened.
    fn cells(reference: &str, values: &[(&str, Value)]) -> Vec<Observation> {
        values
            .iter()
            .map(|(metric, value)| Observation {
                observation_id: format!("measurement-{reference}-{metric}"),
                measured_at_unix_ms: 5000,
                window: Window {
                    started_at_unix_ms: 4990,
                    finished_at_unix_ms: 5000,
                    duration_ms: 10,
                },
                entity_ref: reference.into(),
                metric: (*metric).into(),
                value: value.clone(),
                unit: "fixture".into(),
                status: if value.is_null() {
                    CoverageStatus::Unsupported
                } else {
                    CoverageStatus::Ok
                },
                source: "fixture".into(),
                origin: pcagent_shared::Origin::SyntheticFixture,
            })
            .collect()
    }

    fn inspection(
        observations: Vec<Observation>,
        scope: &str,
        status: CoverageStatus,
    ) -> Inspection {
        Inspection {
            status: Status::Partial,
            measured_at_unix_ms: 5010,
            window: Window {
                started_at_unix_ms: 4980,
                finished_at_unix_ms: 5010,
                duration_ms: 30,
            },
            origin: pcagent_shared::Origin::SyntheticFixture,
            observations,
            coverage: vec![pcagent_inspect::Coverage {
                scope: scope.into(),
                status,
                source: "fixture".into(),
                detail: "Synthetic.".into(),
            }],
            names: Default::default(),
        }
    }

    /// The dev laptop of docs/battery.md, plugged in and full.
    fn plugged_in(reference: &str) -> Vec<Observation> {
        use serde_json::json;
        cells(
            reference,
            &[
                ("battery_kind", json!("system")),
                ("battery_capacity_units", json!("milliwatt_hours")),
                ("battery_rechargeable", json!(true)),
                ("battery_design_capacity_mwh", json!(47310)),
                ("battery_full_charge_capacity_mwh", json!(39900)),
                ("battery_critical_bias_mwh", json!(0)),
                ("battery_cycle_count", Value::Null),
                ("battery_capacities_consistent", json!(true)),
                (
                    "battery_capacity_retention_percent",
                    json!(39900.0 / 47310.0 * 100.0),
                ),
                (
                    "battery_estimated_wear_percent",
                    json!(100.0 - 39900.0 / 47310.0 * 100.0),
                ),
                ("battery_remaining_capacity_mwh", json!(39900)),
                ("battery_charge_percent", json!(100.0)),
                ("battery_power_rate_mw", Value::Null),
                ("battery_discharge_power_mw", Value::Null),
                ("battery_charge_power_mw", Value::Null),
                ("battery_voltage_mv", json!(12900)),
                ("battery_charge_state", json!("idle")),
                ("battery_on_ac", json!(true)),
                ("battery_critical", json!(false)),
                ("battery_state_consistent", json!(true)),
                ("battery_estimated_runtime_windows_seconds", Value::Null),
                (
                    "battery_estimated_runtime_at_current_rate_seconds",
                    Value::Null,
                ),
            ],
        )
    }

    fn discharging(reference: &str, charge: f64) -> Vec<Observation> {
        use serde_json::json;
        cells(
            reference,
            &[
                ("battery_kind", json!("system")),
                ("battery_capacity_units", json!("milliwatt_hours")),
                ("battery_design_capacity_mwh", json!(57000)),
                ("battery_full_charge_capacity_mwh", json!(52800)),
                ("battery_remaining_capacity_mwh", json!(41250)),
                ("battery_charge_percent", json!(charge)),
                ("battery_power_rate_mw", json!(-8400)),
                ("battery_discharge_power_mw", json!(8400)),
                ("battery_voltage_mv", json!(11400)),
                ("battery_charge_state", json!("discharging")),
                ("battery_on_ac", json!(false)),
                ("battery_critical", json!(false)),
                ("battery_state_consistent", json!(true)),
                ("battery_capacities_consistent", json!(true)),
                ("battery_cycle_count", json!(212)),
                ("battery_estimated_runtime_windows_seconds", json!(17400)),
                (
                    "battery_estimated_runtime_at_current_rate_seconds",
                    json!(17678.6),
                ),
            ],
        )
    }

    #[test]
    fn battery_unknowns_stay_none_and_never_zero() {
        let read = battery_read(inspection(
            plugged_in(B1),
            "battery_devices",
            CoverageStatus::Ok,
        ));
        assert!(!read.busy && !read.unavailable);
        let b = &read.batteries[0];
        assert_eq!(b.battery_ref, B1);
        assert_eq!(b.kind.as_deref(), Some("system"));
        assert_eq!(b.charge_percent, Some(100.0));
        assert_eq!(b.charge_state.as_deref(), Some("idle"));
        assert_eq!(b.on_ac, Some(true));
        assert_eq!(b.critical, Some(false));
        assert_eq!(b.full_charge_capacity_mwh, Some(39900));
        assert_eq!(b.design_capacity_mwh, Some(47310));
        assert_eq!(b.remaining_capacity_mwh, Some(39900));
        assert_eq!(b.power_rate_mw, None);
        assert_eq!(b.cycle_count, None);
        assert_eq!(b.estimated_runtime_windows_seconds, None);
        assert_eq!(b.estimated_runtime_at_current_rate_seconds, None);
        assert!((b.estimated_wear_percent.unwrap() - 15.6627).abs() < 1e-4);
        assert!((b.capacity_retention_percent.unwrap() - 84.3373).abs() < 1e-4);
        assert!(!b.inconsistent);
        assert_eq!(b.measured_at_unix_ms, 5000);
        let json = serde_json::to_string(b).unwrap();
        assert!(json.contains(r#""cycle_count":null"#));
        for field in [
            "power_rate_mw",
            "cycle_count",
            "estimated_runtime_windows_seconds",
            "estimated_runtime_at_current_rate_seconds",
        ] {
            assert!(json.contains(&format!(r#""{field}":null"#)), "{field}");
            assert!(!json.contains(&format!(r#""{field}":0"#)), "{field}");
        }
    }

    #[test]
    fn discharging_battery_keeps_signed_power_and_both_runtimes_apart() {
        let read = battery_read(inspection(
            discharging(B1, 78.1),
            "battery_devices",
            CoverageStatus::Ok,
        ));
        let b = &read.batteries[0];
        assert_eq!(b.power_rate_mw, Some(-8400));
        assert_eq!(b.estimated_runtime_windows_seconds, Some(17400));
        assert_eq!(b.estimated_runtime_at_current_rate_seconds, Some(17678.6));
        assert_eq!(b.charge_state.as_deref(), Some("discharging"));
        assert_eq!(b.on_ac, Some(false));
        assert_eq!(b.cycle_count, Some(212));
        assert_eq!(b.charge_percent, Some(78.1));
    }

    #[test]
    fn partial_or_contradicting_values_are_not_shown_as_measurements() {
        let mut observations = discharging(B1, 64.0);
        for o in &mut observations {
            match o.metric.as_str() {
                "battery_power_rate_mw" => o.status = CoverageStatus::Partial,
                "battery_state_consistent" => o.value = Value::Bool(false),
                _ => {}
            }
        }
        let read = battery_read(inspection(
            observations,
            "battery_devices",
            CoverageStatus::Ok,
        ));
        let b = &read.batteries[0];
        assert_eq!(b.power_rate_mw, None);
        assert!(b.inconsistent);
        assert_eq!(b.charge_percent, Some(64.0));
        // The contradicting reading still reaches the preview with its partial status.
        let source = read.source.unwrap();
        let power = source
            .observations
            .iter()
            .find(|o| o.metric == "battery_power_rate_mw")
            .unwrap();
        assert_eq!(power.status, CoverageStatus::Partial);
    }

    #[test]
    fn desktop_other_systems_and_busy_reads_show_no_battery() {
        // A desktop: Windows lists no battery.
        let desktop = battery_read(inspection(
            vec![],
            "battery_devices",
            CoverageStatus::Unsupported,
        ));
        assert!(desktop.batteries.is_empty() && desktop.source.is_none());
        assert!(!desktop.busy && !desktop.unavailable);
        // Other systems.
        let other = battery_read(inspection(vec![], "windows", CoverageStatus::Unsupported));
        assert!(other.batteries.is_empty() && !other.unavailable && !other.busy);
        // A read refused or timed out keeps the caller's previous values.
        let busy = battery_read(inspection(vec![], "collector", CoverageStatus::Partial));
        assert!(busy.busy && busy.batteries.is_empty() && busy.source.is_none());
        assert!(!busy.unavailable);
        // Windows lists a battery that could not be read: say so instead of hiding it.
        let unreadable = battery_read(inspection(
            vec![],
            "battery_devices",
            CoverageStatus::Partial,
        ));
        assert!(unreadable.unavailable && unreadable.batteries.is_empty());
        let mut silent = plugged_in(B1);
        silent.retain(|o| o.metric == "battery_kind" || o.metric == "battery_voltage_mv");
        let silent = battery_read(inspection(silent, "battery_devices", CoverageStatus::Ok));
        assert!(silent.unavailable && silent.batteries.is_empty() && silent.source.is_none());
    }

    #[test]
    fn ups_and_non_system_batteries_are_left_out_and_batteries_are_never_summed() {
        use serde_json::json;
        let mut observations = discharging(B1, 80.0);
        observations.extend(cells(
            UPS,
            &[
                ("battery_kind", json!("short_term")),
                ("battery_charge_percent", json!(100.0)),
                ("battery_on_ac", json!(true)),
            ],
        ));
        observations.extend(cells(
            "battery-00000000-0000-4000-8000-00000000c002",
            &[
                ("battery_kind", json!("non_system")),
                ("battery_charge_percent", json!(55.0)),
            ],
        ));
        observations.extend(discharging(B2, 40.0));
        let read = battery_read(inspection(
            observations,
            "battery_devices",
            CoverageStatus::Ok,
        ));
        assert!(!read.unavailable);
        let shown: Vec<_> = read
            .batteries
            .iter()
            .map(|b| (b.battery_ref.as_str(), b.charge_percent))
            .collect();
        assert_eq!(shown, [(B1, Some(80.0)), (B2, Some(40.0))]);
        let source = read.source.unwrap();
        assert!(source
            .observations
            .iter()
            .all(|o| o.entity_ref == B1 || o.entity_ref == B2));
        // Only what the window displays leaves in an Explain preview.
        for hidden in [
            "battery_voltage_mv",
            "battery_discharge_power_mw",
            "battery_charge_power_mw",
            "battery_rechargeable",
            "battery_critical_bias_mwh",
        ] {
            assert!(
                source.observations.iter().all(|o| o.metric != hidden),
                "{hidden}"
            );
        }
        assert!(source
            .observations
            .iter()
            .all(|o| BATTERY_METRICS.contains(&o.metric.as_str())));
        assert_eq!(source.coverage[0].scope, "battery_devices");
        assert_eq!(source.window.duration_ms, 30);
        // The registry accepts it as the battery resource.
        let source = pcagent_agent::selection::SelectionSource::Battery(source);
        assert_eq!(source.targets().len(), 1);
    }

    #[test]
    fn relative_units_never_show_energy_or_power() {
        use serde_json::json;
        let mut observations = discharging(B1, 64.0);
        for o in &mut observations {
            if o.metric == "battery_capacity_units" {
                o.value = json!("relative");
            }
        }
        let read = battery_read(inspection(
            observations,
            "battery_devices",
            CoverageStatus::Ok,
        ));
        let b = &read.batteries[0];
        assert_eq!(b.capacity_units.as_deref(), Some("relative"));
        assert_eq!(b.charge_percent, Some(64.0));
        assert_eq!(b.charge_state.as_deref(), Some("discharging"));
        assert_eq!(b.estimated_runtime_windows_seconds, Some(17400));
        for value in [
            b.remaining_capacity_mwh,
            b.full_charge_capacity_mwh,
            b.design_capacity_mwh,
        ] {
            assert_eq!(value, None);
        }
        assert_eq!(b.power_rate_mw, None);
        assert_eq!(b.estimated_runtime_at_current_rate_seconds, None);
        assert_eq!(b.capacity_retention_percent, None);
        assert_eq!(b.estimated_wear_percent, None);
    }

    fn shown(live: &LiveBattery, now: Instant) -> Dashboard {
        let mut d = dashboard(&fixture());
        live.show(&mut d, now);
        d
    }
    fn one_battery() -> BatteryRead {
        battery_read(inspection(
            discharging(B1, 78.1),
            "battery_devices",
            CoverageStatus::Ok,
        ))
    }
    fn nothing() -> BatteryRead {
        battery_read(inspection(
            vec![],
            "battery_devices",
            CoverageStatus::Unsupported,
        ))
    }
    fn busy() -> BatteryRead {
        battery_read(inspection(vec![], "collector", CoverageStatus::Partial))
    }

    #[test]
    fn live_battery_cadence_follows_what_the_read_shows() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut live = LiveBattery::default();
        assert!(live.due(start), "the first dashboard asks at once");
        // A laptop: every second 5-second tick, stamped when asked.
        live.requested(at(0));
        live.record(one_battery(), Some("selection-source-1".into()), at(1));
        assert!(!live.due(at(6)));
        assert!(live.due(at(10)));
        let d = shown(&live, at(6));
        assert_eq!(d.batteries.len(), 1);
        assert_eq!(d.battery_source_id.as_deref(), Some("selection-source-1"));
        assert!(!d.battery_unavailable);
        // A desktop: a complete read that shows nothing is repeated every five minutes only.
        live.requested(at(12));
        live.record(nothing(), None, at(12));
        assert!(!live.due(at(30)));
        assert!(!live.due(at(311)));
        assert!(live.due(at(312)));
        let d = shown(&live, at(13));
        assert!(d.batteries.is_empty() && !d.battery_unavailable);
        // A battery Windows lists but cannot read is retried at the normal pace and said so.
        let mut unreadable = LiveBattery::default();
        unreadable.requested(at(0));
        unreadable.record(
            battery_read(inspection(
                vec![],
                "battery_devices",
                CoverageStatus::Partial,
            )),
            None,
            at(0),
        );
        assert!(unreadable.due(at(10)));
        assert!(shown(&unreadable, at(1)).battery_unavailable);
        // A busy read retries soon and keeps the previous values while they are recent.
        let mut stalled = LiveBattery::default();
        stalled.requested(at(0));
        stalled.record(one_battery(), Some("selection-source-2".into()), at(0));
        stalled.requested(at(10));
        stalled.record(busy(), None, at(10));
        assert!(stalled.due(at(15)));
        let d = shown(&stalled, at(15));
        assert_eq!(d.batteries.len(), 1);
        assert_eq!(d.battery_source_id.as_deref(), Some("selection-source-2"));
    }

    #[test]
    fn stale_or_stalled_battery_values_are_not_shown_as_current() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut live = LiveBattery::default();
        live.requested(at(0));
        live.record(one_battery(), Some("selection-source-1".into()), at(0));
        assert_eq!(shown(&live, at(30)).batteries.len(), 1);
        // Old values (the window was hidden) are neither shown nor a failure.
        let d = shown(&live, at(31));
        assert!(d.batteries.is_empty() && d.battery_source_id.is_none());
        assert!(!d.battery_unavailable);
        // Reads that stop finishing while a battery was known: no stale charge, no expired
        // source, and an explicit "not reported".
        live.record(busy(), None, at(20));
        let d = shown(&live, at(25));
        assert_eq!(d.batteries.len(), 1, "recent values survive a busy read");
        let d = shown(&live, at(31));
        assert!(d.batteries.is_empty() && d.battery_source_id.is_none());
        assert!(d.battery_unavailable);
        // The next read that finishes shows the battery again.
        live.record(one_battery(), Some("selection-source-3".into()), at(40));
        let d = shown(&live, at(41));
        assert_eq!(d.batteries.len(), 1);
        assert_eq!(d.battery_source_id.as_deref(), Some("selection-source-3"));
        assert!(!d.battery_unavailable);
        // The dashboard JSON carries the three battery fields.
        let json = serde_json::to_value(&d).unwrap();
        assert!(json["batteries"].is_array());
        assert_eq!(json["battery_source_id"], "selection-source-3");
        assert_eq!(json["battery_unavailable"], false);
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
        // A model that wrote the name and the reference does not get the name twice.
        assert_eq!(
            localize(
                "editor.exe (proc-00000001) uses 5 GiB; proc-00000001 grew.",
                &inv.local_processes
            ),
            "editor.exe (PID 4242) uses 5 GiB; editor.exe (PID 4242) grew."
        );
    }

    fn observed(tool: &str, cloud: Value) -> pcagent_agent::Observation {
        pcagent_agent::Observation {
            observation_id: format!("obs-{tool}"),
            citation: String::new(),
            tool: tool.into(),
            measured_at_unix_ms: 0,
            status: pcagent_shared::Status::Unsupported,
            citable: false,
            facts: Vec::new(),
            limitations: Vec::new(),
            observation: cloud,
            storage_items: Vec::new(),
        }
    }

    #[test]
    fn admin_hints_come_only_from_needs_admin_coverage_of_offered_checks() {
        let mut inv = pcagent_agent::local_report(&fixture());
        assert!(report(&inv, "q").admin_hints.is_empty());
        inv.observations.push(observed(
            "get_approved_elevated_inspection",
            serde_json::json!({"task":"disk_reliability","coverage":[{"status":"needs_admin"}]}),
        ));
        inv.observations.push(observed(
            "get_approved_elevated_inspection",
            serde_json::json!({"task":"performance_trace","coverage":[{"status":"needs_admin"}]}),
        ));
        inv.observations.push(observed(
            "get_approved_elevated_inspection",
            serde_json::json!({"task":"battery_energy","coverage":[{"status":"ok"}]}),
        ));
        inv.observations.push(observed(
            "get_cleanup_opportunities",
            serde_json::json!({"coverage":[
                {"scope":"previous_windows","status":"needs_admin"},
                {"scope":"temporary_files","status":"ok"}]}),
        ));
        inv.observations.push(observed(
            "get_cleanup_opportunities",
            serde_json::json!({"coverage":[{"scope":"hibernation_file","status":"needs_admin"}]}),
        ));
        inv.observations.push(observed(
            "get_system_state",
            serde_json::json!({"coverage":[{"scope":"x","status":"needs_admin"}]}),
        ));
        assert_eq!(
            report(&inv, "q").admin_hints,
            vec!["system_caches".to_owned(), "disk_reliability".to_owned()]
        );
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
