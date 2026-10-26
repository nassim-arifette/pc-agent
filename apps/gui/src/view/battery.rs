//! Battery presentation and bounded freshness/cadence state.
use super::Dashboard;
use pcagent_agent::selection::is_battery_ref;
use pcagent_inspect::{CoverageStatus, Inspection, Observation};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

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
pub(super) const BATTERY_METRICS: [&str; 17] = [
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
pub(super) struct BatteryValues<'a> {
    observations: Vec<&'a Observation>,
    measured_at: u64,
    inconsistent: bool,
}
impl BatteryValues<'_> {
    /// A usable value: status ok, not null and of the expected type. A partial (contradicting)
    /// value is never shown; it marks the battery inconsistent instead.
    pub(super) fn take<T>(
        &mut self,
        metric: &str,
        convert: impl Fn(&Value) -> Option<T>,
    ) -> Option<T> {
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
    pub(super) fn number(&mut self, metric: &str) -> Option<f64> {
        self.take(metric, |v| v.as_f64().filter(|v| v.is_finite()))
    }
    pub(super) fn count(&mut self, metric: &str) -> Option<u64> {
        self.take(metric, Value::as_u64)
    }
    pub(super) fn flag(&mut self, metric: &str) -> Option<bool> {
        self.take(metric, Value::as_bool)
    }
    pub(super) fn category(&mut self, metric: &str, known: &[&str]) -> Option<String> {
        self.take(metric, |v| {
            v.as_str()
                .filter(|text| known.contains(text))
                .map(str::to_owned)
        })
    }
}

/// Why a battery of the read is not shown.
pub(super) enum NotShown {
    /// A UPS (`short_term`) or a non-system battery: not the PC's battery.
    Other,
    /// A system battery without any charge, state or AC value.
    Unread,
}

pub(super) fn battery(reference: &str, read: &Inspection) -> Result<BatteryView, NotShown> {
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
impl BatteryRead {
    /// `Capability::Battery`: Windows lists a system battery, readable or not. `None` for a busy
    /// read, which says nothing about the PC.
    pub fn battery_listed(&self) -> Option<bool> {
        (!self.busy).then_some(!self.batteries.is_empty() || self.unavailable)
    }
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
pub(super) const BATTERY_MAX_AGE: Duration = Duration::from_secs(10);
/// After a complete read that shows no system battery (a desktop, or only a UPS).
pub(super) const BATTERY_ABSENT_RECHECK: Duration = Duration::from_secs(300);
/// After a busy read, while a previous read is still blocked in the driver.
pub(super) const BATTERY_BUSY_RETRY: Duration = Duration::from_secs(5);
/// Values older than this are no longer shown as current, and their selection source (well
/// inside its five-minute lifetime) is no longer offered.
pub(super) const BATTERY_STALE_AFTER: Duration = Duration::from_secs(30);

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
