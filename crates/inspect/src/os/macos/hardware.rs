//! Public Metal/Foundation readings; no private SMC keys or inferred temperatures.
use super::native;
use crate::{CoverageStatus as C, Inspection};
use objc2_foundation::{NSProcessInfo, NSProcessInfoThermalState};
use objc2_metal::MTLDevice;
use pcagent_shared::{new_id, Observed, Status};

fn status(status: Status) -> C {
    match status {
        Status::Ok => C::Ok,
        Status::PermissionDenied => C::Denied,
        Status::Unsupported => C::Unsupported,
        Status::Error => C::Error,
        _ => C::Partial,
    }
}

fn reading<T: serde::Serialize>(
    out: &mut Inspection,
    entity: &str,
    key: &str,
    unit: &str,
    observed: Observed<T>,
) {
    value(
        out,
        entity,
        key,
        observed
            .value
            .and_then(|value| serde_json::to_value(value).ok()),
        unit,
        &observed.source,
        status(observed.status),
    );
}

fn value(
    out: &mut Inspection,
    entity: &str,
    key: &str,
    value: Option<serde_json::Value>,
    unit: &str,
    source: &str,
    status: C,
) {
    out.value(entity, key, value.unwrap_or_default(), unit, source);
    out.observations.last_mut().expect("just appended").status = status;
}

pub(super) fn collect() -> Inspection {
    let mut out = Inspection::new();
    let inventory = pcagent_telemetry::Collector::new().hardware_inventory();
    reading(&mut out, "system", "os_version", "", inventory.os_version);
    reading(
        &mut out,
        "system",
        "architecture",
        "",
        inventory.architecture,
    );
    reading(
        &mut out,
        "system",
        "logical_processor_count",
        "count",
        inventory.logical_processors,
    );
    reading(
        &mut out,
        "system",
        "installed_memory_bytes",
        "bytes",
        inventory.installed_memory_bytes,
    );
    out.coverage(
        "hardware",
        C::Partial,
        "Darwin.sysctl",
        "OS version, architecture, logical processors and memory; no serials or machine identifiers.",
    );
    for volume in inventory.volumes.value.unwrap_or_default() {
        let entity = new_id("volume");
        value(
            &mut out,
            &entity,
            "volume_total_bytes",
            volume.total_bytes.map(Into::into),
            "bytes",
            &volume.source,
            status(volume.status),
        );
        value(
            &mut out,
            &entity,
            "volume_free_bytes",
            volume.free_bytes.map(Into::into),
            "bytes",
            &volume.source,
            status(volume.status),
        );
    }
    out.coverage(
        "volumes",
        status(inventory.volumes.status),
        "Darwin.getfsstat",
        "Mounted volumes, not physical disk inventory; APFS volumes can share one container's space.",
    );
    let devices = objc2_metal::MTLCopyAllDevices();
    value(
        &mut out,
        "system",
        "gpu_adapter_count",
        Some(devices.len().into()),
        "count",
        "Metal.MTLCopyAllDevices",
        C::Ok,
    );
    for device in devices.to_vec() {
        let entity = new_id("gpu");
        out.name_value(
            &entity,
            "gpu_name",
            Some(device.name().to_string()),
            "Metal.MTLDevice",
        );
        value(
            &mut out,
            &entity,
            "gpu_unified_memory",
            Some(device.hasUnifiedMemory().into()),
            "",
            "Metal.MTLDevice.hasUnifiedMemory",
            C::Ok,
        );
        value(
            &mut out,
            &entity,
            "gpu_low_power",
            Some(device.isLowPower().into()),
            "",
            "Metal.MTLDevice.isLowPower",
            C::Ok,
        );
        value(
            &mut out,
            &entity,
            "gpu_removable",
            Some(device.isRemovable().into()),
            "",
            "Metal.MTLDevice.isRemovable",
            C::Ok,
        );
        value(
            &mut out,
            &entity,
            "gpu_recommended_working_set_bytes",
            Some(device.recommendedMaxWorkingSetSize().into()),
            "bytes",
            "Metal.MTLDevice.recommendedMaxWorkingSetSize",
            C::Ok,
        );
    }
    out.coverage(
        "gpu",
        C::Partial,
        "Metal.MTLCopyAllDevices",
        "Only Metal-visible devices. Recommended working set is not VRAM capacity or usage; utilization is unavailable.",
    );
    thermal(&mut out);
    out.finish()
}

fn thermal(out: &mut Inspection) {
    let state = match NSProcessInfo::processInfo().thermalState() {
        NSProcessInfoThermalState::Nominal => Some("nominal_or_unavailable"),
        NSProcessInfoThermalState::Fair => Some("fair"),
        NSProcessInfoThermalState::Serious => Some("serious"),
        NSProcessInfoThermalState::Critical => Some("critical"),
        _ => None,
    };
    value(
        out,
        "system",
        "thermal_state",
        state.map(Into::into),
        "",
        "Foundation.NSProcessInfo.thermalState",
        C::Partial,
    );
    out.coverage(
        "thermal",
        C::Partial,
        "Foundation.NSProcessInfo.thermalState",
        "Thermal pressure, not a temperature. Apple returns nominal when unsupported; nominal is not proof of a measured normal temperature.",
    );
    out.coverage(
        "temperature_sensors",
        C::Unsupported,
        "macOS public APIs",
        "No stable public CPU/GPU temperature API; private SMC properties are not guessed.",
    );
    out.coverage(
        "powermetrics",
        C::NeedsAdmin,
        "macOS.powermetrics",
        "Hardware power samplers require separately approved elevated collection and are not run here.",
    );
}

pub(super) fn power() -> Inspection {
    let mut out = super::live_battery();
    // The property is available from macOS 12; deployment/runtime supports that boundary.
    value(
        &mut out,
        "system",
        "low_power_mode_enabled",
        Some(native::low_power_mode().into()),
        "",
        "Foundation.NSProcessInfo.isLowPowerModeEnabled",
        C::Partial,
    );
    out.coverage(
        "low_power_mode",
        C::Partial,
        "Foundation.NSProcessInfo.isLowPowerModeEnabled",
        "False can also mean unsupported; no unsupported system is reported as having measured power policy.",
    );
    thermal(&mut out);
    out.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_partial_readings_keep_their_status() {
        let mut out = Inspection::new();
        reading::<u64>(
            &mut out,
            "test",
            "bytes",
            "bytes",
            Observed::missing("fixture", Status::PermissionDenied),
        );
        assert!(out.observations[0].value.is_null());
        assert_eq!(out.observations[0].status, C::Denied);
        reading(
            &mut out,
            "test",
            "bytes",
            "bytes",
            Observed {
                value: Some(12_u64),
                source: "fixture".into(),
                status: Status::Partial,
            },
        );
        assert_eq!(out.observations[1].value, 12);
        assert_eq!(out.observations[1].status, C::Partial);
    }

    #[test]
    fn native_hardware_has_coverage_without_exporting_gpu_names() {
        let out = collect();
        assert!(out
            .coverage
            .iter()
            .any(|c| c.scope == "thermal" && c.status == C::Partial));
        assert!(out
            .coverage
            .iter()
            .any(|c| c.scope == "temperature_sensors" && c.status == C::Unsupported));
        let json = serde_json::to_string(&out).unwrap();
        for name in out.names.values() {
            assert!(!json.contains(name));
        }
        assert!(!json.contains("registryID"));
    }
}
