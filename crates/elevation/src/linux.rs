//! A root-owned fixed helper, ordinary polkit authorization and private inherited pipes.
//! Requests contain no command, device path or output path. Only numeric SMART data returns.
use super::{
    lifecycle::{run_supervised, ElevationCompletion, HostJob},
    protocol::{
        ElevationError, ElevationRequest, ElevationResponse, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    },
    system::{DiskReliabilitySample, SystemReadLimit, SystemReadSummary, MAX_SYSTEM_ROWS},
    worker::ElevatedProcess,
    ElevationLimit, ElevationStatus,
};
use pcagent_shared::{unix_ms, Window};
use pcagent_storage::CancelToken;
use serde_json::Value;
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, Read, Write},
    os::unix::fs::{FileTypeExt, MetadataExt},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};

const HELPER: &str = "/usr/libexec/pcagent-elevated";
const PKEXEC: &str = "/usr/bin/pkexec";
const SMARTCTL: &str = "/usr/sbin/smartctl";
const SMART_BYTES: usize = 1_048_576;

fn trusted_program(path: &str) -> bool {
    let Ok(canonical) = fs::canonicalize(path) else {
        return false;
    };
    let Ok(file) = fs::metadata(&canonical) else {
        return false;
    };
    if !file.is_file() || file.mode() & 0o111 == 0 {
        return false;
    }
    canonical.ancestors().all(|p| {
        fs::symlink_metadata(p)
            .is_ok_and(|m| m.uid() == 0 && m.mode() & 0o022 == 0 && !m.is_symlink())
    })
}

pub(super) fn helper_installed() -> bool {
    trusted_program(HELPER) && trusted_program(PKEXEC)
}

type ReadResult = Result<Vec<u8>, std::io::Error>;
pub(crate) struct LinuxJob {
    request: ElevationRequest,
    cancelled: Arc<AtomicBool>,
    output: Mutex<Option<mpsc::Receiver<ReadResult>>>,
    completion: ElevationCompletion,
}
impl LinuxJob {
    pub(crate) fn create(request: &ElevationRequest) -> Result<Self, ElevationError> {
        if !helper_installed() {
            return Err(ElevationError::Unsupported);
        }
        Ok(Self {
            request: request.clone(),
            cancelled: Arc::new(AtomicBool::new(false)),
            output: Mutex::new(None),
            completion: ElevationCompletion::pending(),
        })
    }
    pub(crate) fn completion(&self) -> ElevationCompletion {
        self.completion.clone()
    }
}
impl Drop for LinuxJob {
    fn drop(&mut self) {
        self.completion.complete();
    }
}
impl HostJob for LinuxJob {
    type Response = ElevationResponse;
    fn signal_cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    fn response(&self) -> Result<Self::Response, ElevationError> {
        let receiver = self
            .output
            .lock()
            .map_err(|_| ElevationError::InvalidResponse)?
            .take()
            .ok_or(ElevationError::InvalidResponse)?;
        let bytes = receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| ElevationError::InvalidResponse)??;
        ElevationResponse::from_bytes(&bytes, &self.request)
    }
}

