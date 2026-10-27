//! Read-only Darwin ABI boundary. No paths, arguments, or machine identifiers are retained.
use pcagent_shared::{Status, VolumeInfo, VolumeKind};
use std::{
    ffi::CStr,
    mem::{size_of, zeroed},
    ptr,
};

extern "C" {
    fn mach_port_deallocate(
        task: libc::mach_port_t,
        name: libc::mach_port_t,
    ) -> libc::kern_return_t;
    fn host_page_size(host: libc::host_t, size: *mut libc::vm_size_t) -> libc::kern_return_t;
}

fn status() -> Status {
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EACCES | libc::EPERM) => Status::PermissionDenied,
        Some(libc::ESRCH | libc::ENOENT) => Status::Stale,
        _ => Status::Error,
    }
}

fn sysctl<T: Copy>(name: &CStr) -> Result<T, Status> {
    // SAFETY: private callers use only plain Darwin sysctl scalar/timeval types. The kernel
    // receives their exact writable size; short or oversized results are never returned.
    unsafe {
        let mut value: T = zeroed();
        let mut len = size_of::<T>();
        if libc::sysctlbyname(
            name.as_ptr(),
            ptr::from_mut(&mut value).cast(),
            &mut len,
            ptr::null_mut(),
            0,
        ) != 0
        {
            return Err(status());
        }
        if len != size_of::<T>() {
            return Err(Status::Partial);
        }
        Ok(value)
    }
}

pub(super) fn memory_total() -> Result<u64, Status> {
    sysctl(c"hw.memsize")
}
pub(super) fn processors() -> Result<u32, Status> {
    sysctl(c"hw.logicalcpu")
}
pub(crate) fn boot_id() -> Option<String> {
    let boot: libc::timeval = sysctl(c"kern.boottime").ok()?;
    Some(format!("boot-estimate-unix-s-{}", boot.tv_sec / 60 * 60))
}
pub(super) fn text(name: &CStr) -> Result<String, Status> {
    let mut bytes = [0u8; 256];
    let mut len = bytes.len();
    // SAFETY: read-only sysctl with fixed writable capacity and NUL-terminated key.
    if unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            bytes.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    } != 0
    {
        return Err(status());
    }
    bytes
        .get(..len)
        .and_then(|b| CStr::from_bytes_until_nul(b).ok())
        .and_then(|b| b.to_str().ok())
        .map(str::to_owned)
        .ok_or(Status::Partial)
}

struct Host(libc::mach_port_t);
impl Host {
    #[allow(deprecated)]
    fn new() -> Self {
        // SAFETY: returns an owned send right for the current host, no elevated rights.
        Self(unsafe { libc::mach_host_self() })
    }
}
impl Drop for Host {
    #[allow(deprecated)]
    fn drop(&mut self) {
        // SAFETY: release exactly the send right acquired by mach_host_self.
        unsafe {
            mach_port_deallocate(libc::mach_task_self(), self.0);
        }
    }
}

pub(super) fn cpu_ticks() -> Result<[u64; 3], Status> {
    let host = Host::new();
    // SAFETY: zero is valid for this plain integer output structure.
    let mut info: libc::host_cpu_load_info = unsafe { zeroed() };
    let mut count = libc::HOST_CPU_LOAD_INFO_COUNT;
    // SAFETY: exact ABI structure and count; the host send right remains alive.
    if unsafe {
        libc::host_statistics(
            host.0,
            libc::HOST_CPU_LOAD_INFO,
            ptr::from_mut(&mut info).cast(),
            &mut count,
        )
    } != libc::KERN_SUCCESS
    {
        return Err(Status::Error);
    }
    if count != libc::HOST_CPU_LOAD_INFO_COUNT {
        return Err(Status::Partial);
    }
    let [user, system, idle, nice] = info.cpu_ticks.map(u64::from);
    // The portable CPU reducer expects kernel ticks INCLUDING idle, as on Win32.
    Ok([idle, system + idle, user + nice])
}

