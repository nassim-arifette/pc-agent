//! The only unsafe boundary: least-privilege handles, no elevation, no forced termination, no
//! shell interpreter, no permanent deletion.
use crate::{
    explorer_select_arg, EffectError, EffectOutcome, FileIdentity, FileTarget, ProcessIdentity,
    ProcessTarget, Refusal, SettingsPage,
};
use pcagent_shared::Status;
use std::{
    ffi::OsStr,
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    ptr::{null, null_mut},
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{
        CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
        ERROR_INVALID_PARAMETER, ERROR_PATH_NOT_FOUND, FILETIME, HANDLE, HWND,
        INVALID_HANDLE_VALUE, LPARAM, WAIT_OBJECT_0,
    },
    Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, GetFinalPathNameByHandleW,
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        VOLUME_NAME_DOS,
    },
    System::{
        SystemInformation::{GetSystemTimeAsFileTime, GetTickCount64, GetWindowsDirectoryW},
        Threading::{
            GetProcessTimes, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SYNCHRONIZE,
        },
    },
    UI::{
        Shell::{
            SHFileOperationW, SHQueryRecycleBinW, FOF_ALLOWUNDO, FOF_NOCONFIRMATION,
            FOF_NOCONFIRMMKDIR, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FO_DELETE,
            SHFILEOPSTRUCTW, SHQUERYRBINFO,
        },
        WindowsAndMessaging::{
            EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
            GW_OWNER, WM_CLOSE,
        },
    },
};

struct OwnedHandle(HANDLE);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle was returned by CreateFileW/OpenProcess and has one owning wrapper.
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
fn status_of(code: u32) -> Status {
    if code == ERROR_ACCESS_DENIED {
        Status::PermissionDenied
    } else {
        Status::Error
    }
}
fn wide(s: &OsStr) -> Option<Vec<u16>> {
    let mut w: Vec<u16> = s.encode_wide().collect();
    if w.contains(&0) {
        return None;
    }
    w.push(0);
    Some(w)
}

fn open_attributes(path: &Path) -> Result<Option<OwnedHandle>, Status> {
    let w = wide(path.as_os_str()).ok_or(Status::Error)?;
    // SAFETY: w is NUL-terminated and outlives the call; attribute-only access, the reparse point
    // itself is opened rather than followed, no security attributes or template handle.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return match last_error() {
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(None),
            code => Err(status_of(code)),
        };
    }
    Ok(Some(OwnedHandle(h)))
}
fn final_path(h: &OwnedHandle) -> Result<String, Status> {
    let mut buf = vec![0_u16; 32768];
    // SAFETY: buf is writable for its full length, which is passed as the capacity.
    let n = unsafe {
        GetFinalPathNameByHandleW(
            h.0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    } as usize;
    if n == 0 || n >= buf.len() {
        return Err(status_of(last_error()));
    }
    let text = String::from_utf16(&buf[..n]).map_err(|_| Status::Error)?;
    Ok(text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned())
}
fn same_path(a: &str, b: &str) -> bool {
    a.trim_end_matches('\\')
        .eq_ignore_ascii_case(b.trim_end_matches('\\'))
}
fn identity(h: &OwnedHandle, requested: &Path) -> Result<FileIdentity, Status> {
    // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data and the output pointer is valid.
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = zeroed();
        if GetFileInformationByHandle(h.0, &mut info) == 0 {
            return Err(status_of(last_error()));
        }
        info
    };
    let redirected = !requested
        .to_str()
        .is_some_and(|r| same_path(&final_path(h).unwrap_or_default(), r));
    Ok(FileIdentity {
        volume_serial: u64::from(info.dwVolumeSerialNumber),
        file_id: u128::from((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)),
        size_bytes: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        modified_filetime: filetime(info.ftLastWriteTime),
        is_directory: info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_reparse_point: info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 || redirected,
    })
}
pub fn observe_file(path: &Path) -> Result<Option<FileIdentity>, Status> {
    match open_attributes(path)? {
        Some(h) => identity(&h, path).map(Some),
        None => Ok(None),
    }
}

