//! Plain data for the desktop interface, built from the core's own types. Every number keeps its
//! raw value next to the text the core formats; a missing measurement carries its status and
//! reason and is never shown as zero. This is a local window, so local names (process names,
//! full paths) may appear here; nothing in this module is sent to a provider.
use pcagent_agent::selection::is_battery_ref;
use pcagent_agent::{
    credentials::{Credentials, Source},
    AgentError, Answer, Category, CitationKind, Confidence, Config, Cost, ExecutionStatus,
    ExtensionRequest, Investigation, IssueKind, ModelRequestUsage, Pricing, RequestUsage,
    SensorCoverage, Stats,
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
    /// For a process shown by its readable name: its executable name, the label's tooltip.
    pub executable_name: Option<String>,
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

/// A proposal as the person reviews it. A process target reads as the program it is ("Google
/// Chrome", its executable name as tooltip); its PID shows only when two targets would read the
/// same, or when no name is known at all.
pub fn plan(
    plan: &pcagent_actions::ActionPlan,
    decision: Option<&str>,
    readable: Readable<'_>,
) -> PlanView {
    use pcagent_actions::ResolvedTarget;
    let target = |kind: &str, label: String, files, bytes| TargetView {
        kind: kind.into(),
        label,
        executable_name: None,
        files,
        bytes,
    };
    // Each process target's readable and executable names, read once.
    let named: Vec<(Option<String>, Option<String>)> = plan
        .items()
        .iter()
        .map(|item| match &item.target {
            ResolvedTarget::Process(t) => {
                let instance = ProcessInstance {
                    entity_ref: String::new(),
                    pid: t.identity.pid,
                    creation_time_filetime: t.identity.creation_time_filetime,
                    boot_id: None,
                    executable_name: t.executable_name.clone(),
                };
                let executable = t
                    .executable_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned);
                (readable_name(&instance, readable), executable)
            }
            _ => (None, None),
        })
        .collect();
    let shown = |index: usize| {
        let (readable, executable) = &named[index];
        readable.as_deref().or(executable.as_deref())
    };
    let targets = plan
        .items()
        .iter()
        .enumerate()
        .map(|(index, item)| match &item.target {
            ResolvedTarget::File(f) => target("file", f.path.display().to_string(), Some(1), None),
            ResolvedTarget::SettingsPage { page } => {
                target("settings_page", page.uri().into(), None, None)
            }
            ResolvedTarget::CleanupAdapter(c) => target(
                "cleanup",
                c.root.display().to_string(),
                Some(c.files),
                c.bytes,
            ),
            ResolvedTarget::Process(t) => {
                let name = shown(index);
                let shared = name.is_some_and(|name| {
                    (0..named.len()).any(|other| {
                        other != index
                            && shown(other).is_some_and(|o| o.to_lowercase() == name.to_lowercase())
                    })
                });
                let label = match name {
                    Some(name) if !shared => name.to_owned(),
                    Some(name) => format!("{name} (PID {})", t.identity.pid),
                    None => format!("process (PID {})", t.identity.pid),
                };
                let (readable, executable) = &named[index];
                TargetView {
                    executable_name: readable.as_ref().and(executable.clone()),
                    ..target("process", label, None, None)
                }
            }
            ResolvedTarget::UserStartup(t) => target("startup", t.value_name().into(), None, None),
            ResolvedTarget::RecycleCleanup(c) => target(
                "cleanup",
                c.root().path.display().to_string(),
                Some(c.files().len() as u64),
                c.bytes(),
            ),
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
    /// The executable name: the tooltip of `readable_name`, and the only name a question may
    /// carry (C5).
    pub name: String,
    /// What the window shows when known: the executable's own description ("Google Chrome").
    /// Local display only, never sent anywhere.
    pub readable_name: Option<String>,
    pub pid: u32,
    pub cpu: Measure,
    pub working_set: Measure,
    pub services: Vec<ServiceView>,
}
/// An application group: its measurements are sums over its members during the window.
#[derive(Debug, Clone, Serialize)]
pub struct ApplicationView {
    pub entity_ref: String,
    /// The executable the group is named after, as for `MemberView::name`.
    pub name: String,
    /// The readable name of the member the group is named after, as for `MemberView`.
    pub readable_name: Option<String>,
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

/// The member an application group is named after, as the collector names it: the earliest
/// instance (creation time, then PID) of the group's executable among its listed members.
/// `instance` resolves a member reference, including members ranked too low to be returned.
fn naming_member(
    app: &pcagent_telemetry::ApplicationActivity,
    instance: &impl Fn(&str) -> Option<ProcessInstance>,
) -> Option<ProcessInstance> {
    let name = app.name.as_deref()?;
    app.process_refs
        .iter()
        .filter_map(|reference| instance(reference))
        .filter(|p| p.executable_name.as_deref() == Some(name))
        .min_by_key(|p| (p.creation_time_filetime, p.pid))
}

/// Application groups with their members and services, for the local Live PC view. `instance`
/// resolves member references (the live collector's registry); `readable` gives readable names.
pub fn activity(
    a: &pcagent_telemetry::ProcessActivity,
    instance: impl Fn(&str) -> Option<ProcessInstance>,
    readable: Readable<'_>,
) -> ActivityView {
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
                    readable_name: readable_name(&p.instance, readable),
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
                readable_name: naming_member(app, &instance)
                    .and_then(|p| readable_name(&p, readable)),
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

/// Readable program names by process instance (PID, creation time): the description or product
/// name of the executable's version resource, as Task Manager shows them ("Google Chrome"), read
/// on a worker thread (`pcagent_telemetry::ProgramNames`). They are for this window only: never
/// part of a model payload, a preview, a question or the conversation context (C5, note of 26
/// October), which keep executable names or opaque references.
pub type Readable<'a> = &'a dyn Fn(u32, u64) -> Option<String>;

/// For views built without readable names (a turn that failed before measuring anything).
pub fn no_readable_names(_pid: u32, _created: u64) -> Option<String> {
    None
}

/// The readable name the window shows for a process, when one is known and says more than its
/// executable name.
fn readable_name(p: &ProcessInstance, readable: Readable<'_>) -> Option<String> {
    let name = readable(p.pid, p.creation_time_filetime)?;
    let name = name.trim();
    let same = p
        .executable_name
        .as_deref()
        .is_some_and(|exe| exe.trim().eq_ignore_ascii_case(name));
    (!name.is_empty() && !same).then(|| name.to_owned())
}

/// How the window names a process: its readable name, else its executable name.
fn display_name(p: &ProcessInstance, readable: Readable<'_>) -> String {
    readable_name(p, readable).unwrap_or_else(|| process_name(p))
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
/// An exact sum of request costs, which may span several prices, and its display form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AmountView {
    /// Rounded to four decimals, or exact when that would show zero, like [`CostView::usd`].
    pub usd: String,
    pub exact_usd: String,
}
/// One model's share of a turn (C3 complement of 4 October): after an escalation, Super's and
/// Ultra's requests, tokens and costs stay apart. A sum is given only when all of it is known.
#[derive(Debug, Clone, Serialize)]
pub struct ModelUsageView {
    pub model_id: Option<String>,
    pub requests: usize,
    /// Sums over this model's requests; unknown as soon as one of them did not report the count.
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
    /// This model's requests without a cost: no reported usage, or no verified price.
    pub unpriced_requests: usize,
    /// The exact sum of this model's request costs; absent unless every one of them is priced.
    pub cost: Option<AmountView>,
    /// The verified, dated price of its priced requests, when they all used the same one.
    pub price: Option<PriceView>,
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
    /// `model_requests` grouped by model, in the order the models were first used.
    pub models: Vec<ModelUsageView>,
    /// `total_cost_usd` for display: absent exactly when it is, so never one model's price
    /// applied to another model's tokens.
    pub total_cost: Option<AmountView>,
    /// Without a total, the sum of the requests that have a cost, each at its own model's
    /// price: the turn cost at least this much. Never shown as a total.
    pub priced_cost_lower_bound: Option<AmountView>,
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

/// Whether `reference` stands at byte `at` of `text` as a whole word.
fn reference_at(text: &str, at: usize, reference: &str) -> bool {
    text[at..].starts_with(reference)
        && text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
        && text[at + reference.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_alphanumeric())
}

fn cites(text: &str, reference: &str) -> bool {
    text.match_indices(reference)
        .any(|(at, _)| reference_at(text, at, reference))
}

/// One process cited in a text: its reference, its executable name as the model may have written
/// it before the reference, and how the window names it there.
struct Label {
    reference: String,
    written: String,
    label: String,
}

/// How this window names the processes that texts shown together (an answer, a finding, a source)
/// cite by opaque reference (`proc-1a2b3c4d`): each by its readable name ("Google Chrome"), else
/// its executable name, with " (PID n)" only when two different processes cited there would
/// otherwise read the same. Display only: the model's own text, kept for the conversation
/// context, never changes.
pub struct ProcessLabels {
    /// Longest reference first, so a reference never matches inside a longer one.
    labels: Vec<Label>,
}
impl ProcessLabels {
    pub fn new(texts: &[&str], processes: &[ProcessInstance], readable: Readable<'_>) -> Self {
        let cited: Vec<(&ProcessInstance, String)> = processes
            .iter()
            .filter(|p| {
                !p.entity_ref.is_empty() && texts.iter().any(|text| cites(text, &p.entity_ref))
            })
            .map(|p| (p, display_name(p, readable)))
            .collect();
        let mut labels: Vec<Label> = cited
            .iter()
            .map(|(p, name)| {
                let shared = cited.iter().any(|(other, other_name)| {
                    other.entity_ref != p.entity_ref
                        && other_name.to_lowercase() == name.to_lowercase()
                });
                Label {
                    reference: p.entity_ref.clone(),
                    written: process_name(p),
                    label: if shared {
                        format!("{name} (PID {})", p.pid)
                    } else {
                        name.clone()
                    },
                }
            })
            .collect();
        labels.sort_by_key(|l| std::cmp::Reverse(l.reference.len()));
        Self { labels }
    }
    /// The text with each cited reference replaced by its label, in one pass: a label is never
    /// read again, so a program whose name looks like a reference cannot stand for another. A
    /// model that wrote the executable name before the reference ("chrome.exe (proc-1a2b3c4d)",
    /// as it does when several processes share a name) gets the label once.
    pub fn apply(&self, text: &str) -> String {
        if self.labels.is_empty() {
            return text.to_owned();
        }
        let mut out = String::with_capacity(text.len());
        // Where the last inserted label ends: what precedes it is never rewritten.
        let mut fixed = 0;
        let mut at = 0;
        'scan: while at < text.len() {
            for label in &self.labels {
                if !reference_at(text, at, &label.reference) {
                    continue;
                }
                let end = at + label.reference.len();
                let written = format!("{} (", label.written);
                let start = out.len().saturating_sub(written.len());
                let named = text[end..].starts_with(')')
                    && start >= fixed
                    && out.is_char_boundary(start)
                    && out[start..].eq_ignore_ascii_case(&written);
                if named {
                    out.truncate(start);
                }
                out.push_str(&label.label);
                fixed = out.len();
                at = if named { end + 1 } else { end };
                continue 'scan;
            }
            let c = text[at..]
                .chars()
                .next()
                .expect("at is a character boundary");
            out.push(c);
            at += c.len_utf8();
        }
        out
    }
    fn all(&self, texts: &[String]) -> Vec<String> {
        texts.iter().map(|text| self.apply(text)).collect()
    }
}

/// One text as the window shows it (`ProcessLabels`). The streamed draft goes through this same
/// function as the reviewed answer's text, so the answer does not change when the run ends.
pub fn localize(text: &str, processes: &[ProcessInstance], readable: Readable<'_>) -> String {
    ProcessLabels::new(&[text], processes, readable).apply(text)
}

pub fn finding(f: &Finding, processes: &[ProcessInstance], readable: Readable<'_>) -> FindingView {
    let mut texts = vec![
        f.support_reason.as_str(),
        f.summary.as_str(),
        f.next_step.as_str(),
    ];
    texts.extend(f.facts.iter().map(|fact| fact.text.as_str()));
    texts.extend(f.hypothesis.as_deref());
    texts.extend(
        [&f.counter_evidence, &f.missing, &f.limitations]
            .into_iter()
            .flatten()
            .map(String::as_str),
    );
    // A finding reads as one block: a process keeps one label across its texts.
    let labels = ProcessLabels::new(&texts, processes, readable);
    FindingView {
        classification: f.classification,
        label: f.classification.label(),
        support: f.support,
        support_reason: labels.apply(&f.support_reason),
        summary: labels.apply(&f.summary),
        facts: f
            .facts
            .iter()
            .map(|fact| labels.apply(&fact.text))
            .collect(),
        hypothesis: f.hypothesis.as_deref().map(|text| labels.apply(text)),
        counter_evidence: labels.all(&f.counter_evidence),
        missing: labels.all(&f.missing),
        next_step: labels.apply(&f.next_step),
        limitations: labels.all(&f.limitations),
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
/// What the window shows of an exact amount: four decimals rounded half-up, or the exact amount
/// when that would read zero.
fn shown_usd(rounded: String, exact: &str) -> String {
    if rounded == "0.0000" && exact != "0" {
        exact.to_owned()
    } else {
        rounded
    }
}
fn cost(c: &Cost) -> CostView {
    CostView {
        usd: shown_usd(c.rounded_usd(4), &c.usd),
        exact_usd: c.usd.clone(),
        price: price(&c.pricing),
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
/// The exact amount and its display form, rounded half-up to four decimals like a single cost.
fn amount(pico: u128) -> AmountView {
    let (whole, fraction) = (pico / PICO_PER_USD, pico % PICO_PER_USD);
    let exact_usd = if fraction == 0 {
        whole.to_string()
    } else {
        format!(
            "{whole}.{}",
            format!("{fraction:012}").trim_end_matches('0')
        )
    };
    // Picodollars per unit of the fourth decimal.
    const STEP: u128 = 100_000_000;
    let rounded = pico / STEP + u128::from(pico % STEP >= STEP / 2);
    AmountView {
        usd: shown_usd(
            format!("{}.{:04}", rounded / 10_000, rounded % 10_000),
            &exact_usd,
        ),
        exact_usd,
    }
}
/// The exact sum of these costs, each at its own price; `None` if one does not parse.
fn sum_costs<'a>(costs: impl IntoIterator<Item = &'a Cost>) -> Option<u128> {
    costs
        .into_iter()
        .try_fold(0u128, |sum, c| sum.checked_add(pico(&c.usd)?))
}
/// Requests grouped by model, in the order the models were first used.
fn models(requests: &[ModelRequestUsage]) -> Vec<ModelUsageView> {
    let mut groups: Vec<(Option<&str>, Vec<&ModelRequestUsage>)> = Vec::new();
    for request in requests {
        let id = request.model_id.as_deref();
        match groups.iter_mut().find(|(model, _)| *model == id) {
            Some((_, group)) => group.push(request),
            None => groups.push((id, vec![request])),
        }
    }
    groups
        .into_iter()
        .map(|(model_id, group)| {
            let tokens = |count: fn(&RequestUsage) -> Option<u64>| {
                group
                    .iter()
                    .try_fold(0u64, |sum, r| sum.checked_add(count(&r.usage)?))
            };
            let priced: Vec<&Cost> = group.iter().filter_map(|r| r.cost.as_ref()).collect();
            ModelUsageView {
                model_id: model_id.map(str::to_owned),
                requests: group.len(),
                prompt_tokens: tokens(|u| u.prompt_tokens),
                completion_tokens: tokens(|u| u.completion_tokens),
                reasoning_tokens: tokens(|u| u.reasoning_tokens),
                unpriced_requests: group.len() - priced.len(),
                cost: (priced.len() == group.len())
                    .then(|| sum_costs(priced.iter().copied()))
                    .flatten()
                    .map(amount),
                price: priced
                    .first()
                    .filter(|first| priced.iter().all(|c| c.pricing == first.pricing))
                    .map(|c| price(&c.pricing)),
            }
        })
        .collect()
}
pub fn usage(s: &Stats) -> Usage {
    let priced: Vec<&Cost> = s
        .model_requests
        .iter()
        .filter_map(|r| r.cost.as_ref())
        .collect();
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
        models: models(&s.model_requests),
        total_cost: s.total_cost_usd.as_deref().and_then(pico).map(amount),
        priced_cost_lower_bound: (s.total_cost_usd.is_none() && !priced.is_empty())
            .then(|| sum_costs(priced.iter().copied()))
            .flatten()
            .map(amount),
        known_prompt_tokens: s.known_prompt_tokens,
        known_completion_tokens: s.known_completion_tokens,
        unknown_requests: s.unknown_requests,
        cost_lower_bound: s.cost_lower_bound.as_ref().map(cost),
    }
}
fn answer(r: &Investigation, a: &Answer, readable: Readable<'_>) -> AnswerView {
    let processes = &r.local_processes;
    // The text is labelled on its own, exactly as its streamed draft was; quoted passages keep
    // its labels.
    let text = ProcessLabels::new(&[&a.text], processes, readable);
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
            // Each source reads as one block, labelled on its own.
            let (title, facts, earlier) = match c.kind {
                CitationKind::Observation => {
                    let o = r
                        .observations
                        .iter()
                        .chain(std::iter::once(&r.initial_observation))
                        .find(|o| o.observation_id == c.target)?;
                    (
                        o.tool.clone(),
                        local_block(&o.facts, processes, readable),
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
                    let facts: Vec<String> = std::iter::once(f.summary.clone())
                        .chain(f.facts.iter().map(|fact| fact.text.clone()))
                        .collect();
                    (
                        f.classification.label().to_owned(),
                        local_block(&facts, processes, readable),
                        earlier,
                    )
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
        text: text.apply(&a.text),
        confidence: a.confidence,
        category: a.category,
        citations,
        // Sent as the next question when clicked: they keep the model's own references.
        follow_ups: a.follow_ups.clone(),
        action_refs: a.action_refs.clone(),
        unverified: a
            .unverified
            .iter()
            .map(|i| IssueView {
                kind: i.kind,
                excerpt: text.apply(&i.excerpt),
            })
            .collect(),
        repairs: a.repairs,
    }
}
/// Texts shown together (the facts of one source), with one label per process.
fn local_block(
    texts: &[String],
    processes: &[ProcessInstance],
    readable: Readable<'_>,
) -> Vec<String> {
    let all: Vec<&str> = texts.iter().map(String::as_str).collect();
    ProcessLabels::new(&all, processes, readable).all(texts)
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
/// The finished turn as the window shows it. Process references in its texts become readable
/// names (`ProcessLabels`); `context` keeps the model's own text for the next turns.
pub fn report(r: &Investigation, question: &str, readable: Readable<'_>) -> Report {
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
            facts: local_block(&o.facts, processes, readable),
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
        answer: r.answer.as_ref().map(|a| answer(r, a, readable)),
        selected: finding(&r.selected, processes, readable),
        alternatives: r
            .alternatives
            .iter()
            .map(|f| finding(f, processes, readable))
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
        let instance = |r: &str, pid: u32, created: u64, exe: &str| serde_json::json!({"entity_ref":r,"pid":pid,"creation_time_filetime":created,"boot_id":null,"executable_name":exe});
        let process = |r: &str, pid: u32, created: u64, services: serde_json::Value| {
            serde_json::json!({
            "instance":instance(r, pid, created, "browser.exe"),
            "parent_ref":null,"application_ref":"app-1","role":"application","handles":1,"threads":1,"session":1,"services":services})
        };
        let raw: pcagent_telemetry::ProcessActivity = serde_json::from_value(serde_json::json!({
            "schema_version":1,"id":"a","session_id":"s","origin":"synthetic_fixture","status":"ok","window":window,
            "processes":[process("proc-1",10,5,serde_json::json!([])),
                process("proc-2",11,6,serde_json::json!([{"pid":11,"service_name":"Spooler","display_name":"Print Spooler","state":"running"}]))],
            "applications":[{"entity_ref":"app-1","name":"browser.exe","process_refs":["proc-1","proc-2","proc-0","proc-w"],"process_count":12,"members_truncated":false}],
            "evidence":[ev("process_cpu_basis_points","app-1",Some(940)),ev("process_working_set_bytes","app-1",Some(3 << 30)),
                ev("process_cpu_basis_points","proc-1",Some(700)),ev("process_gpu_basis_points","app-1",None)],
            "coverage":{"processes":{"status":"ok","enumerated":2,"readable":2,"permission_denied":0,"unavailable":0,"returned":2,"enumeration_truncated":false},
                "stable_instances":2,"new_instances":0,"exited_instances":0,"pseudo_processes":[],"services_status":"ok",
                "services_attributed":1,"services_unattributed":0,"gpu_status":"unsupported","gpu_processes":0,
                "gpu_instances_rejected":0,"gpu_instances_unattributed":0,"applications_total":5,"applications_returned":1},
            "limitations":[]
        })).unwrap();
        // The collector's registry also knows members ranked too low to be returned: the group's
        // earliest browser process, and an even earlier embedded runtime it started.
        let registry: Vec<ProcessInstance> = raw
            .processes
            .iter()
            .map(|p| p.instance.clone())
            .chain([
                serde_json::from_value(instance("proc-0", 9, 3, "browser.exe")).unwrap(),
                serde_json::from_value(instance("proc-w", 8, 2, "runtime.exe")).unwrap(),
            ])
            .collect();
        let lookup = |r: &str| registry.iter().find(|p| p.entity_ref == r).cloned();
        let names = |pid: u32, created: u64| match (pid, created) {
            (9, 3) => Some("Example Browser".to_owned()),
            (10, 5) => Some("  Example Browser ".to_owned()),
            (8, 2) => Some("Example Runtime".to_owned()),
            // A process that refused a query-only handle has no readable name.
            _ => None,
        };
        let view = activity(&raw, lookup, &names);
        let app = &view.applications[0];
        assert_eq!((app.name.as_str(), app.process_count), ("browser.exe", 12));
        // Named after its earliest browser process, never after the runtime it embeds.
        assert_eq!(app.readable_name.as_deref(), Some("Example Browser"));
        // The group's own sum, not its first member's value.
        assert_eq!(app.cpu.value, Some(940));
        assert_eq!(app.members.len(), 2);
        assert_eq!(app.members[0].cpu.value, Some(700));
        assert_eq!(
            (
                app.members[0].name.as_str(),
                app.members[0].readable_name.as_deref()
            ),
            ("browser.exe", Some("Example Browser"))
        );
        // Unknown: the page shows the executable name.
        assert_eq!(app.members[1].readable_name, None);
        assert_eq!(app.services[0].display_name, "Print Spooler");
        // Unknown GPU stays unknown, with its reason.
        assert_eq!(app.gpu.value, None);
        assert_eq!(app.gpu.note.as_deref(), Some("not measured"));
        assert_eq!(view.applications_total, 5);
        // Without readable names, or with one that only repeats the executable name, the page
        // keeps the executable name.
        let plain = activity(&raw, lookup, &no_readable_names);
        assert!(plain.applications[0].readable_name.is_none());
        assert!(plain.applications[0]
            .members
            .iter()
            .all(|m| m.readable_name.is_none()));
        let echo = activity(&raw, lookup, &|_, _| Some("BROWSER.EXE".to_owned()));
        assert!(echo.applications[0].readable_name.is_none());
        // A group whose members are all unknown to the registry stays unnamed.
        let unknown = activity(&raw, |_| None, &names);
        assert!(unknown.applications[0].readable_name.is_none());
        // Readable names stay in the view: the selection source is the raw activity.
        let shared = serde_json::to_string(&raw).unwrap();
        assert!(!shared.contains("Example Browser"));
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
        let r = report(&inv, "Why is my PC slow?", &names);
        assert_eq!(r.mode, "local_rules");
        assert!(r.usage.is_none());
        assert_eq!(r.question, "Why is my PC slow?");
        let contributor = inv
            .findings
            .iter()
            .find(|f| f.classification == Classification::LargestMemoryContributor)
            .expect("the fixture has a largest memory contributor");
        // Named as the window names it, without a PID: no other cited process reads the same.
        for (readable, shown) in [
            (&names as Readable<'_>, "Example Editor"),
            (&no_readable_names as Readable<'_>, "editor.exe"),
        ] {
            let text = serde_json::to_string(&finding(contributor, &inv.local_processes, readable))
                .unwrap();
            assert!(!text.contains("proc-00000001"), "{text}");
            assert!(text.contains(shown) && !text.contains("PID"), "{text}");
        }
        // A model that wrote the name and the reference does not get the name twice.
        assert_eq!(
            localize(
                "editor.exe (proc-00000001) uses 5 GiB; proc-00000001 grew.",
                &inv.local_processes,
                &names
            ),
            "Example Editor uses 5 GiB; Example Editor grew."
        );
        assert_eq!(
            localize(
                "Editor.EXE (proc-00000001) uses 5 GiB.",
                &inv.local_processes,
                &no_readable_names
            ),
            "editor.exe uses 5 GiB."
        );
    }

    /// Readable names of the synthetic processes: the fixture's editor (PID 4242), two browser
    /// processes and a program whose name differs only in case. Others have none.
    fn names(pid: u32, _created: u64) -> Option<String> {
        match pid {
            4242 => Some("Example Editor".to_owned()),
            40 | 41 => Some("Example Browser".to_owned()),
            50 => Some("example browser".to_owned()),
            _ => None,
        }
    }

    fn process(reference: &str, pid: u32, exe: &str) -> ProcessInstance {
        ProcessInstance {
            entity_ref: reference.into(),
            pid,
            creation_time_filetime: u64::from(pid),
            boot_id: None,
            executable_name: Some(exe.into()),
        }
    }

    #[test]
    fn the_pid_is_shown_only_when_two_cited_processes_would_read_the_same() {
        let processes = [
            process("proc-0000000a", 40, "browser.exe"),
            process("proc-0000000b", 41, "browser.exe"),
            process("proc-0000000c", 50, "other.exe"),
            process("proc-0000000d", 60, "tool.exe"),
            process("proc-0000000e", 61, "tool.exe"),
            process("proc-00000001", 4242, "editor.exe"),
        ];
        let l = |text: &str| localize(text, &processes, &names);
        // One browser process cited: its name alone, even though another one runs.
        assert_eq!(
            l("proc-0000000a uses 2 GiB [O1]."),
            "Example Browser uses 2 GiB [O1]."
        );
        // Two different processes that would read the same both get their PID; others do not.
        assert_eq!(
            l("browser.exe (proc-0000000a) and browser.exe (proc-0000000b) use 3 GiB; proc-00000001 is idle."),
            "Example Browser (PID 40) and Example Browser (PID 41) use 3 GiB; Example Editor is idle."
        );
        // Names that differ only in case read the same.
        assert_eq!(
            l("proc-0000000a, proc-0000000c"),
            "Example Browser (PID 40), example browser (PID 50)"
        );
        // Without readable names, executable names decide.
        assert_eq!(
            l("proc-0000000d then proc-0000000e"),
            "tool.exe (PID 60) then tool.exe (PID 61)"
        );
        assert_eq!(l("proc-0000000d alone"), "tool.exe alone");
        // The same process cited twice is still one process.
        assert_eq!(
            l("proc-0000000a, again proc-0000000a"),
            "Example Browser, again Example Browser"
        );
    }

    #[test]
    fn references_are_replaced_whole_and_in_one_pass() {
        let readable = |pid: u32, _: u64| match pid {
            1 => Some("proc-00000002".to_owned()),
            2 => Some("Real Program".to_owned()),
            3 => Some("My editor.exe".to_owned()),
            _ => None,
        };
        let processes = [
            process("proc-00000001", 1, "fake.exe"),
            process("proc-00000002", 2, "real.exe"),
            process("proc-00000003", 3, "x.exe"),
            process("proc-00000004", 4, "editor.exe"),
        ];
        let l = |text: &str| localize(text, &processes, &readable);
        // A readable name that looks like a reference is shown as written, never replaced again.
        assert_eq!(
            l("proc-00000001 and proc-00000002"),
            "proc-00000002 and Real Program"
        );
        // An inserted name is never taken for the model's own "name (reference)".
        assert_eq!(
            l("proc-00000003 (proc-00000004)"),
            "My editor.exe (editor.exe)"
        );
        // Whole references only: not inside a longer token, and not a draft's partial reference.
        assert_eq!(
            l("proc-000000011, xproc-00000001, proc-0000000"),
            "proc-000000011, xproc-00000001, proc-0000000"
        );
        assert_eq!(
            l("Évité : **proc-00000002**, déjà fermé."),
            "Évité : **Real Program**, déjà fermé."
        );
        assert_eq!(l("No process here."), "No process here.");
    }

    #[test]
    fn an_answer_names_programs_locally_and_its_context_keeps_the_model_text() {
        use pcagent_agent::{Answer, Citation, Issue};
        let mut inv = pcagent_agent::local_report(&fixture());
        inv.local_processes.extend([
            process("proc-0000000a", 40, "browser.exe"),
            process("proc-0000000b", 41, "browser.exe"),
        ]);
        let mut source = observed("get_processes", serde_json::json!({}));
        source.observation_id = "obs-1".into();
        source.facts = vec![
            "proc-0000000a used 2.00 GiB.".into(),
            "proc-0000000b used 1.00 GiB.".into(),
        ];
        inv.observations.push(source);
        inv.observation_refs.push("obs-1".into());
        let text = "Close browser.exe (proc-0000000a) to free 2 GiB [O1]; proc-00000001 can stay.";
        inv.answer = Some(Answer {
            text: text.into(),
            confidence: Confidence::Medium,
            category: Category::Memory,
            citations: vec![Citation {
                reference: "O1".into(),
                kind: CitationKind::Observation,
                target: "obs-1".into(),
            }],
            action_refs: Vec::new(),
            follow_ups: vec!["Why does proc-0000000a grow?".into()],
            unverified: vec![Issue {
                kind: IssueKind::UnsupportedNumber,
                excerpt: "proc-0000000a to free 2 GiB".into(),
            }],
            repairs: 0,
        });
        let r = report(&inv, "Why is my PC slow?", &names);
        let answer = r.answer.as_ref().unwrap();
        // One browser process in the text: no PID there, though its source cites two. The text
        // is exactly what its streamed draft showed.
        assert_eq!(
            answer.text,
            "Close Example Browser to free 2 GiB [O1]; Example Editor can stay."
        );
        assert_eq!(answer.text, localize(text, &inv.local_processes, &names));
        assert_eq!(
            answer.unverified[0].excerpt,
            "Example Browser to free 2 GiB"
        );
        // The source reads as one block: its two browser processes are told apart.
        assert_eq!(
            answer.citations[0].facts,
            [
                "Example Browser (PID 40) used 2.00 GiB.",
                "Example Browser (PID 41) used 1.00 GiB."
            ]
        );
        assert_eq!(r.observations[0].facts, answer.citations[0].facts);
        // A follow-up is sent as the next question when clicked: it keeps the model's reference.
        assert_eq!(answer.follow_ups, ["Why does proc-0000000a grow?"]);
        // The next turns' context keeps the model's own text: no readable name leaves the PC.
        assert_eq!(r.context.answer, text);
        let context = serde_json::to_string(&r.context).unwrap();
        assert!(!context.contains("Example"), "{context}");
    }

    #[test]
    fn a_process_target_reads_as_its_program_with_a_pid_only_to_tell_two_apart() {
        let target = |reference: &str, pid: u32, exe: Option<&str>| {
            serde_json::json!({"target_ref":reference,"target":{"kind":"process",
                "identity":{"pid":pid,"creation_time_filetime":u64::from(pid),"boot_id":"boot"},
                "executable_name":exe}})
        };
        let proposal: pcagent_actions::ActionPlan = serde_json::from_value(serde_json::json!({
            "schema_version":1,"plan_id":"plan-1","state":"awaiting_approval",
            "action_kind":"request_normal_close","target_refs":["proc-a","proc-b","proc-c","proc-d"],
            "items":[target("proc-a",40,Some("browser.exe")),target("proc-b",41,Some("browser.exe")),
                target("proc-c",4242,Some("editor.exe")),target("proc-d",77,None)],
            "evidence_refs":[],"estimated_immediate_reclaim_bytes":null,
            "recovery":"user_can_restart_application","preconditions":["same_process_instance"],
            "requires_local_approval":true,"created_at_unix_ms":0,"expires_at_unix_ms":1,
            "fingerprint":"fixture"
        }))
        .unwrap();
        let shown = |view: &PlanView| -> Vec<(String, Option<String>)> {
            view.targets
                .iter()
                .map(|t| (t.label.clone(), t.executable_name.clone()))
                .collect()
        };
        let named = |label: &str, exe: Option<&str>| (label.to_owned(), exe.map(str::to_owned));
        assert_eq!(
            shown(&plan(&proposal, None, &names)),
            [
                named("Example Browser (PID 40)", Some("browser.exe")),
                named("Example Browser (PID 41)", Some("browser.exe")),
                named("Example Editor", Some("editor.exe")),
                named("process (PID 77)", None),
            ]
        );
        // Without readable names: executable names, with no tooltip repeating them.
        assert_eq!(
            shown(&plan(&proposal, None, &no_readable_names))[1..3],
            [
                named("browser.exe (PID 41)", None),
                named("editor.exe", None)
            ]
        );
    }

    /// Per-model usage through the real engine, with scripted transports, synthetic prices and an
    /// in-memory calibration only: no network, no saved file, no claim about model quality.
    mod model_usage {
        use super::*;
        use pcagent_agent::routing::{
            Calibration, CalibrationCase, CalibrationOrigin, CalibrationRuntime, EscalationReason,
            RoutedModel, RoutingMode, RoutingSettings, CALIBRATION_SCHEMA_VERSION,
        };
        use pcagent_agent::{FunctionCall, Model, Reply, SnapshotTools, ToolCall, ToolChoice};
        use pcagent_shared::Origin;
        use std::collections::VecDeque;

        const SUPER: &str = "nvidia/nemotron-super-fixture";
        const ULTRA: &str = "nvidia/nemotron-ultra-fixture";
        type Replies = Vec<Result<Reply, AgentError>>;

        struct Scripted {
            id: &'static str,
            price: Pricing,
            replies: VecDeque<Result<Reply, AgentError>>,
        }
        impl Model for Scripted {
            fn complete(
                &mut self,
                _: &[Value],
                _: &Value,
                _: ToolChoice,
                _: Duration,
            ) -> Result<Reply, AgentError> {
                self.replies
                    .pop_front()
                    .expect("unexpected inference request")
            }
            fn model_id(&self) -> Option<&str> {
                Some(self.id)
            }
            fn pricing(&self) -> Option<Pricing> {
                Some(self.price.clone())
            }
        }
        fn scripted(id: &'static str, replies: Replies) -> Scripted {
            // Ultra's synthetic price is ten times Super's.
            let (prompt, completion) = if id == SUPER {
                ("0.0000003", "0.0000009")
            } else {
                ("0.000003", "0.000009")
            };
            Scripted {
                id,
                price: Pricing {
                    model_id: id.into(),
                    prompt_usd_per_token: prompt.into(),
                    completion_usd_per_token: completion.into(),
                    source: "synthetic fixture".into(),
                    read_at_unix_ms: 1_000,
                },
                replies: replies.into(),
            }
        }
        fn call(name: &str, arguments: Value, prompt: u64, completion: u64) -> Reply {
            Reply {
                calls: vec![ToolCall {
                    id: format!("call-{prompt}"),
                    kind: "function".into(),
                    function: FunctionCall {
                        name: name.into(),
                        arguments: arguments.to_string(),
                    },
                }],
                prompt_tokens: Some(prompt),
                completion_tokens: Some(completion),
                total_tokens: Some(prompt + completion),
                ..Reply::default()
            }
        }
        fn inspect() -> Result<Reply, AgentError> {
            Ok(call(
                "get_system_snapshot",
                serde_json::json!({}),
                4_700,
                80,
            ))
        }
        fn answer(confidence: &str, prompt: u64, completion: u64) -> Result<Reply, AgentError> {
            let arguments = serde_json::json!({
                "answer": "See [O1].", "confidence": confidence, "category": "other"
            });
            Ok(call(
                pcagent_agent::SUBMIT_ANSWER,
                arguments,
                prompt,
                completion,
            ))
        }
        fn snapshot() -> Snapshot {
            let mut s = Snapshot::empty("synthetic-model-usage");
            s.origin = Origin::SyntheticFixture;
            s.evidence.push(Evidence::measured(
                Metric::CpuBusyBasisPoints,
                "system",
                500,
                "synthetic fixture",
                Window::instant(),
            ));
            s
        }
        fn run(model: &mut impl Model) -> Report {
            let inv = pcagent_agent::investigate(
                model,
                "Inspect",
                &snapshot(),
                &mut SnapshotTools::new(|_, _| snapshot()),
                |_| {},
            );
            report(&inv, "Inspect", &no_readable_names)
        }
        /// Super's low-confidence answer hands the question to Ultra.
        fn escalated(ultra: Replies) -> Report {
            let settings = RoutingSettings {
                mode: RoutingMode::Automatic,
                super_model_id: Some(SUPER.into()),
                ultra_model_id: Some(ULTRA.into()),
            };
            let calibration = Calibration {
                schema_version: CALIBRATION_SCHEMA_VERSION,
                runtime: CalibrationRuntime::current(),
                super_model_id: SUPER.into(),
                ultra_model_id: ULTRA.into(),
                reviewed_at_unix_ms: 1,
                maximum_extra_cost_usd: "0.1".into(),
                cases: (1..=3)
                    .map(|i| CalibrationCase {
                        capture_sha256: format!("{i:064x}"),
                        primary_trace_sha256: format!("{:064x}", i + 10),
                        ultra_trace_sha256: format!("{:064x}", i + 20),
                        origin: CalibrationOrigin::Live,
                        human_reviewed: true,
                        trigger: EscalationReason::LowConfidence,
                        primary_requests: 2,
                        primary_quality: 1,
                        ultra_quality: 3,
                        primary_critical_error: false,
                        ultra_critical_error: false,
                        primary_cost_usd: "0.01".into(),
                        ultra_cost_usd: "0.02".into(),
                        primary_duration_ms: 10,
                        ultra_duration_ms: 20,
                    })
                    .collect(),
            };
            let primary = scripted(SUPER, vec![inspect(), answer("low", 5_000, 20)]);
            let mut model = RoutedModel::new(
                primary,
                Some(scripted(ULTRA, ultra)),
                &settings,
                Some(&calibration),
            )
            .unwrap();
            run(&mut model)
        }

        #[test]
        fn one_model_gets_one_priced_line_equal_to_the_legacy_cost() {
            let mut model = scripted(SUPER, vec![inspect(), answer("medium", 5_000, 20)]);
            let usage = run(&mut model).usage.unwrap();
            assert_eq!(usage.models.len(), 1);
            let only = &usage.models[0];
            assert_eq!(only.model_id.as_deref(), Some(SUPER));
            assert_eq!(only.requests, 2);
            assert_eq!(
                (only.prompt_tokens, only.completion_tokens),
                (Some(9_700), Some(100))
            );
            assert_eq!(only.unpriced_requests, 0);
            // 9,700 tokens in at $0.30 and 100 out at $0.90 per million: exactly $0.003.
            let cost = only.cost.clone().unwrap();
            assert_eq!(
                (cost.usd.as_str(), cost.exact_usd.as_str()),
                ("0.0030", "0.003")
            );
            let price = only.price.as_ref().unwrap();
            assert_eq!(price.model_id, SUPER);
            assert_eq!(price.input_per_million.as_deref(), Some("0.30"));
            assert_eq!(price.output_per_million.as_deref(), Some("0.90"));
            assert_eq!(price.read_at_unix_ms, 1_000);
            assert_eq!(usage.total_cost, Some(cost.clone()));
            assert_eq!(usage.total_cost_usd.as_deref(), Some("0.003"));
            assert_eq!(usage.cost.unwrap().exact_usd, cost.exact_usd);
            assert!(usage.priced_cost_lower_bound.is_none());
        }

        #[test]
        fn an_escalation_keeps_each_models_share_and_a_total_only_from_both_prices() {
            let r = escalated(vec![answer("medium", 6_000, 50)]);
            assert_eq!(r.answer.as_ref().unwrap().confidence, Confidence::Medium);
            let usage = r.usage.unwrap();
            let change = usage.routing.as_ref().unwrap().change.clone().unwrap();
            assert_eq!(
                (change.from_model_id.as_str(), change.to_model_id.as_str()),
                (SUPER, ULTRA)
            );
            assert_eq!(change.after_requests, 2);
            let ids: Vec<_> = usage.models.iter().map(|m| m.model_id.as_deref()).collect();
            assert_eq!(ids, [Some(SUPER), Some(ULTRA)]);
            let [first, second] = &usage.models[..] else {
                panic!("two models")
            };
            assert_eq!((first.requests, second.requests), (2, 1));
            assert_eq!(
                (second.prompt_tokens, second.completion_tokens),
                (Some(6_000), Some(50))
            );
            assert_eq!(first.cost.as_ref().unwrap().exact_usd, "0.003");
            // 6,000 in at $3 and 50 out at $9 per million: $0.01845, shown rounded half-up.
            let ultra = second.cost.clone().unwrap();
            assert_eq!(
                (ultra.usd.as_str(), ultra.exact_usd.as_str()),
                ("0.0185", "0.01845")
            );
            assert_eq!(
                second.price.as_ref().unwrap().input_per_million.as_deref(),
                Some("3.00")
            );
            let total = usage.total_cost.clone().unwrap();
            assert_eq!(
                (total.usd.as_str(), total.exact_usd.as_str()),
                ("0.0215", "0.02145")
            );
            assert_eq!(usage.total_cost_usd.as_deref(), Some("0.02145"));
            // Never both models' tokens at the last model's price.
            assert!(usage.cost.is_none() && usage.cost_lower_bound.is_none());
            assert!(usage.priced_cost_lower_bound.is_none());
        }

        #[test]
        fn an_unpriced_request_leaves_its_model_without_cost_and_the_turn_without_total() {
            let r = escalated(vec![Err(AgentError::Timeout)]);
            // Ultra's only request failed: the answer kept is Super's.
            assert_eq!(r.answer.as_ref().unwrap().confidence, Confidence::Low);
            let usage = r.usage.unwrap();
            let routing = usage.routing.as_ref().unwrap();
            assert_eq!(routing.last_request_error, Some(AgentError::Timeout));
            let ultra = &usage.models[1];
            assert_eq!(ultra.model_id.as_deref(), Some(ULTRA));
            assert_eq!((ultra.requests, ultra.unpriced_requests), (1, 1));
            assert!(ultra.prompt_tokens.is_none() && ultra.completion_tokens.is_none());
            assert!(ultra.cost.is_none() && ultra.price.is_none());
            assert!(usage.models[0].cost.is_some());
            assert!(usage.total_cost.is_none() && usage.total_cost_usd.is_none());
            // At least Super's priced requests, never presented as the total.
            let lower = usage.priced_cost_lower_bound.unwrap();
            assert_eq!(
                (lower.usd.as_str(), lower.exact_usd.as_str()),
                ("0.0030", "0.003")
            );
        }

        #[test]
        fn exact_amounts_parse_strictly_and_round_half_up() {
            assert_eq!(pico("0.0223698"), Some(22_369_800_000));
            assert_eq!(pico("2"), Some(2 * PICO_PER_USD));
            assert_eq!(pico("0.000000000001"), Some(1));
            for bad in [
                "",
                ".3",
                "3.",
                "-1",
                "1e-7",
                "0,3",
                " 1",
                "0.0000000000001",
                "NaN",
            ] {
                assert_eq!(pico(bad), None, "{bad:?}");
            }
            let shown = |usd: &str| amount(pico(usd).unwrap());
            assert_eq!(shown("0.0223698").usd, "0.0224");
            assert_eq!(shown("0.00005").usd, "0.0001");
            assert_eq!(shown("0.0000432").usd, "0.0000432");
            assert_eq!(shown("0").usd, "0.0000");
            assert_eq!(shown("12.5").usd, "12.5000");
            assert_eq!(shown("0.0000432").exact_usd, "0.0000432");
        }
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
        assert!(report(&inv, "q", &no_readable_names).admin_hints.is_empty());
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
            report(&inv, "q", &no_readable_names).admin_hints,
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
