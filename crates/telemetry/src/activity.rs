//! Identity-safe two-sample process activity and application grouping.
#![cfg_attr(not(windows), allow(dead_code))]
use crate::{
    counters::{clamp_share, scaled, Instances, Sampled},
    identity::CounterSample,
    process_cpu_basis_points,
    services::{ServiceProcess, ServiceProcesses},
    Collector,
};
use pcagent_shared::{
    new_id, new_short_ref, Evidence, InstanceState, Metric, Origin, ProcessCoverage,
    ProcessInstance, Status, Window, SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    time::Duration,
};

pub const DEFAULT_ACTIVITY_WINDOW_SECONDS: u64 = 3;
pub const MAX_ACTIVITY_WINDOW_SECONDS: u64 = 30;
const MAX_MEMBERS: usize = 50;
const SOURCE: &str = "NtQuerySystemInformation.SystemProcessInformation";
const GPU_SOURCE: &str = crate::counters::SRC_GPU;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProcessRole {
    Application,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityProcess {
    pub instance: ProcessInstance,
    pub parent_ref: Option<String>,
    pub application_ref: String,
    pub role: ProcessRole,
    pub handles: u32,
    pub threads: u32,
    pub session: u32,
    pub services: Vec<ServiceProcess>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationActivity {
    pub entity_ref: String,
    /// Observed executable image of the earliest member, preferring an application over a hosted
    /// runtime such as WebView2. This is a local heuristic label.
    pub name: Option<String>,
    pub process_refs: Vec<String>,
    pub process_count: usize,
    pub members_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityCoverage {
    pub processes: ProcessCoverage,
    pub stable_instances: usize,
    pub new_instances: usize,
    pub exited_instances: usize,
    pub pseudo_processes: Vec<String>,
    pub services_status: Status,
    pub services_attributed: usize,
    pub services_unattributed: usize,
    pub gpu_status: Status,
    pub gpu_processes: usize,
    pub gpu_instances_rejected: usize,
    pub gpu_instances_unattributed: usize,
    pub applications_total: usize,
    pub applications_returned: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessActivity {
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub origin: Origin,
    pub status: Status,
    pub window: Window,
    pub processes: Vec<ActivityProcess>,
    pub applications: Vec<ApplicationActivity>,
    pub evidence: Vec<Evidence>,
    pub coverage: ActivityCoverage,
    pub limitations: Vec<String>,
}

impl ProcessActivity {
    pub(crate) fn unavailable(session: &str, status: Status, note: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: new_id("activity"),
            session_id: session.into(),
            origin: Origin::Live,
            status,
            window: Window::instant(),
            processes: Vec::new(),
            applications: Vec::new(),
            evidence: Vec::new(),
            coverage: ActivityCoverage {
                processes: ProcessCoverage {
                    status,
                    ..ProcessCoverage::default()
                },
                stable_instances: 0,
                new_instances: 0,
                exited_instances: 0,
                pseudo_processes: Vec::new(),
                services_status: status,
                services_attributed: 0,
                services_unattributed: 0,
                gpu_status: status,
                gpu_processes: 0,
                gpu_instances_rejected: 0,
                gpu_instances_unattributed: 0,
                applications_total: 0,
                applications_returned: 0,
            },
            limitations: vec![note.into()],
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProcessReading {
    pub pid: u32,
    pub parent_pid: u32,
    pub created: u64,
    pub cpu_100ns: u64,
    pub working_set: u64,
    pub private_commit: u64,
    pub io: [u64; 3],
    pub handles: u32,
    pub threads: u32,
    pub session: u32,
    pub name: Option<String>,
}

pub(crate) struct Collection {
    pub before: Result<Vec<ProcessReading>, Status>,
    pub after: Result<Vec<ProcessReading>, Status>,
    pub window: Window,
    pub gpu: Sampled<Instances>,
    pub gpu_window: Window,
    pub services: ServiceProcesses,
    pub logical_processors: u32,
}

#[derive(Default)]
struct GpuReadings {
    /// Physical engine keys never leave this module; only measured shares are exported.
    engines: HashMap<u32, BTreeMap<String, u64>>,
    rejected: usize,
    affected_pids: HashSet<u32>,
    unscoped_rejected: usize,
    unattributed: usize,
}

fn gpu_readings(instances: &Instances, stable_pids: &HashSet<u32>) -> GpuReadings {
    let mut out = GpuReadings {
        rejected: instances.invalid,
        unscoped_rejected: instances.invalid,
        ..GpuReadings::default()
    };
    let mut seen = HashSet::new();
    for (name, percent) in &instances.valid {
        let Some((pid, engine)) = gpu_identity(name) else {
            out.rejected += 1;
            out.unscoped_rejected += 1;
            continue;
        };
        if !stable_pids.contains(&pid) {
            out.unattributed += 1;
            continue;
        }
        let Some(value) = scaled(*percent, 100).and_then(clamp_share) else {
            out.rejected += 1;
            out.affected_pids.insert(pid);
            continue;
        };
        if !seen.insert((pid, engine.to_owned())) {
            out.rejected += 1;
            out.affected_pids.insert(pid);
            continue;
        }
        out.engines
            .entry(pid)
            .or_default()
            .insert(engine.to_owned(), value);
    }
    out
}

fn gpu_identity(instance: &str) -> Option<(u32, &str)> {
    let (pid, rest) = instance.strip_prefix("pid_")?.split_once("_luid_")?;
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (engine, kind) = rest.rsplit_once("_engtype_")?;
    if kind.is_empty() || !engine.contains("_phys_") || !engine.contains("_eng_") {
        return None;
    }
    Some((pid.parse().ok()?, engine))
}

/// Aggregate each physical engine across members, then take the busiest. Summing already
/// computed per-process maxima would mix unrelated engines and can overstate application GPU.
fn busiest_gpu(pids: impl Iterator<Item = u32>, gpu: &GpuReadings) -> Option<u64> {
    let mut engines: BTreeMap<&str, Option<u64>> = BTreeMap::new();
    for pid in pids {
        if let Some(values) = gpu.engines.get(&pid) {
            for (engine, value) in values {
                let total = engines.entry(engine).or_insert(Some(0));
                *total = total.and_then(|total| total.checked_add(*value));
            }
        }
    }
    let mut maximum = None;
    for value in engines.values() {
        // A rejected engine makes the maximum unknown; do not silently choose another one.
        let value = (*value).and_then(clamp_share)?;
        maximum = maximum.max(Some(value));
    }
    maximum
}

/// A retained exited process object still has creation times, and its exit code can be 259.
/// A running identity therefore requires both a known zero exit time and the running code.
pub(crate) fn handle_instance_state(
    expected_created: u64,
    times: Option<(u64, u64)>,
    exit_code: Option<u32>,
    running_code: u32,
) -> InstanceState {
    let Some((created, exited)) = times else {
        return InstanceState::Unverified;
    };
    if expected_created == 0 || created == 0 {
        return InstanceState::Unverified;
    }
    if created != expected_created {
        return InstanceState::PidReused;
    }
    if exited != 0 {
        return InstanceState::Exited;
    }
    match exit_code {
        Some(code) if code == running_code => InstanceState::Running,
        Some(_) => InstanceState::Exited,
        None => InstanceState::Unverified,
    }
}

/// The caller supplies identities observed before the PDH baseline and revalidated after
/// its finish. Native PIDs and physical engine identifiers stay inside this computation.
pub(crate) fn recording_gpu_evidence(
    processes: &[ProcessInstance],
    before: &HashSet<(u32, u64)>,
    revalidated: &HashSet<(u32, u64)>,
    instances: &Sampled<Instances>,
    window: &Window,
) -> Vec<Evidence> {
    let stable: HashSet<_> = processes
        .iter()
        .filter(|process| {
            let key = (process.pid, process.creation_time_filetime);
            key.0 != 0 && key.1 != 0 && before.contains(&key) && revalidated.contains(&key)
        })
        .map(|process| process.pid)
        .collect();
    let clock_consistent = window.duration_ms > 0
        && window
            .finished_at_unix_ms
            .checked_sub(window.started_at_unix_ms)
            .is_some_and(|span| span.abs_diff(window.duration_ms) <= 50);
    let readings = instances
        .as_ref()
        .map(|instances| gpu_readings(instances, &stable))
        .unwrap_or_default();
    processes
        .iter()
        .map(|process| {
            let value = (clock_consistent
                && before.contains(&(process.pid, process.creation_time_filetime))
                && revalidated.contains(&(process.pid, process.creation_time_filetime))
                && readings.unscoped_rejected == 0
                && !readings.affected_pids.contains(&process.pid))
                .then(|| busiest_gpu([process.pid].into_iter(), &readings))
                .flatten();
            let mut measured = evidence(
                Metric::ProcessGpuBasisPoints,
                &process.entity_ref,
                value,
                GPU_SOURCE,
                window,
                "No full-window GPU measurement for this retained, unchanged and revalidated process; absence is unknown, not zero.",
            );
            if measured.value.is_none() {
                if let Err(why) = instances {
                    measured.status = why.status;
                } else if instances.as_ref().is_ok_and(|instances| {
                    instances.valid.is_empty() && instances.invalid == 0
                }) {
                    measured.status = Status::Unsupported;
                }
            }
            if readings.rejected > 0 {
                measured.limitations.push(format!(
                    "GPU window rejected {} invalid or duplicate instances; invalid entries without a usable identity prevent complete per-process attribution.",
                    readings.rejected
                ));
            }
            measured.limitations.push("GPU occupancy is the busiest observed physical engine; it cannot establish watts, joules or a battery percentage for an application.".into());
            measured
        })
        .collect()
}

fn rate(bytes: u64, duration_ms: u64) -> Option<u64> {
    if duration_ms == 0 {
        return None;
    }
    u64::try_from(u128::from(bytes) * 1000 / u128::from(duration_ms)).ok()
}

fn total<'a>(
    rows: impl Iterator<Item = &'a ProcessReading>,
    f: impl Fn(&ProcessReading) -> u64,
) -> Option<u64> {
    rows.map(f).try_fold(0_u64, u64::checked_add)
}

fn evidence(
    metric: Metric,
    entity: &str,
    value: Option<u64>,
    source: &str,
    window: &Window,
    why: &str,
) -> Evidence {
    match value {
        Some(value) => Evidence::measured(metric, entity, value, source, window.clone()),
        None => {
            let mut e = Evidence::missing(metric, entity, source, Status::Partial, why);
            e.window = window.clone();
            e
        }
    }
}

fn measures(
    entity: &str,
    rows: &[&ProcessReading],
    before: &HashMap<(u32, u64), &ProcessReading>,
    window: &Window,
    processors: u32,
) -> Vec<Evidence> {
    let instant = Window {
        started_at_unix_ms: window.finished_at_unix_ms,
        finished_at_unix_ms: window.finished_at_unix_ms,
        duration_ms: 0,
    };
    let mut out = Vec::new();
    let first: Option<Vec<_>> = rows
        .iter()
        .map(|p| before.get(&(p.pid, p.created)).copied())
        .collect();
    for (metric, increase, decrease, private) in [
        (
            Metric::ProcessWorkingSetBytes,
            Metric::ProcessWorkingSetIncreaseBytes,
            Metric::ProcessWorkingSetDecreaseBytes,
            false,
        ),
        (
            Metric::ProcessPrivateCommitBytes,
            Metric::ProcessPrivateCommitIncreaseBytes,
            Metric::ProcessPrivateCommitDecreaseBytes,
            true,
        ),
    ] {
        let read = |p: &ProcessReading| {
            if private {
                p.private_commit
            } else {
                p.working_set
            }
        };
        let current = total(rows.iter().copied(), read);
        out.push(evidence(
            metric,
            entity,
            current,
            SOURCE,
            &instant,
            "Memory total overflowed.",
        ));
        let change = first
            .as_ref()
            .and_then(|first| total(first.iter().copied(), read))
            .zip(current);
        out.push(evidence(increase, entity, change.map(|(a,b)| b.saturating_sub(a)), SOURCE, window, "Memory change requires the same instances in both collections and non-overflowing totals."));
        out.push(evidence(decrease, entity, change.map(|(a,b)| a.saturating_sub(b)), SOURCE, window, "Memory change requires the same instances in both collections and non-overflowing totals."));
    }
    let cpu = first
        .as_ref()
        .and_then(|first| {
            first
                .iter()
                .zip(rows)
                .map(|(a, b)| b.cpu_100ns.checked_sub(a.cpu_100ns))
                .try_fold(0_u64, |sum, v| sum.checked_add(v?))
        })
        .and_then(|delta| {
            process_cpu_basis_points(
                0,
                delta,
                Duration::from_millis(window.duration_ms),
                processors,
            )
        });
    out.push(evidence(
        Metric::ProcessCpuBasisPoints,
        entity,
        cpu,
        SOURCE,
        window,
        "CPU requires matching instances, monotonic counters and nonzero total processor capacity.",
    ));
    for (index, metric, per_second) in [
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
        (
            2,
            Metric::ProcessIoOtherBytes,
            Metric::ProcessIoOtherBytesPerSecond,
        ),
    ] {
        let delta = first.as_ref().and_then(|first| {
            first
                .iter()
                .zip(rows)
                .map(|(a, b)| b.io[index].checked_sub(a.io[index]))
                .try_fold(0_u64, |sum, v| sum.checked_add(v?))
        });
        for (metric, value) in [
            (metric, delta),
            (per_second, delta.and_then(|v| rate(v, window.duration_ms))),
        ] {
            let mut e = evidence(metric,entity,value,SOURCE,window,"I/O requires matching instances, monotonic counters and a nonzero measurement window.");
            e.limitations.push("Process I/O includes cached, network and device traffic; it is not physical-disk throughput.".into());
            out.push(e);
        }
    }
    out
}

fn boundary(name: Option<&str>) -> bool {
    let Some(name) = name else {
        return true;
    };
    matches!(
        name.to_ascii_lowercase().as_str(),
        "system"
            | "registry"
            | "memory compression"
            | "svchost.exe"
            | "services.exe"
            | "smss.exe"
            | "wininit.exe"
            | "winlogon.exe"
            | "explorer.exe"
            | "cmd.exe"
            | "powershell.exe"
            | "pwsh.exe"
            | "windowsterminal.exe"
            | "conhost.exe"
            | "taskhostw.exe"
            | "rundll32.exe"
            | "dllhost.exe"
    )
}

/// A shared runtime that many applications start as their own child process, so its image names
/// the runtime, not the application. It joins only through its validated parent chain: a
/// Microsoft Edge WebView2 browser process joins the application that started it, and its
/// renderer, GPU, utility and crash-handler children join that browser process.
fn hosted_runtime(name: &str) -> bool {
    name.eq_ignore_ascii_case("msedgewebview2.exe")
}

fn find_root(parents: &mut [usize], mut i: usize) -> usize {
    while parents[i] != i {
        parents[i] = parents[parents[i]];
        i = parents[i];
    }
    i
}
fn join(parents: &mut [usize], a: usize, b: usize) {
    let a = find_root(parents, a);
    let b = find_root(parents, b);
    if a != b {
        parents[a] = b;
    }
}

/// Same basename within one Windows session, plus observed descendants. Generic launchers and
/// service hosts are boundaries, so Explorer or svchost cannot merge unrelated applications.
/// Hosted runtimes skip the image join: one WebView2 group would merge every application that
/// embeds it, since each WebView2 tree also joins its own host.
fn groups(rows: &[ProcessReading]) -> Vec<Vec<usize>> {
    let mut parents: Vec<_> = (0..rows.len()).collect();
    let by_pid: HashMap<_, _> = rows.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let mut by_image = HashMap::new();
    for (i, p) in rows.iter().enumerate() {
        if boundary(p.name.as_deref()) {
            continue;
        }
        if let Some(name) = p.name.as_deref().filter(|&name| !hosted_runtime(name)) {
            if let Some(&other) = by_image.get(&(p.session, name.to_ascii_lowercase())) {
                join(&mut parents, i, other);
            } else {
                by_image.insert((p.session, name.to_ascii_lowercase()), i);
            }
        }
        if let Some(&j) = by_pid.get(&p.parent_pid) {
            let parent = &rows[j];
            if parent.pid != p.pid
                && parent.created > 0
                && parent.created <= p.created
                && parent.session == p.session
                && !boundary(parent.name.as_deref())
            {
                join(&mut parents, i, j);
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..rows.len() {
        groups
            .entry(find_root(&mut parents, i))
            .or_default()
            .push(i);
    }
    groups.into_values().collect()
}

fn rank(evidence: &[Evidence]) -> (Option<u64>, Option<u64>, Option<u64>) {
    let value = |metric| {
        evidence
            .iter()
            .find(|e| e.metric == metric)
            .and_then(Evidence::usable_value)
    };
    (
        value(Metric::ProcessCpuBasisPoints),
        value(Metric::ProcessIoReadBytesPerSecond)
            .zip(value(Metric::ProcessIoWriteBytesPerSecond))
            .and_then(|(a, b)| a.checked_add(b)),
        value(Metric::ProcessWorkingSetBytes),
    )
}

pub(crate) fn assemble(
    collector: &mut Collector,
    collection: Collection,
    limit: usize,
) -> ProcessActivity {
    let Collection {
        before,
        after,
        window,
        gpu,
        gpu_window,
        services,
        logical_processors,
    } = collection;
    let mut out = ProcessActivity::unavailable(&collector.session_id, Status::Ok, "CPU and I/O are window averages; memory is measured at the end. Rates and memory changes use PID plus creation time, never a PID alone.");
    out.window = window.clone();
    let after = match after {
        Ok(after) => after,
        Err(status) => {
            out.status = status;
            out.coverage.processes.status = status;
            out.limitations
                .push("The final system process list could not be read.".into());
            return out;
        }
    };
    let before = match before {
        Ok(before) => before,
        Err(_) => {
            out.limitations.push("The initial process list was unavailable; memory is measured but rates and changes are unknown.".into());
            Vec::new()
        }
    };
    let rows: Vec<_> = after
        .iter()
        .filter(|p| p.pid != 0 && p.created != 0)
        .cloned()
        .collect();
    let first: HashMap<_, _> = before
        .iter()
        .filter(|p| p.pid != 0 && p.created != 0)
        .map(|p| ((p.pid, p.created), p))
        .collect();
    let keys: HashSet<_> = rows.iter().map(|p| (p.pid, p.created)).collect();
    let stable_pids: HashSet<_> = keys
        .iter()
        .filter(|key| first.contains_key(key))
        .map(|key| key.0)
        .collect();
    out.coverage.processes = ProcessCoverage {
        status: Status::Ok,
        enumerated: after.iter().filter(|p| p.pid != 0).count(),
        readable: rows.len(),
        permission_denied: 0,
        unavailable: after
            .iter()
            .filter(|p| p.pid != 0 && p.created == 0)
            .count(),
        returned: rows.len().min(limit.clamp(1, 50)),
        enumeration_truncated: false,
    };
    out.coverage.stable_instances = stable_pids.len();
    out.coverage.new_instances = rows.len() - stable_pids.len();
    out.coverage.exited_instances = first.keys().filter(|key| !keys.contains(key)).count();
    if after.iter().any(|p| p.pid == 0) {
        out.coverage.pseudo_processes.push("System Idle Process: processor idle time, excluded from application CPU and memory totals.".into());
    }
    if out.coverage.processes.unavailable != 0 {
        out.coverage.processes.status = Status::Partial;
    }
    out.coverage.services_status = services.status;
    let mut services_by_pid: HashMap<u32, Vec<ServiceProcess>> = HashMap::new();
    for service in services.services {
        if service.pid == 0 {
            continue;
        }
        if stable_pids.contains(&service.pid) {
            out.coverage.services_attributed += 1;
            services_by_pid
                .entry(service.pid)
                .or_default()
                .push(service);
        } else {
            out.coverage.services_unattributed += 1;
        }
    }
    out.limitations.extend(services.limitations);
    let mut gpu_status = Status::Ok;
    let gpu_readings = match gpu {
        Ok(instances) => {
            let values = gpu_readings(&instances, &stable_pids);
            if instances.valid.is_empty() && instances.invalid == 0 {
                gpu_status = Status::Unsupported;
            } else if values.rejected != 0 || values.unattributed != 0 {
                gpu_status = Status::Partial;
            }
            values
        }
        Err(why) => {
            gpu_status = why.status;
            out.limitations.push(why.reason);
            GpuReadings::default()
        }
    };
    out.coverage.gpu_status = gpu_status;
    out.coverage.gpu_processes = gpu_readings.engines.len();
    out.coverage.gpu_instances_rejected = gpu_readings.rejected;
    out.coverage.gpu_instances_unattributed = gpu_readings.unattributed;
    let mut instances = Vec::new();
    for p in &rows {
        let tracked = collector
            .registry
            .entry(p.pid, p.created, collector.boot_id.as_deref());
        tracked.instance.executable_name = p.name.clone();
        instances.push(tracked.instance.clone());
    }
    collector
        .registry
        .sweep(&keys, &after.iter().map(|p| p.pid).collect());
    let by_pid: HashMap<_, _> = rows.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    let mut processes = Vec::new();
    for (i, p) in rows.iter().enumerate() {
        let entity = &instances[i].entity_ref;
        let mut ev = measures(entity, &[p], &first, &window, logical_processors);
        let mut gpu=evidence(Metric::ProcessGpuBasisPoints,entity,busiest_gpu([p.pid].into_iter(),&gpu_readings),GPU_SOURCE,&gpu_window,"No valid GPU engine was attributed to this unchanged process instance; absence is unknown, not zero.");
        if gpu.value.is_none() && gpu_status == Status::Unsupported {
            gpu.status = Status::Unsupported;
        }
        ev.push(gpu);
        let tracked = collector
            .registry
            .entry(p.pid, p.created, collector.boot_id.as_deref());
        tracked.evidence = ev.clone();
        tracked.observed_unix_ms = Some(window.finished_at_unix_ms);
        tracked.counters = Some(CounterSample {
            cpu_100ns: Some(p.cpu_100ns),
            io_bytes: Some(p.io),
            at: collector.started.elapsed(),
            unix_ms: window.finished_at_unix_ms,
        });
        let parent_ref = by_pid
            .get(&p.parent_pid)
            .filter(|&&j| j != i && rows[j].created <= p.created)
            .map(|&j| instances[j].entity_ref.clone());
        processes.push((
            ActivityProcess {
                instance: instances[i].clone(),
                parent_ref,
                application_ref: String::new(),
                role: if p.pid == 4 {
                    ProcessRole::System
                } else {
                    ProcessRole::Application
                },
                handles: p.handles,
                threads: p.threads,
                session: p.session,
                services: services_by_pid.remove(&p.pid).unwrap_or_default(),
            },
            ev,
        ));
    }
    let mut applications = Vec::new();
    for members in groups(&rows) {
        let entity = new_short_ref("app");
        let observed: Vec<_> = members.iter().map(|&i| &rows[i]).collect();
        let mut ev = measures(&entity, &observed, &first, &window, logical_processors);
        let mut gpu=evidence(Metric::ProcessGpuBasisPoints,&entity,busiest_gpu(observed.iter().map(|p|p.pid),&gpu_readings),GPU_SOURCE,&gpu_window,"Application GPU is unknown when no valid physical engine can be attributed to its unchanged members.");
        if gpu.value.is_none() && gpu_status == Status::Unsupported {
            gpu.status = Status::Unsupported;
        }
        ev.push(gpu);
        // A child's creation time can equal its parent's, and its PID can be lower: prefer an
        // application over a hosted runtime so a WebView2 tree never renames its host's group.
        let primary = members
            .iter()
            .min_by_key(|&&i| {
                let runtime = rows[i].name.as_deref().is_some_and(hosted_runtime);
                (runtime, rows[i].created, rows[i].pid)
            })
            .copied()
            .expect("nonempty group");
        for &i in &members {
            processes[i].0.application_ref = entity.clone();
        }
        applications.push((
            ApplicationActivity {
                entity_ref: entity,
                name: rows[primary].name.clone(),
                process_refs: members
                    .iter()
                    .take(MAX_MEMBERS)
                    .map(|&i| instances[i].entity_ref.clone())
                    .collect(),
                process_count: members.len(),
                members_truncated: members.len() > MAX_MEMBERS,
            },
            ev,
        ));
    }
    out.coverage.applications_total = applications.len();
    processes.sort_by_key(|(_, ev)| std::cmp::Reverse(rank(ev)));
    applications.sort_by_key(|(_, ev)| std::cmp::Reverse(rank(ev)));
    for (process, ev) in processes.into_iter().take(limit.clamp(1, 50)) {
        out.processes.push(process);
        out.evidence.extend(ev);
    }
    for (app, ev) in applications.into_iter().take(limit.clamp(1, 50)) {
        out.applications.push(app);
        out.evidence.extend(ev);
    }
    out.coverage.applications_returned = out.applications.len();
    if out.coverage.processes.status != Status::Ok
        || out.coverage.new_instances != 0
        || out.coverage.exited_instances != 0
        || services.status != Status::Ok
        || gpu_status != Status::Ok
    {
        out.status = Status::Partial;
    }
    out.limitations.extend([
        "Application groups are an image-and-parent heuristic within one Windows session, not verified product identities. Executable basenames can coincide. Generic shells and service hosts form separate boundaries; all members are aggregated before the returned lists are capped.".into(),
        // No literal image name: free text is anonymized against the observed names.
        "Microsoft Edge WebView2 runtime processes join only through their observed parent chain, never by image: each WebView2 tree counts toward the application that started it. A tree whose starter is a shell or service host, has exited, runs in another session or has a reused PID forms its own group; a WebView2 browser process shared by several applications (same user data folder) counts toward the one that started it.".into(),
        "Application references are scoped to this observation; process references remain stable for PID plus creation time in the collector session.".into(),
        "Working-set sums include shared pages; they are not exclusive physical RAM. Memory increase/decrease is observed net change, not evidence of a leak.".into(),
        "GPU is the busiest observed physical engine. Application GPU sums members on each physical engine before taking the maximum; missing or newly created counter instances are counted in coverage. No GPU instance means unknown, never zero.".into(),
        "Exited processes are missing from end-of-window attribution, and newly started or reused instances have no full-window rate. Short windows cannot establish sustained contention.".into(),
        "Rows are ranked by CPU, then I/O throughput, then working set; service and raw process metadata remain local until privacy projection.".into(),
    ]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: u32, created: u64, name: &str) -> ProcessReading {
        ProcessReading {
            pid,
            parent_pid: parent,
            created,
            cpu_100ns: 10_000_000,
            working_set: 1000,
            private_commit: 2000,
            io: [100, 200, 300],
            handles: 4,
            threads: 2,
            session: 1,
            name: Some(name.into()),
        }
    }
    fn collection(before: Vec<ProcessReading>, after: Vec<ProcessReading>) -> Collection {
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 3000,
            duration_ms: 2000,
        };
        Collection {
            before: Ok(before),
            after: Ok(after),
            window: window.clone(),
            gpu: Ok(Instances::default()),
            gpu_window: window,
            services: ServiceProcesses {
                services: Vec::new(),
                status: Status::Ok,
                limitations: Vec::new(),
            },
            logical_processors: 4,
        }
    }
    fn value(activity: &ProcessActivity, entity: &str, metric: Metric) -> Option<u64> {
        activity
            .evidence
            .iter()
            .find(|e| e.entity_ref == entity && e.metric == metric)
            .and_then(Evidence::usable_value)
    }
    fn gpu(instances: &[(u32, &str, f64)]) -> Instances {
        Instances {
            valid: instances
                .iter()
                .map(|(pid, engine, value)| {
                    (
                        format!("pid_{pid}_luid_0x0_0x1_phys_0_eng_{engine}_engtype_3D"),
                        *value,
                    )
                })
                .collect(),
            invalid: 0,
        }
    }

    #[test]
    fn rates_and_separate_memory_directions_use_one_monotonic_window() {
        let first = row(10, 0, 10, "browser.exe");
        let mut last = first.clone();
        last.cpu_100ns += 20_000_000;
        last.working_set += 600;
        last.private_commit -= 200;
        last.io = [6100, 2200, 300];
        let mut collector = Collector::new();
        let activity = assemble(
            &mut collector,
            collection(vec![first.clone()], vec![last.clone()]),
            50,
        );
        let entity = &activity.processes[0].instance.entity_ref;
        assert_eq!(
            value(&activity, entity, Metric::ProcessCpuBasisPoints),
            Some(2500)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessWorkingSetIncreaseBytes),
            Some(600)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessWorkingSetDecreaseBytes),
            Some(0)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessPrivateCommitIncreaseBytes),
            Some(0)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessPrivateCommitDecreaseBytes),
            Some(200)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessIoReadBytesPerSecond),
            Some(3000)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessIoOtherBytesPerSecond),
            Some(0)
        );
        let next = assemble(&mut collector, collection(vec![first], vec![last]), 50);
        assert_eq!(entity, &next.processes[0].instance.entity_ref);
    }

    #[test]
    fn reused_pids_cannot_inherit_cpu_growth_gpu_or_service_attribution() {
        let first = row(10, 0, 10, "old.exe");
        let mut last = row(10, 0, 20, "new.exe");
        last.cpu_100ns = 30_000_000;
        let mut input = collection(vec![first], vec![last]);
        input.gpu = Ok(gpu(&[(10, "0", 42.0)]));
        input.services.services.push(ServiceProcess {
            pid: 10,
            service_name: "example".into(),
            display_name: "Example".into(),
            state: "running".into(),
        });
        let activity = assemble(&mut Collector::new(), input, 50);
        let process = &activity.processes[0];
        let entity = &process.instance.entity_ref;
        assert_eq!(
            value(&activity, entity, Metric::ProcessWorkingSetBytes),
            Some(1000)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessCpuBasisPoints),
            None
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessWorkingSetIncreaseBytes),
            None
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessGpuBasisPoints),
            None
        );
        assert!(process.services.is_empty());
        assert_eq!(activity.coverage.new_instances, 1);
        assert_eq!(activity.coverage.exited_instances, 1);
        assert_eq!(activity.coverage.gpu_instances_unattributed, 1);
        assert_eq!(activity.coverage.services_unattributed, 1);
    }

    #[test]
    fn application_grouping_respects_sessions_launchers_and_reused_parents() {
        let mut other_session = row(60, 10, 60, "browser.exe");
        other_session.session = 2;
        let rows = vec![
            row(1, 0, 1, "explorer.exe"),
            row(10, 1, 10, "browser.exe"),
            row(20, 10, 20, "browser.exe"),
            row(30, 10, 30, "helper.exe"),
            row(40, 1, 40, "editor.exe"),
            row(50, 90, 50, "independent.exe"),
            other_session,
            row(90, 1, 90, "reused-parent.exe"),
            row(100, 0, 1, "svchost.exe"),
            row(110, 0, 1, "svchost.exe"),
        ];
        let groups = groups(&rows);
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), rows.len());
        assert_eq!(groups.len(), 8);
        let browser = groups.iter().find(|group| group.contains(&1)).unwrap();
        assert_eq!(browser, &vec![1, 2, 3]);
        assert!(!browser.contains(&6));
        assert!(groups
            .iter()
            .all(|group| !(group.contains(&5) && group.contains(&7))));
    }

    /// Teams, Outlook and PC Agent each start one WebView2 tree (a browser process and its
    /// renderer, GPU or crash-handler children); the other trees have no attributable host.
    fn webview2_rows() -> Vec<ProcessReading> {
        let in_session = |mut p: ProcessReading, session| {
            p.session = session;
            p
        };
        vec![
            row(1, 0, 1, "explorer.exe"),
            row(100, 1, 100, "ms-teams.exe"),
            row(110, 100, 110, "msedgewebview2.exe"),
            row(111, 110, 111, "msedgewebview2.exe"),
            row(112, 110, 112, "msedgewebview2.exe"),
            row(200, 1, 200, "olk.exe"),
            row(210, 200, 210, "MSEdgeWebView2.exe"),
            row(211, 210, 211, "msedgewebview2.exe"),
            row(300, 1, 300, "pcagent-gui.exe"),
            row(310, 300, 310, "msedgewebview2.exe"),
            row(311, 310, 311, "msedgewebview2.exe"),
            // The host exited: PID 400 is no longer observed.
            row(410, 400, 410, "msedgewebview2.exe"),
            row(411, 410, 411, "msedgewebview2.exe"),
            // PID 500 was reused by a process created after the tree it would have started.
            row(500, 1, 600, "Widgets.exe"),
            row(510, 500, 510, "msedgewebview2.exe"),
            row(511, 510, 511, "msedgewebview2.exe"),
            // Trees started by a COM surrogate, Explorer and a service host: all boundaries.
            row(800, 0, 2, "svchost.exe"),
            row(700, 800, 700, "dllhost.exe"),
            row(710, 700, 710, "msedgewebview2.exe"),
            row(711, 710, 711, "msedgewebview2.exe"),
            row(20, 1, 20, "msedgewebview2.exe"),
            row(21, 20, 21, "msedgewebview2.exe"),
            row(810, 800, 810, "msedgewebview2.exe"),
            row(811, 810, 811, "msedgewebview2.exe"),
            // Teams in another session with its own tree, and a tree whose host is there.
            in_session(row(900, 0, 900, "ms-teams.exe"), 2),
            in_session(row(905, 900, 905, "msedgewebview2.exe"), 2),
            row(910, 900, 910, "msedgewebview2.exe"),
        ]
    }

    #[test]
    fn webview2_trees_join_the_application_that_started_them_never_each_other() {
        let rows = webview2_rows();
        let mut found: Vec<Vec<u32>> = groups(&rows)
            .into_iter()
            .map(|group| {
                let mut pids: Vec<_> = group.into_iter().map(|i| rows[i].pid).collect();
                pids.sort_unstable();
                pids
            })
            .collect();
        found.sort();
        let mut expected = vec![
            vec![100, 110, 111, 112],
            vec![200, 210, 211],
            vec![300, 310, 311],
            vec![900, 905],
            vec![410, 411],
            vec![510, 511],
            vec![710, 711],
            vec![20, 21],
            vec![810, 811],
            vec![910],
            vec![1],
            vec![500],
            vec![700],
            vec![800],
        ];
        expected.sort();
        assert_eq!(found, expected);
    }

    #[test]
    fn webview2_applications_are_named_after_their_host_with_summed_members() {
        let first = webview2_rows();
        let mut last = first.clone();
        for p in &mut last {
            p.working_set = u64::from(p.pid) * 1000;
            // 100 basis points of a two-second window on four logical processors.
            p.cpu_100ns += 800_000;
        }
        let activity = assemble(&mut Collector::new(), collection(first, last), 50);
        let application_of = |pid: u32| {
            activity
                .processes
                .iter()
                .find(|p| p.instance.pid == pid)
                .map(|p| p.application_ref.as_str())
                .unwrap()
        };
        let mut seen = HashSet::new();
        for (name, members) in [
            ("ms-teams.exe", &[100, 110, 111, 112][..]),
            ("olk.exe", &[200, 210, 211]),
            ("pcagent-gui.exe", &[300, 310, 311]),
            ("ms-teams.exe", &[900, 905]),
            ("msedgewebview2.exe", &[410, 411]),
            ("msedgewebview2.exe", &[510, 511]),
            ("msedgewebview2.exe", &[710, 711]),
            ("msedgewebview2.exe", &[20, 21]),
            ("msedgewebview2.exe", &[810, 811]),
            ("msedgewebview2.exe", &[910]),
        ] {
            let reference = application_of(members[0]);
            assert!(seen.insert(reference), "{members:?} shares a group");
            assert!(members.iter().all(|&pid| application_of(pid) == reference));
            let app = activity
                .applications
                .iter()
                .find(|app| app.entity_ref == reference)
                .unwrap();
            assert_eq!(app.name.as_deref(), Some(name), "{members:?}");
            assert_eq!(app.process_count, members.len());
            assert_eq!(app.process_refs.len(), members.len());
            assert!(!app.members_truncated);
            let memory = members.iter().map(|&pid| u64::from(pid) * 1000).sum();
            assert_eq!(
                value(&activity, reference, Metric::ProcessWorkingSetBytes),
                Some(memory)
            );
            assert_eq!(
                value(&activity, reference, Metric::ProcessCpuBasisPoints),
                Some(100 * members.len() as u64)
            );
        }
        // Explorer, svchost, dllhost and the process that reused PID 500 stand alone.
        assert_eq!(activity.coverage.applications_total, seen.len() + 4);
        // Observed names are masked in free text before sharing; the rule must stay readable.
        assert!(activity
            .limitations
            .iter()
            .any(|text| text.contains("WebView2")));
        assert!(activity
            .limitations
            .iter()
            .all(|text| !text.to_ascii_lowercase().contains("msedgewebview2")));
    }

    #[test]
    fn a_host_keeps_its_group_name_when_its_webview2_child_ties_on_creation_time() {
        // The parent check accepts an equal creation time, and this browser process also has a
        // lower PID than its host. A group without a hosted runtime keeps its earliest member.
        let rows = vec![
            row(300, 1, 300, "pcagent-gui.exe"),
            row(296, 300, 300, "msedgewebview2.exe"),
            row(304, 296, 310, "msedgewebview2.exe"),
            row(60, 50, 60, "app.exe"),
            row(50, 1, 50, "launcher.exe"),
        ];
        let activity = assemble(&mut Collector::new(), collection(rows.clone(), rows), 50);
        let mut names: Vec<_> = activity
            .applications
            .iter()
            .map(|app| (app.name.as_deref(), app.process_count))
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [(Some("launcher.exe"), 2), (Some("pcagent-gui.exe"), 3)]
        );
    }

    #[test]
    fn application_totals_include_every_member_before_row_limiting() {
        let first: Vec<_> = (1..=5)
            .map(|i| row(i, 0, u64::from(i), "browser.exe"))
            .collect();
        let mut last = first.clone();
        for p in &mut last {
            p.working_set += 100;
            p.cpu_100ns += 1_000_000;
        }
        let activity = assemble(&mut Collector::new(), collection(first, last), 1);
        assert_eq!(activity.processes.len(), 1);
        assert_eq!(activity.coverage.processes.readable, 5);
        let app = &activity.applications[0];
        assert_eq!(app.process_count, 5);
        assert_eq!(
            value(&activity, &app.entity_ref, Metric::ProcessWorkingSetBytes),
            Some(5500)
        );
        assert_eq!(
            value(
                &activity,
                &app.entity_ref,
                Metric::ProcessWorkingSetIncreaseBytes
            ),
            Some(500)
        );
    }

    #[test]
    fn gpu_grouping_sums_identical_physical_engines_then_takes_the_maximum() {
        let first = vec![
            row(10, 0, 10, "browser.exe"),
            row(20, 10, 20, "browser.exe"),
        ];
        let mut input = collection(first.clone(), first);
        input.gpu = Ok(gpu(&[
            (10, "0", 20.0),
            (10, "1", 5.0),
            (20, "0", 10.0),
            (20, "1", 40.0),
        ]));
        let activity = assemble(&mut Collector::new(), input, 50);
        assert_eq!(
            value(
                &activity,
                &activity.applications[0].entity_ref,
                Metric::ProcessGpuBasisPoints
            ),
            Some(4500)
        );
        assert_eq!(activity.coverage.gpu_processes, 2);
        let text = serde_json::to_string(&activity).unwrap();
        assert!(!text.contains("luid_") && !text.contains("pid_"));
    }

    #[test]
    fn malformed_duplicate_and_unattributed_gpu_instances_are_counted() {
        let mut data = gpu(&[(10, "0", 20.0), (10, "0", 20.0), (20, "0", 30.0)]);
        data.valid.push((
            "pid_not-a-number_luid_x_phys_0_eng_0_engtype_3D".into(),
            1.0,
        ));
        data.valid
            .push(("pid_10_luid_x_phys_0_eng_0_engtype_3D".into(), f64::NAN));
        data.invalid = 2;
        let readings = gpu_readings(&data, &[10].into_iter().collect());
        assert_eq!(readings.rejected, 5);
        assert_eq!(readings.unattributed, 1);
        assert_eq!(busiest_gpu([10].into_iter(), &readings), Some(2000));
        assert!(gpu_identity("pid_-1_luid_x_phys_0_eng_0_engtype_3D").is_none());
        assert!(gpu_identity("pid_10_luid_x").is_none());
    }

    fn recording_process(pid: u32, created: u64, reference: &str) -> ProcessInstance {
        ProcessInstance {
            entity_ref: reference.into(),
            pid,
            creation_time_filetime: created,
            boot_id: None,
            executable_name: Some("private-local-name.exe".into()),
        }
    }

    #[test]
    fn recorded_gpu_keeps_identity_native_window_units_and_measured_zero() {
        let processes = [
            recording_process(10, 100, "proc-11112222"),
            recording_process(20, 200, "proc-33334444"),
        ];
        let identities = [(10, 100), (20, 200)].into_iter().collect();
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 3000,
            duration_ms: 2000,
        };
        let evidence = recording_gpu_evidence(
            &processes,
            &identities,
            &identities,
            &Ok(gpu(&[(10, "0", 37.0), (10, "1", 73.0), (20, "0", 0.0)])),
            &window,
        );
        assert_eq!(evidence.len(), 2);
        assert_eq!(evidence[0].entity_ref, "proc-11112222");
        assert_eq!(evidence[0].usable_value(), Some(7300));
        assert_eq!(evidence[1].usable_value(), Some(0));
        for value in &evidence {
            assert_eq!(value.metric, Metric::ProcessGpuBasisPoints);
            assert_eq!(value.unit, Metric::ProcessGpuBasisPoints.unit());
            assert_eq!(value.source, GPU_SOURCE);
            assert_eq!(value.window.started_at_unix_ms, window.started_at_unix_ms);
            assert_eq!(value.window.finished_at_unix_ms, window.finished_at_unix_ms);
            assert_eq!(value.window.duration_ms, window.duration_ms);
        }
        let wire = serde_json::to_string(&evidence).unwrap();
        for private in ["pid_", "luid_", "creation_time", "private-local-name"] {
            assert!(!wire.contains(private));
        }
    }

    #[test]
    fn recorded_gpu_requires_the_same_instance_before_and_after_the_exact_window() {
        let process = recording_process(10, 100, "proc-11112222");
        let same = [(10, 100)].into_iter().collect();
        let reused = [(10, 99)].into_iter().collect();
        let absent = HashSet::new();
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 3000,
            duration_ms: 2000,
        };
        for (before, verified) in [
            (&absent, &same),
            (&reused, &same),
            (&same, &absent),
            (&same, &reused),
        ] {
            let values = recording_gpu_evidence(
                std::slice::from_ref(&process),
                before,
                verified,
                &Ok(gpu(&[(10, "0", 73.0)])),
                &window,
            );
            assert_eq!(values[0].value, None);
            assert_eq!(values[0].status, Status::Partial);
        }
        let mut skewed = window.clone();
        skewed.finished_at_unix_ms += 500;
        for window in [skewed, Window::instant()] {
            assert_eq!(
                recording_gpu_evidence(
                    std::slice::from_ref(&process),
                    &same,
                    &same,
                    &Ok(gpu(&[(10, "0", 73.0)])),
                    &window,
                )[0]
                .value,
                None
            );
        }
    }

    #[test]
    fn recorded_gpu_preserves_unavailable_and_invalid_coverage() {
        let processes = [recording_process(10, 100, "proc-11112222")];
        let identities = [(10, 100)].into_iter().collect();
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 3000,
            duration_ms: 2000,
        };
        for (instances, expected) in [
            (Ok(Instances::default()), Status::Unsupported),
            (Ok(gpu(&[(10, "0", 120.0)])), Status::Partial),
            (Ok(gpu(&[(20, "0", 73.0)])), Status::Partial),
            (
                Err(crate::counters::Unavailable::new(
                    Status::PermissionDenied,
                    "GPU counter denied.",
                )),
                Status::PermissionDenied,
            ),
        ] {
            let values =
                recording_gpu_evidence(&processes, &identities, &identities, &instances, &window);
            assert_eq!(values[0].value, None);
            assert_eq!(values[0].status, expected);
        }
    }

    #[test]
    fn recorded_gpu_cannot_hide_a_rejected_engine_behind_a_valid_zero() {
        let processes = [
            recording_process(10, 100, "proc-11112222"),
            recording_process(20, 200, "proc-33334444"),
        ];
        let identities = [(10, 100), (20, 200)].into_iter().collect();
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 3000,
            duration_ms: 2000,
        };
        for (engine, invalid) in [("1", f64::NAN), ("1", 120.0), ("0", 25.0)] {
            let instances = gpu(&[(10, "0", 0.0), (10, engine, invalid), (20, "0", 73.0)]);
            let values = recording_gpu_evidence(
                &processes,
                &identities,
                &identities,
                &Ok(instances),
                &window,
            );
            assert_eq!(values[0].value, None);
            assert_eq!(values[0].status, Status::Partial);
            assert!(values[0]
                .limitations
                .iter()
                .any(|limitation| limitation.contains("rejected 1")));
            assert_eq!(values[1].usable_value(), Some(7300));
        }
        for invalid_identity in [false, true] {
            let mut instances = gpu(&[(10, "0", 0.0), (20, "0", 73.0)]);
            if invalid_identity {
                instances.valid.push(("unrecognized-instance".into(), 30.0));
            } else {
                instances.invalid = 1;
            }
            let values = recording_gpu_evidence(
                &processes,
                &identities,
                &identities,
                &Ok(instances),
                &window,
            );
            assert!(values
                .iter()
                .all(|value| value.value.is_none() && value.status == Status::Partial));
        }
    }

    #[test]
    fn process_handle_liveness_requires_creation_exit_time_and_exit_code() {
        for (times, code, expected) in [
            (Some((100, 0)), Some(259), InstanceState::Running),
            (Some((100, 3001)), Some(259), InstanceState::Exited),
            (Some((100, 3001)), Some(0), InstanceState::Exited),
            (Some((100, 0)), Some(0), InstanceState::Exited),
            (Some((99, 0)), Some(259), InstanceState::PidReused),
            (Some((0, 0)), Some(259), InstanceState::Unverified),
            (Some((100, 0)), None, InstanceState::Unverified),
            (None, Some(259), InstanceState::Unverified),
        ] {
            assert_eq!(handle_instance_state(100, times, code, 259), expected);
        }
        let process = recording_process(10, 100, "proc-11112222");
        let identities = [(10, 100)].into_iter().collect();
        let after_window = handle_instance_state(100, Some((100, 3001)), Some(259), 259);
        let verified = if after_window == InstanceState::Running {
            [(10, 100)].into_iter().collect()
        } else {
            HashSet::new()
        };
        let values = recording_gpu_evidence(
            &[process],
            &identities,
            &verified,
            &Ok(gpu(&[(10, "0", 73.0)])),
            &Window {
                started_at_unix_ms: 1000,
                finished_at_unix_ms: 3000,
                duration_ms: 2000,
            },
        );
        assert_eq!(values[0].value, None);
        assert_eq!(values[0].status, Status::Partial);
    }

    #[test]
    fn missing_or_regressed_counters_are_unknown_never_zero() {
        let first = row(10, 0, 10, "app.exe");
        let mut last = first.clone();
        last.cpu_100ns -= 1;
        last.io[0] -= 1;
        let activity = assemble(
            &mut Collector::new(),
            collection(vec![first], vec![last]),
            50,
        );
        let entity = &activity.processes[0].instance.entity_ref;
        for metric in [
            Metric::ProcessCpuBasisPoints,
            Metric::ProcessIoReadBytesPerSecond,
            Metric::ProcessGpuBasisPoints,
        ] {
            assert_eq!(value(&activity, entity, metric), None);
        }
        assert_eq!(rate(1, 0), None);
        assert_eq!(rate(u64::MAX, 1), None);
    }

    #[test]
    fn new_members_make_whole_application_growth_unknown() {
        let first = row(10, 0, 10, "app.exe");
        let mut second = first.clone();
        second.working_set += 50;
        let activity = assemble(
            &mut Collector::new(),
            collection(vec![first], vec![second, row(20, 10, 20, "app.exe")]),
            50,
        );
        let app = &activity.applications[0];
        assert_eq!(
            value(&activity, &app.entity_ref, Metric::ProcessWorkingSetBytes),
            Some(2050)
        );
        assert_eq!(
            value(
                &activity,
                &app.entity_ref,
                Metric::ProcessWorkingSetIncreaseBytes
            ),
            None
        );
        assert_eq!(
            value(&activity, &app.entity_ref, Metric::ProcessCpuBasisPoints),
            None
        );
    }

    #[test]
    fn unavailable_baseline_preserves_memory_but_not_a_fabricated_rate() {
        let last = row(10, 0, 10, "app.exe");
        let mut input = collection(Vec::new(), vec![last]);
        input.before = Err(Status::Error);
        let activity = assemble(&mut Collector::new(), input, 50);
        let entity = &activity.processes[0].instance.entity_ref;
        assert_eq!(
            value(&activity, entity, Metric::ProcessWorkingSetBytes),
            Some(1000)
        );
        assert_eq!(
            value(&activity, entity, Metric::ProcessCpuBasisPoints),
            None
        );
    }

    #[test]
    fn idle_and_unverifiable_instances_are_explicitly_excluded() {
        let rows = vec![
            row(0, 0, 0, "idle"),
            row(10, 0, 0, "unknown.exe"),
            row(20, 0, 20, "known.exe"),
        ];
        let activity = assemble(&mut Collector::new(), collection(rows.clone(), rows), 50);
        assert_eq!(activity.coverage.processes.enumerated, 2);
        assert_eq!(activity.coverage.processes.readable, 1);
        assert_eq!(activity.coverage.processes.unavailable, 1);
        assert_eq!(activity.coverage.pseudo_processes.len(), 1);
        assert_eq!(activity.processes.len(), 1);
    }

    #[cfg(not(any(windows, target_os = "linux")))]
    #[test]
    fn unsupported_activity_never_returns_fixture_measurements() {
        let activity = Collector::new().process_activity(3, 20);
        assert_eq!(activity.status, Status::Unsupported);
        assert!(activity.evidence.is_empty() && activity.processes.is_empty());
    }
}
