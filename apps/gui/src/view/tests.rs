use super::*;
use super::{
    battery::{BatteryRead, BATTERY_METRICS},
    measurement::MISSING_REASONS,
    report::{NOTES, RANKED, RANKED_KEYS},
    storage::item,
    usage::{amount, pico, PICO_PER_USD},
};
use pcagent_agent::capabilities::{Capability, Probes};
use pcagent_agent::credentials::Credentials;
use pcagent_agent::{AgentError, Category, CitationKind, Confidence, IssueKind, Pricing};
use pcagent_diagnostics::Classification;
use pcagent_inspect::{CoverageStatus, Inspection, Observation};
use pcagent_shared::{
    Evidence, GpuDevice, HardwareInventory, Metric, Phrase, ProcessInstance, Quantity, Snapshot,
    Status, VolumeKind, Window,
};
use pcagent_storage::{ItemKind, ItemView};
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

mod actions;
mod activity;
mod battery;
mod dashboard;
mod labels;
mod model_usage;
mod notes;
mod reports;
mod settings;
mod storage;

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
