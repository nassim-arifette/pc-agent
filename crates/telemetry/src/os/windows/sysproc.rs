//! Every process at once through `NtQuerySystemInformation(SystemProcessInformation)`: memory,
//! CPU time, I/O counters and image names, without opening any process. Processes of other users
//! and services are measured without administrator rights; Task Manager reads the same data.
use pcagent_shared::Status;
use std::mem::size_of;
use windows_sys::{
    Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation},
    Win32::Foundation::STATUS_INFO_LENGTH_MISMATCH,
};

/// One process as the system reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RawProcess {
    pub pid: u32,
    pub parent_pid: u32,
    /// Creation time in 100 ns FILETIME units: with the PID, the instance identity.
    pub created: u64,
    /// User plus kernel time in 100 ns units.
    pub cpu_100ns: u64,
    pub working_set: u64,
    /// Commit charge (`PagefileUsage`), the same value as `PrivateUsage` of
    /// `PROCESS_MEMORY_COUNTERS_EX`.
    pub private_commit: u64,
    /// Read, write and other transfer bytes, like `IO_COUNTERS`.
    pub io: [u64; 3],
    pub handles: u32,
    pub threads: u32,
    pub session: u32,
    /// Image file name without its folder, when the system reports one.
    pub name: Option<String>,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: usize,
}

/// `SYSTEM_PROCESS_INFORMATION` up to its I/O counters, with the fields the documented
/// structure calls `Reserved*` named as in the public NT headers. Thread entries follow and are
/// not read. The layout is checked against `windows-sys` in the tests.
#[repr(C)]
#[derive(Clone, Copy)]
struct Entry {
    next_entry_offset: u32,
    number_of_threads: u32,
    working_set_private_size: i64,
    hard_fault_count: u32,
    number_of_threads_high_watermark: u32,
    cycle_time: u64,
    create_time: i64,
    user_time: i64,
    kernel_time: i64,
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    unique_process_key: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    read_operation_count: i64,
    write_operation_count: i64,
    other_operation_count: i64,
    read_transfer_count: i64,
    write_transfer_count: i64,
    other_transfer_count: i64,
}

/// A process list larger than this is refused rather than allocated.
const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;

/// Queries every process. The buffer grows until the list fits, within a fixed bound.
pub(super) fn query() -> Result<Vec<RawProcess>, Status> {
    let mut words: Vec<u64> = vec![0; 128 * 1024];
    for _ in 0..8 {
        let bytes = words.len() * size_of::<u64>();
        let mut needed = 0_u32;
        // SAFETY: the buffer is writable for `bytes` bytes and 8-byte aligned; `needed` is a
        // valid ULONG output. The call only writes inside the buffer.
        let status = unsafe {
            NtQuerySystemInformation(
                SystemProcessInformation,
                words.as_mut_ptr().cast(),
                u32::try_from(bytes).map_err(|_| Status::Error)?,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // Processes can start between two calls: leave some room.
            let wanted = (needed as usize).max(bytes * 2) + 64 * 1024;
            if wanted > MAX_BUFFER_BYTES {
                return Err(Status::Error);
            }
            words = vec![0; wanted.div_ceil(size_of::<u64>())];
            continue;
        }
        if status < 0 {
            return Err(Status::Error);
        }
        // SAFETY: a byte view of the same initialized allocation, for its full length.
        let view = unsafe {
            std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u64>())
        };
        return Ok(parse(view));
    }
    Err(Status::Error)
}

/// Reads the linked entries of a filled buffer. Every read is bounds-checked; an image name is
/// read only when it lies inside the buffer, where the system places it.
fn parse(buffer: &[u8]) -> Vec<RawProcess> {
    let base = buffer.as_ptr() as usize;
    let mut processes = Vec::new();
    let mut offset = 0_usize;
    loop {
        let Some(end) = offset.checked_add(size_of::<Entry>()) else {
            break;
        };
        if end > buffer.len() {
            break;
        }
        // SAFETY: `offset..end` lies inside the buffer and `Entry` is plain data, so an
        // unaligned read of its bytes is valid.
        let e: Entry =
            unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset).cast::<Entry>()) };
        let as_u64 = |v: i64| u64::try_from(v).unwrap_or(0);
        processes.push(RawProcess {
            pid: u32::try_from(e.unique_process_id).unwrap_or(u32::MAX),
            parent_pid: u32::try_from(e.inherited_from_unique_process_id).unwrap_or(0),
            created: as_u64(e.create_time),
            cpu_100ns: as_u64(e.user_time).saturating_add(as_u64(e.kernel_time)),
            working_set: e.working_set_size as u64,
            private_commit: e.pagefile_usage as u64,
            io: [
                as_u64(e.read_transfer_count),
                as_u64(e.write_transfer_count),
                as_u64(e.other_transfer_count),
            ],
            handles: e.handle_count,
            threads: e.number_of_threads,
            session: e.session_id,
            name: name(buffer, base, &e.image_name),
        });
        if e.next_entry_offset == 0 {
            break;
        }
        match offset.checked_add(e.next_entry_offset as usize) {
            Some(next) => offset = next,
            None => break,
        }
    }
    processes
}

