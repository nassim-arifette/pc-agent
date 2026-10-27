//! Separate, bounded owner of a private logger. Production collection remains gated.
use super::{
    native::is_elevated,
    protocol::{ElevatedTask, ElevationError, ElevationRequest, MAX_REQUEST_BYTES},
    trace_native::{stop_session, Identity, OwnedSession, Properties},
};
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    mem::size_of,
    os::windows::io::AsRawHandle,
    os::windows::process::CommandExt,
    process::{Child, ChildStderr, ChildStdin, Command, Stdio},
    ptr::null_mut,
    time::{Duration, Instant},
};
use windows_sys::{
    core::GUID,
    Win32::{
        Foundation::{
            CloseHandle, GetLastError, LocalFree, ERROR_ALREADY_EXISTS, ERROR_BROKEN_PIPE,
            ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, ERROR_WMI_INSTANCE_NOT_FOUND, HANDLE,
            WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::{
            Authorization::ConvertStringSidToSidW, GetTokenInformation, IsValidSid, TokenElevation,
            TOKEN_ELEVATION, TOKEN_QUERY,
        },
        Storage::FileSystem::{GetFileType, ReadFile, FILE_TYPE_PIPE},
        System::{
            Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE},
            Diagnostics::Etw::{
                ControlTraceW, EventAccessControl, EventAccessRemove, EventSecurityAddDACL,
                CONTROLTRACE_HANDLE, EVENT_TRACE_CONTROL_QUERY, TRACELOG_CREATE_REALTIME,
                TRACELOG_GUID_ENABLE, WMIGUID_QUERY,
            },
            JobObjects::IsProcessInJob,
            Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe},
            Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_BINARY},
            Threading::{
                GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
                WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB, CREATE_NO_WINDOW,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            },
        },
    },
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(10);
// QUERY + CREATE_REALTIME + GUID_ENABLE. No group or provider permission is touched.
const PARENT_RIGHTS: u32 = WMIGUID_QUERY | TRACELOG_CREATE_REALTIME | TRACELOG_GUID_ENABLE;
const MAX_MESSAGE: usize = MAX_REQUEST_BYTES + 512;

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: exclusively owned non-null handle obtained from OpenProcess/OpenProcessToken.
        unsafe { CloseHandle(self.0) };
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    request: ElevationRequest,
    owner_sid: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Ready {},
    Finished { lost: Option<(u64, u64)> },
}