pub(super) fn available_memory() -> Result<u64, Status> {
    let host = Host::new();
    // SAFETY: this is a plain integer Darwin output structure with no invalid bit patterns.
    let mut info: libc::vm_statistics64 = unsafe { zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    // SAFETY: the buffer has the size expressed in integer_t units by count.
    if unsafe {
        libc::host_statistics64(
            host.0,
            libc::HOST_VM_INFO64,
            ptr::from_mut(&mut info).cast(),
            &mut count,
        )
    } != libc::KERN_SUCCESS
    {
        return Err(Status::Error);
    }
    if count < 4 {
        return Err(Status::Partial);
    }
    let mut pagesize = 0;
    // SAFETY: pagesize is writable and host is live.
    if unsafe { host_page_size(host.0, &mut pagesize) } != libc::KERN_SUCCESS || pagesize == 0 {
        return Err(Status::Error);
    }
    available_bytes(info.free_count, info.inactive_count, pagesize as u64).ok_or(Status::Error)
}

fn available_bytes(free: u32, inactive: u32, page_size: u64) -> Option<u64> {
    // free_count already includes speculative pages; do not add them again.
    (u64::from(free) + u64::from(inactive)).checked_mul(page_size)
}

pub(super) struct Process {
    pub pid: u32,
    pub parent: u32,
    pub created: u64,
    pub name: Option<String>,
    pub resident: u64,
    pub cpu_100ns: u64,
    pub io: Option<[u64; 2]>,
    pub threads: u32,
    pub files: u32,
    pub session: u32,
    pub role: crate::ProcessRole,
}

fn bsd(pid: u32) -> Result<libc::proc_bsdinfo, Status> {
    let pid = i32::try_from(pid).map_err(|_| Status::Error)?;
    // SAFETY: proc_bsdinfo consists of integers and fixed byte arrays.
    let mut info: libc::proc_bsdinfo = unsafe { zeroed() };
    // SAFETY: exact buffer and size for the requested BSDINFO flavor.
    let len = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            ptr::from_mut(&mut info).cast(),
            size_of::<libc::proc_bsdinfo>() as i32,
        )
    };
    if len <= 0 {
        return Err(status());
    }
    if len as usize != size_of::<libc::proc_bsdinfo>() {
        return Err(Status::Partial);
    }
    Ok(info)
}

fn created(info: &libc::proc_bsdinfo) -> Option<u64> {
    // Keep the shared FILETIME unit/epoch even though the native source is Unix microseconds.
    info.pbi_start_tvsec
        .checked_add(11_644_473_600)?
        .checked_mul(10_000_000)?
        .checked_add(info.pbi_start_tvusec.checked_mul(10)?)
}

pub(super) fn creation(pid: u32) -> Result<u64, Status> {
    created(&bsd(pid)?).ok_or(Status::Partial)
}

pub(super) fn pids() -> Result<Vec<u32>, Status> {
    // SAFETY: null/zero is libproc's size query and writes nothing.
    let count = unsafe { libc::proc_listallpids(ptr::null_mut(), 0) };
    if count <= 0 {
        return Err(status());
    }
    let mut capacity = count as usize + 256;
    for _ in 0..4 {
        let mut pids = vec![0i32; capacity];
        let bytes = capacity
            .checked_mul(size_of::<i32>())
            .and_then(|n| i32::try_from(n).ok())
            .ok_or(Status::Error)?;
        // SAFETY: writable i32 PID buffer with byte length, no pointers to process memory.
        let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
        if count <= 0 {
            return Err(status());
        }
        if (count as usize) < capacity {
            pids.truncate(count as usize);
            return Ok(pids
                .into_iter()
                .filter_map(|pid| u32::try_from(pid).ok())
                .filter(|pid| *pid > 0)
                .collect());
        }
        capacity = capacity.checked_mul(2).ok_or(Status::Error)?;
    }
    Err(Status::Partial)
}

