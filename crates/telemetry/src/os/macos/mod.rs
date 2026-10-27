//! macOS collectors; missing Windows-only semantics are not replaced by unrelated counters.
pub(crate) mod activity;
#[allow(unsafe_code)]
mod native;

use crate::{identity::CounterSample, Collector, ProcessSort, TickSample};
pub(crate) use native::{boot_id, resource_usage};

pub(crate) struct Native;

impl super::Backend for Native {
    fn boot_id() -> Option<String> {
        boot_id()
    }
    fn resource_usage() -> crate::ResourceUsage {
        resource_usage()
    }
    fn collect(
        collector: &mut Collector,
        processes: bool,
        limit: usize,
        fast: bool,
        sort: ProcessSort,
    ) -> Snapshot {
        collect(collector, processes, limit, fast, sort)
    }
    fn collect_recording(collector: &mut Collector, limit: usize, sort: ProcessSort) -> Snapshot {
        collect(collector, true, limit, true, sort)
    }
    fn activity(collector: &mut Collector, seconds: u64, limit: usize) -> crate::ProcessActivity {
        activity::collect(collector, seconds, limit)
    }
    fn process_tree(collector: &mut Collector) -> ProcessTree {
        process_tree(collector)
    }
    fn revalidate(pid: u32, created: u64) -> InstanceState {
        revalidate(pid, created)
    }
    fn hardware_inventory() -> HardwareInventory {
        hardware_inventory()
    }
}
use objc2_metal::MTLDevice;
use pcagent_shared::{
    new_id, unix_ms, Evidence, HardwareInventory, InstanceState, Metric, Observed, Origin,
    OsVersion, ProcessCoverage, ProcessTree, ProcessTreeNode, Snapshot, Status, Window,
    SCHEMA_VERSION,
};
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};

const MEMORY: &str = "Mach.host_statistics64(HOST_VM_INFO64)";
const PROCESS: &str = "libproc.proc_pidinfo(PROC_PIDTASKINFO)";

fn reading(metric: Metric, source: &str, value: Result<u64, Status>) -> Evidence {
    match value {
        Ok(v) => Evidence::measured(metric, "system", v, source, Window::instant()),
        Err(status) => Evidence::missing(
            metric,
            "system",
            source,
            status,
            "Native query unavailable.",
        ),
    }
}

