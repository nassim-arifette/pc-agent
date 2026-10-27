use super::{number, read};
use crate::{CoverageStatus as C, Inspection};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Mutex, OnceLock},
};

fn reference(path: &Path) -> String {
    static REFS: OnceLock<Mutex<BTreeMap<std::path::PathBuf, String>>> = OnceLock::new();
    REFS.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .entry(path.into())
        .or_insert_with(|| pcagent_shared::new_id("battery"))
        .clone()
}
fn energy(path: &Path, field: &str, charge_field: &str) -> Option<u64> {
    number(&path.join(field)).map(|n| n / 1000).or_else(|| {
        // Charge (uAh) * design voltage (uV) -> mWh. No percentage is treated as energy.
        number(&path.join(charge_field))?
            .checked_mul(number(&path.join("voltage_min_design"))?)?
            .checked_div(1_000_000_000)
    })
}
pub(super) fn collect(out: &mut Inspection, root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => {
            out.coverage(
                "battery_devices",
                C::Unsupported,
                "sysfs power_supply",
                "Power-supply class is unavailable.",
            );
            return;
        }
    };
    let mut batteries = Vec::new();
    let mut ac = Vec::new();
    let mut partial = false;
    for entry in entries {
        let Ok(entry) = entry else {
            partial = true;
            continue;
        };
        let path = entry.path();
        match read(&path.join("type")).as_deref().map(str::trim) {
            Some("Battery") => batteries.push(path),
            Some("Mains" | "USB" | "USB_C" | "USB_PD") => {
                ac.push(number(&path.join("online")).map(|n| n != 0))
            }
            _ => {}
        }
    }
    for path in &batteries {
        if number(&path.join("present")) == Some(0) {
            continue;
        }
        let entity = reference(path);
        let remaining = energy(path, "energy_now", "charge_now");
        let full = energy(path, "energy_full", "charge_full");
        let design = energy(path, "energy_full_design", "charge_full_design");
        let status = read(&path.join("status"));
        let state = match status.as_deref().map(str::trim) {
            Some("Charging") => Some("charging"),
            Some("Discharging") => Some("discharging"),
            Some("Full") => Some("full"),
            Some("Not charging") => Some("idle"),
            _ => None,
        };
        let voltage = number(&path.join("voltage_now")).map(|n| n / 1000);
        let power = number(&path.join("power_now"))
            .map(|n| n / 1000)
            .or_else(|| {
                number(&path.join("current_now"))?
                    .checked_mul(number(&path.join("voltage_now"))?)?
                    .checked_div(1_000_000_000)
            });
        let signed = power
            .and_then(|p| i64::try_from(p).ok())
            .and_then(|p| match state {
                Some("charging") => Some(p),
                Some("discharging") => Some(-p),
                _ => None,
            });
        let percent = number(&path.join("capacity"))
            .or_else(|| remaining?.checked_mul(100)?.checked_div(full?))
            .filter(|p| *p <= 100);
        let consistent = full
            .zip(design)
            .map(|(f, d)| f > 0 && d > 0 && f <= d.saturating_mul(2));
        let retention = full
            .zip(design)
            .and_then(|(f, d)| f.checked_mul(100)?.checked_div(d))
            .filter(|v| *v <= 200);
        for (metric, value, unit) in [
            ("battery_kind", json!("system"), "category"),
            (
                "battery_capacity_units",
                json!("milliwatt_hours"),
                "category",
            ),
            (
                "battery_design_capacity_mwh",
                json!(design),
                "milliwatt_hours",
            ),
            (
                "battery_full_charge_capacity_mwh",
                json!(full),
                "milliwatt_hours",
            ),
            (
                "battery_remaining_capacity_mwh",
                json!(remaining),
                "milliwatt_hours",
            ),
            ("battery_charge_percent", json!(percent), "percent"),
            ("battery_voltage_mv", json!(voltage), "millivolts"),
            ("battery_power_rate_mw", json!(signed), "milliwatts"),
            (
                "battery_discharge_power_mw",
                json!(if state == Some("discharging") {
                    power
                } else {
                    None
                }),
                "milliwatts",
            ),
            (
                "battery_charge_power_mw",
                json!(if state == Some("charging") {
                    power
                } else {
                    None
                }),
                "milliwatts",
            ),
            ("battery_charge_state", json!(state), "category"),
            (
                "battery_on_ac",
                json!(if ac.contains(&Some(true)) {
                    Some(true)
                } else if !ac.is_empty() && ac.iter().all(Option::is_some) {
                    Some(false)
                } else {
                    None
                }),
                "boolean",
            ),
            (
                "battery_capacities_consistent",
                json!(consistent),
                "boolean",
            ),
            (
                "battery_capacity_retention_percent",
                json!(retention),
                "percent",
            ),
            (
                "battery_estimated_wear_percent",
                json!(retention.map(|p| 100u64.saturating_sub(p))),
                "percent",
            ),
            (
                "battery_cycle_count",
                json!(number(&path.join("cycle_count"))),
                "count",
            ),
            (
                "battery_estimated_runtime_at_current_rate_seconds",
                json!(if state == Some("discharging") {
                    remaining.and_then(|e| e.checked_mul(3600)?.checked_div(power?))
                } else {
                    None
                }),
                "seconds",
            ),
        ] {
            partial |= value.is_null();
            out.value(
                &entity,
                metric,
                value,
                unit,
                "/sys/class/power_supply (micro-unit conversion)",
            );
        }
    }
    out.coverage("battery_devices",if partial{C::Partial}else{C::Ok},"/sys/class/power_supply",
        if batteries.is_empty(){"No battery enumerated; this is not a zero-capacity battery."}else{"Only numeric energy/charge/status fields read; serials and manufacturer strings omitted. Runtime is an estimate at the current discharge rate."});
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sysfs_micro_units_and_missing_fields() {
        let dir = std::env::temp_dir().join(pcagent_shared::new_id("battery-fixture"));
        std::fs::create_dir_all(dir.join("BAT0")).unwrap();
        for (name, value) in [
            ("type", "Battery"),
            ("energy_now", "30000000"),
            ("energy_full", "50000000"),
            ("energy_full_design", "60000000"),
            ("status", "Discharging"),
            ("power_now", "10000000"),
            ("capacity", "60"),
        ] {
            std::fs::write(dir.join("BAT0").join(name), value).unwrap();
        }
        let mut out = Inspection::new();
        collect(&mut out, &dir);
        let v = |m| {
            out.observations
                .iter()
                .find(|o| o.metric == m)
                .unwrap()
                .value
                .clone()
        };
        assert_eq!(v("battery_remaining_capacity_mwh"), 30000);
        assert_eq!(v("battery_power_rate_mw"), -10000);
        assert_eq!(
            v("battery_estimated_runtime_at_current_rate_seconds"),
            10800
        );
        assert!(v("battery_cycle_count").is_null());
        assert!(!serde_json::to_string(&out)
            .unwrap()
            .contains(dir.to_str().unwrap()));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
