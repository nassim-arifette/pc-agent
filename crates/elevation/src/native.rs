//! Native boundary for restricted local jobs and one fixed elevated executable.
use super::{
    cache::{self, CacheRoot},
    lifecycle::{retain_until_exit, run_supervised, ElevationCompletion, HostJob},
    protocol::{
        digest, launch_error, CacheCategory, ElevatedTask, ElevationError, ElevationRequest,
        ElevationResponse, WorkerArguments, MAX_BUDGET_MS, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    },
    worker::ElevatedProcess,
    ElevationLimit, ElevationStatus,
};
use pcagent_shared::unix_ms;
use pcagent_storage::CancelToken;
use std::{
    ffi::{c_void, OsStr, OsString},
    fs::File,
    io::{Read, Write},
    mem::{offset_of, size_of},
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Component, Path, PathBuf, Prefix},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, LocalFree, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        STILL_ACTIVE, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    },
    Globalization::GetUserDefaultUILanguage,
    Security::{
        AclSizeInformation,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        GetAce, GetAclInformation, GetSecurityDescriptorControl, GetTokenInformation, IsValidSid,
        TokenElevation, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
        SECURITY_ATTRIBUTES, SE_DACL_PROTECTED, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, GetDriveTypeW, GetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        READ_CONTROL,
    },
    System::{
        SystemInformation::{GetSystemDirectoryW, GetWindowsDirectoryW},
        Threading::{
            GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, TerminateProcess,
            WaitForSingleObject,
        },
        WindowsProgramming::DRIVE_FIXED,
    },
    UI::{
        Shell::{
            ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS,
            SHELLEXECUTEINFOW,
        },
        WindowsAndMessaging::{
            MessageBoxW, MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, SW_HIDE,
        },
    },
};

const REQUEST_FILE: &str = "request.json";
const RESPONSE_FILE: &str = "response.json";
const CANCEL_FILE: &str = "cancel";
const CLAIM_FILE: &str = "running";
const ENERGY_FILE: &str = "energy.csv";
const FIXED_FILES: [&str; 5] = [
    REQUEST_FILE,
    RESPONSE_FILE,
    CANCEL_FILE,
    CLAIM_FILE,
    ENERGY_FILE,
];

/// All ancestors remain open without delete sharing while any worker/launcher can use this job.
pub(crate) struct NativeJob {
    path: PathBuf,
    pins: Vec<File>,
    request_pin: Option<File>,
    sid: String,
    request_digest: String,
    cleanup: bool,
    completion: ElevationCompletion,
}
impl NativeJob {
    pub(super) fn owner_sid(&self) -> &str {
        &self.sid
    }

    /// The fixed E3 export is private and pinned against replacement while its writer runs.
    pub(super) fn energy_output(&self) -> Result<(PathBuf, File), ElevationError> {
        let path = self.path.join(ENERGY_FILE);
        let descriptor = private_descriptor(&self.sid)?;
        let attributes = descriptor.attributes();
        let wide = wide_path(&path)?;
        // SAFETY: the pinned private directory and descriptor outlive this CREATE_NEW call.
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let file = file_from_handle(handle)?;
        validate_regular(&file)?;
        validate_acl(&file, &self.sid)?;
        Ok((path, file))
    }

    /// Read only the already pinned export after its owned writer has exited.
    pub(super) fn energy_bytes(
        &self,
        file: &mut File,
        maximum: usize,
    ) -> Result<Vec<u8>, ElevationError> {
        validate_regular(file)?;
        validate_acl(file, &self.sid)?;
        read_bounded(file, maximum)
    }

    pub(crate) fn create(request: &ElevationRequest) -> Result<Self, ElevationError> {
        Self::create_in(request, &std::env::temp_dir())
    }

