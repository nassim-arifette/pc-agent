//! Linux inspections expose only positive numeric/category allowlists; names remain local.
mod battery;
mod cleanup;
mod command;
mod events;
mod hardware;
mod packages;
mod startup;
mod systemd;
use crate::{CoverageStatus as C, Inspection, Query};
use serde_json::json;
use std::{io::Read, path::Path, sync::atomic::AtomicBool, time::Duration};

fn read(path: &Path) -> Option<String> {
    let mut out = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(1_048_577)
        .read_to_string(&mut out)
        .ok()?;
    (out.len() <= 1_048_576).then_some(out)
}
fn number(path: &Path) -> Option<u64> {
    read(path)?.trim().parse().ok()
}

impl super::Inspector for super::Native {
    fn inspect(query: Query<'_>) -> Inspection {
        static BUSY: AtomicBool = AtomicBool::new(false);
        let query = match query {
            Query::Allowlisted { name, limit } => {
                let Some(name) = crate::QUERY_NAMES.iter().find(|n| **n == name) else {
                    let mut out = Inspection::new();
                    out.coverage(
                        "query",
                        C::Unsupported,
                        "Linux allowlist",
                        "Unknown query name.",
                    );
                    return out.finish();
                };
                Query::Allowlisted { name, limit }
            }
            Query::Startup => Query::Startup,
            Query::Services { limit } => Query::Services { limit },
            Query::Events { days, limit } => Query::Events { days, limit },
            Query::SystemState => Query::SystemState,
            Query::Power => Query::Power,
            Query::Battery => Query::Battery,
            Query::Apps { limit } => Query::Apps { limit },
            Query::Hardware => Query::Hardware,
            Query::Drivers {
                limit,
                windows_update,
            } => Query::Drivers {
                limit,
                windows_update,
            },
            Query::Cleanup => Query::Cleanup,
        };
        crate::bounded(
            &BUSY,
            "pcagent-linux-inspection",
            Duration::from_secs(30),
            move || {
                let mut out = Inspection::new();
                match query {
                    Query::Startup => startup::collect(&mut out),
                    Query::Services { limit } => {
                        systemd::units(&mut out, limit, false);
                        systemd::units(&mut out, limit, true);
                    }
                    Query::Events { days, limit } => events::collect(&mut out, days, limit),
                    Query::SystemState => system(&mut out),
                    Query::Power => {
                        battery::collect(&mut out, Path::new("/sys/class/power_supply"));
                        thermal(&mut out);
                    }
                    Query::Battery => {
                        battery::collect(&mut out, Path::new("/sys/class/power_supply"))
                    }
                    Query::Apps { limit } => packages::installed(&mut out, limit),
                    Query::Hardware => hardware::collect(&mut out),
                    Query::Drivers {
                        limit,
                        windows_update,
                    } => {
                        drivers(&mut out, limit);
                        packages::updates(&mut out, limit, windows_update);
                    }
                    Query::Cleanup => cleanup::collect(&mut out),
                    Query::Allowlisted { name, limit } => match name {
                        "system_events" | "application_events" => {
                            events::collect(&mut out, 7, limit)
                        }
                        "thermal_zones" => thermal(&mut out),
                        "storage_devices" | "physical_memory" => hardware::collect(&mut out),
                        "last_updates" => {
                            packages::updates(&mut out, limit, crate::WindowsUpdateMode::Offline)
                        }
                        _ => system(&mut out),
                    },
                }
                out.finish()
            },
        )
    }
    fn live_battery() -> Inspection {
        static BATTERY: AtomicBool = AtomicBool::new(false);
        crate::bounded(
            &BATTERY,
            "pcagent-linux-battery",
            Duration::from_secs(6),
            || {
                let mut out = Inspection::new();
                battery::collect(&mut out, Path::new("/sys/class/power_supply"));
                out.finish()
            },
        )
    }
    fn region_country() -> Option<String> {
        let locale = std::env::var("LC_TIME")
            .or_else(|_| std::env::var("LANG"))
            .ok()?;
        let country = locale.split_once('_')?.1.split(['.', '@']).next()?;
        (country.len() == 2 && country.bytes().all(|b| b.is_ascii_alphabetic()))
            .then(|| country.to_ascii_uppercase())
    }
}
fn system(out: &mut Inspection) {
    out.value("system", "os_family", "linux", "category", "Compiled OS");
    if let Some(load) = read(Path::new("/proc/loadavg")) {
        for (field, value) in ["load_average_1m", "load_average_5m", "load_average_15m"]
            .into_iter()
            .zip(load.split_whitespace())
        {
            out.value(
                "system",
                field,
                json!(value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && *v >= 0.0)),
                "runnable_tasks",
                "/proc/loadavg",
            );
        }
    } else {
        out.coverage(
            "load",
            C::Denied,
            "/proc/loadavg",
            "Load averages unreadable.",
        );
    }
    for resource in ["cpu", "memory", "io"] {
        let Some(text) = read(&Path::new("/proc/pressure").join(resource)) else {
            out.coverage(
                "pressure",
                C::Unsupported,
                "Linux PSI",
                "Pressure-stall information not available.",
            );
            continue;
        };
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let Some(kind) = fields.next().filter(|k| matches!(*k, "some" | "full")) else {
                continue;
            };
            for part in fields {
                let Some((window, value)) = part.split_once('=') else {
                    continue;
                };
                if !matches!(window, "avg10" | "avg60" | "avg300") {
                    continue;
                }
                let n = value
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite() && (0.0..=100.0).contains(v));
                out.value(
                    "system",
                    &format!("pressure_{resource}_{kind}_{window}"),
                    json!(n),
                    "percent",
                    "/proc/pressure (PSI)",
                );
            }
        }
    }
    let reboot = Path::new("/var/run/reboot-required").try_exists().ok();
    out.value(
        "system",
        "reboot_pending",
        json!(reboot),
        "boolean",
        "Distribution reboot-required marker",
    );
    out.coverage("system_state",C::Partial,"Linux procfs","Load includes runnable and uninterruptible tasks, not CPU percentage. PSI is kernel stall time; reboot markers are distribution-specific.");
}
fn thermal(out: &mut Inspection) {
    let Ok(entries) = std::fs::read_dir("/sys/class/thermal") else {
        out.coverage(
            "thermal",
            C::Unsupported,
            "sysfs thermal",
            "Thermal class unavailable.",
        );
        return;
    };
    let mut found = false;
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with("thermal_zone") {
            continue;
        }
        let entity = out.entity("thermal-zone", None);
        let temp = read(&e.path().join("temp"))
            .and_then(|s| s.trim().parse::<i64>().ok())
            .filter(|t| (-100_000..=250_000).contains(t))
            .map(|t| t as f64 / 1000.0);
        out.value(
            &entity,
            "temperature_celsius",
            json!(temp),
            "celsius",
            "sysfs thermal millidegrees",
        );
        found = true;
    }
    out.coverage(
        "thermal",
        if found { C::Partial } else { C::Unsupported },
        "sysfs thermal",
        "Only exposed thermal zones are covered; absent sensors are unknown.",
    );
}
fn drivers(out: &mut Inspection, limit: usize) {
    let Some(text) = read(Path::new("/proc/modules")) else {
        out.coverage(
            "drivers",
            C::Unsupported,
            "/proc/modules",
            "Loaded modules unavailable.",
        );
        return;
    };
    for line in text.lines().take(limit) {
        let Some(name) = line.split_whitespace().next() else {
            continue;
        };
        let entity = out.entity("driver", Some(name.into()));
        out.value(&entity, "driver_loaded", true, "boolean", "/proc/modules");
    }
    out.coverage("drivers",C::Partial,"/proc/modules","Loaded kernel modules only; built-in drivers and proprietary version metadata are not inferred.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os::Inspector;
    #[test]
    fn native_system_inspection_contains_real_pressure_and_no_command_lines() {
        let result = super::super::Native::inspect(Query::SystemState);
        assert!(result
            .observations
            .iter()
            .any(|o| o.metric == "load_average_1m" && !o.value.is_null()));
        assert!(result
            .observations
            .iter()
            .any(|o| o.metric == "os_family" && o.value == "linux"));
        assert!(!serde_json::to_string(&result).unwrap().contains("cmdline"));
    }
    #[test]
    fn battery_absence_is_an_explicit_empty_enumeration() {
        let root = std::env::temp_dir().join(pcagent_shared::new_id("empty-battery"));
        std::fs::create_dir(&root).unwrap();
        let mut result = Inspection::new();
        battery::collect(&mut result, &root);
        assert!(result.observations.is_empty());
        assert_eq!(result.coverage[0].status, C::Ok);
        assert!(result.coverage[0].detail.contains("No battery"));
        std::fs::remove_dir(root).unwrap();
    }
}
