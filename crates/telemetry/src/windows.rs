//! The only unsafe boundary: documented, read-only Windows API calls.
use super::{cpu_busy_basis_points, Collector};
use pcagent_shared::{new_id, unix_ms, Evidence, Metric, ProcessCoverage, ProcessInstance, Snapshot, Status, Window};
use std::{collections::HashSet, mem::{size_of, zeroed}, time::{Duration, Instant}};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, FILETIME, HANDLE, ERROR_ACCESS_DENIED},
    System::{
        ProcessStatus::{K32EnumProcesses, K32GetPerformanceInfo, K32GetProcessMemoryInfo, PERFORMANCE_INFORMATION, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{GetActiveProcessorCount, GetProcessTimes, GetSystemTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by OpenProcess and has one owning wrapper.
        unsafe { CloseHandle(self.0); }
    }
}
fn filetime(t: FILETIME) -> u64 { (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime) }
fn last_status() -> Status {
    // SAFETY: thread-local error retrieval has no memory preconditions.
    if unsafe { GetLastError() } == ERROR_ACCESS_DENIED { Status::PermissionDenied } else { Status::Error }
}
fn cpu_ticks() -> Result<[u64; 3], Status> {
    // SAFETY: FILETIME is plain data; all output pointers are valid for the call.
    unsafe {
        let (mut idle, mut kernel, mut user): (FILETIME, FILETIME, FILETIME) = (zeroed(), zeroed(), zeroed());
        if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 { return Err(last_status()); }
        Ok([filetime(idle), filetime(kernel), filetime(user)])
    }
}
fn collect_cpu(result: &mut Snapshot) {
    const SOURCE: &str = "Win32.GetSystemTimes";
    // SAFETY: 0xffff means ALL_PROCESSOR_GROUPS; the API has no pointer arguments.
    let count = unsafe { GetActiveProcessorCount(0xffff) };
    if count == 0 || count > 64 {
        result.evidence.push(Evidence::missing(Metric::CpuBusyBasisPoints, "system", SOURCE,
            Status::Unsupported, "Aggregate CPU coverage is unavailable or spans multiple processor groups."));
        return;
    }
    let started = unix_ms();
    let clock = Instant::now();
    let first = cpu_ticks();
    std::thread::sleep(Duration::from_secs(1));
    let last = cpu_ticks();
    let window = Window::finish(started, clock);
    let reading = match (first, last) {
        (Ok(a), Ok(b)) => match cpu_busy_basis_points(a, b) {
            Some(value) => Evidence::measured(Metric::CpuBusyBasisPoints, "system", value, SOURCE, window),
            None => Evidence::missing(Metric::CpuBusyBasisPoints, "system", SOURCE, Status::Partial,
                "Counter reset, inconsistent values or zero elapsed counter interval."),
        },
        (Err(status), _) | (_, Err(status)) => Evidence::missing(Metric::CpuBusyBasisPoints, "system", SOURCE,
            status, "CPU counter query failed."),
    };
    result.evidence.push(reading);
}
fn collect_memory(result: &mut Snapshot) {
    const SOURCE: &str = "Win32.K32GetPerformanceInfo";
    let metrics = [Metric::PhysicalTotalBytes, Metric::PhysicalAvailableBytes, Metric::SystemCommitBytes, Metric::SystemCommitLimitBytes];
    // SAFETY: the initialized structure and its byte count match the documented ABI.
    unsafe {
        let mut info: PERFORMANCE_INFORMATION = zeroed();
        info.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
        if K32GetPerformanceInfo(&mut info, size_of::<PERFORMANCE_INFORMATION>() as u32) == 0 {
            let status = last_status();
            for metric in metrics {
                result.evidence.push(Evidence::missing(metric, "system", SOURCE, status, "System memory query failed."));
            }
            return;
        }
        let values = [info.PhysicalTotal, info.PhysicalAvailable, info.CommitTotal, info.CommitLimit];
        let window = Window::instant();
        for (metric, pages) in metrics.into_iter().zip(values) {
            let bytes = if info.PageSize == 0 { None } else { (pages as u64).checked_mul(info.PageSize as u64) };
            result.evidence.push(match bytes {
                Some(value) => Evidence::measured(metric, "system", value, SOURCE, window.clone()),
                None => Evidence::missing(metric, "system", SOURCE, Status::Error, "Invalid page size or byte count overflow."),
            });
        }
    }
}
fn process(handle: HANDLE, pid: u32, collector: &mut Collector) -> Result<(ProcessInstance, Vec<Evidence>, u64), Status> {
    // SAFETY: handle remains owned by the caller throughout all queries; output buffers
    // have the exact API sizes. No process memory content or command line is read.
    unsafe {
        let (mut created, mut exit, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) = (zeroed(), zeroed(), zeroed(), zeroed());
        if GetProcessTimes(handle, &mut created, &mut exit, &mut kernel, &mut user) == 0 { return Err(last_status()); }
        let created = filetime(created);
        if created == 0 { return Err(Status::Partial); }
        let mut memory: PROCESS_MEMORY_COUNTERS_EX = zeroed();
        memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
        if K32GetProcessMemoryInfo(handle, (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX).cast::<PROCESS_MEMORY_COUNTERS>(), memory.cb) == 0 {
            return Err(last_status());
        }
        let entity = collector.identities.entry((pid, created)).or_insert_with(|| new_id("proc")).clone();
        let mut path = vec![0_u16; 32768];
        let mut len = path.len() as u32;
        let name = if QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut len) != 0 {
            let text = String::from_utf16_lossy(&path[..len as usize]);
            text.rsplit('\\').next().map(str::to_owned)
        } else { None };
        let instance = ProcessInstance { entity_ref: entity.clone(), pid, creation_time_filetime: created,
            boot_id: None, executable_name: name };
        let window = Window::instant();
        let readings = vec![
            Evidence::measured(Metric::ProcessWorkingSetBytes, &entity, memory.WorkingSetSize as u64, "Win32.PROCESS_MEMORY_COUNTERS_EX.WorkingSetSize", window.clone()),
            Evidence::measured(Metric::ProcessPrivateCommitBytes, &entity, memory.PrivateUsage as u64, "Win32.PROCESS_MEMORY_COUNTERS_EX.PrivateUsage", window),
        ];
        Ok((instance, readings, memory.WorkingSetSize as u64))
    }
}
fn collect_processes(result: &mut Snapshot, collector: &mut Collector, limit: usize) {
    // A fixed upper bound prevents unbounded allocation; a full buffer is explicitly partial.
    let mut pids = vec![0_u32; 16384];
    let capacity_bytes = (pids.len() * size_of::<u32>()) as u32;
    let mut needed = 0_u32;
    // SAFETY: the buffer is writable for capacity_bytes and needed points to a valid DWORD.
    if unsafe { K32EnumProcesses(pids.as_mut_ptr(), capacity_bytes, &mut needed) } == 0 {
        result.process_coverage.status = last_status();
        result.limitations.push("Process enumeration failed; coverage is unknown.".into());
        return;
    }
    pids.truncate((needed as usize / size_of::<u32>()).min(pids.len()));
    let mut coverage = ProcessCoverage { status: Status::Ok, enumerated: pids.len(),
        enumeration_truncated: needed >= capacity_bytes, ..ProcessCoverage::default() };
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for pid in pids {
        // SAFETY: only query rights are requested; the handle is closed by RAII.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        let row = if handle.is_null() { Err(last_status()) } else { process(OwnedHandle(handle).0, pid, collector) };
        match row {
            Ok(row) => { coverage.readable += 1; seen.insert((row.0.pid, row.0.creation_time_filetime)); rows.push(row); }
            Err(Status::PermissionDenied) => coverage.permission_denied += 1,
            Err(_) => coverage.unavailable += 1,
        }
    }
    // Identity storage is bounded to the currently observed process set, not a persistent history.
    collector.identities.retain(|key, _| seen.contains(key));
    rows.sort_by_key(|row| std::cmp::Reverse(row.2));
    rows.truncate(limit);
    coverage.returned = rows.len();
    if coverage.permission_denied > 0 || coverage.unavailable > 0 || coverage.enumeration_truncated { coverage.status = Status::Partial; }
    for (instance, readings, _) in rows { result.processes.push(instance); result.evidence.extend(readings); }
    result.process_coverage = coverage;
    result.limitations.extend([
        "Process list is ranked by readable working set; inaccessible processes may be larger.".into(),
        "Working sets include shared pages and must not be summed as exclusive physical RAM.".into(),
        "Process identities are session-scoped (PID plus creation time); boot ID is not yet collected.".into(),
    ]);
}

pub(super) fn collect(collector: &mut Collector, include_processes: bool, limit: usize) -> Snapshot {
    let mut result = Snapshot::empty(&collector.session_id);
    collect_cpu(&mut result);
    collect_memory(&mut result);
    if include_processes { collect_processes(&mut result, collector, limit); }
    result.limitations.push("Single short sample: no sustained contention, memory leak or causal diagnosis can be established. Disk, GPU and hard-fault activity are not collected yet.".into());
    result
}