/// Boot identity derived from wall clock minus uptime, bucketed per minute. Clock adjustments can
/// change it, which only causes a refusal.
fn boot_id() -> Option<String> {
    // SAFETY: FILETIME is plain data and the pointer is valid; GetTickCount64 has no arguments.
    let (now, ticks) = unsafe {
        let mut now: FILETIME = zeroed();
        GetSystemTimeAsFileTime(&mut now);
        (filetime(now), GetTickCount64())
    };
    let boot = now.checked_sub(ticks.checked_mul(10_000)?)?;
    Some(format!("boot-{}", boot / 600_000_000))
}
fn open_process(pid: u32) -> Result<Option<(OwnedHandle, ProcessIdentity)>, Status> {
    // SAFETY: only query and synchronize rights are requested; the handle is owned by RAII.
    let h = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if h.is_null() {
        return match last_error() {
            ERROR_INVALID_PARAMETER => Ok(None),
            code => Err(status_of(code)),
        };
    }
    let h = OwnedHandle(h);
    // SAFETY: the handle stays open for both calls; FILETIME outputs are valid plain data.
    let created = unsafe {
        if WaitForSingleObject(h.0, 0) == WAIT_OBJECT_0 {
            return Ok(None);
        }
        let (mut c, mut e, mut k, mut u): (FILETIME, FILETIME, FILETIME, FILETIME) =
            (zeroed(), zeroed(), zeroed(), zeroed());
        if GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) == 0 {
            return Err(status_of(last_error()));
        }
        filetime(c)
    };
    let identity = ProcessIdentity {
        pid,
        creation_time_filetime: created,
        boot_id: boot_id(),
    };
    Ok(Some((h, identity)))
}
pub fn observe_process(pid: u32) -> Result<Option<ProcessIdentity>, Status> {
    Ok(open_process(pid)?.map(|(_, id)| id))
}

fn windows_dir() -> Result<PathBuf, EffectError> {
    let mut buf = vec![0_u16; 1024];
    // SAFETY: buf is writable for the capacity passed.
    let n = unsafe { GetWindowsDirectoryW(buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return Err(EffectError::new(
            Status::Error,
            "Windows directory unavailable",
        ));
    }
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}
fn spawn_explorer(arg: &str) -> Result<EffectOutcome, EffectError> {
    Command::new(windows_dir()?.join("explorer.exe"))
        .raw_arg(arg)
        .spawn()
        .map(|_| EffectOutcome::Done(None))
        .map_err(|_| EffectError::new(Status::Error, "explorer.exe could not be started"))
}
pub fn open_in_explorer(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    let arg = explorer_select_arg(&file.path)
        .ok_or_else(|| EffectError::new(Status::Error, "path cannot be passed to Explorer"))?;
    spawn_explorer(&arg)
}
pub fn open_settings_page(page: SettingsPage) -> Result<EffectOutcome, EffectError> {
    spawn_explorer(page.uri())
}

struct WindowSearch {
    pid: u32,
    windows: Vec<HWND>,
}
unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: lparam is the WindowSearch pointer passed to EnumWindows, which is synchronous.
    unsafe {
        let search = &mut *(lparam as *mut WindowSearch);
        let mut owner = 0_u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner == search.pid && GetWindow(hwnd, GW_OWNER).is_null() && IsWindowVisible(hwnd) != 0
        {
            search.windows.push(hwnd);
        }
        i32::from(search.windows.len() < 64)
    }
}
/// Posts `WM_CLOSE` to the verified instance's visible top-level windows and waits, bounded.
/// The process handle is held throughout, so the PID cannot be reused meanwhile.
pub fn request_normal_close(
    target: &ProcessTarget,
    wait: Duration,
) -> Result<EffectOutcome, EffectError> {
    let Some((h, identity)) = open_process(target.identity.pid)
        .map_err(|s| EffectError::new(s, "process could not be opened"))?
    else {
        return Ok(EffectOutcome::Refused(Refusal::TargetMissing));
    };
    if !target.identity.same_instance(&identity) {
        return Ok(EffectOutcome::Refused(Refusal::ProcessReplaced));
    }
    let mut search = WindowSearch {
        pid: identity.pid,
        windows: Vec::new(),
    };
    // SAFETY: the callback only touches `search`, which outlives the synchronous enumeration.
    unsafe {
        EnumWindows(
            Some(collect_window),
            &mut search as *mut WindowSearch as LPARAM,
        );
    }
    if search.windows.is_empty() {
        return Ok(EffectOutcome::Refused(Refusal::NoTopLevelWindow));
    }
    for hwnd in &search.windows {
        // SAFETY: posting a message to a possibly stale HWND fails harmlessly.
        unsafe {
            PostMessageW(*hwnd, WM_CLOSE, 0, 0);
        }
    }
    let ms = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: h is a valid process handle with SYNCHRONIZE access.
    if unsafe { WaitForSingleObject(h.0, ms) } == WAIT_OBJECT_0 {
        Ok(EffectOutcome::Done(Some(
            "application exited after a normal close request".into(),
        )))
    } else {
        Ok(EffectOutcome::Refused(
            Refusal::StillRunningAfterCloseRequest,
        ))
    }
}