pub(super) fn process(pid: u32) -> Result<Process, Status> {
    let first = bsd(pid)?;
    let created = created(&first).ok_or(Status::Partial)?;
    // SAFETY: task/rusage structures consist of integers and arrays; zero is valid.
    let (mut task, mut usage): (libc::proc_taskinfo, libc::rusage_info_v2) =
        unsafe { (zeroed(), zeroed()) };
    // SAFETY: matching TASKINFO output flavor and exact writable structure size.
    let len = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTASKINFO,
            0,
            ptr::from_mut(&mut task).cast(),
            size_of::<libc::proc_taskinfo>() as i32,
        )
    };
    if len <= 0 {
        return Err(status());
    }
    if len as usize != size_of::<libc::proc_taskinfo>() {
        return Err(Status::Partial);
    }
    // SAFETY: libproc writes the v2 structure selected by the flavor. Its header declares an
    // opaque rusage_info_t pointer although the ABI expects the structure address.
    let io = (unsafe {
        libc::proc_pid_rusage(
            pid as i32,
            libc::RUSAGE_INFO_V2,
            ptr::from_mut(&mut usage).cast(),
        )
    } == 0)
        .then_some([usage.ri_diskio_bytesread, usage.ri_diskio_byteswritten]);
    let role = process_role(pid).unwrap_or(if first.pbi_uid < 500 {
        crate::ProcessRole::System
    } else {
        crate::ProcessRole::Application
    });
    // SAFETY: getsid is a read-only query for this validated PID.
    let session = u32::try_from(unsafe { libc::getsid(pid as i32) }).map_err(|_| status())?;
    if creation(pid)? != created {
        return Err(Status::Stale);
    }
    let name = first
        .pbi_name
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect::<Vec<_>>();
    let name = String::from_utf8(name).ok().filter(|s| !s.is_empty());
    Ok(Process {
        pid,
        parent: first.pbi_ppid,
        created,
        name,
        resident: task.pti_resident_size,
        cpu_100ns: task
            .pti_total_user
            .checked_add(task.pti_total_system)
            .ok_or(Status::Error)?
            / 100,
        io,
        threads: task.pti_threadnum.try_into().map_err(|_| Status::Error)?,
        files: first.pbi_nfiles,
        session,
        role,
    })
}

fn process_role(pid: u32) -> Option<crate::ProcessRole> {
    let max: i32 = sysctl(c"kern.argmax").ok()?;
    let size = usize::try_from(max).ok()?.min(1024 * 1024);
    let mut bytes = vec![0u8; size];
    let mut len = bytes.len();
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROCARGS2,
        i32::try_from(pid).ok()?,
    ];
    // SAFETY: caller-owned byte buffer and a fixed sysctl MIB with a native-enumerated PID.
    let code = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as u32,
            bytes.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    let role = (code == 0)
        .then(|| classify_args(bytes.get(..len)?))
        .flatten();
    // No command text is retained in a struct, observation, error, or local diagnostic.
    bytes.fill(0);
    role
}

fn classify_args(bytes: &[u8]) -> Option<crate::ProcessRole> {
    let count = i32::from_ne_bytes(bytes.get(..4)?.try_into().ok()?);
    if count <= 0 {
        return None;
    }
    let tail = bytes.get(4..)?;
    let end = tail.iter().position(|b| *b == 0)?;
    let executable = std::str::from_utf8(&tail[..end]).ok()?;
    // The first NUL-delimited entry is the executable, followed by padding, argv, then env.
    // Only argc arguments are inspected, never the environment.
    let mut args = tail
        .get(end..)?
        .split(|b| *b == 0)
        .skip_while(|a| a.is_empty());
    let mut application = executable.contains(".app/Contents/MacOS/");
    for _ in 0..count {
        let argument = args.next()?;
        application |= matches!(
            argument,
            b"--type=renderer" | b"--type=gpu-process" | b"--type=utility" | b"--type=zygote"
        );
    }
    application.then_some(crate::ProcessRole::Application)
}

pub(crate) fn resource_usage() -> crate::ResourceUsage {
    // SAFETY: zeroed plain output structure for the calling thread, not any other task.
    let mut time: libc::timespec = unsafe { zeroed() };
    // SAFETY: clock_gettime writes an exact timespec for the calling thread's CPU clock.
    let cpu = (unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) } == 0)
        .then(|| {
            u64::try_from(time.tv_sec)
                .ok()?
                .checked_mul(10_000_000)?
                .checked_add(u64::try_from(time.tv_nsec).ok()? / 100)
        })
        .flatten();
    crate::ResourceUsage {
        thread_cpu_100ns: cpu,
        process_working_set_bytes: process(std::process::id()).ok().map(|p| p.resident),
        logical_processors: processors().ok(),
    }
}