pub(crate) fn collect(
    collector: &mut Collector,
    processes: bool,
    limit: usize,
    reuse: bool,
    sort: ProcessSort,
) -> Snapshot {
    let prior = collector.last_ticks.filter(|sample| {
        reuse
            && (Duration::from_millis(250)..=Duration::from_secs(60))
                .contains(&collector.started.elapsed().saturating_sub(sample.at))
    });
    if processes && prior.is_none() && !reuse {
        // Warm up every readable identity before the window, not just the previous top rows.
        let mut baseline = Snapshot::empty(&collector.session_id);
        collect_processes(collector, &mut baseline, limit, sort);
    }
    let first = prior.map(Ok).unwrap_or_else(|| {
        native::cpu_ticks().map(|ticks| TickSample {
            ticks,
            at: collector.started.elapsed(),
            unix_ms: unix_ms(),
        })
    });
    if prior.is_none() && !reuse {
        std::thread::sleep(Duration::from_secs(1));
    }
    let last = native::cpu_ticks().map(|ticks| TickSample {
        ticks,
        at: collector.started.elapsed(),
        unix_ms: unix_ms(),
    });
    collector.last_ticks = last.as_ref().ok().copied();
    let mut result = Snapshot::empty(&collector.session_id);
    result.evidence.push(match (first, last) {
        _ if reuse && prior.is_none() => Evidence::missing(
            Metric::CpuBusyBasisPoints,
            "system",
            "Mach.host_statistics(HOST_CPU_LOAD_INFO)",
            Status::Partial,
            "Counter warm-up: a later sample is needed for a measurable window.",
        ),
        (Ok(a), Ok(b)) => match crate::cpu_busy_basis_points(a.ticks, b.ticks) {
            Some(value) => Evidence::measured(
                Metric::CpuBusyBasisPoints,
                "system",
                value,
                "Mach.host_statistics(HOST_CPU_LOAD_INFO)",
                Window {
                    started_at_unix_ms: a.unix_ms,
                    finished_at_unix_ms: b.unix_ms,
                    duration_ms: b.at.saturating_sub(a.at).as_millis() as u64,
                },
            ),
            None => Evidence::missing(
                Metric::CpuBusyBasisPoints,
                "system",
                "Mach.host_statistics",
                Status::Partial,
                "CPU ticks regressed, wrapped, or did not advance.",
            ),
        },
        _ => Evidence::missing(
            Metric::CpuBusyBasisPoints,
            "system",
            "Mach.host_statistics",
            Status::Error,
            "Host CPU counters unavailable.",
        ),
    });
    result.evidence.push(reading(
        Metric::PhysicalTotalBytes,
        "sysctl.hw.memsize",
        native::memory_total(),
    ));
    let mut available = reading(
        Metric::PhysicalAvailableBytes,
        MEMORY,
        native::available_memory(),
    );
    available.limitations.push("Free plus inactive pages, including speculative pages once; an estimate of reclaimable memory, not Activity Monitor's memory pressure.".into());
    result.evidence.push(available);
    for metric in [Metric::SystemCommitBytes, Metric::SystemCommitLimitBytes] {
        result.evidence.push(Evidence::missing(metric, "system", "Darwin", Status::Unsupported,
            "Darwin has no equivalent Windows commit counter; swap or virtual address space is not substituted."));
    }
    for metric in crate::counters::COUNTER_METRICS {
        result.evidence.push(Evidence::missing(
            metric,
            "system",
            "Darwin",
            Status::Unsupported,
            "This counter has no validated native source on this host.",
        ));
    }
    if processes {
        collect_processes(collector, &mut result, limit, sort);
    }
    result
}