fn last_error() -> ElevationError {
    // SAFETY: thread-local error code; no pointers.
    ElevationError::LaunchFailed(unsafe { GetLastError() })
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn image(handle: HANDLE) -> Result<Vec<u16>, ElevationError> {
    let mut path = vec![0u16; 32768];
    let mut length = path.len() as u32;
    // SAFETY: live process handle, initialized output allocation and its exact capacity.
    if unsafe { QueryFullProcessImageNameW(handle, 0, path.as_mut_ptr(), &mut length) } == 0 {
        return Err(last_error());
    }
    path.truncate(length as usize);
    Ok(path)
}

fn privileged(handle: HANDLE) -> Result<bool, ElevationError> {
    let mut token = null_mut();
    // SAFETY: borrowed live process handle; output receives a newly owned query-only token.
    if unsafe { OpenProcessToken(handle, TOKEN_QUERY, &mut token) } == 0 {
        return Err(last_error());
    }
    let token = Handle(token);
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0;
    // SAFETY: exact initialized TOKEN_ELEVATION output and byte length; token stays alive.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    } == 0
        || length as usize != size_of::<TOKEN_ELEVATION>()
    {
        return Err(ElevationError::InsecureJob);
    }
    Ok(elevation.TokenIsElevated != 0)
}

/// Only an already privileged instance of this exact helper can supply the inherited pipe.
/// NativeJob authentication happens in that worker before Client::start is reachable.
fn controller(input: HANDLE) -> Result<Handle, ElevationError> {
    let mut pid = 0;
    // SAFETY: borrowed stdio handle; only its kernel-reported type/server identity is inspected.
    if unsafe { GetFileType(input) } != FILE_TYPE_PIPE {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: pipe type was verified above and PID output is initialized.
    if unsafe { GetNamedPipeServerProcessId(input, &mut pid) } == 0 {
        return Err(last_error());
    }
    if pid == 0 {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: kernel-reported process ID; query/synchronize only, no modification rights.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return Err(last_error());
    }
    let handle = Handle(handle);
    // SAFETY: pseudo-handle is only borrowed, never closed.
    let current = unsafe { GetCurrentProcess() };
    let privileged = privileged(handle.0)?;
    let same_image = image(handle.0)? == image(current)?;
    let alive = alive(handle.0)?;
    if !privileged || !same_image || !alive {
        #[cfg(test)]
        eprintln!("guardian controller validation privileged={privileged} same_image={same_image} alive={alive}");
        return Err(ElevationError::InsecureJob);
    }
    Ok(handle)
}

fn alive(process: HANDLE) -> Result<bool, ElevationError> {
    // SAFETY: live process handle retained for lifetime identity; zero wait never blocks.
    match unsafe { WaitForSingleObject(process, 0) } {
        WAIT_TIMEOUT => Ok(true),
        WAIT_OBJECT_0 => Ok(false),
        _ => Err(last_error()),
    }
}

fn available(pipe: HANDLE) -> Result<Option<u32>, ElevationError> {
    let mut bytes = 0;
    // SAFETY: live pipe, no payload requested, initialized byte-count output.
    if unsafe { PeekNamedPipe(pipe, null_mut(), 0, null_mut(), &mut bytes, null_mut()) } == 0 {
        // SAFETY: last error belongs to the immediately preceding PeekNamedPipe call.
        if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
            return Ok(None);
        }
        return Err(last_error());
    }
    Ok(Some(bytes))
}

fn read_exact(pipe: HANDLE, buffer: &mut [u8], deadline: Instant) -> Result<(), ElevationError> {
    let mut offset = 0;
    while offset < buffer.len() {
        if Instant::now() >= deadline {
            return Err(ElevationError::TimedOut);
        }
        match available(pipe)? {
            None => return Err(ElevationError::InsecureJob),
            Some(0) => std::thread::sleep(POLL),
            Some(bytes) => {
                let length = (buffer.len() - offset).min(bytes as usize);
                let mut read = 0;
                // SAFETY: read at most PeekNamedPipe's available bytes into a checked mutable slice.
                if unsafe {
                    ReadFile(
                        pipe,
                        buffer[offset..].as_mut_ptr(),
                        length as u32,
                        &mut read,
                        null_mut(),
                    )
                } == 0
                    || read == 0
                {
                    return Err(ElevationError::InsecureJob);
                }
                offset += read as usize;
            }
        }
    }
    Ok(())
}

fn receive<T: serde::de::DeserializeOwned>(
    pipe: HANDLE,
    deadline: Instant,
) -> Result<T, ElevationError> {
    let mut header = [0; 4];
    read_exact(pipe, &mut header, deadline)?;
    let length = u32::from_le_bytes(header) as usize;
    if length == 0 || length > MAX_MESSAGE {
        return Err(ElevationError::InvalidRequest);
    }
    let mut bytes = vec![0; length];
    read_exact(pipe, &mut bytes, deadline)?;
    serde_json::from_slice(&bytes).map_err(|_| ElevationError::InvalidRequest)
}

fn send(output: &mut impl Write, message: &impl Serialize) -> Result<(), ElevationError> {
    let bytes = serde_json::to_vec(message).map_err(|_| ElevationError::InvalidRequest)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(ElevationError::InvalidRequest);
    }
    output
        .write_all(&(bytes.len() as u32).to_le_bytes())
        .and_then(|()| output.write_all(&bytes))
        .and_then(|()| output.flush())
        .map_err(|_| ElevationError::InsecureJob)
}

fn acl_absent(guid: &GUID) -> Result<bool, ElevationError> {
    let value = wide(&format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7],
    ));
    let key = wide(r"SYSTEM\CurrentControlSet\Control\WMI\Security");
    let mut size = 0;
    // SAFETY: fixed read-only registry key, single secret-derived GUID value, no data or writes.
    match unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            null_mut(),
            null_mut(),
            &mut size,
        )
    } {
        ERROR_FILE_NOT_FOUND => Ok(true),
        ERROR_SUCCESS => Ok(false),
        code => Err(ElevationError::LaunchFailed(code)),
    }
}

