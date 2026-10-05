//! The only unsafe boundary: least-privilege handles, no elevation, no forced termination, no
//! shell interpreter. Permanent deletion is confined to the approved synthetic cache manifest.
pub(crate) mod startup;
use crate::{
    explorer_select_arg, same_object, verify_recycled, BaselineProtection, EffectError,
    EffectOutcome, FileIdentity, FileTarget, ProcessIdentity, ProcessTarget, ProtectionCheck,
    RecycleEvidence, Refusal, SettingsPage,
};
use pcagent_shared::Status;
use std::{
    cell::{Cell, RefCell},
    ffi::{c_void, OsStr},
    mem::{size_of, zeroed},
    os::windows::{ffi::OsStrExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    ptr::{null, null_mut},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::Duration,
};
use windows_sys::{
    core::{BOOL, GUID, HRESULT},
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND,
            ERROR_INVALID_PARAMETER, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, E_ABORT,
            E_NOINTERFACE, E_POINTER, FILETIME, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, S_OK,
            WAIT_OBJECT_0,
        },
        Storage::FileSystem::{
            CreateFileW, FileDispositionInfo, GetFileInformationByHandle,
            GetFinalPathNameByHandleW, GetVolumeInformationByHandleW, SetFileInformationByHandle,
            BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
            FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING, SYNCHRONIZE,
            VOLUME_NAME_DOS,
        },
        System::{
            Com::{
                CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
                COINIT_APARTMENTTHREADED,
            },
            SystemInformation::{GetSystemTimeAsFileTime, GetTickCount64, GetWindowsDirectoryW},
            Threading::{
                GetProcessTimes, OpenProcess, WaitForSingleObject,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            },
        },
        UI::{
            Shell::{
                FileOperation, SHCreateItemFromParsingName, SHQueryRecycleBinW, FOFX_EARLYFAILURE,
                FOFX_RECYCLEONDELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOCONFIRMMKDIR,
                FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, SHQUERYRBINFO, SIGDN,
                SIGDN_FILESYSPATH, TSF_DELETE_RECYCLE_IF_POSSIBLE,
            },
            WindowsAndMessaging::{
                EnumWindows, GetWindow, GetWindowThreadProcessId, IsWindowVisible, PostMessageW,
                GW_OWNER, WM_CLOSE,
            },
        },
    },
};

pub(crate) struct OwnedHandle(HANDLE);
pub(crate) static ACTION_CANCELLED: AtomicBool = AtomicBool::new(false);
static ACTION_HANDLER_INSTALLED: AtomicBool = AtomicBool::new(false);
unsafe extern "system" fn action_interrupt(event: u32) -> BOOL {
    use windows_sys::Win32::System::Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT};
    if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        ACTION_CANCELLED.store(true, Ordering::SeqCst);
        1
    } else {
        0
    }
}
pub(crate) fn begin_action_cancellation() -> std::io::Result<()> {
    if ACTION_HANDLER_INSTALLED.swap(true, Ordering::SeqCst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "an action is already active",
        ));
    }
    ACTION_CANCELLED.store(false, Ordering::SeqCst);
    // SAFETY: the static callback only writes an atomic; registration is balanced by Drop.
    if unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(action_interrupt), 1)
    } == 0
    {
        ACTION_HANDLER_INSTALLED.store(false, Ordering::SeqCst);
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
pub(crate) fn end_action_cancellation() {
    // SAFETY: unregisters the same static callback installed by begin_action_cancellation.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(action_interrupt), 0);
    }
    ACTION_HANDLER_INSTALLED.store(false, Ordering::SeqCst);
}
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