struct Process {
    child: Child,
    input: Option<ChildStdin>,
    cancelled: Arc<AtomicBool>,
}
impl ElevatedProcess for Process {
    fn poll(&mut self) -> Result<Option<u32>, ElevationError> {
        if self.cancelled.load(Ordering::Acquire) {
            self.terminate()?;
        }
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        match status.code() {
            Some(126) => Err(ElevationError::UacDeclined),
            Some(127) => Err(ElevationError::LaunchFailed(127)),
            Some(code) => Ok(Some(code as u32)),
            None => Ok(Some(1)),
        }
    }
    fn has_exited(&mut self) -> Result<bool, ElevationError> {
        Ok(self.child.try_wait()?.is_some())
    }
    fn terminate(&mut self) -> Result<(), ElevationError> {
        // A standard user cannot signal the root worker. EOF cancels its reads cooperatively.
        self.input.take();
        // This succeeds while pkexec still belongs to the user, e.g. during the consent dialog.
        let _ = self.child.kill();
        Ok(())
    }
}
fn launch(job: &LinuxJob) -> Result<Process, ElevationError> {
    job.request.validate_at(unix_ms())?;
    if job.cancelled.load(Ordering::Acquire) {
        return Err(ElevationError::Cancelled);
    }
    if !helper_installed() {
        return Err(ElevationError::InsecureJob);
    }
    let bytes = job.request.encode()?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err(ElevationError::InvalidRequest);
    }
    let mut child = Command::new(PKEXEC)
        .args(["--disable-internal-agent", HELPER, "--linux-pipe-v1"])
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().ok_or(ElevationError::InvalidRequest)?;
    let output = child.stdout.take().ok_or(ElevationError::InvalidResponse)?;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take((MAX_RESPONSE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    *job.output
        .lock()
        .map_err(|_| ElevationError::InvalidResponse)? = Some(rx);
    if let Err(error) = input
        .write_all(&bytes)
        .and_then(|_| input.write_all(b"\n"))
        .and_then(|_| input.flush())
    {
        drop(input);
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    Ok(Process {
        child,
        input: Some(input),
        cancelled: job.cancelled.clone(),
    })
}
pub(crate) fn run_approved(
    job: Arc<LinuxJob>,
    request: &ElevationRequest,
    cancel: &CancelToken,
) -> Result<ElevationResponse, ElevationError> {
    run_supervised(job, cancel, request.budget(), launch)
}

pub(super) fn worker(args: impl IntoIterator<Item = OsString>) -> Result<(), ElevationError> {
    let args: Vec<_> = args.into_iter().take(2).collect();
    if args != [OsString::from("--linux-pipe-v1")] {
        return Err(ElevationError::InvalidArguments);
    }
    // SAFETY: these credential getters take no pointers and have no initialization preconditions.
    if unsafe { libc::geteuid() != 0 || libc::getuid() != 0 }
        || std::env::var("PKEXEC_UID")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .is_none_or(|uid| uid == 0)
    {
        return Err(ElevationError::ApprovalRequired);
    }
    let cancel = CancelToken::default();
    let reader_cancel = cancel.clone();
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut bytes = Vec::new();
        let result = Read::by_ref(&mut input)
            .take((MAX_REQUEST_BYTES + 1) as u64)
            .read_until(b'\n', &mut bytes)
            .map_err(ElevationError::from)
            .and_then(|_| {
                if bytes.last() != Some(&b'\n') {
                    return Err(ElevationError::InvalidRequest);
                }
                bytes.pop();
                ElevationRequest::from_bytes(&bytes)
            });
        let _ = tx.send(result);
        let mut extra = [0u8; 1];
        let _ = input.read(&mut extra);
        reader_cancel.cancel();
    });
    let request = rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|_| ElevationError::TimedOut)??;
    if !super::task_supported(&request.task) {
        return Err(ElevationError::Unsupported);
    }
    let started = Instant::now();
    let window = Window::instant();
    let budget = request.budget().min(Duration::from_millis(
        request.expires_at_unix_ms.saturating_sub(unix_ms()),
    ));
    let summary = collect_disks(&cancel, started, budget)?;
    let mut response =
        ElevationResponse::empty(&request, summary.status, ElevationLimit::MetadataSnapshot);
    response.disk_reliability = Some(summary);
    response.window = Window {
        started_at_unix_ms: window.started_at_unix_ms,
        finished_at_unix_ms: unix_ms(),
        duration_ms: started.elapsed().as_millis() as u64,
    };
    response.validate(&request)?;
    let bytes = serde_json::to_vec(&response).map_err(|_| ElevationError::InvalidResponse)?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(ElevationError::InvalidResponse);
    }
    std::io::stdout().write_all(&bytes)?;
    Ok(())
}
fn check(cancel: &CancelToken, started: Instant, budget: Duration) -> Result<(), ElevationError> {
    if cancel.is_cancelled() {
        Err(ElevationError::Cancelled)
    } else if started.elapsed() >= budget {
        Err(ElevationError::TimedOut)
    } else {
        Ok(())
    }
}
fn collect_disks(
    cancel: &CancelToken,
    started: Instant,
    budget: Duration,
) -> Result<SystemReadSummary<DiskReliabilitySample>, ElevationError> {
    if !trusted_program(SMARTCTL) {
        return Ok(SystemReadSummary::unavailable(
            ElevationStatus::Unsupported,
            SystemReadLimit::ProviderUnavailable,
        ));
    }
    let mut out = SystemReadSummary {
        status: ElevationStatus::Partial,
        samples: Vec::new(),
        truncated: false,
        read_errors: 0,
        limitations: vec![
            SystemReadLimit::StorageCountersAreDeviceReported,
            SystemReadLimit::ProviderValueMissing,
        ],
    };
    for entry in fs::read_dir("/sys/block")? {
        check(cancel, started, budget)?;
        let Ok(entry) = entry else {
            out.read_errors += 1;
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|n| valid_device_name(n)) else {
            continue;
        };
        let device = format!("/dev/{name}");
        if !fs::symlink_metadata(&device).is_ok_and(|m| m.file_type().is_block_device()) {
            continue;
        }
        if out.samples.len() >= MAX_SYSTEM_ROWS {
            out.truncated = true;
            break;
        }
        match smartctl(&device, cancel, started, budget) {
            Ok(value) => out.samples.push(sample(&value)),
            Err(ElevationError::Cancelled) => return Err(ElevationError::Cancelled),
            Err(ElevationError::TimedOut) if started.elapsed() >= budget => {
                return Err(ElevationError::TimedOut)
            }
            Err(_) => out.read_errors += 1,
        }
    }
    if out.truncated {
        out.limitations.push(SystemReadLimit::RowLimit);
    }
    if out.samples.is_empty() && out.read_errors > 0 {
        out.status = ElevationStatus::Error;
        out.limitations.push(SystemReadLimit::EnumerationIncomplete);
    }
    Ok(out)
}
fn valid_device_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with("loop")
        && !name.starts_with("ram")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn smartctl(
    device: &str,
    cancel: &CancelToken,
    started: Instant,
    budget: Duration,
) -> Result<Value, ElevationError> {
    let mut child = Command::new(SMARTCTL)
        .args(["--all", "--json", device])
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let output = child.stdout.take().ok_or(ElevationError::InvalidResponse)?;
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = output
            .take((SMART_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let local = Instant::now();
    let status = loop {
        if let Err(error) = check(cancel, started, budget).and_then(|_| {
            if local.elapsed() > Duration::from_secs(8) {
                Err(ElevationError::TimedOut)
            } else {
                Ok(())
            }
        }) {
            let _ = child.kill();
            child.wait()?;
            return Err(error);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error.into());
            }
        }
    };
    let bytes = rx
        .recv_timeout(Duration::from_secs(1))
        .map_err(|_| ElevationError::InvalidResponse)??;
    // smartctl's upper bits describe health findings, not a failed invocation.
    if bytes.len() > SMART_BYTES || status.code().is_none_or(|s| s & 7 != 0) {
        return Err(ElevationError::InvalidResponse);
    }
    serde_json::from_slice(&bytes).map_err(|_| ElevationError::InvalidResponse)
}
fn sample(value: &Value) -> DiskReliabilitySample {
    let u8v = |v: &Value| v.as_u64().and_then(|n| u8::try_from(n).ok());
    DiskReliabilitySample {
        temperature_celsius: u8v(&value["temperature"]["current"]),
        temperature_limit_celsius: None,
        wear_percent: u8v(&value["nvme_smart_health_information_log"]["percentage_used"]),
        power_on_hours: value["power_on_time"]["hours"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok()),
        read_errors_total: None,
        read_errors_corrected: None,
        read_errors_uncorrected: None,
        write_errors_total: None,
        write_errors_corrected: None,
        write_errors_uncorrected: None,
        start_stop_cycles: None,
        load_unload_cycles: None,
        maximum_read_latency_ms: None,
        maximum_write_latency_ms: None,
        maximum_flush_latency_ms: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ElevatedTask;
    #[test]
    fn no_arbitrary_command_path_or_windows_task_can_reach_the_worker() {
        for name in ["../sda", "/dev/sda", "sda;id", "", "loop0", "ram0"] {
            assert!(!valid_device_name(name));
        }
        for name in ["sda", "nvme0n1", "dm-0"] {
            assert!(valid_device_name(name));
        }
        assert!(super::super::task_supported(&ElevatedTask::DiskReliability));
        assert!(!super::super::task_supported(&ElevatedTask::SystemCaches));
        assert_eq!(
            worker([OsString::from("--command")]),
            Err(ElevationError::InvalidArguments)
        );
    }
    #[test]
    fn smart_projection_drops_serials_names_and_missing_or_overflowing_values() {
        let value = serde_json::json!({"serial_number":"private","device":{"name":"/dev/sda"},
            "temperature":{"current":41},"power_on_time":{"hours":70000},
            "nvme_smart_health_information_log":{"percentage_used":110}});
        let projected = sample(&value);
        assert_eq!(projected.temperature_celsius, Some(41));
        assert_eq!(projected.wear_percent, Some(110));
        assert_eq!(projected.power_on_hours, None);
        assert_eq!(projected.read_errors_total, None);
        let bytes = serde_json::to_string(&projected).unwrap();
        assert!(!bytes.contains("private") && !bytes.contains("/dev/"));
        let empty = sample(&serde_json::json!({}));
        assert_eq!(empty.temperature_celsius, None);
        assert_eq!(empty.wear_percent, None);
    }
    #[test]
    fn user_owned_programs_cannot_be_selected_for_elevation() {
        assert!(!trusted_program("/does-not-exist"));
        assert!(!trusted_program(
            std::env::current_exe().unwrap().to_str().unwrap()
        ));
    }
}
