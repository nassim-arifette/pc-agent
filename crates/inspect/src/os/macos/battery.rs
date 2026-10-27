use crate::{CoverageStatus as C, Inspection};
use pcagent_shared::{new_id, unix_ms, Window};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, OnceLock},
    time::Instant,
};

const SOURCE: &str = "IOKit.IOPSCopyPowerSourcesInfo/IOPSGetPowerSourceDescription";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    System,
    Ups,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Power {
    Ac,
    Battery,
    Offline,
}

#[derive(Default)]
pub(super) struct Battery {
    pub local_id: Option<i64>,
    pub present: Option<bool>,
    pub kind: Option<Kind>,
    pub current: Option<i64>,
    pub maximum: Option<i64>,
    pub charging: Option<bool>,
    pub power: Option<Power>,
    pub voltage_mv: Option<i64>,
    pub temperature_c: Option<i64>,
    pub empty_minutes: Option<i64>,
    pub full_minutes: Option<i64>,
}

fn percent(current: Option<i64>, maximum: Option<i64>) -> Option<f64> {
    let (current, maximum) = (current?, maximum?);
    (maximum > 0 && current >= 0 && current <= maximum)
        .then(|| current as f64 * 100.0 / maximum as f64)
}

fn seconds(minutes: Option<i64>) -> Option<i64> {
    minutes.filter(|minutes| *minutes >= 0)?.checked_mul(60)
}

fn project(out: &mut Inspection, entity: &str, battery: &Battery) {
    let consistent = match (battery.power, battery.charging) {
        (Some(Power::Battery | Power::Offline), Some(true)) => Some(false),
        (Some(_), Some(_)) => Some(true),
        _ => None,
    };
    let state = match (battery.power, battery.charging, consistent) {
        (Some(Power::Ac), Some(true), Some(true)) => Some("charging"),
        (Some(Power::Battery), Some(false), Some(true)) => Some("discharging"),
        (Some(Power::Ac), Some(false), Some(true)) => Some("idle"),
        _ => None,
    };
    let charge = percent(battery.current, battery.maximum);
    let capacity_consistent = match (battery.current, battery.maximum) {
        (Some(_), Some(_)) => Some(charge.is_some()),
        _ => None,
    };
    let values = [
        (
            "battery_kind",
            battery
                .kind
                .map(|kind| match kind {
                    Kind::System => "system",
                    Kind::Ups => "short_term",
                })
                .into(),
            "category",
        ),
        (
            "battery_capacity_units",
            Value::from("relative"),
            "category",
        ),
        ("battery_charge_percent", charge.into(), "percent"),
        (
            "battery_capacities_consistent",
            capacity_consistent.into(),
            "boolean",
        ),
        ("battery_state_consistent", consistent.into(), "boolean"),
        ("battery_charge_state", state.into(), "category"),
        (
            "battery_on_ac",
            battery
                .power
                .and_then(|power| match power {
                    Power::Ac => Some(true),
                    Power::Battery => Some(false),
                    Power::Offline => None,
                })
                .into(),
            "boolean",
        ),
        (
            "battery_voltage_mv",
            battery.voltage_mv.filter(|v| *v > 0).into(),
            "millivolts",
        ),
        (
            "battery_temperature_celsius",
            battery.temperature_c.into(),
            "celsius",
        ),
        (
            "battery_estimated_runtime_macos_seconds",
            (state == Some("discharging"))
                .then(|| seconds(battery.empty_minutes))
                .flatten()
                .into(),
            "seconds",
        ),
        (
            "battery_estimated_time_to_full_seconds",
            (state == Some("charging"))
                .then(|| seconds(battery.full_minutes))
                .flatten()
                .into(),
            "seconds",
        ),
    ];
    for (metric, value, unit) in values {
        out.value(entity, metric, value, unit, SOURCE);
    }
    // IOPS capacity is relative, usually percent or mAh, not energy in mWh.
    for (metric, unit) in [
        ("battery_design_capacity_mwh", "milliwatt_hours"),
        ("battery_full_charge_capacity_mwh", "milliwatt_hours"),
        ("battery_remaining_capacity_mwh", "milliwatt_hours"),
        ("battery_power_rate_mw", "milliwatts"),
        ("battery_capacity_retention_percent", "percent"),
        ("battery_estimated_wear_percent", "percent"),
        ("battery_cycle_count", "count"),
    ] {
        out.value(entity, metric, Value::Null, unit, SOURCE);
    }
    out.coverage(
        "battery_measurements",
        C::Partial,
        SOURCE,
        "Per-source relative capacity and state only; optional missing fields remain null. No energy, wear or per-application attribution is inferred from IOPS relative units.",
    );
    if consistent == Some(false) || capacity_consistent == Some(false) {
        out.coverage(
            "battery_consistency",
            C::Partial,
            SOURCE,
            "Native capacity bounds or state flags disagree; dependent percentages/state estimates are omitted.",
        );
    }
}

#[derive(Default)]
struct References(BTreeMap<i64, String>);

impl References {
    fn get(&mut self, id: Option<i64>) -> String {
        id.map(|id| {
            self.0
                .entry(id)
                .or_insert_with(|| new_id("battery"))
                .clone()
        })
        .unwrap_or_else(|| new_id("battery"))
    }
    fn retain(&mut self, present: &BTreeSet<i64>) {
        self.0.retain(|id, _| present.contains(id));
    }
}