struct Lease {
    session: Option<OwnedSession>,
    guid: GUID,
    acl_changed: bool,
}
impl Lease {
    fn start(start: &Start) -> Result<Self, ElevationError> {
        let identity = Identity::request(&start.request)?;
        let mut properties = Properties::new(&identity);
        // SAFETY: only the approved request's name is queried, before any permission is changed.
        let query = unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                identity.name.as_ptr(),
                properties.get_mut(),
                EVENT_TRACE_CONTROL_QUERY,
            )
        };
        if query != ERROR_WMI_INSTANCE_NOT_FOUND {
            return Err(ElevationError::LaunchFailed(if query == ERROR_SUCCESS {
                ERROR_ALREADY_EXISTS
            } else {
                query
            }));
        }
        if !acl_absent(&identity.guid)? {
            return Err(ElevationError::InsecureJob);
        }
        let sid_text = &start.owner_sid;
        if sid_text.len() > 184
            || !sid_text.starts_with("S-1-5-21-")
            || !sid_text
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b'S')
        {
            return Err(ElevationError::InvalidRequest);
        }
        let sid_text = wide(sid_text);
        let mut sid = null_mut();
        // SAFETY: bounded, NUL-terminated SID syntax; output is LocalAlloc-owned.
        if unsafe { ConvertStringSidToSidW(sid_text.as_ptr(), &mut sid) } == 0 {
            return Err(ElevationError::InvalidRequest);
        }
        struct Sid(*mut std::ffi::c_void);
        impl Drop for Sid {
            fn drop(&mut self) {
                // SAFETY: owned ConvertStringSidToSidW allocation, freed exactly once.
                unsafe { LocalFree(self.0) };
            }
        }
        let sid = Sid(sid);
        // SAFETY: valid SID allocation retained through the grant.
        if unsafe { IsValidSid(sid.0) } == 0 {
            return Err(ElevationError::InvalidRequest);
        }
        // Install the session ACL before StartTrace so its security captures the requester rights.
        // Arm rollback before either resource exists. A StartTrace collision owns no session.
        let mut lease = Self {
            guid: identity.guid,
            session: None,
            acl_changed: true,
        };
        // SAFETY: only the approved private session GUID, bounded requester SID.
        // Marked for rollback before the call so even a partial failure removes our new value.
        let status = unsafe {
            EventAccessControl(
                &lease.guid,
                EventSecurityAddDACL as u32,
                sid.0,
                PARENT_RIGHTS,
                true,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(ElevationError::LaunchFailed(status));
        }
        if acl_absent(&lease.guid)? {
            #[cfg(test)]
            eprintln!("guardian ACL absent after successful grant");
            return Err(ElevationError::InsecureJob);
        }
        lease.session = Some(OwnedSession::start(&start.request)?);
        Ok(lease)
    }

    fn finish(&mut self) -> Result<Option<(u64, u64)>, ElevationError> {
        let lost = match self.session.as_ref() {
            Some(session) => stop_session(session)?,
            None => None,
        };
        self.session.take();
        if self.acl_changed {
            // SAFETY: only our private GUID, proved absent before creation; no global/provider ACL.
            let status = unsafe { EventAccessRemove(&self.guid) };
            if status != ERROR_SUCCESS {
                return Err(ElevationError::LaunchFailed(status));
            }
            if !acl_absent(&self.guid)? {
                return Err(ElevationError::InsecureJob);
            }
            self.acl_changed = false;
        }
        Ok(lost)
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        for _ in 0..3 {
            if self.finish().is_ok() {
                break;
            }
            std::thread::sleep(POLL);
        }
    }
}

pub(super) fn worker() -> Result<(), ElevationError> {
    if !is_elevated()? {
        return Err(ElevationError::InsecureJob);
    }
    let mut in_job = 0;
    // SAFETY: current-process pseudo-handle, null selects any job, initialized BOOL output.
    if unsafe { IsProcessInJob(GetCurrentProcess(), null_mut(), &mut in_job) } == 0 {
        return Err(last_error());
    }
    if in_job != 0 {
        return Err(ElevationError::InsecureJob);
    }
    // SAFETY: process-owned borrowed stdio handles; neither is closed here.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    // SAFETY: only inspect the borrowed output handle type.
    if unsafe { GetFileType(output) } != FILE_TYPE_PIPE {
        return Err(ElevationError::InsecureJob);
    }
    let controller = controller(input)?;
    let start: Start = receive(input, Instant::now() + STARTUP_TIMEOUT)?;
    start.request.validate_at(pcagent_shared::unix_ms())?;
    let ElevatedTask::PerformanceTrace { duration_seconds } = start.request.task else {
        return Err(ElevationError::InvalidRequest);
    };
    if !alive(controller.0)? {
        return Err(ElevationError::InsecureJob);
    }
    let deadline = Instant::now() + Duration::from_secs(u64::from(duration_seconds) + 3);
    // This process owns StartTrace itself: no controller/proof-write crash gap.
    let mut lease = Lease::start(&start)?;
    let mut output = std::io::stderr().lock();
    send(&mut output, &Reply::Ready {})?;
    while Instant::now() < deadline && alive(controller.0)? {
        match available(input)? {
            Some(0) => std::thread::sleep(POLL),
            // EOF is cancellation; any post-start byte requests cleanup, never another operation.
            None | Some(_) => break,
        }
    }
    let lost = lease.finish()?;
    send(&mut output, &Reply::Finished { lost })
}

