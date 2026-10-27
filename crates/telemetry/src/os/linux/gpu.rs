use super::{procfs, value};
use crate::gpu::{self, DeviceReading, Query};
use nvml_wrapper::{
    enum_wrappers::device::{Clock, TemperatureSensor},
    error::NvmlError,
    Nvml,
};
use pcagent_shared::{GpuDevice, Metric, Snapshot, Window};
use std::path::Path;

fn query<T>(result: Result<T, NvmlError>) -> Query<T> {
    result.map_err(|error| match error {
        NvmlError::NotSupported => Some(3),
        NvmlError::NoPermission => Some(4),
        NvmlError::FailedToLoadSymbol(_) => None,
        _ => Some(999),
    })
}
pub(super) fn collect(out: &mut Snapshot, nvml: &mut Option<Result<Nvml, ()>>) {
    if let Ok(api) = nvml.get_or_insert_with(|| Nvml::init().map_err(|_| ())) {
        if let Ok(count) = api.device_count() {
            for index in 0..count {
                let Ok(device) = api.device_by_index(index) else {
                    out.limitations
                        .push("An NVML device could not be opened.".into());
                    continue;
                };
                let reading = DeviceReading {
                    index,
                    name: device.name().ok(),
                    utilization_percent: query(device.utilization_rates().map(|u| u.gpu)),
                    memory: query(device.memory_info().map(|m| (m.total, m.used))),
                    temperature_celsius: query(
                        device.temperature(TemperatureSensor::Gpu).map(i64::from),
                    ),
                    temperature_source: "NVML.nvmlDeviceGetTemperature",
                    sm_clock_mhz: query(device.clock_info(Clock::SM)),
                    power_milliwatts: query(device.power_usage()),
                    event_reasons: query(device.current_throttle_reasons().map(|r| r.bits())),
                    reasons_source: "NVML.nvmlDeviceGetCurrentClocksThrottleReasons",
                };
                out.gpus.push(GpuDevice {
                    entity_ref: gpu::entity(index),
                    name: reading.name.clone(),
                    source: "NVML".into(),
                });
                out.evidence
                    .extend(gpu::device_evidence(&reading, &Window::instant()));
            }
        } else {
            out.limitations
                .push("NVML could not enumerate devices.".into());
        }
    }
    drm(out, Path::new("/sys/class/drm"), !out.gpus.is_empty());
    if out.gpus.is_empty() {
        out.limitations
            .push("No readable GPU was found through NVML or DRM sysfs.".into());
    }
}

fn number(path: &Path) -> Option<u64> {
    procfs::read(path).ok()?.trim().parse().ok()
}
pub(super) fn drm(out: &mut Snapshot, root: &Path, nvidia_found: bool) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_prefix("card")
            .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let device = entry.path().join("device");
        let vendor = procfs::read(&device.join("vendor")).ok();
        if nvidia_found && vendor.as_deref().is_some_and(|v| v.trim() == "0x10de") {
            continue;
        }
        let entity = format!("gpu-drm-{name}");
        out.gpus.push(GpuDevice {
            entity_ref: entity.clone(),
            name: Some(format!("DRM {name}")),
            source: "/sys/class/drm".into(),
        });
        let window = Window::instant();
        for (metric, file, scale) in [
            (Metric::GpuUtilizationBasisPoints, "gpu_busy_percent", 100),
            (Metric::GpuMemoryUsedBytes, "mem_info_vram_used", 1),
            (Metric::GpuMemoryTotalBytes, "mem_info_vram_total", 1),
        ] {
            let n = number(&device.join(file))
                .and_then(|n| n.checked_mul(scale))
                .filter(|n| metric != Metric::GpuUtilizationBasisPoints || *n <= 10000);
            value(
                out,
                metric,
                &entity,
                n,
                "/sys/class/drm/device",
                &window,
                "This DRM driver does not export the requested counter.",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permissions_and_unsupported_are_not_zero() {
        assert_eq!(query::<u32>(Err(NvmlError::NoPermission)), Err(Some(4)));
        assert_eq!(query::<u32>(Err(NvmlError::NotSupported)), Err(Some(3)));
    }
    #[test]
    fn drm_fixture_has_vram_and_unknown_utilization() {
        let root = std::env::temp_dir().join(pcagent_shared::new_id("drm-fixture"));
        std::fs::create_dir_all(root.join("card0/device")).unwrap();
        std::fs::write(root.join("card0/device/mem_info_vram_total"), "4294967296").unwrap();
        std::fs::write(root.join("card0/device/mem_info_vram_used"), "1048576").unwrap();
        let mut out = Snapshot::empty("fixture");
        drm(&mut out, &root, false);
        assert_eq!(out.gpus.len(), 1);
        assert_eq!(
            out.evidence
                .iter()
                .find(|e| e.metric == Metric::GpuMemoryTotalBytes)
                .unwrap()
                .value,
            Some(4294967296)
        );
        assert!(out
            .evidence
            .iter()
            .find(|e| e.metric == Metric::GpuUtilizationBasisPoints)
            .unwrap()
            .value
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