    fn create_in(request: &ElevationRequest, parent: &Path) -> Result<Self, ElevationError> {
        request.validate_at(unix_ms())?;
        let bytes = request.encode()?;
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(ElevationError::InvalidRequest);
        }
        let mut pins = pin_chain(parent)?;
        let sid = current_sid()?;
        let path = parent.join(format!("pcagent-{}", request.job_id));
        let descriptor = private_descriptor(&sid)?;
        let attributes = descriptor.attributes();
        let wide = wide_path(&path)?;
        // SAFETY: the null-terminated path and descriptor are live throughout CreateDirectoryW.
        if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let root = match open_directory(&path).and_then(|root| {
            validate_acl(&root, &sid)?;
            Ok(root)
        }) {
            Ok(root) => root,
            Err(error) => {
                // No file has been created and no final-directory pin was acquired. Remove
                // only the empty directory under the pinned ancestors before returning.
                let cleanup = std::sync::Mutex::new(PrivateCleanup {
                    path,
                    pins,
                    files_removed: true,
                    completion: ElevationCompletion::pending(),
                });
                wait_for_private_cleanup(&cleanup);
                return Err(error);
            }
        };
        pins.push(root);
        let job = Self {
            path,
            pins,
            request_pin: None,
            sid,
            request_digest: String::new(),
            cleanup: true,
            completion: ElevationCompletion::pending(),
        };
        job.finish_creation(|job| {
            let mut file = job.create_file(REQUEST_FILE)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            job.request_digest = digest(&bytes);
            job.request_pin = Some(job.open_file(REQUEST_FILE)?);
            Ok(())
        })
    }

    fn finish_creation(
        mut self,
        initialize: impl FnOnce(&mut Self) -> Result<(), ElevationError>,
    ) -> Result<Self, ElevationError> {
        match initialize(&mut self) {
            Ok(()) => Ok(self),
            Err(error) => {
                // The caller has no completion capability on Err. Do not return before
                // the private resources have actually gone, even when a local reader
                // briefly locks a partially written request against deletion.
                let completion = self.completion();
                drop(self);
                completion.wait_until_complete();
                Err(error)
            }
        }
    }

    pub(crate) fn completion(&self) -> ElevationCompletion {
        self.completion.clone()
    }

    fn open_worker(args: &WorkerArguments) -> Result<(Self, ElevationRequest), ElevationError> {
        let pins = pin_chain(&args.job_dir)?;
        let sid = current_sid()?;
        validate_acl(pins.last().ok_or(ElevationError::InsecureJob)?, &sid)?;
        let mut job = Self {
            path: args.job_dir.clone(),
            pins,
            request_pin: None,
            sid,
            request_digest: args.request_digest.clone(),
            cleanup: false,
            completion: ElevationCompletion::pending(),
        };
        let mut file = job.open_file(REQUEST_FILE)?;
        let bytes = read_bounded(&mut file, MAX_REQUEST_BYTES)?;
        if digest(&bytes) != args.request_digest {
            return Err(ElevationError::DigestMismatch);
        }
        let request = ElevationRequest::from_bytes(&bytes)?;
        if request.nonce != args.nonce
            || job.path.file_name() != Some(OsStr::new(&format!("pcagent-{}", request.job_id)))
        {
            return Err(ElevationError::InvalidRequest);
        }
        job.request_pin = Some(file);
        // CREATE_NEW prevents a second helper from accepting the same job.
        drop(job.create_file(CLAIM_FILE)?);
        Ok((job, request))
    }

    fn create_file(&self, name: &str) -> Result<File, ElevationError> {
        if !FIXED_FILES.contains(&name) {
            return Err(ElevationError::InvalidArguments);
        }
        let descriptor = private_descriptor(&self.sid)?;
        let attributes = descriptor.attributes();
        let path = wide_path(&self.path.join(name))?;
        // SAFETY: path/attributes stay live; CREATE_NEW refuses overwrite; the parent is pinned.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE | READ_CONTROL,
                FILE_SHARE_READ,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let file = file_from_handle(handle)?;
        validate_regular(&file)?;
        validate_acl(&file, &self.sid)?;
        Ok(file)
    }

    fn open_file(&self, name: &str) -> Result<File, ElevationError> {
        if !FIXED_FILES.contains(&name) {
            return Err(ElevationError::InvalidArguments);
        }
        let path = wide_path(&self.path.join(name))?;
        // SAFETY: the parent is pinned, final links are opened as links, and writes/rename are blocked.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | READ_CONTROL,
                FILE_SHARE_READ,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        let file = file_from_handle(handle)?;
        validate_regular(&file)?;
        validate_acl(&file, &self.sid)?;
        Ok(file)
    }

    fn cancelled(&self) -> Result<bool, ElevationError> {
        match std::fs::symlink_metadata(self.path.join(CANCEL_FILE)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
            Ok(_) => {
                let file = self.open_file(CANCEL_FILE)?;
                if file.metadata()?.len() != 0 {
                    return Err(ElevationError::InsecureJob);
                }
                Ok(true)
            }
        }
    }
    fn signal_cancel(&self) {
        // It is an empty, fixed-name file in this job only; process termination is independent.
        let _ = self.create_file(CANCEL_FILE);
    }
    fn response(&self, request: &ElevationRequest) -> Result<ElevationResponse, ElevationError> {
        let mut file = self.open_file(RESPONSE_FILE)?;
        ElevationResponse::from_bytes(&read_bounded(&mut file, MAX_RESPONSE_BYTES)?, request)
    }
}
impl Drop for NativeJob {
    fn drop(&mut self) {
        if !self.cleanup {
            return;
        }
        self.request_pin.take();
        let mut cleanup = PrivateCleanup {
            path: self.path.clone(),
            pins: std::mem::take(&mut self.pins),
            files_removed: false,
            completion: self.completion.clone(),
        };
        if cleanup.attempt() {
            return;
        }
        // A killed exporter or a local reader can briefly retain a deny-delete handle.
        // Keep the pins and admission closed until the actual private files disappear.
        let pending = Arc::new(std::sync::Mutex::new(cleanup));
        let worker = pending.clone();
        if std::thread::Builder::new()
            .name("pcagent-private-cleanup".into())
            .spawn(move || wait_for_private_cleanup(&worker))
            .is_err()
        {
            // Resource exhaustion must not announce cleanup or lose the pinned ownership.
            wait_for_private_cleanup(&pending);
        }
    }
}

