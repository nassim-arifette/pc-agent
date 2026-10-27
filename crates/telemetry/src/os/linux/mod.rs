//! Read-only Linux collectors. Process identities use boot-relative start ticks, never bare PIDs.
mod disks;
mod gpu;
mod inventory;
mod procfs;

use super::Backend;
use crate::{Collector, ProcessSort, ResourceUsage};
use pcagent_shared::{
    new_id, unix_ms, Evidence, HardwareInventory, InstanceState, Metric, Origin, ProcessTree,
    ProcessTreeNode, Snapshot, Status, Window, SCHEMA_VERSION,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    time::{Duration, Instant},
};

pub(crate) struct Native;
#[derive(Default)]
pub(crate) struct State {
    last: Option<Sample>,
    nvml: Option<Result<nvml_wrapper::Nvml, ()>>,
}
struct Sample {
    at: Instant,
    unix_ms: u64,
    cpu: BTreeMap<String, procfs::Cpu>,
    disks: BTreeMap<String, disks::Disk>,
    processes: procfs::Processes,
}
impl Sample {
    fn take(processes: bool) -> Self {
        Self {
            at: Instant::now(),
            unix_ms: unix_ms(),
            cpu: procfs::cpus(&procfs::read(Path::new("/proc/stat")).unwrap_or_default()),
            disks: disks::read(Path::new("/proc"), Path::new("/sys")),
            processes: if processes {
                procfs::processes(Path::new("/proc"))
            } else {
                procfs::Processes {
                    rows: Vec::new(),
                    pids: HashSet::new(),
                    coverage: Default::default(),
                    complete: false,
                }
            },
        }
    }
}
pub(super) fn processors() -> u32 {
    procfs::read(Path::new("/proc/stat"))
        .ok()
        .map(|s| procfs::cpus(&s).len().saturating_sub(1) as u32)
        .filter(|n| *n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|p| p.get() as u32)
                .unwrap_or(1)
        })
}
pub(super) fn value(
    out: &mut Snapshot,
    metric: Metric,
    entity: &str,
    n: Option<u64>,
    source: &str,
    window: &Window,
    reason: &str,
) {
    out.evidence.push(match n {
        Some(n) => Evidence::measured(metric, entity, n, source, window.clone()),
        None => Evidence::missing(metric, entity, source, Status::Unsupported, reason),
    });
}
fn memory(out: &mut Snapshot) {
    let text = procfs::read(Path::new("/proc/meminfo"));
    let fields = text.as_deref().map(procfs::fields).unwrap_or_default();
    for (key, metric) in [
        ("MemTotal", Metric::PhysicalTotalBytes),
        ("MemAvailable", Metric::PhysicalAvailableBytes),
        ("Committed_AS", Metric::SystemCommitBytes),
        ("CommitLimit", Metric::SystemCommitLimitBytes),
    ] {
        value(
            out,
            metric,
            "system",
            fields.get(key).copied(),
            &format!("/proc/meminfo.{key}"),
            &Window::instant(),
            "Kernel memory field is missing or invalid.",
        );
        if let Err(e) = &text {
            out.evidence.last_mut().unwrap().status = procfs::status(e);
        }
    }
    out.limitations.push("Linux Committed_AS is virtual memory promised under overcommit policy, not resident RAM. CommitLimit is kernel policy; it can be exceeded in permissive overcommit modes.".into());
}
fn rows(
    collector: &mut Collector,
    out: &mut Snapshot,
    before: &Sample,
    after: &Sample,
    window: &Window,
    limit: usize,
    sort: ProcessSort,
) {
    let previous: HashMap<_, _> = before
        .processes
        .rows
        .iter()
        .map(|p| ((p.pid, p.start), p))
        .collect();
    let mut ranked = Vec::new();
    for p in &after.processes.rows {
        let tracked = collector
            .registry
            .entry(p.pid, p.start, collector.boot_id.as_deref());
        tracked.instance.executable_name = Some(p.name.clone());
        let entity = tracked.instance.entity_ref.clone();
        let start = out.evidence.len();
        value(
            out,
            Metric::ProcessWorkingSetBytes,
            &entity,
            p.rss,
            "/proc/pid/stat.rss",
            &Window::instant(),
            "Resident page count is invalid.",
        );
        value(out,Metric::ProcessPrivateCommitBytes,&entity,None,"Linux memory semantics",&Window::instant(),"Linux has no per-process Windows private commit equivalent; virtual size is not substituted.");
        let prev = previous.get(&(p.pid, p.start)).copied();
        let cpu = prev.and_then(|b| {
            crate::process_cpu_basis_points(
                b.cpu,
                p.cpu,
                after.at.saturating_duration_since(before.at),
                processors(),
            )
        });
        value(
            out,
            Metric::ProcessCpuBasisPoints,
            &entity,
            cpu,
            "/proc/pid/stat.utime+stime",
            window,
            "No matching start-time identity or valid CPU interval.",
        );
        for (index, metric, rate) in [
            (
                0,
                Metric::ProcessIoReadBytes,
                Metric::ProcessIoReadBytesPerSecond,
            ),
            (
                1,
                Metric::ProcessIoWriteBytes,
                Metric::ProcessIoWriteBytesPerSecond,
            ),
        ] {
            let delta = prev.and_then(|b| p.io?[index].checked_sub(b.io?[index]));
            let source = if index == 0 {
                "/proc/pid/io.rchar"
            } else {
                "/proc/pid/io.wchar"
            };
            value(
                out,
                metric,
                &entity,
                delta,
                source,
                window,
                "I/O may be permission denied, or this process has no matching earlier reading.",
            );
            value(
                out,
                rate,
                &entity,
                delta.and_then(|n| n.checked_mul(1000)?.checked_div(window.duration_ms)),
                source,
                window,
                "No valid I/O interval.",
            );
            if p.io.is_none() {
                for e in &mut out.evidence[start..] {
                    if e.metric == metric || e.metric == rate {
                        e.status = Status::PermissionDenied;
                    }
                }
            }
        }
        value(
            out,
            Metric::ProcessIoOtherBytes,
            &entity,
            None,
            "Linux /proc",
            window,
            "Linux does not expose a Windows-style other-I/O byte count.",
        );
        for (metric, increase) in [
            (Metric::ProcessWorkingSetIncreaseBytes, true),
            (Metric::ProcessWorkingSetDecreaseBytes, false),
        ] {
            let n = prev.and_then(|b| {
                Some(if increase {
                    p.rss?.saturating_sub(b.rss?)
                } else {
                    b.rss?.saturating_sub(p.rss?)
                })
            });
            value(
                out,
                metric,
                &entity,
                n,
                "/proc/pid/stat.rss",
                window,
                "Memory growth needs two readings of the same start-time identity.",
            );
        }
        tracked.evidence = out.evidence.drain(start..).collect();
        tracked.observed_unix_ms = Some(after.unix_ms);
        ranked.push((tracked.instance.clone(), tracked.evidence.clone()));
    }
    if after.processes.complete {
        let seen = after
            .processes
            .rows
            .iter()
            .map(|p| (p.pid, p.start))
            .collect();
        collector.registry.sweep(&seen, &after.processes.pids);
    }
    ranked.sort_by(|a, b| {
        sort.rank(&b.1)
            .cmp(&sort.rank(&a.1))
            .then(a.0.pid.cmp(&b.0.pid))
    });
    out.process_coverage = after.processes.coverage.clone();
    for (instance, evidence) in ranked.into_iter().take(limit) {
        out.processes.push(instance);
        out.evidence.extend(evidence);
    }
    out.process_coverage.returned = out.processes.len();
    out.limitations.push("Process I/O is rchar/wchar including cached and non-disk I/O. Linux RSS is an approximate resident page count. PID/start ticks are local identity only. Processes may exit or be hidden by procfs mount permissions.".into());
}
impl Backend for Native {
    fn boot_id() -> Option<String> {
        let text = procfs::read(Path::new("/proc/stat")).ok()?;
        let seconds = text
            .lines()
            .find_map(|l| l.strip_prefix("btime ")?.parse::<u64>().ok())?;
        Some(format!("boot-estimate-unix-s-{}", seconds / 60 * 60))
    }
    fn resource_usage() -> ResourceUsage {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        ResourceUsage {
            thread_cpu_100ns: u64::try_from(t.tv_sec).ok().and_then(|s| {
                s.checked_mul(10_000_000)?
                    .checked_add(u64::try_from(t.tv_nsec).ok()? / 100)
            }),
            process_working_set_bytes: procfs::process(Path::new("/proc"), std::process::id())
                .ok()
                .and_then(|p| p.rss),
            logical_processors: Some(processors()),
        }
    }
    fn collect(
        c: &mut Collector,
        processes: bool,
        limit: usize,
        fast: bool,
        sort: ProcessSort,
    ) -> Snapshot {
        let saved = c.linux.last.take().filter(|s| {
            fast && s.at.elapsed() >= Duration::from_millis(250)
                && s.at.elapsed() <= Duration::from_secs(60)
                && (!processes || s.processes.complete)
        });
        let before = match saved {
            Some(s) => s,
            None => {
                let s = Sample::take(processes);
                std::thread::sleep(Duration::from_secs(1));
                s
            }
        };
        let after = Sample::take(processes);
        let window = Window {
            started_at_unix_ms: before.unix_ms,
            finished_at_unix_ms: after.unix_ms,
            duration_ms: after.at.saturating_duration_since(before.at).as_millis() as u64,
        };
        let mut out = Snapshot::empty(&c.session_id);
        memory(&mut out);
        let busy = before
            .cpu
            .get("cpu")
            .zip(after.cpu.get("cpu"))
            .and_then(|(b, a)| procfs::busy(b, a));
        value(
            &mut out,
            Metric::CpuBusyBasisPoints,
            "system",
            busy,
            "/proc/stat (excluding guest double-counting)",
            &window,
            "CPU ticks are unavailable or regressed.",
        );
        let core = after
            .cpu
            .iter()
            .filter(|(k, _)| k.as_str() != "cpu")
            .filter_map(|(k, a)| procfs::busy(before.cpu.get(k)?, a))
            .max();
        value(
            &mut out,
            Metric::CpuBusiestCoreBasisPoints,
            "system",
            core,
            "/proc/stat (busiest logical CPU busy time)",
            &window,
            "No matching logical CPU tick window.",
        );
        disks::evidence(&mut out, &before.disks, &after.disks, &window);
        for metric in [
            Metric::HardFaultPagesPerSecondX100,
            Metric::GpuBusiestEngineBasisPoints,
        ] {
            value(
                &mut out,
                metric,
                "system",
                None,
                "Linux native coverage",
                &window,
                "No equivalent to Windows hard-fault pages or busiest GPU engine is measured.",
            );
        }
        gpu::collect(&mut out, &mut c.linux.nvml);
        if processes {
            rows(c, &mut out, &before, &after, &window, limit, sort);
        }
        c.linux.last = Some(after);
        out
    }
    fn collect_recording(c: &mut Collector, limit: usize, sort: ProcessSort) -> Snapshot {
        Self::collect(c, true, limit, true, sort)
    }
    fn activity(c: &mut Collector, seconds: u64, limit: usize) -> crate::ProcessActivity {
        let before = Sample::take(true);
        std::thread::sleep(Duration::from_secs(seconds));
        let after = Sample::take(true);
        let window = Window {
            started_at_unix_ms: before.unix_ms,
            finished_at_unix_ms: after.unix_ms,
            duration_ms: after.at.saturating_duration_since(before.at).as_millis() as u64,
        };
        let mut out = Snapshot::empty(&c.session_id);
        rows(
            c,
            &mut out,
            &before,
            &after,
            &window,
            limit,
            ProcessSort::Cpu,
        );
        let old: HashSet<_> = before
            .processes
            .rows
            .iter()
            .map(|p| (p.pid, p.start))
            .collect();
        let new: HashSet<_> = after
            .processes
            .rows
            .iter()
            .map(|p| (p.pid, p.start))
            .collect();
        let mut result=crate::ProcessActivity::unavailable(&c.session_id,Status::Partial,"Linux process I/O and descriptor access depend on procfs permissions. Private commit and per-process GPU attribution are unavailable.");
        result.window = window;
        result.evidence = out.evidence;
        result.coverage.processes = out.process_coverage;
        result.coverage.stable_instances = old.intersection(&new).count();
        result.coverage.new_instances = new.difference(&old).count();
        result.coverage.exited_instances = old.difference(&new).count();
        result.coverage.services_status = Status::Unsupported;
        result.coverage.gpu_status = Status::Unsupported;
        for instance in out.processes {
            let Some(p) = after
                .processes
                .rows
                .iter()
                .find(|p| p.pid == instance.pid && p.start == instance.creation_time_filetime)
            else {
                continue;
            };
            // The existing row contract requires an observed descriptor count; do not invent zero.
            let Some(handles) = p.descriptors else {
                continue;
            };
            let application_ref = new_id("app");
            let parent_ref = after
                .processes
                .rows
                .iter()
                .find(|parent| parent.pid == p.parent && parent.start <= p.start)
                .and_then(|parent| c.registry.get((parent.pid, parent.start)))
                .map(|t| t.instance.entity_ref.clone());
            result.applications.push(crate::ApplicationActivity {
                entity_ref: application_ref.clone(),
                name: instance.executable_name.clone(),
                process_refs: vec![instance.entity_ref.clone()],
                process_count: 1,
                members_truncated: false,
            });
            result.processes.push(crate::ActivityProcess {
                instance,
                parent_ref,
                application_ref,
                role: if p.kind == "kernel_or_unavailable" {
                    crate::ProcessRole::System
                } else {
                    crate::ProcessRole::Application
                },
                handles,
                threads: p.threads,
                session: p.session,
                services: Vec::new(),
            });
        }
        result.coverage.applications_total = result.applications.len();
        result.coverage.applications_returned = result.applications.len();
        result.coverage.processes.returned = result.processes.len();
        result
    }
    fn process_tree(c: &mut Collector) -> ProcessTree {
        let list = procfs::processes(Path::new("/proc"));
        let nodes: Vec<_> = list
            .rows
            .iter()
            .map(|p| {
                let t = c.registry.entry(p.pid, p.start, c.boot_id.as_deref());
                t.instance.executable_name = Some(p.name.clone());
                ProcessTreeNode {
                    pid: p.pid,
                    parent_pid: p.parent,
                    entity_ref: Some(t.instance.entity_ref.clone()),
                    creation_time_filetime: Some(p.start),
                    executable_name: Some(p.name.clone()),
                    status: Status::Ok,
                }
            })
            .collect();
        ProcessTree {schema_version:SCHEMA_VERSION,id:new_id("tree"),session_id:c.session_id.clone(),origin:Origin::Live,status:list.coverage.status,
            window:Window::instant(),edges:crate::tree::link(&nodes),nodes,truncated:!list.complete,
            limitations:vec!["Observed Linux parent relationships can change through reparenting; they never authorize an action.".into()]}
    }
    fn revalidate(pid: u32, created: u64) -> InstanceState {
        match procfs::process(Path::new("/proc"), pid) {
            Ok(p) if p.start == created => InstanceState::Running,
            Ok(_) => InstanceState::PidReused,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => InstanceState::Exited,
            Err(_) => InstanceState::Unverified,
        }
    }
    fn hardware_inventory() -> HardwareInventory {
        inventory::collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_procfs_has_measurements_and_self_identity() {
        let mut c = Collector::new();
        let snapshot = c.snapshot(true, 50);
        assert!(snapshot.system_value(Metric::PhysicalTotalBytes).unwrap().1 > 0);
        assert!(snapshot.system_value(Metric::CpuBusyBasisPoints).unwrap().1 <= 10000);
        assert!(snapshot.process_coverage.readable > 0);
        let tree = c.process_tree();
        let me = tree
            .nodes
            .iter()
            .find(|p| p.pid == std::process::id())
            .unwrap();
        assert_eq!(
            c.process_details(me.entity_ref.as_deref().unwrap()).state,
            InstanceState::Running
        );
        assert!(c
            .hardware_inventory()
            .volumes
            .value
            .unwrap()
            .iter()
            .any(|v| v.total_bytes.is_some()));
        assert!(Native::resource_usage().thread_cpu_100ns.is_some());
    }
}