pub(super) struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStderr,
}
impl Client {
    pub(super) fn start(
        request: &ElevationRequest,
        owner_sid: &str,
    ) -> Result<Self, ElevationError> {
        let mut command =
            Command::new(std::env::current_exe().map_err(|_| ElevationError::InsecureJob)?);
        #[cfg(not(test))]
        command.arg("--trace-guardian");
        #[cfg(test)]
        command.args([
            "--ignored",
            "--exact",
            "trace_guardian::tests::guardian_entry",
            "--nocapture",
        ]);
        let mut child = command
            .creation_flags(CREATE_BREAKAWAY_FROM_JOB | CREATE_NO_WINDOW)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|error| {
                ElevationError::LaunchFailed(error.raw_os_error().unwrap_or(0) as u32)
            })?;
        let input = child.stdin.take().ok_or(ElevationError::InsecureJob)?;
        let output = child.stderr.take().ok_or(ElevationError::InsecureJob)?;
        let mut client = Self {
            child,
            input: Some(input),
            output,
        };
        send(
            client.input.as_mut().unwrap(),
            &Start {
                request: request.clone(),
                owner_sid: owner_sid.to_owned(),
            },
        )?;
        let reply = receive(
            client.output.as_raw_handle(),
            Instant::now() + STARTUP_TIMEOUT,
        );
        #[cfg(test)]
        if let Err(error) = &reply {
            use std::io::Read;
            let mut remaining = String::new();
            let _ = (&mut client.output)
                .take(4096)
                .read_to_string(&mut remaining);
            eprintln!("guardian reply error {error:?}: {remaining}");
        }
        match reply? {
            Reply::Ready {} => Ok(client),
            _ => Err(ElevationError::InsecureJob),
        }
    }

    pub(super) fn finish(&mut self) -> Result<Option<(u64, u64)>, ElevationError> {
        self.input.take();
        let deadline = Instant::now() + CLEANUP_TIMEOUT;
        let reply: Reply = receive(self.output.as_raw_handle(), deadline)?;
        loop {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|_| ElevationError::InsecureJob)?
            {
                if !status.success() {
                    return Err(ElevationError::InsecureJob);
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err(ElevationError::TimedOut);
            }
            std::thread::sleep(POLL);
        }
        match reply {
            Reply::Finished { lost } => Ok(lost),
            _ => Err(ElevationError::InsecureJob),
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // Do not kill the guardian or put it in the controller's kill-on-close job.
        // Closing the inherited pipe wakes its cleanup even after TerminateProcess.
        self.input.take();
    }
}