struct PrivateCleanup {
    path: PathBuf,
    pins: Vec<File>,
    files_removed: bool,
    completion: ElevationCompletion,
}
impl PrivateCleanup {
    fn attempt(&mut self) -> bool {
        if !self.files_removed {
            // No recursive deletion: only the finite names in the still-pinned job directory.
            let mut removed = true;
            for name in FIXED_FILES {
                if let Err(error) = std::fs::remove_file(self.path.join(name)) {
                    removed &= error.kind() == std::io::ErrorKind::NotFound;
                }
            }
            if !removed {
                return false;
            }
            self.files_removed = true;
            // Release only the final directory pin; ancestors remain pinned during rmdir.
            self.pins.pop();
        }
        if let Err(error) = std::fs::remove_dir(&self.path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return false;
            }
        }
        self.pins.clear();
        self.completion.complete();
        true
    }
}
fn wait_for_private_cleanup(pending: &std::sync::Mutex<PrivateCleanup>) {
    loop {
        if pending.lock().unwrap_or_else(|p| p.into_inner()).attempt() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The user's Windows display language is French (primary language 0x0c, any region).
pub(crate) fn ui_language_is_french() -> bool {
    // SAFETY: GetUserDefaultUILanguage has no pointer arguments or safety preconditions.
    unsafe { GetUserDefaultUILanguage() & 0x03ff == 0x0c }
}

pub(crate) fn show_elevated_host_notice() {
    let french = ui_language_is_french();
    let (title, body) = super::elevated_host_notice(french);
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let body: Vec<u16> = body.encode_utf16().chain(std::iter::once(0)).collect();

    // SAFETY: The null parent is valid; both strings are live, null-terminated UTF-16 buffers.
    unsafe {
        MessageBoxW(
            null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONWARNING | MB_SETFOREGROUND | MB_TOPMOST,
        );
    }
}

pub(crate) fn is_elevated() -> Result<bool, ElevationError> {
    let token = current_token()?;
    let mut value: TOKEN_ELEVATION = Default::default();
    let mut length = 0;
    // SAFETY: the owned token is live; the output points to an initialized structure of this size.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenElevation,
            (&mut value as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if length as usize != size_of::<TOKEN_ELEVATION>() {
        return Err(ElevationError::InsecureJob);
    }
    Ok(value.TokenIsElevated != 0)
}

pub(crate) fn run_approved(
    job: Arc<NativeJob>,
    request: &ElevationRequest,
    cancel: &CancelToken,
) -> Result<ElevationResponse, ElevationError> {
    request.validate_at(unix_ms())?;
    if is_elevated()? {
        return Err(ElevationError::ApprovalRequired);
    }
    let remaining_ttl = Duration::from_millis(request.expires_at_unix_ms.saturating_sub(unix_ms()));
    let budget = request.budget().min(remaining_ttl);
    let job = Arc::new(SupervisedJob {
        native: job,
        request: request.clone(),
        launched: AtomicBool::new(false),
    });
    run_supervised(job, cancel, budget, |job| {
        let process = launch(&job.native, &job.request.nonce)?;
        job.launched.store(true, Ordering::Release);
        Ok(process)
    })
}

struct SupervisedJob {
    native: Arc<NativeJob>,
    request: ElevationRequest,
    launched: AtomicBool,
}
impl Drop for SupervisedJob {
    fn drop(&mut self) {
        if self.launched.load(Ordering::Acquire)
            && matches!(self.request.task, ElevatedTask::PerformanceTrace { .. })
        {
            // The supervisor retains this job until the owned helper has exited.
            // The current gate returns Unvalidated without any trace or ACL effect.
            let _ =
                super::trace_native::cleanup_for_request(&self.request, self.native.owner_sid());
        }
    }
}
impl HostJob for SupervisedJob {
    type Response = ElevationResponse;
    fn signal_cancel(&self) {
        self.native.signal_cancel();
    }
    fn response(&self) -> Result<ElevationResponse, ElevationError> {
        self.native.response(&self.request)
    }
}

struct Process {
    handle: OwnedHandle,
    exit_code: Option<u32>,
    exit_observed: bool,
}
impl Drop for Process {
    fn drop(&mut self) {
        // Also retains ownership if the supervisor unwinds. Process exists only on its worker.
        retain_until_exit(self);
    }
}
impl ElevatedProcess for Process {
    fn poll(&mut self) -> Result<Option<u32>, ElevationError> {
        if self.exit_code.is_some() {
            return Ok(self.exit_code);
        }
        let mut code = 0;
        // SAFETY: this is exactly the owned process handle returned by ShellExecuteExW.
        if unsafe { GetExitCodeProcess(self.handle.as_raw_handle(), &mut code) } == 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if code != STILL_ACTIVE as u32 {
            self.exit_code = Some(code);
            self.exit_observed = true;
        }
        Ok(self.exit_code)
    }
    fn has_exited(&mut self) -> Result<bool, ElevationError> {
        if self.exit_observed {
            return Ok(true);
        }
        // SAFETY: this is the owned launch handle; a zero timeout only queries its signalled state.
        match unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) } {
            WAIT_OBJECT_0 => {
                self.exit_observed = true;
                Ok(true)
            }
            WAIT_TIMEOUT => Ok(false),
            WAIT_FAILED => Err(std::io::Error::last_os_error().into()),
            _ => Err(ElevationError::InsecureJob),
        }
    }
    fn terminate(&mut self) -> Result<(), ElevationError> {
        if self.has_exited()? {
            return Ok(());
        }
        // SAFETY: only this launch's owned helper handle is terminated, never an arbitrary PID.
        let failure = if unsafe { TerminateProcess(self.handle.as_raw_handle(), 125) } == 0 {
            // Capture the native error before querying the handle can overwrite LastError.
            Some(std::io::Error::last_os_error())
        } else {
            None
        };
        if self.has_exited()? {
            return Ok(());
        }
        if let Some(error) = failure {
            return Err(error.into());
        }
        wait_for_process_exit(Duration::from_secs(2), || self.has_exited())
    }
}

/// TerminateProcess is asynchronous: completion requires an observed exit on the owned handle.
fn wait_for_process_exit(
    timeout: Duration,
    mut poll: impl FnMut() -> Result<bool, ElevationError>,
) -> Result<(), ElevationError> {
    let started = Instant::now();
    loop {
        if poll()? {
            return Ok(());
        }
        let remaining = timeout.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(ElevationError::TimedOut);
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
}

fn launch(job: &NativeJob, nonce: &str) -> Result<Process, ElevationError> {
    let executable = std::env::current_exe()?
        .parent()
        .ok_or(ElevationError::InvalidArguments)?
        .join(super::HELPER_FILE_NAME);
    let _pins = pin_chain(
        executable
            .parent()
            .ok_or(ElevationError::InvalidArguments)?,
    )?;
    let _executable_pin = open_executable(&executable)?;
    let file = wide_path(&executable)?;
    let directory = system_directory(false)?;
    let working_directory = wide_path(&directory)?;
    let parameters = quote_arguments(&job.path, nonce, &job.request_digest)?;
    let parameters = wide(OsStr::new(&parameters));
    let verb = wide(OsStr::new("runas"));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        lpVerb: verb.as_ptr(),
        lpFile: file.as_ptr(),
        lpParameters: parameters.as_ptr(),
        lpDirectory: working_directory.as_ptr(),
        nShow: SW_HIDE,
        ..Default::default()
    };
    // SAFETY: every referenced UTF-16 string is live; only the fixed sibling helper is launched.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        // SAFETY: GetLastError is read immediately after the failing native call.
        return Err(launch_error(unsafe { GetLastError() }));
    }
    if info.hProcess.is_null() {
        return Err(ElevationError::LaunchFailed(0));
    }
    // SAFETY: NOCLOSEPROCESS transfers ownership of this non-null process handle to the caller.
    Ok(Process {
        // SAFETY: NOCLOSEPROCESS transferred this non-null handle to the caller.
        handle: unsafe { OwnedHandle::from_raw_handle(info.hProcess) },
        exit_code: None,
        exit_observed: false,
    })
}