pub(super) fn collect() -> Inspection {
    let started = unix_ms();
    let clock = Instant::now();
    let mut out = Inspection::new();
    let rows = match super::native::batteries() {
        Ok(rows) => rows,
        Err(()) => {
            out.coverage(
                "battery_devices",
                C::Error,
                SOURCE,
                "The native power-source snapshot or list could not be read.",
            );
            return out.finish();
        }
    };
    static REFERENCES: OnceLock<Mutex<References>> = OnceLock::new();
    let Ok(mut references) = REFERENCES
        .get_or_init(|| Mutex::new(References::default()))
        .lock()
    else {
        out.coverage(
            "battery_reference",
            C::Error,
            SOURCE,
            "The local identity cache was unavailable.",
        );
        return out.finish();
    };
    let mut ids = BTreeMap::<i64, usize>::new();
    for row in rows
        .iter()
        .flatten()
        .filter(|row| row.present == Some(true))
    {
        if let Some(id) = row.local_id {
            *ids.entry(id).or_default() += 1;
        }
    }
    let mut present = BTreeSet::new();
    let mut readable = 0;
    let mut complete = true;
    for row in rows {
        let Some(row) = row else {
            complete = false;
            continue;
        };
        if row.present == Some(false) {
            continue;
        }
        if row.present.is_none() || row.local_id.is_some_and(|id| ids[&id] != 1) {
            complete = false;
            continue;
        }
        if let Some(id) = row.local_id {
            present.insert(id);
        } else {
            out.coverage("battery_identity", C::Partial, SOURCE,
                "This source has no native identity; its new opaque reference must not be correlated across samples.");
        }
        let reference = references.get(row.local_id);
        project(&mut out, &reference, &row);
        readable += 1;
    }
    if complete {
        references.retain(&present);
    }
    out.coverage(
        "battery_devices",
        if !complete { C::Partial } else if readable == 0 { C::Unsupported } else { C::Ok },
        SOURCE,
        if !complete { "Some source descriptions, presence flags or identities were unavailable or ambiguous." }
        else if readable == 0 { "No present battery or UPS was reported by IOKit." }
        else { "All present power sources were inspected; native identifiers stay only in the local reference cache." },
    );
    out.window = Window::finish(started, clock);
    out.measured_at_unix_ms = out.window.finished_at_unix_ms;
    for observation in &mut out.observations {
        observation.window = out.window.clone();
        observation.measured_at_unix_ms = out.measured_at_unix_ms;
    }
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn value(out: &Inspection, metric: &str) -> Value {
        out.observations
            .iter()
            .find(|o| o.metric == metric)
            .unwrap()
            .value
            .clone()
    }

    #[test]
    fn relative_units_never_become_energy_and_source_ids_never_serialize() {
        let row = Battery {
            local_id: Some(918_273_645),
            present: Some(true),
            kind: Some(Kind::System),
            current: Some(50),
            maximum: Some(100),
            charging: Some(false),
            power: Some(Power::Battery),
            empty_minutes: Some(120),
            ..Default::default()
        };
        let mut out = Inspection::new();
        project(&mut out, "battery-fixture", &row);
        assert_eq!(value(&out, "battery_charge_percent"), 50.0);
        assert_eq!(value(&out, "battery_estimated_runtime_macos_seconds"), 7200);
        assert_eq!(value(&out, "battery_charge_state"), "discharging");
        assert!(value(&out, "battery_remaining_capacity_mwh").is_null());
        assert!(value(&out, "battery_estimated_time_to_full_seconds").is_null());
        assert!(out.names.is_empty());
        assert!(!serde_json::to_string(&out).unwrap().contains("918273645"));
    }

    #[test]
    fn invalid_capacities_and_unknown_minutes_are_not_zero() {
        for (current, maximum) in [(0, 0), (-1, 100), (101, 100)] {
            assert_eq!(percent(Some(current), Some(maximum)), None);
        }
        assert_eq!(percent(Some(0), Some(100)), Some(0.0));
        assert_eq!(percent(None, Some(100)), None);
        assert_eq!(seconds(Some(-1)), None);
        assert_eq!(seconds(Some(i64::MAX)), None);
        assert_eq!(seconds(Some(0)), Some(0));
    }

    #[test]
    fn contradictory_power_flags_suppress_dependent_state_and_estimate() {
        let row = Battery {
            current: Some(60),
            maximum: Some(50),
            power: Some(Power::Battery),
            charging: Some(true),
            empty_minutes: Some(20),
            full_minutes: Some(30),
            ..Default::default()
        };
        let mut out = Inspection::new();
        project(&mut out, "fixture", &row);
        assert_eq!(value(&out, "battery_state_consistent"), false);
        assert!(value(&out, "battery_charge_percent").is_null());
        assert!(value(&out, "battery_charge_state").is_null());
        assert!(value(&out, "battery_estimated_runtime_macos_seconds").is_null());
        assert!(value(&out, "battery_estimated_time_to_full_seconds").is_null());
    }

    #[test]
    fn references_are_stable_only_for_present_identified_sources() {
        let mut references = References::default();
        let first = references.get(Some(10));
        assert_eq!(references.get(Some(10)), first);
        assert_ne!(references.get(None), references.get(None));
        references.retain(&BTreeSet::new());
        assert_ne!(references.get(Some(10)), first);
    }
}