fn name(buffer: &[u8], base: usize, s: &UnicodeString) -> Option<String> {
    let len = usize::from(s.length);
    if len == 0 || len % 2 != 0 {
        return None;
    }
    let start = s.buffer.checked_sub(base)?;
    let bytes = buffer.get(start..start.checked_add(len)?)?;
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    text.rsplit('\\').next().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::offset_of;
    use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION as Documented;

    #[test]
    fn layout_matches_the_documented_structure() {
        assert_eq!(
            offset_of!(Entry, image_name),
            offset_of!(Documented, ImageName)
        );
        assert_eq!(
            offset_of!(Entry, unique_process_id),
            offset_of!(Documented, UniqueProcessId)
        );
        assert_eq!(
            offset_of!(Entry, handle_count),
            offset_of!(Documented, HandleCount)
        );
        assert_eq!(
            offset_of!(Entry, session_id),
            offset_of!(Documented, SessionId)
        );
        assert_eq!(
            offset_of!(Entry, working_set_size),
            offset_of!(Documented, WorkingSetSize)
        );
        assert_eq!(
            offset_of!(Entry, pagefile_usage),
            offset_of!(Documented, PagefileUsage)
        );
        assert_eq!(
            offset_of!(Entry, read_operation_count),
            offset_of!(Documented, Reserved7)
        );
        assert_eq!(size_of::<Entry>(), size_of::<Documented>());
        // CPU times and creation time sit in the documented Reserved1 block.
        assert_eq!(offset_of!(Entry, create_time), 32);
        assert_eq!(
            offset_of!(Entry, kernel_time) + 8,
            offset_of!(Entry, image_name)
        );
    }

    /// Builds a two-entry buffer the way the system lays it out, names included.
    fn fixture() -> Vec<u8> {
        let entry = size_of::<Entry>();
        let stride = entry + 64;
        let mut buffer = vec![0_u8; stride * 2];
        let base = buffer.as_ptr() as usize;
        for (i, (pid, name, next)) in [
            (4242_usize, "C:\\apps\\editor.exe", stride as u32),
            (7, "", 0),
        ]
        .into_iter()
        .enumerate()
        {
            let at = i * stride;
            let units: Vec<u16> = name.encode_utf16().collect();
            let name_at = at + entry;
            for (k, u) in units.iter().enumerate() {
                buffer[name_at + 2 * k..name_at + 2 * k + 2].copy_from_slice(&u.to_le_bytes());
            }
            let e = Entry {
                next_entry_offset: next,
                number_of_threads: 9,
                working_set_private_size: 0,
                hard_fault_count: 0,
                number_of_threads_high_watermark: 0,
                cycle_time: 0,
                create_time: 133_000_000_000_000_000,
                user_time: 20_000_000,
                kernel_time: 5_000_000,
                image_name: UnicodeString {
                    length: (units.len() * 2) as u16,
                    maximum_length: (units.len() * 2) as u16,
                    buffer: if units.is_empty() { 0 } else { base + name_at },
                },
                base_priority: 8,
                unique_process_id: pid,
                inherited_from_unique_process_id: 1,
                handle_count: 300,
                session_id: 1,
                unique_process_key: pid,
                peak_virtual_size: 0,
                virtual_size: 0,
                page_fault_count: 0,
                peak_working_set_size: 0,
                working_set_size: 2_480_000_000,
                quota_peak_paged_pool_usage: 0,
                quota_paged_pool_usage: 0,
                quota_peak_non_paged_pool_usage: 0,
                quota_non_paged_pool_usage: 0,
                pagefile_usage: 1_700_000_000,
                peak_pagefile_usage: 0,
                private_page_count: 0,
                read_operation_count: 0,
                write_operation_count: 0,
                other_operation_count: 0,
                read_transfer_count: 10,
                write_transfer_count: 20,
                other_transfer_count: 30,
            };
            // SAFETY: `at..at + entry` lies inside the buffer; Entry is plain data.
            unsafe {
                std::ptr::write_unaligned(buffer.as_mut_ptr().add(at).cast::<Entry>(), e);
            }
        }
        buffer
    }

    #[test]
    fn entries_are_read_with_their_names_and_counters() {
        let buffer = fixture();
        let processes = parse(&buffer);
        assert_eq!(processes.len(), 2);
        let p = &processes[0];
        assert_eq!(p.pid, 4242);
        assert_eq!(p.name.as_deref(), Some("editor.exe"));
        assert_eq!(p.cpu_100ns, 25_000_000);
        assert_eq!(p.working_set, 2_480_000_000);
        assert_eq!(p.private_commit, 1_700_000_000);
        assert_eq!(p.io, [10, 20, 30]);
        assert_eq!(
            (p.parent_pid, p.handles, p.threads, p.session),
            (1, 300, 9, 1)
        );
        assert_eq!(processes[1].name, None);
    }

    #[test]
    fn a_name_outside_the_buffer_or_a_truncated_entry_is_never_read() {
        let mut buffer = fixture();
        // Point the first name far outside the buffer.
        let name_field = offset_of!(Entry, image_name) + offset_of!(UnicodeString, buffer);
        buffer[name_field..name_field + size_of::<usize>()]
            .copy_from_slice(&usize::MAX.to_le_bytes());
        assert_eq!(parse(&buffer)[0].name, None);
        // A buffer cut in the middle of the second entry yields only the first.
        let cut = size_of::<Entry>() + 64 + 10;
        assert_eq!(parse(&buffer[..cut]).len(), 1);
    }

    #[test]
    fn the_live_list_includes_this_test_process_with_its_name() {
        let processes = query().expect("the system process list is readable without admin");
        let me = processes
            .iter()
            .find(|p| p.pid == std::process::id())
            .expect("this process is listed");
        assert!(me.name.as_deref().is_some_and(|n| n.ends_with(".exe")));
        assert!(me.working_set > 0 && me.created > 0);
        // Services and system processes are listed with their memory too.
        assert!(processes.iter().filter(|p| p.working_set > 0).count() > 20);
    }
}