pub(crate) fn worker(arguments: WorkerArguments) -> Result<(), ElevationError> {
    // Cover setup and any blocked filesystem call too; this timer starts at worker entry.
    let _hard_deadline = HardDeadline::start()?;
    if !is_elevated()? {
        return Err(ElevationError::ApprovalRequired);
    }
    let (job, request) = NativeJob::open_worker(&arguments)?;
    let job = Arc::new(job);
    let disarmed = Arc::new(AtomicBool::new(false));
    let guard = Watchdog(Arc::clone(&disarmed));
    let watchdog_job = Arc::clone(&job);
    let budget = request.budget();
    std::thread::Builder::new()
        .name("pcagent-elevation-watchdog".into())
        .spawn(move || {
            let clock = Instant::now();
            while !disarmed.load(Ordering::Acquire) {
                if clock.elapsed() >= budget.min(Duration::from_millis(MAX_BUDGET_MS)) {
                    worker_exit(124);
                }
                match watchdog_job.cancelled() {
                    Ok(true) => worker_exit(125),
                    Err(_) => worker_exit(1),
                    Ok(false) => {}
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })?;
    let started_at = pcagent_shared::unix_ms();
    let started_clock = Instant::now();
    let mut response = ElevationResponse::empty(
        &request,
        ElevationStatus::Partial,
        ElevationLimit::MetadataSnapshot,
    );
    let remaining = || {
        request
            .budget()
            .checked_sub(started_clock.elapsed())
            .filter(|d| !d.is_zero())
            .ok_or(ElevationError::TimedOut)
    };
    match &request.task {
        ElevatedTask::SystemCaches => {
            let windows = system_directory(true)?;
            let resolution = super::system_native::delivery_cache(&windows);
            let mut roots = system_roots()?;
            roots.retain(|r| r.category != CacheCategory::DeliveryOptimization);
            if let Some(path) = &resolution.path {
                roots.push(CacheRoot {
                    category: CacheCategory::DeliveryOptimization,
                    path: path.clone(),
                });
            }
            response = cache::collect(&request, &roots, &mut || job.cancelled())?;
            if resolution.path.is_none() {
                response
                    .caches
                    .push(super::protocol::CacheSummary::unavailable(
                        CacheCategory::DeliveryOptimization,
                        resolution.status,
                        ElevationLimit::SourceUnavailable,
                    ));
            } else if resolution.source
                == super::system_native::DeliveryCacheSource::GroupPolicyFolder
            {
                if let Some(cache) = response
                    .caches
                    .iter_mut()
                    .find(|c| c.category == CacheCategory::DeliveryOptimization)
                {
                    cache
                        .limitations
                        .retain(|l| *l != ElevationLimit::DefaultCacheLocationOnly);
                    cache
                        .limitations
                        .push(ElevationLimit::PolicyCacheLocationUnconfirmed);
                }
            }
            if resolution.status == ElevationStatus::Partial {
                if let Some(cache) = response
                    .caches
                    .iter_mut()
                    .find(|c| c.category == CacheCategory::DeliveryOptimization)
                {
                    if cache.status == ElevationStatus::Completed {
                        cache.status = ElevationStatus::Partial;
                    }
                }
                if response.status == ElevationStatus::Completed {
                    response.status = ElevationStatus::Partial;
                }
            }
            // The resolver's owned directory handles pin the selected cache until scanning ends.
            drop(resolution);
        }
        ElevatedTask::MftInventory { volume } => {
            let result = super::mft_native::collect(*volume, remaining()?, &mut || {
                job.cancelled()
                    .map_err(|_| super::mft::CollectError::CancelCheck)
            });
            match result {
                Ok(value) => {
                    response.status = match value.end {
                        super::mft::CollectionEnd::Cancelled => ElevationStatus::Cancelled,
                        super::mft::CollectionEnd::TimedOut => ElevationStatus::TimedOut,
                        _ => ElevationStatus::Partial,
                    };
                    response.mft = Some(value);
                }
                Err(error) => {
                    response.status = match error {
                        super::mft::CollectError::Denied => ElevationStatus::Denied,
                        super::mft::CollectError::Unsupported => ElevationStatus::Unsupported,
                        _ => ElevationStatus::Error,
                    };
                    response.limitations.push(ElevationLimit::SourceUnavailable);
                }
            }
        }
        ElevatedTask::ShadowStorage => {
            let value =
                super::system_native::shadow_storage(remaining()?, &mut || job.cancelled())?;
            response.status = value.status;
            response.shadow_storage = Some(value);
        }
        ElevatedTask::DiskReliability => {
            let value =
                super::system_native::disk_reliability(remaining()?, &mut || job.cancelled())?;
            response.status = value.status;
            response.disk_reliability = Some(value);
        }
        ElevatedTask::ProcessImages { targets } => {
            let value = super::system_native::process_images(targets, remaining()?, &mut || {
                job.cancelled()
            })?;
            response.status = value.status;
            response.process_images = Some(value);
        }
        ElevatedTask::BatteryEnergy => {
            let value = super::energy_native::collect(&job, remaining()?, &mut || job.cancelled())?;
            response.status = value.status;
            response.battery_energy = Some(value);
        }
        ElevatedTask::PerformanceTrace { duration_seconds } => {
            let value = super::trace_native::collect(
                &request,
                job.owner_sid(),
                Duration::from_secs(u64::from(*duration_seconds)),
                &mut || job.cancelled(),
            )?;
            response.status = value.status;
            response.performance_trace = Some(value);
        }
    }
    response.window = pcagent_shared::Window {
        started_at_unix_ms: started_at,
        finished_at_unix_ms: pcagent_shared::unix_ms(),
        duration_ms: started_clock.elapsed().as_millis() as u64,
    };
    if response.window.duration_ms > request.budget_ms {
        return Err(ElevationError::TimedOut);
    }
    response.validate(&request)?;
    let bytes = serde_json::to_vec(&response).map_err(|_| ElevationError::InvalidResponse)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ElevationError::InvalidResponse);
    }
    let mut file = job.create_file(RESPONSE_FILE)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(guard);
    Ok(())
}
fn worker_exit(code: i32) -> ! {
    let _ = super::trace_native::abort_owned_session();
    std::process::exit(code)
}
struct HardDeadline {
    disarmed: Arc<AtomicBool>,
    thread: std::thread::Thread,
}
impl HardDeadline {
    fn start() -> Result<Self, ElevationError> {
        let disarmed = Arc::new(AtomicBool::new(false));
        let worker_flag = Arc::clone(&disarmed);
        let join = std::thread::Builder::new()
            .name("pcagent-elevation-deadline".into())
            .spawn(move || {
                let clock = Instant::now();
                loop {
                    if worker_flag.load(Ordering::Acquire) {
                        return;
                    }
                    let Some(remaining) =
                        Duration::from_millis(MAX_BUDGET_MS).checked_sub(clock.elapsed())
                    else {
                        worker_exit(124);
                    };
                    std::thread::park_timeout(remaining);
                }
            })?;
        Ok(Self {
            disarmed,
            thread: join.thread().clone(),
        })
    }
}
impl Drop for HardDeadline {
    fn drop(&mut self) {
        self.disarmed.store(true, Ordering::Release);
        self.thread.unpark();
    }
}
struct Watchdog(Arc<AtomicBool>);
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn system_roots() -> Result<Vec<CacheRoot>, ElevationError> {
    let windows = system_directory(true)?;
    let volume_root = windows
        .ancestors()
        .last()
        .ok_or(ElevationError::InvalidArguments)?;
    Ok(vec![
        CacheRoot {
            category: CacheCategory::PreviousWindows,
            path: volume_root.join("Windows.old"),
        },
        CacheRoot {
            category: CacheCategory::WindowsUpdateDownload,
            path: windows.join("SoftwareDistribution").join("Download"),
        },
        CacheRoot {
            category: CacheCategory::DeliveryOptimization,
            path: windows
                .join("ServiceProfiles")
                .join("NetworkService")
                .join("AppData")
                .join("Local")
                .join("Microsoft")
                .join("Windows")
                .join("DeliveryOptimization")
                .join("Cache"),
        },
    ])
}
fn system_directory(windows: bool) -> Result<PathBuf, ElevationError> {
    let mut buffer = vec![0_u16; 32768];
    // SAFETY: the buffer has exactly the supplied capacity; no environment path is accepted.
    let length = unsafe {
        if windows {
            GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        } else {
            GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32)
        }
    } as usize;
    if length == 0 || length >= buffer.len() {
        return Err(ElevationError::InsecureJob);
    }
    let path = PathBuf::from(
        String::from_utf16(&buffer[..length]).map_err(|_| ElevationError::InsecureJob)?,
    );
    validate_path(&path)?;
    Ok(path)
}