fn collect_processes(
    collector: &mut Collector,
    result: &mut Snapshot,
    limit: usize,
    sort: ProcessSort,
) {
    let pids = match native::pids() {
        Ok(pids) => pids,
        Err(status) => {
            result.process_coverage.status = status;
            return;
        }
    };
    let mut coverage = ProcessCoverage {
        enumerated: pids.len(),
        status: Status::Ok,
        ..Default::default()
    };
    let enumerated = pids.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    let processors = native::processors().unwrap_or(0);
    for pid in pids {
        let p = match native::process(pid) {
            Ok(p) => p,
            Err(Status::PermissionDenied) => {
                coverage.permission_denied += 1;
                continue;
            }
            Err(_) => {
                coverage.unavailable += 1;
                continue;
            }
        };
        coverage.readable += 1;
        seen.insert((p.pid, p.created));
        let sample = CounterSample {
            cpu_100ns: Some(p.cpu_100ns),
            io_bytes: p.io.map(|io| [io[0], io[1], 0]),
            at: collector.started.elapsed(),
            unix_ms: unix_ms(),
        };
        let tracked = collector
            .registry
            .entry(p.pid, p.created, collector.boot_id.as_deref());
        tracked.instance.executable_name = p.name;
        let entity = &tracked.instance.entity_ref;
        let previous = tracked
            .counters
            .filter(|s| sample.at.saturating_sub(s.at) <= Duration::from_secs(60));
        let window = previous
            .map(|s| Window {
                started_at_unix_ms: s.unix_ms,
                finished_at_unix_ms: sample.unix_ms,
                duration_ms: sample.at.saturating_sub(s.at).as_millis() as u64,
            })
            .unwrap_or_else(Window::instant);
        let mut evidence = vec![
            Evidence::measured(
                Metric::ProcessWorkingSetBytes,
                entity,
                p.resident,
                PROCESS,
                Window::instant(),
            ),
            Evidence::missing(
                Metric::ProcessPrivateCommitBytes,
                entity,
                "Darwin",
                Status::Unsupported,
                "Resident footprint and virtual address space are not Windows private commit.",
            ),
        ];
        let delta = |metric, value, source| match value {
            Some(v) => Evidence::measured(metric, entity, v, source, window.clone()),
            None => Evidence::missing(
                metric,
                entity,
                source,
                Status::Partial,
                "Counter unavailable or no stable previous instance/window.",
            ),
        };
        evidence.push(delta(
            Metric::ProcessCpuBasisPoints,
            previous.and_then(|s| {
                crate::process_cpu_basis_points(
                    s.cpu_100ns?,
                    p.cpu_100ns,
                    sample.at.checked_sub(s.at)?,
                    processors,
                )
            }),
            PROCESS,
        ));
        for (i, metric) in [Metric::ProcessIoReadBytes, Metric::ProcessIoWriteBytes]
            .into_iter()
            .enumerate()
        {
            let mut e = delta(
                metric,
                previous.and_then(|s| p.io?[i].checked_sub(s.io_bytes?[i])),
                "libproc.proc_pid_rusage(RUSAGE_INFO_V2)",
            );
            e.limitations.push(
                "Physical disk I/O bytes for this process; excludes network and cache-only I/O."
                    .into(),
            );
            evidence.push(e);
        }
        evidence.push(Evidence::missing(
            Metric::ProcessIoOtherBytes,
            entity,
            "Darwin",
            Status::Unsupported,
            "No equivalent other-I/O counter.",
        ));
        tracked.evidence = evidence;
        tracked.counters = Some(sample);
        tracked.observed_unix_ms = Some(sample.unix_ms);
        rows.push(tracked.clone());
    }
    collector.registry.sweep(&seen, &enumerated);
    rows.sort_by_key(|r| (std::cmp::Reverse(sort.rank(&r.evidence)), r.instance.pid));
    coverage.returned = rows.len().min(limit);
    if coverage.permission_denied > 0 || coverage.unavailable > 0 {
        coverage.status = Status::Partial;
    }
    result.process_coverage = coverage;
    for row in rows.into_iter().take(limit) {
        result.processes.push(row.instance);
        result.evidence.extend(row.evidence);
    }
}

pub(crate) fn revalidate(pid: u32, expected: u64) -> InstanceState {
    match native::creation(pid) {
        Ok(created) if created == expected => InstanceState::Running,
        Ok(_) => InstanceState::PidReused,
        Err(Status::Stale) => InstanceState::Exited,
        Err(_) => InstanceState::Unverified,
    }
}

pub(crate) fn process_tree(collector: &mut Collector) -> ProcessTree {
    let start = unix_ms();
    let clock = Instant::now();
    let mut tree = ProcessTree {
        schema_version: SCHEMA_VERSION,
        id: new_id("tree"),
        session_id: collector.session_id.clone(),
        origin: Origin::Live,
        status: Status::Ok,
        window: Window::instant(),
        nodes: vec![],
        edges: vec![],
        truncated: false,
        limitations: vec![
            "Observed parent relations can change through reparenting; never authorize actions."
                .into(),
        ],
    };
    match native::pids() {
        Err(status) => tree.status = status,
        Ok(pids) => {
            for pid in pids {
                match native::process(pid) {
                    Ok(p) => {
                        let tracked =
                            collector
                                .registry
                                .entry(pid, p.created, collector.boot_id.as_deref());
                        tracked.instance.executable_name = p.name.clone();
                        tree.nodes.push(ProcessTreeNode {
                            pid,
                            parent_pid: p.parent,
                            entity_ref: Some(tracked.instance.entity_ref.clone()),
                            creation_time_filetime: Some(p.created),
                            executable_name: p.name,
                            status: Status::Ok,
                        });
                    }
                    Err(status) => {
                        tree.status = Status::Partial;
                        tree.nodes.push(ProcessTreeNode {
                            pid,
                            parent_pid: 0,
                            entity_ref: None,
                            creation_time_filetime: None,
                            executable_name: None,
                            status,
                        });
                    }
                }
            }
        }
    }
    tree.edges = crate::tree::link(&tree.nodes);
    tree.window = Window::finish(start, clock);
    tree
}

