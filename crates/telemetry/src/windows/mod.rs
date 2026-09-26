//! The only unsafe boundary: documented, read-only Windows API calls.
mod inventory;
mod tree;
pub(super) use inventory::hardware_inventory;
pub(super) use tree::process_tree;

use super::{cpu_busy_basis_points, process_cpu_basis_points, Collector, TickSample};
use crate::identity::{CounterSample, Key};
use pcagent_shared::{
    unix_ms, Evidence, InstanceState, Metric, ProcessCoverage, Snapshot, Status, Window,
};
use std::{
    collections::HashSet,
    mem::{size_of, zeroed},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, HANDLE,
        STILL_ACTIVE,
    },
    System::{
        ProcessStatus::{
            K32EnumProcesses, K32GetPerformanceInfo, K32GetProcessMemoryInfo,
            PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
        },
        SystemInformation::{GetSystemTimeAsFileTime, GetTickCount64},
        Threading::{
            GetActiveProcessorCount, GetExitCodeProcess, GetProcessIoCounters, GetProcessTimes,
            GetSystemTimes, OpenProcess, QueryFullProcessImageNameW, ALL_PROCESSOR_GROUPS,
            IO_COUNTERS, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

const FILETIME_UNIX_EPOCH_MS: u64 = 11_644_473_600_000;
const BOOT_NOTE: &str = "boot_id is an estimate (GetSystemTimeAsFileTime minus GetTickCount64, rounded to one minute), fixed per collector; clock changes can shift it between sessions.";

struct OwnedHandle(HANDLE);
impl OwnedHandle {
    /// Query-only rights; never VM read/write or terminate.
    fn open(pid: u32) -> Result<Self, Status> {
        // SAFETY: no pointer arguments; a non-null result is owned by exactly one wrapper.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            Err(last_status())
        } else {
            Ok(Self(handle))
        }
    }
}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and has one owning wrapper.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
fn filetime(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}
fn last_error() -> u32 {
    // SAFETY: thread-local error retrieval has no memory preconditions.
    unsafe { GetLastError() }
}
fn last_status() -> Status {
    if last_error() == ERROR_ACCESS_DENIED {
        Status::PermissionDenied
    } else {
        Status::Error
    }
}
fn processor_count() -> u32 {
    // SAFETY: no pointer arguments.
    unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) }
}
pub(super) fn boot_id() -> Option<String> {
    // SAFETY: FILETIME is plain data and the output pointer is valid; GetTickCount64 has no arguments.
    let (now, uptime) = unsafe {
        let mut now: FILETIME = zeroed();
        GetSystemTimeAsFileTime(&mut now);
        (filetime(now), GetTickCount64())
    };
    let now_ms = (now / 10_000).checked_sub(FILETIME_UNIX_EPOCH_MS)?;
    super::boot_estimate_id(now_ms, uptime)
}
/// Returns (creation time, kernel + user time), both in 100 ns FILETIME units.
fn times(handle: &OwnedHandle) -> Result<(u64, Option<u64>), Status> {
    // SAFETY: the handle is valid for the call; FILETIME outputs are plain data.
    unsafe {
        let (mut created, mut exit, mut kernel, mut user): (
            FILETIME,
            FILETIME,
            FILETIME,
            FILETIME,
        ) = (zeroed(), zeroed(), zeroed(), zeroed());
        if GetProcessTimes(handle.0, &mut created, &mut exit, &mut kernel, &mut user) == 0 {
            return Err(last_status());
        }
        match filetime(created) {
            0 => Err(Status::Partial),
            c => Ok((c, filetime(kernel).checked_add(filetime(user)))),
        }
    }
}
pub(super) fn revalidate(pid: u32, created: u64) -> InstanceState {
    let handle = match OwnedHandle::open(pid) {
        Ok(h) => h,
        Err(_) if last_error() == ERROR_INVALID_PARAMETER => return InstanceState::Exited,
        Err(_) => return InstanceState::Unverified,
    };
    match times(&handle) {
        Ok((c, _)) if c != created => InstanceState::PidReused,
        Ok(_) => {
            let mut code = 0_u32;
            // SAFETY: the handle is valid; code is a valid DWORD output.
            if unsafe { GetExitCodeProcess(handle.0, &mut code) } == 0 {
                InstanceState::Unverified
            } else if code == STILL_ACTIVE as u32 {
                InstanceState::Running
            } else {
                InstanceState::Exited
            }
        }
        Err(_) => InstanceState::Unverified,
    }
}
fn cpu_ticks() -> Result<[u64; 3], Status> {
    // SAFETY: FILETIME is plain data; all output pointers are valid for the call.
    unsafe {
        let (mut idle, mut kernel, mut user): (FILETIME, FILETIME, FILETIME) =
            (zeroed(), zeroed(), zeroed());
        if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
            return Err(last_status());
        }
        Ok([filetime(idle), filetime(kernel), filetime(user)])
    }
}
fn collect_cpu(result: &mut Snapshot, collector: &mut Collector, reuse: bool) {
    const SOURCE: &str = "Win32.GetSystemTimes";
    let count = processor_count();
    if count == 0 || count > 64 {
        result.evidence.push(Evidence::missing(
            Metric::CpuBusyBasisPoints,
            "system",
            SOURCE,
            Status::Unsupported,
            "Aggregate CPU coverage is unavailable or spans multiple processor groups.",
        ));
        return;
    }
    let now = collector.started.elapsed();
    let prior = collector.last_ticks.filter(|p| {
        reuse
            && now.checked_sub(p.at).is_some_and(|d| {
                (Duration::from_millis(250)..=Duration::from_secs(60)).contains(&d)
            })
    });
    let (first, started_unix, started_at) = match prior {
        Some(p) => (Ok(p.ticks), p.unix_ms, p.at),
        None => {
            let started = (unix_ms(), collector.started.elapsed());
            let first = cpu_ticks();
            std::thread::sleep(Duration::from_secs(1));
            (first, started.0, started.1)
        }
    };
    let last = cpu_ticks();
    let ended_at = collector.started.elapsed();
    let window = Window {
        started_at_unix_ms: started_unix,
        finished_at_unix_ms: unix_ms(),
        duration_ms: ended_at.saturating_sub(started_at).as_millis() as u64,
    };
    if let Ok(ticks) = last {
        collector.last_ticks = Some(TickSample {
            ticks,
            at: ended_at,
            unix_ms: window.finished_at_unix_ms,
        });
    }
    let mut reading = match (first, last) {
        (Ok(a), Ok(b)) => match cpu_busy_basis_points(a, b) {
            Some(value) => {
                Evidence::measured(Metric::CpuBusyBasisPoints, "system", value, SOURCE, window)
            }
            None => Evidence::missing(
                Metric::CpuBusyBasisPoints,
                "system",
                SOURCE,
                Status::Partial,
                "Counter reset, inconsistent values or zero elapsed counter interval.",
            ),
        },
        (Err(status), _) | (_, Err(status)) => Evidence::missing(
            Metric::CpuBusyBasisPoints,
            "system",
            SOURCE,
            status,
            "CPU counter query failed.",
        ),
    };
    if prior.is_some() {
        reading.limitations.push(
            "Window starts at the collector's previous CPU sample; no sleep was needed.".into(),
        );
    }
    result.evidence.push(reading);
}
fn collect_memory(result: &mut Snapshot) {
    const SOURCE: &str = "Win32.K32GetPerformanceInfo";
    let metrics = [
        Metric::PhysicalTotalBytes,
        Metric::PhysicalAvailableBytes,
        Metric::SystemCommitBytes,
        Metric::SystemCommitLimitBytes,
    ];
    // SAFETY: the initialized structure and its byte count match the documented ABI.
    unsafe {
        let mut info: PERFORMANCE_INFORMATION = zeroed();
        info.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
        if K32GetPerformanceInfo(&mut info, size_of::<PERFORMANCE_INFORMATION>() as u32) == 0 {
            let status = last_status();
            for metric in metrics {
                result.evidence.push(Evidence::missing(
                    metric,
                    "system",
                    SOURCE,
                    status,
                    "System memory query failed.",
                ));
            }
            return;
        }
        let values = [
            info.PhysicalTotal,
            info.PhysicalAvailable,
            info.CommitTotal,
            info.CommitLimit,
        ];
        let window = Window::instant();
        for (metric, pages) in metrics.into_iter().zip(values) {
            let bytes = if info.PageSize == 0 {
                None
            } else {
                (pages as u64).checked_mul(info.PageSize as u64)
            };
            result.evidence.push(match bytes {
                Some(value) => Evidence::measured(metric, "system", value, SOURCE, window.clone()),
                None => Evidence::missing(
                    metric,
                    "system",
                    SOURCE,
                    Status::Error,
                    "Invalid page size or byte count overflow.",
                ),
            });
        }
    }
}
fn memory(handle: &OwnedHandle) -> Result<(u64, u64), Status> {
    // SAFETY: the handle is valid; the buffer has the exact declared size.
    unsafe {
        let mut memory: PROCESS_MEMORY_COUNTERS_EX = zeroed();
        memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(
            handle.0,
            (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(),
            memory.cb,
        ) == 0
        {
            return Err(last_status());
        }
        Ok((memory.WorkingSetSize as u64, memory.PrivateUsage as u64))
    }
}
fn io_counters(handle: &OwnedHandle) -> Result<[u64; 3], Status> {
    // SAFETY: the handle is valid; IO_COUNTERS is plain data.
    unsafe {
        let mut io: IO_COUNTERS = zeroed();
        if GetProcessIoCounters(handle.0, &mut io) == 0 {
            return Err(last_status());
        }
        Ok([
            io.ReadTransferCount,
            io.WriteTransferCount,
            io.OtherTransferCount,
        ])
    }
}
fn image_name(handle: &OwnedHandle) -> Option<String> {
    let mut path = vec![0_u16; 32768];
    let mut len = path.len() as u32;
    // SAFETY: the buffer is writable for len UTF-16 units; only the image path is read.
    if unsafe { QueryFullProcessImageNameW(handle.0, 0, path.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    let text = String::from_utf16_lossy(path.get(..len as usize)?);
    text.rsplit('\\').next().map(str::to_owned)
}
fn window_since(prev: &CounterSample, now: &CounterSample) -> Window {
    Window {
        started_at_unix_ms: prev.unix_ms,
        finished_at_unix_ms: now.unix_ms,
        duration_ms: now.at.saturating_sub(prev.at).as_millis() as u64,
    }
}
#[allow(clippy::too_many_arguments)]
fn delta_evidence(
    metric: Metric,
    entity: &str,
    source: &str,
    prev: Option<&CounterSample>,
    now: &CounterSample,
    value: impl Fn(&CounterSample, &CounterSample) -> Option<u64>,
    current: Result<(), Status>,
    limitation: &str,
) -> Evidence {
    let mut e = match (current, prev) {
        (Err(status), _) => Evidence::missing(metric, entity, source, status, "Counter query failed."),
        (Ok(()), None) => Evidence::missing(
            metric,
            entity,
            source,
            Status::Partial,
            "First observation of this instance; a rate needs a second sample of the same instance.",
        ),
        (Ok(()), Some(prev)) => match value(prev, now) {
            Some(v) => Evidence::measured(metric, entity, v, source, window_since(prev, now)),
            None => Evidence::missing(
                metric,
                entity,
                source,
                Status::Partial,
                "Counter regression, zero interval or inconsistent timing.",
            ),
        },
    };
    e.limitations.push(limitation.into());
    e
}
fn process(
    handle: &OwnedHandle,
    pid: u32,
    collector: &mut Collector,
    processors: u32,
) -> Result<(Key, Result<u64, Status>), Status> {
    let (created, cpu) = times(handle)?;
    let io = io_counters(handle);
    let mem = memory(handle);
    let boot = collector.boot_id.clone();
    let now = CounterSample {
        cpu_100ns: cpu,
        io_bytes: io.ok(),
        at: collector.started.elapsed(),
        unix_ms: unix_ms(),
    };
    let tracked = collector.registry.entry(pid, created, boot.as_deref());
    if tracked.instance.executable_name.is_none() {
        tracked.instance.executable_name = image_name(handle);
    }
    let entity = tracked.instance.entity_ref.clone();
    let prev = tracked.counters;
    let window = Window::instant();
    let mut evidence = match mem {
        Ok((ws, private)) => vec![
            Evidence::measured(
                Metric::ProcessWorkingSetBytes,
                &entity,
                ws,
                "Win32.PROCESS_MEMORY_COUNTERS_EX.WorkingSetSize",
                window.clone(),
            ),
            Evidence::measured(
                Metric::ProcessPrivateCommitBytes,
                &entity,
                private,
                "Win32.PROCESS_MEMORY_COUNTERS_EX.PrivateUsage",
                window,
            ),
        ],
        Err(status) => [
            Metric::ProcessWorkingSetBytes,
            Metric::ProcessPrivateCommitBytes,
        ]
        .map(|m| {
            Evidence::missing(
                m,
                &entity,
                "Win32.K32GetProcessMemoryInfo",
                status,
                "Process memory query failed.",
            )
        })
        .into(),
    };
    let cpu_note = format!(
        "Share of total capacity of {processors} logical processors; one saturated thread reads as at most {} basis points.",
        10_000 / processors.max(1)
    );
    evidence.push(delta_evidence(
        Metric::ProcessCpuBasisPoints,
        &entity,
        "Win32.GetProcessTimes",
        prev.as_ref().filter(|p| p.cpu_100ns.is_some()),
        &now,
        |p, n| {
            process_cpu_basis_points(
                p.cpu_100ns?,
                n.cpu_100ns?,
                n.at.checked_sub(p.at)?,
                processors,
            )
        },
        cpu.map(|_| ()).ok_or(Status::Error),
        &cpu_note,
    ));
    const IO_NOTE: &str =
        "GetProcessIoCounters includes cached, network and device I/O; not physical disk activity.";
    for (i, (metric, source)) in [
        (
            Metric::ProcessIoReadBytes,
            "Win32.IO_COUNTERS.ReadTransferCount",
        ),
        (
            Metric::ProcessIoWriteBytes,
            "Win32.IO_COUNTERS.WriteTransferCount",
        ),
        (
            Metric::ProcessIoOtherBytes,
            "Win32.IO_COUNTERS.OtherTransferCount",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        evidence.push(delta_evidence(
            metric,
            &entity,
            source,
            prev.as_ref().filter(|p| p.io_bytes.is_some()),
            &now,
            |p, n| n.io_bytes?[i].checked_sub(p.io_bytes?[i]),
            io.map(|_| ()),
            IO_NOTE,
        ));
    }
    tracked.evidence = evidence;
    tracked.observed_unix_ms = Some(now.unix_ms);
    tracked.counters = Some(now);
    Ok(((pid, created), mem.map(|m| m.0)))
}
fn collect_processes(result: &mut Snapshot, collector: &mut Collector, limit: usize) {
    // A fixed upper bound prevents unbounded allocation; a full buffer is explicitly partial.
    let mut pids = vec![0_u32; 16384];
    let capacity_bytes = (pids.len() * size_of::<u32>()) as u32;
    let mut needed = 0_u32;
    // SAFETY: the buffer is writable for capacity_bytes and needed points to a valid DWORD.
    if unsafe { K32EnumProcesses(pids.as_mut_ptr(), capacity_bytes, &mut needed) } == 0 {
        result.process_coverage.status = last_status();
        result
            .limitations
            .push("Process enumeration failed; coverage is unknown.".into());
        return;
    }
    pids.truncate((needed as usize / size_of::<u32>()).min(pids.len()));
    let mut coverage = ProcessCoverage {
        status: Status::Ok,
        enumerated: pids.len(),
        enumeration_truncated: needed >= capacity_bytes,
        ..ProcessCoverage::default()
    };
    let processors = processor_count();
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for &pid in &pids {
        let row = OwnedHandle::open(pid).and_then(|h| process(&h, pid, collector, processors));
        let row = match row {
            Ok((key, mem)) => {
                seen.insert(key);
                mem.map(|ws| (key, ws))
            }
            Err(status) => Err(status),
        };
        match row {
            Ok(row) => {
                coverage.readable += 1;
                rows.push(row);
            }
            Err(Status::PermissionDenied) => coverage.permission_denied += 1,
            Err(_) => coverage.unavailable += 1,
        }
    }
    if !coverage.enumeration_truncated {
        collector
            .registry
            .sweep(&seen, &pids.iter().copied().collect());
    }
    rows.sort_by_key(|row| std::cmp::Reverse(row.1));
    rows.truncate(limit);
    coverage.returned = rows.len();
    if coverage.permission_denied > 0 || coverage.unavailable > 0 || coverage.enumeration_truncated
    {
        coverage.status = Status::Partial;
    }
    for (key, _) in rows {
        if let Some(t) = collector.registry.get(key) {
            result.processes.push(t.instance.clone());
            result.evidence.extend(t.evidence.iter().cloned());
        }
    }
    result.process_coverage = coverage;
    result.limitations.extend([
        "Process list is ranked by readable working set; inaccessible processes may be larger.".into(),
        "Working sets include shared pages and must not be summed as exclusive physical RAM.".into(),
        "Process identities are session-scoped (PID plus creation time).".into(),
        "Process CPU and I/O are deltas since this collector's previous observation of the same instance; the first observation is unknown.".into(),
        BOOT_NOTE.into(),
    ]);
}

pub(super) fn collect(
    collector: &mut Collector,
    include_processes: bool,
    limit: usize,
    reuse_cpu: bool,
) -> Snapshot {
    let mut result = Snapshot::empty(&collector.session_id);
    collect_cpu(&mut result, collector, reuse_cpu);
    collect_memory(&mut result);
    if include_processes {
        collect_processes(&mut result, collector, limit);
    }
    result.limitations.push("Short sample: no sustained contention, memory leak or causal diagnosis can be established. Disk, GPU and hard-fault activity are not collected yet.".into());
    result
}