#[cfg(test)]
mod tests {
    use super::super::trace_native::{stop_private_request, Properties, TraceCleanupStatus};
    use super::*;
    use windows_sys::Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_WMI_INSTANCE_NOT_FOUND},
        Security::{
            GetSidSubAuthority, GetSidSubAuthorityCount, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL,
        },
        System::{
            Diagnostics::Etw::{ControlTraceW, CONTROLTRACE_HANDLE, EVENT_TRACE_CONTROL_QUERY},
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            },
        },
    };

    #[derive(Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Scenario {
        Normal,
        Cancel,
        BlockedConsumer,
        Watchdog,
        ProcessExit,
        Terminate,
        KillJob,
        DeniedBreakaway,
        StartupFailure,
    }

    fn entry(name: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--ignored", "--exact", name, "--nocapture"]);
        command
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .stdout(Stdio::null());
        command
    }

    fn input() -> HANDLE {
        // SAFETY: borrowed standard input of the controlled subprocess.
        unsafe { GetStdHandle(STD_INPUT_HANDLE) }
    }

    fn kill_on_close_job(child: &Child, allow_breakaway: bool) -> Handle {
        // SAFETY: unnamed, non-inheritable test-owned job with no security descriptor.
        let job = Handle(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
        assert!(!job.0.is_null());
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if allow_breakaway {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_BREAKAWAY_OK;
        }
        // SAFETY: owned job and exactly sized initialized limits; caller owns its sole handle.
        assert_ne!(
            unsafe {
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            },
            0
        );
        // SAFETY: child is waiting for a framed request and cannot start its guardian yet.
        assert_ne!(
            unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) },
            0
        );
        job
    }

    fn query(request: &ElevationRequest) -> u32 {
        let identity = Identity::request(request).unwrap();
        let mut properties = Properties::new(&identity);
        // SAFETY: only the acceptance's newly generated private name, bounded output.
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE { Value: 0 },
                identity.name.as_ptr(),
                properties.get_mut(),
                EVENT_TRACE_CONTROL_QUERY,
            )
        }
    }

    fn absent(request: &ElevationRequest) -> bool {
        query(request) == ERROR_WMI_INSTANCE_NOT_FOUND
            && acl_absent(&Identity::request(request).unwrap().guid).unwrap()
    }

    fn await_absent(request: &ElevationRequest) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !absent(request) {
            std::thread::sleep(POLL);
        }
        assert!(absent(request), "private session or GUID ACL survived");
    }

    fn medium_integrity() -> bool {
        let mut raw_token = null_mut();
        // SAFETY: current process pseudo-handle, query-only token, initialized handle output.
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw_token) },
            0
        );
        let token = Handle(raw_token);
        let mut buffer = [0usize; 128];
        let mut length = 0;
        // SAFETY: aligned writable allocation, exact byte capacity and live query-only token.
        assert_ne!(
            unsafe {
                GetTokenInformation(
                    token.0,
                    TokenIntegrityLevel,
                    buffer.as_mut_ptr().cast(),
                    size_of_val(&buffer) as u32,
                    &mut length,
                )
            },
            0
        );
        assert!(length as usize >= size_of::<TOKEN_MANDATORY_LABEL>());
        assert!(length as usize <= size_of_val(&buffer));
        // SAFETY: successful TokenIntegrityLevel call initialized a label in aligned storage.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>()).Label.Sid };
        // SAFETY: the token query's SID points into the live buffer.
        assert_ne!(unsafe { IsValidSid(sid) }, 0);
        // SAFETY: validated SID, subauthority count is stored in its header.
        let count = unsafe { *GetSidSubAuthorityCount(sid) };
        assert!(count > 0);
        // SAFETY: the last subauthority exists in the validated SID.
        unsafe { *GetSidSubAuthority(sid, u32::from(count - 1)) == 8192 }
    }

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct StandardProbe {
        own: ElevationRequest,
        sentinel: ElevationRequest,
    }

    fn probe_directory() -> std::path::PathBuf {
        std::env::var_os("PCAGENT_ETW_ACCEPTANCE_DIRECTORY")
            .expect("explicit acceptance directory")
            .into()
    }

    struct Sentinel(OwnedSession);
    impl Drop for Sentinel {
        fn drop(&mut self) {
            let _ = stop_session(&self.0);
        }
    }

    #[test]
    #[ignore = "standard-account peer; scripts/etw-standard-acceptance.ps1 coordinates both tokens"]
    fn standard_peer_acceptance() {
        assert!(!is_elevated().unwrap());
        assert!(medium_integrity());
        let directory = probe_directory();
        let probe: StandardProbe =
            serde_json::from_slice(&std::fs::read(directory.join("probe.json")).unwrap()).unwrap();
        assert_eq!(query(&probe.own), ERROR_SUCCESS);
        let guid = Identity::request(&probe.own).unwrap().guid;
        // SAFETY: test-owned session GUID supplied by the acceptance coordinator; no globals.
        assert_eq!(unsafe { EventAccessRemove(&guid) }, ERROR_ACCESS_DENIED);
        assert_eq!(query(&probe.sentinel), ERROR_ACCESS_DENIED);
        assert!(matches!(
            stop_private_request(&probe.sentinel),
            Err(ElevationError::LaunchFailed(ERROR_ACCESS_DENIED))
        ));
        assert!(matches!(
            stop_private_request(&probe.own),
            Ok(TraceCleanupStatus::Stopped)
        ));
        assert_eq!(query(&probe.own), ERROR_WMI_INSTANCE_NOT_FOUND);
        std::fs::write(
            directory.join("result").join("passed"),
            b"standard-peer-passed",
        )
        .unwrap();
        println!("guardian_standard elevated=0 integrity=8192 own_query=0 own_stop=0 own_remove=5 sentinel_query=5 sentinel_stop=5");
    }

    #[test]
    #[ignore = "privileged peer; scripts/etw-standard-acceptance.ps1 coordinates both tokens"]
    fn standard_owner_acceptance() {
        assert!(is_elevated().unwrap());
        let directory = probe_directory();
        let sid = std::env::var("PCAGENT_ETW_ACCEPTANCE_SID").unwrap();
        let make_request = || {
            ElevationRequest::new(
                ElevatedTask::PerformanceTrace {
                    duration_seconds: 60,
                },
                Duration::from_secs(70),
            )
            .unwrap()
        };
        let request = make_request();
        let sentinel_request = make_request();
        let sentinel = Sentinel(OwnedSession::start(&sentinel_request).unwrap());
        let mut guardian = Client::start(&request, &sid).unwrap();
        let probe = StandardProbe {
            own: request.clone(),
            sentinel: sentinel_request.clone(),
        };
        std::fs::write(
            directory.join("probe.json"),
            serde_json::to_vec(&probe).unwrap(),
        )
        .unwrap();
        let result = directory.join("result").join("passed");
        let deadline = Instant::now() + Duration::from_secs(40);
        while !result.exists() && Instant::now() < deadline {
            std::thread::sleep(POLL);
        }
        let passed = result.exists();
        let sentinel_preserved = query(&sentinel_request) == ERROR_SUCCESS;
        // The standard account cannot read WMI's security registry; verify retained ACL here.
        let acl_retained = !acl_absent(&Identity::request(&request).unwrap().guid).unwrap();
        let cleanup = guardian.finish();
        stop_session(&sentinel.0).unwrap();
        cleanup.unwrap();
        await_absent(&request);
        assert!(sentinel_preserved);
        assert!(acl_retained);
        assert!(
            passed,
            "standard peer did not finish within its acceptance window"
        );
        println!("guardian_standard privileged_cleanup=true own_acl_removed=true sentinel_preserved=true");
    }

    #[test]
    #[ignore = "private subprocess entry for controlled Windows acceptance"]
    fn guardian_entry() {
        let result = worker();
        if let Err(error) = &result {
            eprintln!("guardian acceptance error: {error:?}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }

    #[test]
    #[ignore = "private subprocess entry for controlled Windows acceptance"]
    fn controller_entry() {
        let (start, scenario): (Start, Scenario) =
            receive(input(), Instant::now() + STARTUP_TIMEOUT).unwrap();
        let mut output = std::io::stderr().lock();
        if matches!(scenario, Scenario::StartupFailure) {
            let mut guardian = entry("trace_guardian::tests::guardian_entry")
                .spawn()
                .unwrap();
            // Fail before a complete framed request; the guardian must create no resource.
            guardian
                .stdin
                .take()
                .unwrap()
                .write_all(&[255, 255])
                .unwrap();
            guardian.wait().unwrap();
            send(&mut output, &Reply::Finished { lost: None }).unwrap();
            return;
        }
        if matches!(scenario, Scenario::DeniedBreakaway) {
            assert!(matches!(
                Client::start(&start.request, &start.owner_sid),
                Err(ElevationError::LaunchFailed(5))
            ));
            send(&mut output, &Reply::Finished { lost: None }).unwrap();
            return;
        }
        let mut client = Client::start(&start.request, &start.owner_sid).unwrap();
        send(&mut output, &Reply::Ready {}).unwrap();
        match scenario {
            Scenario::Normal => {
                let lost = client.finish().unwrap();
                send(&mut output, &Reply::Finished { lost }).unwrap();
            }
            Scenario::Cancel => {
                // EOF represents local cancellation without relying on controller destructors.
                client.input.take();
                let lost = client.finish().unwrap();
                send(&mut output, &Reply::Finished { lost }).unwrap();
            }
            Scenario::Watchdog => std::process::exit(124),
            Scenario::ProcessExit => std::process::exit(2),
            Scenario::BlockedConsumer | Scenario::Terminate | Scenario::KillJob => {
                // No consumer makes progress; only the independent guardian can expire the lease.
                std::thread::sleep(Duration::from_secs(40));
            }
            Scenario::StartupFailure | Scenario::DeniedBreakaway => unreachable!(),
        }
    }

    #[test]
    #[ignore = "starts real private ETW loggers; run only via scripts/etw-acceptance.ps1"]
    fn native_acceptance() {
        assert!(is_elevated().unwrap());
        let sid = std::env::var("PCAGENT_ETW_ACCEPTANCE_SID").expect("explicit acceptance SID");
        let make_request = || {
            ElevationRequest::new(
                ElevatedTask::PerformanceTrace {
                    duration_seconds: 30,
                },
                Duration::from_secs(40),
            )
            .unwrap()
        };
        let sentinel_request = make_request();
        let sentinel = OwnedSession::start(&sentinel_request).unwrap();
        let _sentinel = Sentinel(sentinel);
        for scenario in [
            Scenario::Normal,
            Scenario::Cancel,
            Scenario::BlockedConsumer,
            Scenario::Watchdog,
            Scenario::ProcessExit,
            Scenario::Terminate,
            Scenario::KillJob,
            Scenario::DeniedBreakaway,
            Scenario::StartupFailure,
        ] {
            let request = make_request();
            let start = Start {
                request: request.clone(),
                owner_sid: sid.clone(),
            };
            let scenario_name = serde_json::to_string(&scenario).unwrap();
            let mut child = entry("trace_guardian::tests::controller_entry")
                .spawn()
                .unwrap();
            let mut job = match scenario {
                Scenario::KillJob => Some(kill_on_close_job(&child, true)),
                Scenario::DeniedBreakaway => Some(kill_on_close_job(&child, false)),
                _ => None,
            };
            let mut input = child.stdin.take().unwrap();
            let mut output = child.stderr.take().unwrap();
            send(&mut input, &(start, &scenario)).unwrap();
            let begin = Instant::now();
            let reply: Reply = receive(output.as_raw_handle(), begin + STARTUP_TIMEOUT * 2)
                .unwrap_or_else(|error| {
                    use std::io::Read;
                    let mut remaining = String::new();
                    let _ = output.read_to_string(&mut remaining);
                    panic!("controller acceptance error {error:?}: {remaining}");
                });
            if !matches!(
                scenario,
                Scenario::StartupFailure | Scenario::DeniedBreakaway
            ) {
                assert!(matches!(reply, Reply::Ready {}));
            }
            match scenario {
                Scenario::Terminate => child.kill().unwrap(),
                Scenario::KillJob => drop(job.take()),
                Scenario::BlockedConsumer => {
                    let deadline = Instant::now() + Duration::from_secs(36);
                    while Instant::now() < deadline && !absent(&request) {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    assert!(
                        absent(&request),
                        "guardian deadline did not clean blocked consumer"
                    );
                    child.kill().unwrap();
                }
                _ => {}
            }
            let deadline = Instant::now() + Duration::from_secs(5);
            while child.try_wait().unwrap().is_none() && Instant::now() < deadline {
                std::thread::sleep(POLL);
            }
            assert!(
                child.try_wait().unwrap().is_some(),
                "controller did not terminate"
            );
            await_absent(&request);
            assert_eq!(query(&sentinel_request), ERROR_SUCCESS);
            assert!(acl_absent(&Identity::request(&sentinel_request).unwrap().guid).unwrap());
            println!("guardian_acceptance scenario={scenario_name} cleanup=true sentinel_preserved=true elapsed_ms={}",
                begin.elapsed().as_millis());
        }
        // A pre-existing matching session is not adopted, stopped or assigned an ACL.
        let duplicate = Start {
            request: sentinel_request.clone(),
            owner_sid: sid,
        };
        assert!(matches!(
            Lease::start(&duplicate),
            Err(ElevationError::LaunchFailed(ERROR_ALREADY_EXISTS))
        ));
        assert_eq!(query(&sentinel_request), ERROR_SUCCESS);
        assert!(acl_absent(&Identity::request(&sentinel_request).unwrap().guid).unwrap());
        println!("guardian_acceptance already_exists=183 no_adoption=true");
    }

    #[test]
    fn start_protocol_rejects_unknown_fields() {
        assert!(serde_json::from_str::<Start>(
            r#"{"request":{},"owner_sid":"S-1-5-21-1","command":"anything"}"#
        )
        .is_err());
        assert!(serde_json::from_str::<Reply>(r#"{"state":"ready","extra":1}"#).is_err());
    }
}
