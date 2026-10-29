//! Live resource and volume projections for the desktop dashboard.
use super::{
    measurement::{measure, ratio},
    process_name, BatteryView, Measure,
};
use pcagent_shared::{
    format_bytes, gpu_slowdown_codes, gpu_slowdown_reasons, unix_ms, HardwareInventory, Metric,
    ProcessCoverage, Snapshot, VolumeKind,
};
use serde::Serialize;

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
    /// The NVML index a temperature watch names (`gpu-nvidia-N`); `None` for another source.
    pub nvml_index: Option<u32>,
    pub temperature: Measure,
    pub memory_used: Measure,
    pub memory_total: Measure,
    pub memory_ratio: Option<f64>,
    pub utilization: Measure,
    pub sm_clock: Measure,
    pub power: Measure,
    pub slowdown: Measure,
    pub slowdown_reasons: Vec<String>,
    /// The same reasons as closed codes (`thermal_driver`…), for the window's language.
    pub slowdown_codes: Vec<&'static str>,
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
    /// `fixed` or `ram_disk`.
    pub kind: VolumeKind,
    /// The file system Windows names for the volume (`NTFS`, `ReFS`, `exFAT`...); `None` when it
    /// could not be read. The quick drive analysis reads fixed NTFS volumes only.
    pub filesystem: Option<String>,
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub free_ratio: f64,
}

/// Fixed and RAM volumes whose size and free space were both read; others are left out rather
/// than shown as empty. `filesystem` names a shown volume's file system from its root.
pub fn volumes(
    inventory: &HardwareInventory,
    filesystem: impl Fn(&str) -> Option<String>,
) -> Vec<VolumeView> {
    let Some(list) = inventory.volumes.value.as_ref() else {
        return Vec::new();
    };
    list.iter()
        .filter(|v| matches!(v.kind, VolumeKind::Fixed | VolumeKind::RamDisk))
        .filter_map(|v| {
            let (total, free) = (v.total_bytes?, v.free_bytes?);
            (total > 0).then(|| VolumeView {
                root: v.root.clone(),
                kind: v.kind,
                filesystem: filesystem(&v.root),
                total_bytes: total,
                free_bytes: free.min(total),
                free_ratio: free.min(total) as f64 / total as f64,
            })
        })
        .collect()
}

/// `Capability::NtfsVolumes` from the shown volumes: a fixed NTFS volume, the only kind the
/// helper's `mft_inventory` reads. Unknown (`None`) while a fixed volume's file system could not
/// be read and no other one is NTFS.
pub fn ntfs_volumes(volumes: &[VolumeView]) -> Option<bool> {
    let fixed = || volumes.iter().filter(|v| v.kind == VolumeKind::Fixed);
    if fixed().any(|v| {
        v.filesystem
            .as_deref()
            .is_some_and(|fs| fs.eq_ignore_ascii_case("NTFS"))
    }) {
        Some(true)
    } else if fixed().any(|v| v.filesystem.is_none()) {
        None
    } else {
        Some(false)
    }
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
                nvml_index: g
                    .entity_ref
                    .strip_prefix("gpu-nvidia-")
                    .and_then(|index| index.parse().ok()),
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
                slowdown_codes: slowdown.value.map(gpu_slowdown_codes).unwrap_or_default(),
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