/// Recycle via SHFileOperationW with undo. The identity is checked on a handle held across the
/// call and the same handle is checked to be inside the recycle bin afterwards. No permanent
/// deletion is attempted when the recycle bin is unavailable.
pub fn recycle_file(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    let Some(h) = open_attributes(&file.path)
        .map_err(|s| EffectError::new(s, "target could not be opened"))?
    else {
        return Ok(EffectOutcome::Refused(Refusal::TargetMissing));
    };
    match identity(&h, &file.path) {
        Ok(id) if id.is_reparse_point => return Ok(EffectOutcome::Refused(Refusal::ReparsePoint)),
        Ok(id) if id == file.identity => {}
        Ok(_) => return Ok(EffectOutcome::Refused(Refusal::TargetReplaced)),
        Err(status) => return Ok(EffectOutcome::Refused(Refusal::Unverifiable { status })),
    }
    let path = file.path.to_str().unwrap_or_default();
    let root = wide(OsStr::new(path.get(..3).unwrap_or_default()))
        .ok_or_else(|| EffectError::new(Status::Error, "invalid volume root"))?;
    // SAFETY: root is NUL-terminated; the info struct is initialized with its exact size.
    let bin = unsafe {
        let mut info: SHQUERYRBINFO = zeroed();
        info.cbSize = size_of::<SHQUERYRBINFO>() as u32;
        SHQueryRecycleBinW(root.as_ptr(), &mut info)
    };
    if bin != 0 {
        return Err(EffectError::new(
            Status::Unsupported,
            "recycle bin unavailable on this volume; permanent deletion not attempted",
        ));
    }
    let mut from = wide(file.path.as_os_str())
        .ok_or_else(|| EffectError::new(Status::Error, "invalid path"))?;
    from.push(0);
    // SAFETY: `from` is double-NUL-terminated and outlives the call; no destination, no window,
    // no name mappings are requested.
    let (code, aborted) = unsafe {
        let mut op: SHFILEOPSTRUCTW = zeroed();
        op.wFunc = FO_DELETE;
        op.pFrom = from.as_ptr();
        op.fFlags = (FOF_ALLOWUNDO
            | FOF_NOCONFIRMATION
            | FOF_WANTNUKEWARNING
            | FOF_NOCONFIRMMKDIR
            | FOF_SILENT
            | FOF_NOERRORUI) as u16;
        let code = SHFileOperationW(&mut op);
        (code, op.fAnyOperationsAborted != 0)
    };
    if code != 0 || aborted {
        return Err(EffectError::new(
            Status::Error,
            "shell recycle operation failed or was aborted",
        ));
    }
    match final_path(&h) {
        Ok(p) if p.to_ascii_lowercase().contains(r"\$recycle.bin\") => {
            Ok(EffectOutcome::Done(Some("moved to the recycle bin".into())))
        }
        _ => Err(EffectError::new(
            Status::Error,
            "operated file could not be confirmed in the recycle bin; inspect manually",
        )),
    }
}