pub(super) fn volumes() -> Result<Vec<VolumeInfo>, Status> {
    // SAFETY: null/zero requests only the count. MNT_NOWAIT avoids blocking on remote mounts.
    let count = unsafe { libc::getfsstat(ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(status());
    }
    let capacity = count as usize + 32;
    // SAFETY: statfs is an integer/byte-array output structure.
    let mut mounts: Vec<libc::statfs> = (0..capacity).map(|_| unsafe { zeroed() }).collect();
    let bytes = i32::try_from(capacity * size_of::<libc::statfs>()).map_err(|_| Status::Error)?;
    // SAFETY: caller-owned buffer avoids getmntinfo's shared static storage across threads.
    let count = unsafe { libc::getfsstat(mounts.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
    if count < 0 {
        return Err(status());
    }
    if count as usize >= capacity {
        return Err(Status::Partial);
    }
    mounts.truncate(count as usize);
    Ok(mounts.iter().filter_map(|m| {
        let name: Vec<u8> = m.f_mntonname.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
        let root = String::from_utf8(name).ok()?;
        let bytes = |n: u64| n.checked_mul(u64::from(m.f_bsize));
        let local = m.f_flags & libc::MNT_LOCAL as u32 != 0;
        Some(VolumeInfo {
            root, kind: if local { VolumeKind::Unknown } else { VolumeKind::Network },
            total_bytes: bytes(m.f_blocks), free_bytes: bytes(m.f_bfree),
            available_to_caller_bytes: bytes(m.f_bavail),
            source: "Darwin.getfsstat(MNT_NOWAIT)".into(),
            status: if local || [bytes(m.f_blocks), bytes(m.f_bfree), bytes(m.f_bavail)].contains(&None) {
                Status::Partial
            } else {
                Status::Ok
            },
            limitations: vec!["Cached mount statistics; APFS volumes can share container capacity. Local mount does not prove a non-removable physical device.".into()],
        })
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_pages_do_not_double_count_speculative_pages_or_overflow() {
        assert_eq!(available_bytes(10, 20, 16_384), Some(491_520));
        assert_eq!(available_bytes(u32::MAX, u32::MAX, u64::MAX), None);
    }

    #[test]
    fn native_readings_include_self_and_real_memory() {
        assert!(memory_total().unwrap() > 0);
        assert!(available_memory().unwrap() <= memory_total().unwrap());
        assert!(processors().unwrap() > 0);
        assert!(pids().unwrap().contains(&std::process::id()));
        let process = process(std::process::id()).unwrap();
        assert!(process.resident > 0);
        assert_eq!(creation(process.pid).unwrap(), process.created);
        assert!(volumes().unwrap().iter().any(|v| v.root == "/"));
        assert!(cpu_ticks().is_ok());
    }

    #[test]
    fn arguments_are_classified_but_never_returned_or_read_as_environment() {
        let fixture = |argc: i32, bytes: &[u8]| {
            let mut buffer = argc.to_ne_bytes().to_vec();
            buffer.extend_from_slice(bytes);
            buffer
        };
        assert_eq!(
            classify_args(&fixture(
                2,
                b"/bin/browser\0\0browser\0--type=renderer\0SECRET=private\0"
            )),
            Some(crate::ProcessRole::Application)
        );
        assert_eq!(
            classify_args(&fixture(
                1,
                b"/usr/bin/service\0\0service\0--type=renderer\0"
            )),
            None
        );
        assert_eq!(
            classify_args(&fixture(
                1,
                b"/Applications/Test.app/Contents/MacOS/Test\0\0Test\0"
            )),
            Some(crate::ProcessRole::Application)
        );
        assert_eq!(classify_args(&fixture(2, b"broken")), None);
    }
}