fn open_with(path: &Path, access: u32, share: u32) -> Result<OwnedHandle, u32> {
    let w = wide(path.as_os_str()).ok_or(ERROR_INVALID_PARAMETER)?;
    // SAFETY: w is NUL-terminated and outlives the call; the reparse point itself is opened rather
    // than followed, no security attributes or template handle.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            access,
            share,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE_VALUE {
        return Err(last_error());
    }
    Ok(OwnedHandle(h))
}
/// Attribute-only handle that shares everything, so it never blocks other opens.
fn open_attributes(path: &Path) -> Result<Option<OwnedHandle>, Status> {
    match open_with(
        path,
        FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    ) {
        Ok(h) => Ok(Some(h)),
        Err(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => Ok(None),
        Err(code) => Err(status_of(code)),
    }
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
    a.trim_start_matches(r"\\?\")
        .trim_end_matches('\\')
        .eq_ignore_ascii_case(b.trim_start_matches(r"\\?\").trim_end_matches('\\'))
}

/// Pin each directory from the volume root downwards to block ancestor renames. Attribute-only
/// reparse changes are still possible: cleanup also matches the complete approved manifest and
/// performs deletion through the verified file handles, never through a subsequently resolved path.
pub(crate) fn pin_directories(root: &Path) -> Result<(Vec<OwnedHandle>, FileIdentity), Refusal> {
    let mut pins = Vec::new();
    let mut observed = None;
    for path in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        if path.as_os_str().is_empty() {
            continue;
        }
        let h = open_with(path, FILE_READ_ATTRIBUTES, FILE_SHARE_READ).map_err(|code| {
            Refusal::Unverifiable {
                status: status_of(code),
            }
        })?;
        let id = identity(&h, path).map_err(|status| Refusal::Unverifiable { status })?;
        if id.is_reparse_point || !id.is_directory {
            return Err(Refusal::ReparsePoint);
        }
        observed = Some(id);
        pins.push(h);
    }
    Ok((pins, observed.ok_or(Refusal::AdapterPreconditionFailed)?))
}

/// Only the synthetic-cache adapter calls this, with a continuously held verified file pin.
pub(crate) fn delete_synthetic_file(pin: OwnedHandle) -> std::io::Result<()> {
    let mut disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: pin owns a DELETE handle; the buffer has the documented structure and size.
    let ok = unsafe {
        SetFileInformationByHandle(
            pin.0,
            FileDispositionInfo,
            (&mut disposition as *mut FILE_DISPOSITION_INFO).cast(),
            size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    drop(pin);
    Ok(())
}
/// Identity of the handle's object (reparse flag from attributes only) and its hard link count.
fn handle_info(h: &OwnedHandle) -> Result<(FileIdentity, u32), Status> {
    // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data and the output pointer is valid.
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = zeroed();
        if GetFileInformationByHandle(h.0, &mut info) == 0 {
            return Err(status_of(last_error()));
        }
        info
    };
    let identity = FileIdentity {
        volume_serial: u64::from(info.dwVolumeSerialNumber),
        file_id: u128::from((u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)),
        size_bytes: (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow),
        modified_filetime: filetime(info.ftLastWriteTime),
        is_directory: info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_reparse_point: info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
    };
    Ok((identity, info.nNumberOfLinks))
}
fn identity(h: &OwnedHandle, requested: &Path) -> Result<FileIdentity, Status> {
    let (mut identity, _) = handle_info(h)?;
    let redirected = !requested
        .to_str()
        .is_some_and(|r| same_path(&final_path(h).unwrap_or_default(), r));
    identity.is_reparse_point |= redirected;
    Ok(identity)
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
/// Opens Explorer with `path` selected, at the user's own request in a local interface. Nothing
/// is modified, and neither a model proposal nor an approval is involved.
pub fn show_in_explorer(path: &Path) -> Result<(), EffectError> {
    let arg = explorer_select_arg(path)
        .ok_or_else(|| EffectError::new(Status::Error, "path cannot be passed to Explorer"))?;
    spawn_explorer(&arg).map(|_| ())
}
/// Opens an `https://` page in the default browser, at the user's click on a cited web source.
/// Only a plain URL is accepted: no other scheme, quote, space or control character.
pub fn open_web_page(url: &str) -> Result<(), EffectError> {
    if !web_url_allowed(url) {
        return Err(EffectError::new(
            Status::Error,
            "only https web pages can be opened",
        ));
    }
    spawn_explorer(url).map(|_| ())
}
pub(crate) fn web_url_allowed(url: &str) -> bool {
    url.len() <= 2048
        && url.strip_prefix("https://").is_some_and(|rest| {
            rest.split('/')
                .next()
                .is_some_and(|host| host.contains('.'))
        })
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'^' | b'`'))
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

/// Exclusive pin on a verified file: `DELETE` access with `FILE_SHARE_READ` only. While it is open
/// no other open can obtain `DELETE` or write access, so the file cannot be renamed, replaced,
/// deleted or modified and its path keeps naming the verified object.
pub(crate) fn pin_verified(file: &FileTarget) -> Result<OwnedHandle, Refusal> {
    if BaselineProtection.is_protected(&file.path) {
        return Err(Refusal::Protected);
    }
    let h = match open_with(
        &file.path,
        DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
        FILE_SHARE_READ,
    ) {
        Ok(h) => h,
        Err(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => return Err(Refusal::TargetMissing),
        Err(ERROR_SHARING_VIOLATION) => return Err(Refusal::InUse),
        Err(ERROR_ACCESS_DENIED) => return Err(Refusal::PermissionDenied),
        Err(code) => {
            return Err(Refusal::Unverifiable {
                status: status_of(code),
            })
        }
    };
    let unverifiable = |status| Refusal::Unverifiable { status };
    let observed = identity(&h, &file.path).map_err(unverifiable)?;
    let (_, links) = handle_info(&h).map_err(unverifiable)?;
    if observed.is_reparse_point {
        return Err(Refusal::ReparsePoint);
    }
    if observed != file.identity || observed.is_directory {
        return Err(Refusal::TargetReplaced);
    }
    if links != 1 {
        return Err(Refusal::MultipleLinks);
    }
    if !file_system_name(&h)
        .map_err(unverifiable)?
        .eq_ignore_ascii_case("NTFS")
    {
        return Err(unverifiable(Status::Unsupported));
    }
    Ok(h)
}
fn file_system_name(h: &OwnedHandle) -> Result<String, Status> {
    let mut name = [0_u16; 64];
    // SAFETY: only the file system name buffer is requested; its capacity is passed.
    let ok = unsafe {
        GetVolumeInformationByHandleW(
            h.0,
            null_mut(),
            0,
            null_mut(),
            null_mut(),
            null_mut(),
            name.as_mut_ptr(),
            name.len() as u32,
        )
    };
    if ok == 0 {
        return Err(status_of(last_error()));
    }
    let n = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    String::from_utf16(&name[..n]).map_err(|_| Status::Error)
}
fn recycle_bin_available(path: &Path) -> Result<(), EffectError> {
    let root = path
        .to_str()
        .and_then(|p| p.get(..3))
        .and_then(|r| wide(OsStr::new(r)))
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
    Ok(())
}

// Minimal COM bindings (layouts from ShObjIdl_core.h; windows-sys exposes no interfaces).
type Raw = *mut c_void;
const IID_IUNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_ISHELLITEM: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);
const IID_IFILEOPERATION: GUID = GUID::from_u128(0x947aab5f_0a5c_4c13_b4d6_4bf7836fc9f8);
const IID_IFILEOPERATIONPROGRESSSINK: GUID =
    GUID::from_u128(0x04b0f1a7_9490_44bc_96e1_4296a31252e2);
fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}
#[repr(C)]
struct UnknownVtbl {
    query_interface: usize,
    add_ref: usize,
    release: unsafe extern "system" fn(Raw) -> u32,
}
#[repr(C)]
struct ShellItemVtbl {
    base: UnknownVtbl,
    bind_to_handler: usize,
    get_parent: usize,
    get_display_name: unsafe extern "system" fn(Raw, SIGDN, *mut *mut u16) -> HRESULT,
    get_attributes: usize,
    compare: usize,
}
#[repr(C)]
struct FileOperationVtbl {
    base: UnknownVtbl,
    advise: unsafe extern "system" fn(Raw, Raw, *mut u32) -> HRESULT,
    unadvise: unsafe extern "system" fn(Raw, u32) -> HRESULT,
    set_operation_flags: unsafe extern "system" fn(Raw, u32) -> HRESULT,
    /// SetProgressMessage .. CopyItems.
    unused: [usize; 12],
    delete_item: unsafe extern "system" fn(Raw, Raw, Raw) -> HRESULT,
    delete_items: usize,
    new_item: usize,
    perform_operations: unsafe extern "system" fn(Raw) -> HRESULT,
    get_any_operations_aborted: unsafe extern "system" fn(Raw, *mut BOOL) -> HRESULT,
}
/// Owned, non-null COM interface pointer whose vtable starts with `UnknownVtbl`.
struct Com<V>(*mut *const V);
impl<V> Com<V> {
    fn raw(&self) -> Raw {
        self.0.cast()
    }
    fn vtbl(&self) -> &V {
        // SAFETY: a live interface pointer points to its vtable pointer.
        unsafe { &**self.0 }
    }
}
impl<V> Drop for Com<V> {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns one reference; every vtable begins with IUnknown.
        unsafe {
            let base = (*self.0).cast::<UnknownVtbl>();
            ((*base).release)(self.raw());
        }
    }
}
fn file_system_path(item: Raw) -> Option<String> {
    if item.is_null() {
        return None;
    }
    let mut out: *mut u16 = null_mut();
    // SAFETY: item is a live IShellItem for the duration of the callback; the returned string is
    // CoTaskMem-allocated, read up to its NUL within a bound, and freed here.
    unsafe {
        let vtbl = &**item.cast::<*const ShellItemVtbl>();
        if (vtbl.get_display_name)(item, SIGDN_FILESYSPATH, &mut out) < 0 || out.is_null() {
            return None;
        }
        let text = (0..32768)
            .find(|&i| *out.add(i) == 0)
            .and_then(|n| String::from_utf16(std::slice::from_raw_parts(out, n)).ok());
        CoTaskMemFree(out.cast_const().cast());
        text
    }
}

/// IFileOperationProgressSink owned by Rust; `Release` never frees it.
#[repr(C)]
struct ProgressSink {
    vtbl: *const SinkVtbl,
    refs: AtomicU32,
    path: PathBuf,
    expected: FileIdentity,
    pin: Cell<Option<OwnedHandle>>,
    tracker: OwnedHandle,
    pre_deletes: Cell<u32>,
    approved: Cell<bool>,
    deleted: Cell<Option<HRESULT>>,
    recycled: RefCell<Option<String>>,
}
#[repr(C)]
struct SinkVtbl {
    query_interface: unsafe extern "system" fn(Raw, *const GUID, *mut Raw) -> HRESULT,
    add_ref: unsafe extern "system" fn(Raw) -> u32,
    release: unsafe extern "system" fn(Raw) -> u32,
    start_operations: unsafe extern "system" fn(Raw) -> HRESULT,
    finish_operations: unsafe extern "system" fn(Raw, HRESULT) -> HRESULT,
    pre_rename_item: unsafe extern "system" fn(Raw, u32, Raw, *const u16) -> HRESULT,
    post_rename_item: unsafe extern "system" fn(Raw, u32, Raw, *const u16, HRESULT, Raw) -> HRESULT,
    pre_move_item: unsafe extern "system" fn(Raw, u32, Raw, Raw, *const u16) -> HRESULT,
    post_move_item:
        unsafe extern "system" fn(Raw, u32, Raw, Raw, *const u16, HRESULT, Raw) -> HRESULT,
    pre_copy_item: unsafe extern "system" fn(Raw, u32, Raw, Raw, *const u16) -> HRESULT,
    post_copy_item:
        unsafe extern "system" fn(Raw, u32, Raw, Raw, *const u16, HRESULT, Raw) -> HRESULT,
    pre_delete_item: unsafe extern "system" fn(Raw, u32, Raw) -> HRESULT,
    post_delete_item: unsafe extern "system" fn(Raw, u32, Raw, HRESULT, Raw) -> HRESULT,
    pre_new_item: unsafe extern "system" fn(Raw, u32, Raw, *const u16) -> HRESULT,
    post_new_item: unsafe extern "system" fn(
        Raw,
        u32,
        Raw,
        *const u16,
        *const u16,
        u32,
        HRESULT,
        Raw,
    ) -> HRESULT,
    update_progress: unsafe extern "system" fn(Raw, u32, u32) -> HRESULT,
    reset_timer: unsafe extern "system" fn(Raw) -> HRESULT,
    pause_timer: unsafe extern "system" fn(Raw) -> HRESULT,
    resume_timer: unsafe extern "system" fn(Raw) -> HRESULT,
}
static SINK_VTBL: SinkVtbl = SinkVtbl {
    query_interface: sink_query_interface,
    add_ref: sink_add_ref,
    release: sink_release,
    start_operations: sink_start,
    finish_operations: sink_finish,
    pre_rename_item: sink_refuse_named,
    post_rename_item: sink_post_rename,
    pre_move_item: sink_refuse_transfer,
    post_move_item: sink_post_transfer,
    pre_copy_item: sink_refuse_transfer,
    post_copy_item: sink_post_transfer,
    pre_delete_item: sink_pre_delete,
    post_delete_item: sink_post_delete,
    pre_new_item: sink_refuse_named,
    post_new_item: sink_post_new,
    update_progress: sink_progress,
    reset_timer: sink_ok,
    pause_timer: sink_ok,
    resume_timer: sink_ok,
};
/// # Safety
/// `this` must be the `ProgressSink` registered with `IFileOperation::Advise`.
unsafe fn sink<'a>(this: Raw) -> &'a ProgressSink {
    // SAFETY: guaranteed by the caller.
    unsafe { &*this.cast_const().cast::<ProgressSink>() }
}
unsafe extern "system" fn sink_query_interface(
    this: Raw,
    riid: *const GUID,
    ppv: *mut Raw,
) -> HRESULT {
    // SAFETY: COM passes a valid IID pointer and output slot; `this` is our sink.
    unsafe {
        if ppv.is_null() {
            return E_POINTER;
        }
        if !riid.is_null()
            && (same_guid(&*riid, &IID_IUNKNOWN)
                || same_guid(&*riid, &IID_IFILEOPERATIONPROGRESSSINK))
        {
            sink_add_ref(this);
            *ppv = this;
            S_OK
        } else {
            *ppv = null_mut();
            E_NOINTERFACE
        }
    }
}
unsafe extern "system" fn sink_add_ref(this: Raw) -> u32 {
    // SAFETY: `this` is our sink.
    let s = unsafe { sink(this) };
    s.refs.fetch_add(1, Ordering::SeqCst).saturating_add(1)
}
unsafe extern "system" fn sink_release(this: Raw) -> u32 {
    // SAFETY: `this` is our sink; memory is owned by Rust and never freed here.
    let s = unsafe { sink(this) };
    s.refs
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |r| r.checked_sub(1))
        .map_or(0, |r| r.saturating_sub(1))
}
unsafe extern "system" fn sink_ok(_: Raw) -> HRESULT {
    S_OK
}
/// The shell opens the file with `DELETE` after `StartOperations` and before `PreDeleteItem`
/// (observed on Windows 11 and the `windows-latest` runner, with or without the early-failure,
/// nuke-warning and recycle flags); the pin would make that open fail with a sharing violation. It is therefore released here, as late as the shell
/// allows. Until now it was held continuously since verification, so the path still names the
/// verified object at this point.
unsafe extern "system" fn sink_start(this: Raw) -> HRESULT {
    // SAFETY: `this` is our sink.
    let s = unsafe { sink(this) };
    drop(s.pin.take());
    S_OK
}
unsafe extern "system" fn sink_finish(_: Raw, _: HRESULT) -> HRESULT {
    S_OK
}
unsafe extern "system" fn sink_progress(_: Raw, _: u32, _: u32) -> HRESULT {
    S_OK
}
unsafe extern "system" fn sink_refuse_named(_: Raw, _: u32, _: Raw, _: *const u16) -> HRESULT {
    E_ABORT
}
unsafe extern "system" fn sink_refuse_transfer(
    _: Raw,
    _: u32,
    _: Raw,
    _: Raw,
    _: *const u16,
) -> HRESULT {
    E_ABORT
}
unsafe extern "system" fn sink_post_rename(
    _: Raw,
    _: u32,
    _: Raw,
    _: *const u16,
    _: HRESULT,
    _: Raw,
) -> HRESULT {
    S_OK
}
unsafe extern "system" fn sink_post_transfer(
    _: Raw,
    _: u32,
    _: Raw,
    _: Raw,
    _: *const u16,
    _: HRESULT,
    _: Raw,
) -> HRESULT {
    S_OK
}
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn sink_post_new(
    _: Raw,
    _: u32,
    _: Raw,
    _: *const u16,
    _: *const u16,
    _: u32,
    _: HRESULT,
    _: Raw,
) -> HRESULT {
    S_OK
}
/// Last check before the shell moves the file: exactly one item, recyclable, the planned path, the
/// tracked object still has one link and the path still names it (not through a reparse point).
unsafe extern "system" fn sink_pre_delete(this: Raw, flags: u32, item: Raw) -> HRESULT {
    // SAFETY: `this` is our sink.
    let s = unsafe { sink(this) };
    let calls = s.pre_deletes.get().saturating_add(1);
    s.pre_deletes.set(calls);
    let recyclable = flags & TSF_DELETE_RECYCLE_IF_POSSIBLE as u32 != 0;
    let same_item = file_system_path(item)
        .zip(s.path.to_str())
        .is_some_and(|(a, b)| same_path(&a, b));
    let tracked = handle_info(&s.tracker)
        .is_ok_and(|(id, links)| same_object(&id, &s.expected) && links == 1);
    let at_path = observe_file(&s.path)
        .ok()
        .flatten()
        .is_some_and(|id| same_object(&id, &s.expected) && !id.is_reparse_point);
    if calls != 1 || !recyclable || !same_item || !tracked || !at_path {
        return E_ABORT;
    }
    s.approved.set(true);
    S_OK
}
unsafe extern "system" fn sink_post_delete(
    this: Raw,
    _: u32,
    _: Raw,
    hr: HRESULT,
    newly_created: Raw,
) -> HRESULT {
    // SAFETY: `this` is our sink.
    let s = unsafe { sink(this) };
    if s.deleted.replace(Some(hr)).is_some() {
        s.approved.set(false);
        return E_ABORT;
    }
    *s.recycled.borrow_mut() = file_system_path(newly_created);
    S_OK
}

struct Apartment;
impl Apartment {
    fn enter() -> Result<Self, EffectError> {
        // SAFETY: called once on a fresh worker thread; no reserved pointer.
        let hr = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
        if hr < 0 {
            return Err(EffectError::new(Status::Error, "COM apartment unavailable"));
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the successful CoInitializeEx on this thread.
        unsafe { CoUninitialize() }
    }
}

struct ShellReport {
    trace: String,
    performed: bool,
    approved: bool,
    deleted: Option<HRESULT>,
    recycled: Option<String>,
    tracked: Option<(String, FileIdentity)>,
}
fn shell_error(detail: &str) -> EffectError {
    EffectError::new(Status::Error, detail)
}
/// `IFileOperation::DeleteItem` with undo (recycle), no UI, stopping at the first error. The pin
/// is handed to the progress sink, which releases it in `StartOperations`; `PreDeleteItem`
/// rechecks the target through the tracking handle and the path before approving the move.
fn shell_recycle(
    file: &FileTarget,
    pin: OwnedHandle,
    tracker: OwnedHandle,
) -> Result<ShellReport, EffectError> {
    let path = wide(file.path.as_os_str()).ok_or_else(|| shell_error("invalid path"))?;
    let mut raw: Raw = null_mut();
    // SAFETY: path is NUL-terminated; IID and output pointer are valid.
    let hr = unsafe {
        SHCreateItemFromParsingName(path.as_ptr(), null_mut(), &IID_ISHELLITEM, &mut raw)
    };
    if hr < 0 || raw.is_null() {
        return Err(shell_error("shell item could not be created"));
    }
    let item = Com::<ShellItemVtbl>(raw.cast());
    let mut raw: Raw = null_mut();
    // SAFETY: CLSID, IID and output pointer are valid; no aggregation.
    let hr = unsafe {
        CoCreateInstance(
            &FileOperation,
            null_mut(),
            CLSCTX_ALL,
            &IID_IFILEOPERATION,
            &mut raw,
        )
    };
    if hr < 0 || raw.is_null() {
        return Err(shell_error("shell file operation unavailable"));
    }
    let op = Com::<FileOperationVtbl>(raw.cast());
    let sink = Box::new(ProgressSink {
        vtbl: &SINK_VTBL,
        refs: AtomicU32::new(1),
        path: file.path.clone(),
        expected: file.identity.clone(),
        pin: Cell::new(Some(pin)),
        tracker,
        pre_deletes: Cell::new(0),
        approved: Cell::new(false),
        deleted: Cell::new(None),
        recycled: RefCell::new(None),
    });
    let sink_raw: Raw = (&*sink as *const ProgressSink).cast_mut().cast();
    let flags = FOF_ALLOWUNDO
        | FOF_NOCONFIRMATION
        | FOF_NOCONFIRMMKDIR
        | FOF_NOERRORUI
        | FOF_SILENT
        | FOF_WANTNUKEWARNING
        | FOFX_RECYCLEONDELETE
        | FOFX_EARLYFAILURE;
    let v = op.vtbl();
    let mut trace = String::new();
    // SAFETY: op and item are live interface pointers on this STA thread; the sink implements
    // IFileOperationProgressSink with the header's vtable layout and outlives `op`.
    let performed = unsafe {
        let mut cookie = 0_u32;
        let mut aborted: BOOL = 0;
        let hr = (v.set_operation_flags)(op.raw(), flags);
        trace.push_str(&format!("flags={hr:#x}"));
        let advised = hr >= 0 && {
            let hr = (v.advise)(op.raw(), sink_raw, &mut cookie);
            trace.push_str(&format!(" advise={hr:#x}"));
            hr >= 0
        };
        if advised {
            let hr = (v.delete_item)(op.raw(), item.raw(), null_mut());
            trace.push_str(&format!(" delete_item={hr:#x}"));
            let done = hr >= 0
                && {
                    let hr = (v.perform_operations)(op.raw());
                    trace.push_str(&format!(" perform={hr:#x}"));
                    hr >= 0
                }
                && {
                    let hr = (v.get_any_operations_aborted)(op.raw(), &mut aborted);
                    trace.push_str(&format!(" aborted={hr:#x}/{aborted}"));
                    hr >= 0 && aborted == 0
                };
            (v.unadvise)(op.raw(), cookie);
            done
        } else {
            false
        }
    };
    drop(op);
    drop(item);
    drop(sink.pin.take());
    if let Some(hr) = sink.deleted.get() {
        trace.push_str(&format!(" post_delete={hr:#x}"));
    }
    trace.push_str(&format!(" pre_deletes={}", sink.pre_deletes.get()));
    let report = ShellReport {
        trace,
        performed,
        approved: sink.approved.get(),
        deleted: sink.deleted.get(),
        recycled: sink.recycled.borrow_mut().take(),
        tracked: final_path(&sink.tracker)
            .ok()
            .zip(handle_info(&sink.tracker).ok().map(|(id, _)| id)),
    };
    if sink.refs.load(Ordering::SeqCst) != 1 {
        // The shell still references the sink: keep it alive instead of freeing it under it.
        Box::leak(sink);
    }
    Ok(report)
}

/// Recycle one verified file (spec 11.3).
///
/// 1. Pin: `DELETE` handle without `FILE_SHARE_DELETE`/`FILE_SHARE_WRITE`; identity (volume
///    serial, file id, size, mtime), no reparse point, final path equal to the planned path,
///    one hard link, NTFS, baseline protection — all checked on that handle.
/// 2. A share-all attribute handle is opened while pinned, so it is bound to the same object and
///    follows it through the move.
/// 3. `IFileOperation` recycle. The pin is released in `StartOperations`; `PreDeleteItem`
///    re-verifies through the tracking handle and the path and aborts on any mismatch.
/// 4. Post-check ([`verify_recycled`]): the item reported by `PostDeleteItem` is in
///    `$Recycle.Bin` on the same volume and carries the same identity, the tracking handle's final
///    path is that item, and the original path no longer names the object.
///
/// The shell only accepts paths/PIDLs and opens the file with `DELETE` itself, both in a
/// pre-flight step between `StartOperations` and `PreDeleteItem` and for the move, so the pin
/// cannot be held across the operation. A swap before `PreDeleteItem` is aborted; a swap between
/// it and the move is detected by step 4 and reported as a failure (the swapped file is then in
/// the recycle bin, not deleted). It cannot be prevented through the shell.
pub fn recycle_file(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| recycle_in_apartment(file))
            .join()
            .unwrap_or_else(|_| Err(shell_error("recycle worker panicked")))
    })
}
fn recycle_in_apartment(file: &FileTarget) -> Result<EffectOutcome, EffectError> {
    let _apartment = Apartment::enter()?;
    let pin = match pin_verified(file) {
        Ok(h) => h,
        Err(refusal) => return Ok(EffectOutcome::Refused(refusal)),
    };
    let tracker = match open_attributes(&file.path) {
        Ok(Some(h)) if handle_info(&h).is_ok_and(|(id, _)| same_object(&id, &file.identity)) => h,
        _ => return Ok(EffectOutcome::Refused(Refusal::TargetReplaced)),
    };
    recycle_bin_available(&file.path)?;
    let report = shell_recycle(file, pin, tracker)?;
    let evidence = RecycleEvidence {
        shell_succeeded: report.performed
            && report.approved
            && report.deleted.is_some_and(|hr| hr >= 0),
        recycled_item: report.recycled.map(|p| {
            let id = observe_file(Path::new(&p)).ok().flatten();
            (p, id)
        }),
        tracked: report.tracked,
        original_now: observe_file(&file.path).ok().flatten(),
    };
    match verify_recycled(&file.path, &file.identity, &evidence) {
        Ok(()) => Ok(EffectOutcome::Done(Some(
            "moved to the recycle bin; identity verified on the recycled item".into(),
        ))),
        Err(reason) if !report.approved => {
            Err(shell_error(&format!("{reason} ({})", report.trace)))
        }
        Err(reason) => Err(EffectError::new(
            Status::Error,
            &format!(
                "{reason} ({}); inspect the recycle bin manually",
                report.trace
            ),
        )),
    }
}