fn observed<T>(result: Result<T, Status>, source: &str) -> Observed<T> {
    match result {
        Ok(v) => Observed::ok(v, source),
        Err(s) => Observed::missing(source, s),
    }
}

pub(crate) fn hardware_inventory() -> HardwareInventory {
    let version = native::text(c"kern.osproductversion").and_then(|text| {
        let parts = text
            .split('.')
            .map(str::parse::<u32>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| Status::Partial)?;
        Ok(OsVersion {
            major: *parts.first().ok_or(Status::Partial)?,
            minor: parts.get(1).copied().unwrap_or(0),
            build: parts.get(2).copied().unwrap_or(0),
            update_build_revision: None,
            display_version: Some(text),
            family: Some("macos".into()),
        })
    });
    let mut inventory = HardwareInventory {
        schema_version: SCHEMA_VERSION, id: new_id("inventory"), origin: Origin::Live,
        status: Status::Ok, collected_at_unix_ms: unix_ms(),
        os_version: observed(version, "sysctl.kern.osproductversion"),
        architecture: observed(native::text(c"hw.machine"), "sysctl.hw.machine"),
        logical_processors: observed(native::processors(), "sysctl.hw.logicalcpu"),
        installed_memory_bytes: observed(native::memory_total(), "sysctl.hw.memsize"),
        volumes: observed(native::volumes(), "Darwin.getfsstat"),
        gpu_adapters: Observed::ok(
            objc2_metal::MTLCopyAllDevices()
                .to_vec().into_iter()
                .map(|device| pcagent_shared::GpuAdapter {
                    description: device.name().to_string(),
                    dedicated_video_memory_bytes: None,
                })
                .collect(),
            "Metal.MTLCopyAllDevices",
        ),
        limitations: vec!["OS build holds the product patch version, not an Apple build identifier. No serial numbers collected.".into()],
    };
    if [
        inventory.os_version.status,
        inventory.architecture.status,
        inventory.logical_processors.status,
        inventory.installed_memory_bytes.status,
        inventory.volumes.status,
        inventory.gpu_adapters.status,
    ]
    .iter()
    .any(|s| *s != Status::Ok)
    {
        inventory.status = Status::Partial;
    }
    inventory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_warmup_is_unknown_and_keeps_the_counter_baseline() {
        let mut collector = Collector::new();
        let first = collector.sample_system();
        assert!(first.system_value(Metric::CpuBusyBasisPoints).is_none());
        assert!(collector.last_ticks.is_some());
        assert!(first.system_value(Metric::PhysicalTotalBytes).is_some());
        assert_eq!(crate::process_limit(usize::MAX), usize::MAX);
        assert_eq!(crate::process_limit(0), 1);
    }

    #[test]
    fn native_snapshot_and_details_preserve_identity_and_measurement_semantics() {
        let mut collector = Collector::new();
        let snapshot = collector.snapshot(true, usize::MAX);
        assert!(snapshot.system_value(Metric::PhysicalTotalBytes).unwrap().1 > 0);
        assert!(snapshot
            .system_value(Metric::PhysicalAvailableBytes)
            .is_some());
        assert!(snapshot.system_value(Metric::CpuBusyBasisPoints).unwrap().1 <= 10_000);
        assert!(snapshot.system_value(Metric::SystemCommitBytes).is_none());
        let own = snapshot
            .processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        let details = collector.process_details(&own.entity_ref);
        assert_eq!(details.state, InstanceState::Running);
        assert_eq!(
            revalidate(own.pid, own.creation_time_filetime + 1),
            InstanceState::PidReused
        );
        let tree = collector.process_tree();
        assert!(tree
            .nodes
            .iter()
            .any(|p| p.pid == own.pid
                && p.creation_time_filetime == Some(own.creation_time_filetime)));
        assert_eq!(
            hardware_inventory()
                .os_version
                .value
                .unwrap()
                .family
                .as_deref(),
            Some("macos")
        );
    }
}