fn quote_arguments(path: &Path, nonce: &str, hash: &str) -> Result<String, ElevationError> {
    validate_path(path)?;
    let args = WorkerArguments::parse([
        OsString::from("--job-dir"),
        path.as_os_str().to_owned(),
        OsString::from("--nonce"),
        nonce.into(),
        OsString::from("--request-digest"),
        hash.into(),
    ])?;
    let path = args
        .job_dir
        .to_str()
        .ok_or(ElevationError::InvalidArguments)?;
    Ok(format!(
        "--job-dir \"{path}\" --nonce {} --request-digest {}",
        args.nonce, args.request_digest
    ))
}

fn validate_path(path: &Path) -> Result<(), ElevationError> {
    let text = path.to_str().ok_or(ElevationError::InvalidArguments)?;
    let bytes = text.as_bytes();
    if bytes.len() < 3
        || bytes.len() > 4000
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || bytes[2] != b'\\'
        || text
            .chars()
            .any(|c| c.is_control() || matches!(c, '"' | '/' | '*' | '?' | '<' | '>' | '|'))
    {
        return Err(ElevationError::InvalidArguments);
    }
    for part in text[3..].split('\\') {
        if part == "." || part == ".." || part.contains(':') || part.ends_with(['.', ' ']) {
            return Err(ElevationError::InvalidArguments);
        }
        let stem = part.split('.').next().unwrap_or("").to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9'))
        {
            return Err(ElevationError::InvalidArguments);
        }
    }
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_)))
        || components.next() != Some(Component::RootDir)
        || components.any(|p| !matches!(p, Component::Normal(_)))
    {
        return Err(ElevationError::InvalidArguments);
    }
    Ok(())
}
fn wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}
fn wide_path(path: &Path) -> Result<Vec<u16>, ElevationError> {
    validate_path(path)?;
    Ok(wide(path.as_os_str()))
}
fn file_from_handle(handle: HANDLE) -> Result<File, ElevationError> {
    if handle == INVALID_HANDLE_VALUE || handle.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: CreateFileW returned this unique valid handle; File now owns it.
    Ok(unsafe { File::from_raw_handle(handle) })
}
fn info(file: &File) -> Result<BY_HANDLE_FILE_INFORMATION, ElevationError> {
    let mut value = Default::default();
    // SAFETY: the live file handle and correctly sized output are valid for this synchronous call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut value) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(value)
}
fn validate_regular(file: &File) -> Result<(), ElevationError> {
    let metadata = info(file)?;
    if metadata.dwFileAttributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_DIRECTORY) != 0
        || metadata.nNumberOfLinks != 1
    {
        return Err(ElevationError::InsecureJob);
    }
    Ok(())
}
fn open_directory(path: &Path) -> Result<File, ElevationError> {
    let wide = wide_path(path)?;
    // SAFETY: only this validated path is opened; final links are not followed and rename is blocked.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    let file = file_from_handle(handle)?;
    let metadata = info(&file)?;
    if metadata.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
    {
        return Err(ElevationError::InsecureJob);
    }
    Ok(file)
}
fn pin_chain(path: &Path) -> Result<Vec<File>, ElevationError> {
    validate_path(path)?;
    let root = path
        .ancestors()
        .last()
        .ok_or(ElevationError::InvalidArguments)?;
    let root = wide_path(root)?;
    // SAFETY: the root is a validated, null-terminated local drive path; network drives are refused.
    if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_FIXED {
        return Err(ElevationError::InsecureJob);
    }
    let mut chain: Vec<_> = path.ancestors().collect();
    chain.reverse();
    chain.into_iter().map(open_directory).collect()
}
fn open_executable(path: &Path) -> Result<File, ElevationError> {
    let path = wide_path(path)?;
    // SAFETY: the file and its ancestors are pinned; no write/delete sharing permits replacement.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT,
            null_mut(),
        )
    };
    let file = file_from_handle(handle)?;
    validate_regular(&file)?;
    Ok(file)
}
fn read_bounded(file: &mut File, maximum: usize) -> Result<Vec<u8>, ElevationError> {
    if file.metadata()?.len() > maximum as u64 {
        return Err(ElevationError::InvalidResponse);
    }
    let mut bytes = Vec::new();
    file.take(maximum as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(ElevationError::InvalidResponse);
    }
    Ok(bytes)
}

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: only LocalAlloc-owned output from documented security APIs is freed, once.
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn current_token() -> Result<OwnedHandle, ElevationError> {
    let mut handle = null_mut();
    // SAFETY: the pseudo current-process handle is valid; the output receives one owned token handle.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: OpenProcessToken succeeded and transferred ownership of this token handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}
fn current_sid() -> Result<String, ElevationError> {
    let token = current_token()?;
    let mut length = 0;
    // SAFETY: a zero-length query writes only the required buffer length.
    unsafe {
        GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length);
    }
    if length as usize > 64 * 1024 || (length as usize) < size_of::<TOKEN_USER>() {
        return Err(ElevationError::InsecureJob);
    }
    let mut buffer = vec![0_u64; (length as usize).div_ceil(size_of::<u64>())];
    let mut returned = 0;
    // SAFETY: the u64 buffer is aligned for TOKEN_USER and holds the queried number of bytes.
    if unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            buffer.as_mut_ptr().cast(),
            length,
            &mut returned,
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    if returned > length || (returned as usize) < size_of::<TOKEN_USER>() {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: a successful query supplied TOKEN_USER in this live aligned buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    sid_string(user.User.Sid)
}
fn sid_string(sid: PSID) -> Result<String, ElevationError> {
    if sid.is_null() {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: caller supplies a SID returned by the token/security APIs; validity is checked first.
    if unsafe { IsValidSid(sid) } == 0 {
        return Err(ElevationError::InsecureJob);
    }
    let mut output = null_mut();
    // SAFETY: the valid SID is live and the output receives a LocalAlloc-owned UTF-16 string.
    if unsafe { ConvertSidToStringSidW(sid, &mut output) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let allocation = LocalAllocation(output.cast());
    let mut length = 0;
    // A Windows SID string is bounded by the maximum SID subauthority count.
    while length < 256 {
        // SAFETY: the API returned a null-terminated SID string (maximum fewer than 256 code units).
        if unsafe { *output.add(length) } == 0 {
            break;
        }
        length += 1;
    }
    if length == 256 {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: length stops before the API-owned terminator; allocation is still live.
    let text = String::from_utf16(unsafe { std::slice::from_raw_parts(output, length) })
        .map_err(|_| ElevationError::InsecureJob)?;
    drop(allocation);
    Ok(text)
}
struct Descriptor(LocalAllocation);
impl Descriptor {
    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0 .0,
            bInheritHandle: 0,
        }
    }
}
fn private_descriptor(sid: &str) -> Result<Descriptor, ElevationError> {
    let text = wide(OsStr::new(&format!(
        "O:{sid}G:{sid}D:P(A;OICI;FA;;;{sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"
    )));
    let mut output: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the SDDL comes only from the current token SID and fixed ACL entries.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            text.as_ptr(),
            SDDL_REVISION_1,
            &mut output,
            null_mut(),
        )
    } == 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Descriptor(LocalAllocation(output)))
}
fn validate_acl(file: &File, expected_owner: &str) -> Result<(), ElevationError> {
    let mut owner: PSID = null_mut();
    let mut dacl: *mut ACL = null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: the file is pinned and outputs refer into the returned LocalAlloc security descriptor.
    let code = unsafe {
        GetSecurityInfo(
            file.as_raw_handle(),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            &mut dacl,
            null_mut(),
            &mut descriptor,
        )
    };
    if code != 0 || descriptor.is_null() {
        return Err(ElevationError::InsecureJob);
    }
    let _allocation = LocalAllocation(descriptor);
    if dacl.is_null() || sid_string(owner)? != expected_owner {
        return Err(ElevationError::InsecureJob);
    }
    let mut control = 0;
    let mut revision = 0;
    let mut acl_info: ACL_SIZE_INFORMATION = Default::default();
    // SAFETY: both pointers refer into the live API-owned descriptor; outputs are correctly sized.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control & SE_DACL_PROTECTED == 0
        // SAFETY: the ACL is live and the output is ACL_SIZE_INFORMATION with its exact size.
        || unsafe { GetAclInformation(dacl, (&mut acl_info as *mut ACL_SIZE_INFORMATION).cast(),
            size_of::<ACL_SIZE_INFORMATION>() as u32, AclSizeInformation) } == 0
        || acl_info.AceCount != 3
    {
        return Err(ElevationError::InsecureJob);
    }
    let allowed = [expected_owner, "S-1-5-18", "S-1-5-32-544"];
    let mut seen = [false; 3];
    for index in 0..acl_info.AceCount {
        let mut pointer = null_mut();
        // SAFETY: index is within the count supplied by GetAclInformation for this live ACL.
        if unsafe { GetAce(dacl, index, &mut pointer) } == 0 || pointer.is_null() {
            return Err(ElevationError::InsecureJob);
        }
        // SAFETY: every ACE contains its fixed header; GetAce returned a valid ACE pointer.
        let header = unsafe { &*pointer.cast::<ACE_HEADER>() };
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        if header.AceType != 0
            || (header.AceSize as usize) < sid_offset + 8
            || header.AceFlags & 0x08 != 0
        {
            return Err(ElevationError::InsecureJob);
        }
        // SAFETY: the type and minimum size above establish the allowed-ACE layout.
        let ace = unsafe { &*pointer.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Mask != FILE_ALL_ACCESS {
            return Err(ElevationError::InsecureJob);
        }
        // SAFETY: the ACE came from GetAce; its declared size covers this bounded SID header.
        let sid_bytes = unsafe {
            std::slice::from_raw_parts(
                pointer.cast::<u8>().add(sid_offset),
                header.AceSize as usize - sid_offset,
            )
        };
        if sid_bytes[0] != 1 || sid_bytes[1] > 15 || 8 + 4 * sid_bytes[1] as usize > sid_bytes.len()
        {
            return Err(ElevationError::InsecureJob);
        }
        let sid = sid_bytes.as_ptr().cast_mut().cast();
        let sid = sid_string(sid)?;
        let Some(position) = allowed.iter().position(|allowed| *allowed == sid) else {
            return Err(ElevationError::InsecureJob);
        };
        if seen[position] {
            return Err(ElevationError::InsecureJob);
        }
        seen[position] = true;
    }
    if !seen.into_iter().all(|value| value) {
        return Err(ElevationError::InsecureJob);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn exit_wait_requires_observed_completion_and_reports_timeout_or_poll_failure() {
        let mut polls = 0;
        assert_eq!(
            wait_for_process_exit(Duration::from_secs(1), || {
                polls += 1;
                Ok(polls == 2)
            }),
            Ok(())
        );
        assert_eq!(polls, 2);
        assert_eq!(
            wait_for_process_exit(Duration::ZERO, || Ok(false)),
            Err(ElevationError::TimedOut)
        );
        assert_eq!(wait_for_process_exit(Duration::ZERO, || Ok(true)), Ok(()));
        assert_eq!(
            wait_for_process_exit(Duration::from_secs(1), || Err(
                ElevationError::LaunchFailed(5)
            )),
            Err(ElevationError::LaunchFailed(5))
        );
    }
    use super::*;
    use pcagent_shared::new_id;

    #[test]
    fn hostile_paths_cannot_be_quoted_or_opened_as_jobs() {
        for path in [
            r"C:\job\..\Windows",
            r"\\server\share\job",
            r"\\?\C:\job",
            r"C:\job:stream",
            "C:\\job\" --task x",
            r"C:\NUL",
            r"C:\job.",
        ] {
            assert!(validate_path(Path::new(path)).is_err());
        }
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let job_path =
            PathBuf::from(r"D:\fixture directory").join(format!("pcagent-{}", request.job_id));
        let text = quote_arguments(&job_path, &request.nonce, &"0".repeat(64)).unwrap();
        assert!(text.starts_with("--job-dir \"D:\\fixture directory\\pcagent-elevjob-"));
        assert!(
            quote_arguments(Path::new(r"D:\fixture"), "nonce\" cmd.exe", &"0".repeat(64)).is_err()
        );
    }

    #[test]
    fn private_fixture_job_rejects_replay_and_truncated_response_without_uac() {
        let parent = std::env::temp_dir().join(new_id("elevation-protocol-fixture"));
        std::fs::create_dir(&parent).unwrap();
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let job = NativeJob::create_in(&request, &parent).unwrap();
        let args = WorkerArguments {
            job_dir: job.path.clone(),
            nonce: request.nonce.clone(),
            request_digest: job.request_digest.clone(),
        };
        let mut wrong_digest = args.clone();
        wrong_digest.request_digest = "0".repeat(64);
        assert!(matches!(
            NativeJob::open_worker(&wrong_digest),
            Err(ElevationError::DigestMismatch)
        ));
        // The pinned request cannot be silently replaced after local approval.
        assert!(std::fs::write(job.path.join(REQUEST_FILE), b"{}").is_err());
        let (worker_job, _) = NativeJob::open_worker(&args).unwrap();
        assert!(NativeJob::open_worker(&args).is_err());
        let mut response = worker_job.create_file(RESPONSE_FILE).unwrap();
        response.write_all(b"{\"protocol_version\":1").unwrap();
        drop(response);
        assert!(job.response(&request).is_err());
        drop(worker_job);
        drop(job);
        std::fs::remove_dir(&parent).unwrap();
    }

    #[test]
    fn private_export_cleanup_stays_pending_until_the_pinned_file_is_released() {
        let parent = std::env::temp_dir().join(new_id("elevation-cleanup-fixture"));
        std::fs::create_dir(&parent).unwrap();
        let request =
            ElevationRequest::new(ElevatedTask::BatteryEnergy, Duration::from_secs(10)).unwrap();
        let job = NativeJob::create_in(&request, &parent).unwrap();
        let path = job.path.clone();
        let completion = job.completion();
        let (export, pin) = job.energy_output().unwrap();
        std::fs::write(&export, b"synthetic fixture only").unwrap();
        drop(job);
        assert!(!completion.is_complete());
        assert!(export.exists());
        drop(pin);
        assert!(completion.wait(Duration::from_secs(3)));
        assert!(!export.exists());
        assert!(!path.exists());
        std::fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn failed_preparation_waits_for_private_cleanup_before_returning_an_error() {
        let parent = std::env::temp_dir().join(new_id("elevation-create-error-fixture"));
        std::fs::create_dir(&parent).unwrap();
        let request =
            ElevationRequest::new(ElevatedTask::BatteryEnergy, Duration::from_secs(10)).unwrap();
        let job = NativeJob::create_in(&request, &parent).unwrap();
        let path = job.path.clone();
        let (export, pin) = job.energy_output().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = job.finish_creation(|_| Err(ElevationError::InvalidRequest));
            sender
                .send(matches!(result, Err(ElevationError::InvalidRequest)))
                .unwrap();
        });
        assert!(receiver.recv_timeout(Duration::from_millis(50)).is_err());
        assert!(export.exists());
        drop(pin);
        assert!(receiver.recv_timeout(Duration::from_secs(3)).unwrap());
        worker.join().unwrap();
        assert!(!path.exists());
        std::fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn cleanup_never_deletes_new_files_after_releasing_the_job_directory_pin() {
        let parent = std::env::temp_dir().join(new_id("elevation-cleanup-fixture"));
        std::fs::create_dir(&parent).unwrap();
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let job = NativeJob::create_in(&request, &parent).unwrap();
        let path = job.path.clone();
        let completion = job.completion();
        let unknown = path.join("unknown-fixture.txt");
        std::fs::write(&unknown, b"synthetic unknown file").unwrap();
        drop(job);
        assert!(!completion.is_complete());
        let replacement = path.join(ENERGY_FILE);
        std::fs::write(&replacement, b"synthetic replacement after pin release").unwrap();
        assert!(!completion.wait(Duration::from_millis(200)));
        assert!(unknown.exists());
        assert!(replacement.exists());
        // Remove only the two files this fixture created, never recurse through a path.
        std::fs::remove_file(unknown).unwrap();
        std::fs::remove_file(replacement).unwrap();
        assert!(completion.wait(Duration::from_secs(3)));
        assert!(!path.exists());
        std::fs::remove_dir(parent).unwrap();
    }

    #[test]
    fn inherited_public_acl_is_rejected_on_a_new_fixture() {
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let parent = std::env::temp_dir().join(new_id("elevation-acl-fixture"));
        let root = parent.join(format!("pcagent-{}", request.job_id));
        std::fs::create_dir(&parent).unwrap();
        std::fs::create_dir(&root).unwrap();
        let arguments = WorkerArguments {
            job_dir: root.clone(),
            nonce: request.nonce.clone(),
            request_digest: "0".repeat(64),
        };
        assert!(matches!(
            NativeJob::open_worker(&arguments),
            Err(ElevationError::InsecureJob)
        ));
        std::fs::remove_dir(&root).unwrap();
        std::fs::remove_dir(&parent).unwrap();
    }
}
