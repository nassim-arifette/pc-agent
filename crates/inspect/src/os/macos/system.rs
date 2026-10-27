//! Public numeric state; uptime/load do not imply a healthy or unhealthy machine.
use super::{native, updates};
use crate::{CoverageStatus as C, Inspection};
use objc2_foundation::NSProcessInfo;

fn finite(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

pub(super) fn collect() -> Inspection {
    let mut out = updates::collect();
    let process = NSProcessInfo::processInfo();
    let version = process.operatingSystemVersion();
    out.value(
        "system",
        "os_version",
        format!(
            "{}.{}.{}",
            version.majorVersion, version.minorVersion, version.patchVersion
        ),
        "version",
        "NSProcessInfo.operatingSystemVersion",
    );
    out.value(
        "system",
        "uptime_seconds",
        finite(process.systemUptime()),
        "seconds",
        "NSProcessInfo.systemUptime",
    );
    out.value(
        "system",
        "active_processor_count",
        process.activeProcessorCount() as u64,
        "count",
        "NSProcessInfo.activeProcessorCount",
    );
    out.value(
        "system",
        "physical_memory_bytes",
        process.physicalMemory(),
        "bytes",
        "NSProcessInfo.physicalMemory",
    );
    out.coverage("system_state", C::Ok, "NSProcessInfo", "Public operating-system version, uptime, active processors and physical memory; no host name or environment.");
    let load = native::load_average();
    for (index, metric) in ["load_average_1m", "load_average_5m", "load_average_15m"]
        .iter()
        .enumerate()
    {
        out.value(
            "system",
            metric,
            load.map(|v| v[index]),
            "runnable_tasks",
            "getloadavg",
        );
    }
    out.coverage(
        "system_load",
        if load.is_some() { C::Ok } else { C::Error },
        "getloadavg",
        "Scheduler load is not CPU percent and is not normalized by processor count.",
    );
    out.coverage("pending_restart", C::Unsupported, "macOS public API", "No public system-wide pending-restart flag is available. An offered update's restart requirement is not a pending restart.");
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_native_numbers_remain_unknown() {
        assert_eq!(finite(-1.0), None);
        assert_eq!(finite(f64::NAN), None);
        assert_eq!(finite(f64::INFINITY), None);
        assert_eq!(finite(123.5), Some(123.5));
        let values = native::load_average().unwrap();
        assert!(values.iter().all(|v| v.is_finite() && *v >= 0.0));
    }
}
