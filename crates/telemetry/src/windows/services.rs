//! Service Control Manager enumeration with read-only rights and bounded buffers.
use crate::services::{attributed_pid, state_name, ServiceProcess, ServiceProcesses};
use pcagent_shared::Status;
use std::{collections::HashSet, mem::size_of, ptr::null};
use windows_sys::Win32::{
    Foundation::{GetLastError, ERROR_ACCESS_DENIED, ERROR_MORE_DATA},
    System::Services::{
        CloseServiceHandle, EnumServicesStatusExW, OpenSCManagerW, ENUM_SERVICE_STATUS_PROCESSW,
        SC_ENUM_PROCESS_INFO, SC_HANDLE, SC_MANAGER_ENUMERATE_SERVICE, SERVICE_STATE_ALL,
        SERVICE_WIN32,
    },
};

const BUFFER_BYTES: usize = 256 * 1024;
const MAX_PAGES: usize = 32;
const MAX_SERVICES: usize = 16_384;

struct Manager(SC_HANDLE);
impl Drop for Manager {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns the open SCM handle.
        unsafe { CloseServiceHandle(self.0) };
    }
}

pub(crate) fn collect() -> ServiceProcesses {
    let mut result = ServiceProcesses {
        services: Vec::new(),
        status: Status::Ok,
        limitations: vec![
            "SCM enumeration covers Win32 services visible to this account; services with a denied SERVICE_QUERY_STATUS right may be omitted by Windows. Kernel drivers are excluded.".into(),
            "Service PIDs are observations, not execution authority; starting and stopping services have no reliable PID attribution.".into(),
        ],
    };
    // SAFETY: null machine/database select the local default; enumeration access cannot mutate services.
    let manager = unsafe { OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE) };
    if manager.is_null() {
        // SAFETY: thread-local error retrieval has no memory preconditions.
        fail(&mut result, unsafe { GetLastError() });
        return result;
    }
    let manager = Manager(manager);
    let mut resume = 0;
    let mut seen = HashSet::new();
    for _ in 0..MAX_PAGES {
        // Word alignment is sufficient for ENUM_SERVICE_STATUS_PROCESSW and UTF-16 strings.
        let mut storage = vec![0_usize; BUFFER_BYTES / size_of::<usize>()];
        let (mut needed, mut count) = (0_u32, 0_u32);
        let previous_resume = resume;
        // SAFETY: manager is open for enumeration; storage is writable for BUFFER_BYTES;
        // every scalar output points to valid memory and null group selects all services.
        let ok = unsafe {
            EnumServicesStatusExW(
                manager.0,
                SC_ENUM_PROCESS_INFO,
                SERVICE_WIN32,
                SERVICE_STATE_ALL,
                storage.as_mut_ptr().cast(),
                BUFFER_BYTES as u32,
                &mut needed,
                &mut count,
                &mut resume,
                null(),
            )
        };
        // SAFETY: GetLastError is thread-local and called immediately after the failed API.
        let error = if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        if ok == 0 && error != ERROR_MORE_DATA {
            fail(&mut result, error);
            return result;
        }
        let base = storage.as_ptr() as usize;
        let end = base + BUFFER_BYTES;
        let bytes = (count as usize).checked_mul(size_of::<ENUM_SERVICE_STATUS_PROCESSW>());
        if bytes.is_none_or(|bytes| bytes > BUFFER_BYTES) {
            result.status = Status::Partial;
            result
                .limitations
                .push("SCM returned an invalid service-array length.".into());
            return result;
        }
        // SAFETY: the checked array fits initialized, suitably aligned storage; the API wrote
        // exactly `count` plain-data records before their in-buffer strings.
        let entries = unsafe {
            std::slice::from_raw_parts(
                storage.as_ptr().cast::<ENUM_SERVICE_STATUS_PROCESSW>(),
                count as usize,
            )
        };
        for entry in entries {
            let Some(name) = bounded_name(entry.lpServiceName, base, end) else {
                result.status = Status::Partial;
                continue;
            };
            let Some(display) = bounded_name(entry.lpDisplayName, base, end) else {
                result.status = Status::Partial;
                continue;
            };
            if seen.insert(name.clone()) {
                result.services.push(ServiceProcess {
                    pid: attributed_pid(
                        entry.ServiceStatusProcess.dwCurrentState,
                        entry.ServiceStatusProcess.dwProcessId,
                    ),
                    service_name: name,
                    display_name: display,
                    state: state_name(entry.ServiceStatusProcess.dwCurrentState).into(),
                });
            }
            if result.services.len() == MAX_SERVICES {
                result.status = Status::Partial;
                result
                    .limitations
                    .push("Service enumeration reached the row bound.".into());
                return result;
            }
        }
        if ok != 0 {
            result
                .services
                .sort_by(|a, b| a.service_name.cmp(&b.service_name));
            if result.status == Status::Partial {
                result
                    .limitations
                    .push("Some SCM strings were invalid and their rows were omitted.".into());
            }
            return result;
        }
        if resume == previous_resume {
            result.status = Status::Partial;
            result.limitations.push(
                "SCM pagination made no progress; remaining services are unavailable.".into(),
            );
            return result;
        }
    }
    result.status = Status::Partial;
    result
        .limitations
        .push("Service enumeration reached the page bound.".into());
    result
}

fn fail(result: &mut ServiceProcesses, code: u32) {
    result.status = if !result.services.is_empty() {
        Status::Partial
    } else if code == ERROR_ACCESS_DENIED {
        Status::PermissionDenied
    } else {
        Status::Error
    };
    result.limitations.push(format!(
        "Service Control Manager enumeration failed (Win32 {code})."
    ));
}

fn bounded_name(ptr: *const u16, base: usize, end: usize) -> Option<String> {
    let address = ptr as usize;
    if address < base || address >= end || !address.is_multiple_of(2) {
        return None;
    }
    let max = ((end - address) / 2).min(4096);
    // SAFETY: the complete UTF-16 slice fits inside the live initialized API buffer.
    let units = unsafe { std::slice::from_raw_parts(ptr, max) };
    let length = units.iter().position(|&value| value == 0)?;
    String::from_utf16(&units[..length]).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_strings_require_an_in_buffer_terminator() {
        let buffer = [b'A' as u16, 0, b'B' as u16];
        let base = buffer.as_ptr() as usize;
        let end = base + size_of::<[u16; 3]>();
        assert_eq!(
            bounded_name(buffer.as_ptr(), base, end).as_deref(),
            Some("A")
        );
        assert!(bounded_name(std::ptr::null(), base, end).is_none());
        assert!(bounded_name((end - 2) as *const u16, base, end).is_none());
        assert!(bounded_name((base + 1) as *const u16, base, end).is_none());
    }

    #[test]
    fn native_services_are_readable_without_administrator_rights() {
        let result = collect();
        assert_eq!(result.status, Status::Ok, "{:?}", result.limitations);
        assert!(!result.services.is_empty());
        assert!(result.services.iter().any(|service| service.pid != 0));
        assert!(result
            .services
            .iter()
            .all(|service| !service.service_name.is_empty()));
    }
}
