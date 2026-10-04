use pcagent_shared::{new_id, unix_ms, Window};
use serde::{Deserialize, Serialize};
#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fmt,
    path::PathBuf,
    time::Duration,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_BUDGET_MS: u64 = 120_000;
pub const MAX_REQUEST_BYTES: usize = 8192;
pub const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriveLetter(char);
impl DriveLetter {
    pub fn parse(text: &str) -> Result<Self, ElevationError> {
        if text.len() == 1 && text.as_bytes()[0].is_ascii_uppercase() {
            Ok(Self(text.chars().next().unwrap()))
        } else {
            Err(ElevationError::InvalidArguments)
        }
    }
    pub fn letter(self) -> char {
        self.0
    }
}
impl Serialize for DriveLetter {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}
impl<'de> Deserialize<'de> for DriveLetter {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ElevatedTask {
    SystemCaches,
    MftInventory { volume: DriveLetter },
}

impl<'de> Deserialize<'de> for ElevatedTask {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        // Empty struct variants enforce closed fields; serde's unit variants ignore extras.
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum WireTask {
            SystemCaches {},
            MftInventory { volume: DriveLetter },
        }
        Ok(match WireTask::deserialize(deserializer)? {
            WireTask::SystemCaches {} => Self::SystemCaches,
            WireTask::MftInventory { volume } => Self::MftInventory { volume },
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElevationRequest {
    pub(crate) protocol_version: u32,
    pub(crate) job_id: String,
    pub(crate) nonce: String,
    pub(crate) task: ElevatedTask,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) expires_at_unix_ms: u64,
    pub(crate) budget_ms: u64,
}
impl fmt::Debug for ElevationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElevationRequest")
            .field("task", &self.task)
            .field("budget_ms", &self.budget_ms)
            .finish_non_exhaustive()
    }
}
impl ElevationRequest {
    pub(crate) fn new(task: ElevatedTask, budget: Duration) -> Result<Self, ElevationError> {
        let budget_ms =
            u64::try_from(budget.as_millis()).map_err(|_| ElevationError::InvalidRequest)?;
        if !(1..=MAX_BUDGET_MS).contains(&budget_ms) {
            return Err(ElevationError::InvalidRequest);
        }
        let now = unix_ms();
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            job_id: new_id("elevjob"),
            nonce: new_id("nonce"),
            task,
            created_at_unix_ms: now,
            expires_at_unix_ms: now.saturating_add(MAX_BUDGET_MS),
            budget_ms,
        })
    }
    pub fn task(&self) -> ElevatedTask {
        self.task
    }
    pub fn budget(&self) -> Duration {
        Duration::from_millis(self.budget_ms)
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ElevationError> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(ElevationError::InvalidRequest);
        }
        let request: Self =
            serde_json::from_slice(bytes).map_err(|_| ElevationError::InvalidRequest)?;
        request.validate_at(unix_ms())?;
        Ok(request)
    }
    pub(crate) fn validate_at(&self, now: u64) -> Result<(), ElevationError> {
        if self.protocol_version != PROTOCOL_VERSION
            || !random_ref(&self.job_id, "elevjob")
            || !random_ref(&self.nonce, "nonce")
            || !(1..=MAX_BUDGET_MS).contains(&self.budget_ms)
            || self.expires_at_unix_ms.checked_sub(self.created_at_unix_ms) != Some(MAX_BUDGET_MS)
            || now < self.created_at_unix_ms
        {
            return Err(ElevationError::InvalidRequest);
        }
        if now >= self.expires_at_unix_ms {
            return Err(ElevationError::Expired);
        }
        Ok(())
    }
    #[cfg(any(windows, test))]
    pub(crate) fn encode(&self) -> Result<Vec<u8>, ElevationError> {
        serde_json::to_vec(self).map_err(|_| ElevationError::InvalidRequest)
    }
}
fn random_ref(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|s| s.strip_prefix('-'))
        .is_some_and(|uuid| {
            uuid.len() == 36
                && uuid.bytes().enumerate().all(|(i, b)| {
                    if [8, 13, 18, 23].contains(&i) {
                        b == b'-'
                    } else {
                        b.is_ascii_hexdigit()
                    }
                })
        })
}
#[cfg(any(windows, test))]
pub(crate) fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(any(windows, test))]
pub(crate) fn launch_error(code: u32) -> ElevationError {
    if code == 1223 {
        ElevationError::UacDeclined
    } else {
        ElevationError::LaunchFailed(code)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationStatus {
    Completed,
    Partial,
    Unsupported,
    Denied,
    Cancelled,
    TimedOut,
    Error,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheCategory {
    PreviousWindows,
    WindowsUpdateDownload,
    DeliveryOptimization,
    RestorePoints,
}
impl CacheCategory {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreviousWindows => "previous_windows",
            Self::WindowsUpdateDownload => "windows_update_download",
            Self::DeliveryOptimization => "delivery_optimization",
            Self::RestorePoints => "restore_points",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElevationLimit {
    MetadataSnapshot,
    AllocationLowerBound,
    EnumerationIncomplete,
    EntryLimit,
    MemoryLimit,
    DurationLimit,
    Cancelled,
    RestorePointsUnimplemented,
    DefaultCacheLocationOnly,
    MftAccuracyUnvalidated,
    SourceUnavailable,
    PlatformUnsupported,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheCoverage {
    pub directories: u64,
    pub denied_directories: u64,
    pub denied_files: u64,
    pub reparse_skipped: u64,
    pub placeholders_skipped: u64,
    pub other_volume_skipped: u64,
    pub special_skipped: u64,
    pub vanished: u64,
    pub hard_link_aliases: u64,
    pub changed_during_scan: u64,
    pub allocation_unknown_files: u64,
    pub errors: u64,
    pub truncated: bool,
    pub cancelled: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheSummary {
    pub category: CacheCategory,
    pub status: ElevationStatus,
    pub exists: Option<bool>,
    pub observed_logical_bytes: Option<u64>,
    /// Sum of known allocation only, deduplicated within this category. Never reclaimable bytes.
    pub observed_allocated_bytes: Option<u64>,
    pub files: Option<u64>,
    pub coverage: Option<CacheCoverage>,
    pub limitations: Vec<ElevationLimit>,
}
impl CacheSummary {
    #[cfg(any(windows, test))]
    pub(crate) fn unavailable(
        category: CacheCategory,
        status: ElevationStatus,
        limit: ElevationLimit,
    ) -> Self {
        Self {
            category,
            status,
            exists: None,
            observed_logical_bytes: None,
            observed_allocated_bytes: None,
            files: None,
            coverage: None,
            limitations: vec![limit],
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ElevationResponse {
    pub(crate) protocol_version: u32,
    pub(crate) job_id: String,
    pub(crate) nonce: String,
    pub task: ElevatedTask,
    pub status: ElevationStatus,
    #[serde(deserialize_with = "deserialize_window")]
    pub window: Window,
    pub caches: Vec<CacheSummary>,
    pub limitations: Vec<ElevationLimit>,
}
impl fmt::Debug for ElevationResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ElevationResponse")
            .field("task", &self.task)
            .field("status", &self.status)
            .field("caches", &self.caches)
            .finish_non_exhaustive()
    }
}
impl ElevationResponse {
    #[cfg(any(windows, test))]
    pub(crate) fn empty(
        request: &ElevationRequest,
        status: ElevationStatus,
        limit: ElevationLimit,
    ) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            job_id: request.job_id.clone(),
            nonce: request.nonce.clone(),
            task: request.task,
            status,
            window: Window::instant(),
            caches: Vec::new(),
            limitations: vec![limit],
        }
    }
    pub fn from_bytes(bytes: &[u8], request: &ElevationRequest) -> Result<Self, ElevationError> {
        if bytes.len() > MAX_RESPONSE_BYTES {
            return Err(ElevationError::InvalidResponse);
        }
        let response: Self =
            serde_json::from_slice(bytes).map_err(|_| ElevationError::InvalidResponse)?;
        response.validate(request)?;
        Ok(response)
    }
    pub(crate) fn validate(&self, request: &ElevationRequest) -> Result<(), ElevationError> {
        if self.protocol_version != PROTOCOL_VERSION
            || self.job_id != request.job_id
            || self.nonce != request.nonce
            || self.task != request.task
            || self.caches.len() > 4
            || self.limitations.len() > 16
            || self.window.started_at_unix_ms < request.created_at_unix_ms
            || self.window.finished_at_unix_ms < self.window.started_at_unix_ms
            || self.window.finished_at_unix_ms > request.expires_at_unix_ms
            || self.window.finished_at_unix_ms > unix_ms().saturating_add(1000)
            || self.window.duration_ms > request.budget_ms
        {
            return Err(ElevationError::InvalidResponse);
        }
        let mut categories = Vec::new();
        for cache in &self.caches {
            if categories.contains(&cache.category)
                || cache.limitations.len() > 16
                || (matches!(
                    cache.status,
                    ElevationStatus::Unsupported | ElevationStatus::Denied | ElevationStatus::Error
                ) && (cache.observed_logical_bytes.is_some()
                    || cache.observed_allocated_bytes.is_some()
                    || cache.files.is_some()))
            {
                return Err(ElevationError::InvalidResponse);
            }
            match cache.exists {
                Some(false)
                    if cache.status == ElevationStatus::Completed
                        && cache.observed_logical_bytes == Some(0)
                        && cache.observed_allocated_bytes == Some(0)
                        && cache.files == Some(0)
                        && cache.coverage.is_none() => {}
                Some(true)
                    if cache.observed_logical_bytes.is_some()
                        && cache.observed_allocated_bytes.is_some()
                        && cache.files.is_some()
                        && cache.coverage.is_some() =>
                {
                    let coverage = cache.coverage.as_ref().unwrap();
                    if coverage.allocation_unknown_files > cache.files.unwrap()
                        || (cache.status == ElevationStatus::Completed
                            && (coverage.truncated
                                || coverage.cancelled
                                || coverage.denied_directories > 0
                                || coverage.denied_files > 0
                                || coverage.changed_during_scan > 0
                                || coverage.vanished > 0
                                || coverage.errors > 0
                                || coverage.allocation_unknown_files > 0))
                    {
                        return Err(ElevationError::InvalidResponse);
                    }
                }
                None if cache.observed_logical_bytes.is_none()
                    && cache.observed_allocated_bytes.is_none()
                    && cache.files.is_none()
                    && cache.coverage.is_none()
                    && cache.status != ElevationStatus::Completed => {}
                _ => return Err(ElevationError::InvalidResponse),
            }
            categories.push(cache.category);
        }
        if matches!(self.task, ElevatedTask::MftInventory { .. }) && !self.caches.is_empty() {
            return Err(ElevationError::InvalidResponse);
        }
        if self.status == ElevationStatus::Completed
            && (self.caches.len() != 4
                || self
                    .caches
                    .iter()
                    .any(|cache| cache.status != ElevationStatus::Completed))
        {
            return Err(ElevationError::InvalidResponse);
        }
        if matches!(
            self.status,
            ElevationStatus::Unsupported | ElevationStatus::Denied | ElevationStatus::Error
        ) && !self.caches.is_empty()
        {
            return Err(ElevationError::InvalidResponse);
        }
        Ok(())
    }
}

fn deserialize_window<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Window, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ClosedWindow {
        started_at_unix_ms: u64,
        finished_at_unix_ms: u64,
        duration_ms: u64,
    }
    let value = ClosedWindow::deserialize(deserializer)?;
    Ok(Window {
        started_at_unix_ms: value.started_at_unix_ms,
        finished_at_unix_ms: value.finished_at_unix_ms,
        duration_ms: value.duration_ms,
    })
}

/// Exact worker CLI: a local job directory and digest only, with no arbitrary output path or task.
#[derive(Clone)]
// Native execution consumes these fields on Windows; portable parsing remains testable.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct WorkerArguments {
    pub(crate) job_dir: PathBuf,
    pub(crate) nonce: String,
    pub(crate) request_digest: String,
}
impl WorkerArguments {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Self, ElevationError> {
        // Bound before allocation even for an adversarial programmatic caller.
        let args: Vec<_> = args.into_iter().take(7).collect();
        if args.len() != 6
            || args[0] != "--job-dir"
            || args[2] != "--nonce"
            || args[4] != "--request-digest"
        {
            return Err(ElevationError::InvalidArguments);
        }
        let job_dir = PathBuf::from(&args[1]);
        let nonce = args[3]
            .to_str()
            .ok_or(ElevationError::InvalidArguments)?
            .to_owned();
        let request_digest = args[5]
            .to_str()
            .ok_or(ElevationError::InvalidArguments)?
            .to_owned();
        if !job_dir.is_absolute()
            || job_dir.as_os_str().len() > 4096
            || !random_ref(&nonce, "nonce")
            || request_digest.len() != 64
            || !request_digest.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ElevationError::InvalidArguments);
        }
        if !job_dir
            .file_name()
            .and_then(OsStr::to_str)
            .and_then(|name| name.strip_prefix("pcagent-"))
            .is_some_and(|job_id| random_ref(job_id, "elevjob"))
        {
            return Err(ElevationError::InvalidArguments);
        }
        Ok(Self {
            job_dir,
            nonce,
            request_digest,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationError {
    InvalidArguments,
    InvalidRequest,
    InvalidResponse,
    InsecureJob,
    DigestMismatch,
    Expired,
    ApprovalRequired,
    ElevatedHost,
    ApprovalConsumed,
    UacDeclined,
    Cancelled,
    TimedOut,
    Unsupported,
    Io(std::io::ErrorKind),
    LaunchFailed(u32),
    HelperFailed(u32),
}
impl fmt::Display for ElevationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for ElevationError {}
impl From<std::io::Error> for ElevationError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.kind())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_dates_must_fit_the_approved_job_and_receipt_time() {
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let mut response = ElevationResponse::empty(
            &request,
            ElevationStatus::Unsupported,
            ElevationLimit::MftAccuracyUnvalidated,
        );
        response.window.started_at_unix_ms = request.created_at_unix_ms;
        response.window.duration_ms = 0;
        for finished in [request.expires_at_unix_ms + 1, unix_ms() + 5000] {
            response.window.finished_at_unix_ms = finished;
            assert_eq!(
                response.validate(&request),
                Err(ElevationError::InvalidResponse)
            );
        }
        response.window.finished_at_unix_ms = request.created_at_unix_ms;
        response.window.started_at_unix_ms = request.created_at_unix_ms - 1;
        assert_eq!(
            response.validate(&request),
            Err(ElevationError::InvalidResponse)
        );
    }
    #[test]
    fn protocol_rejects_unknown_fields_paths_and_truncation() {
        let request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let bytes = request.encode().unwrap();
        assert!(ElevationRequest::from_bytes(&bytes).is_ok());
        assert!(ElevationRequest::from_bytes(&vec![b' '; MAX_REQUEST_BYTES + 1]).is_err());
        for length in [0, 1, bytes.len() - 1] {
            assert!(ElevationRequest::from_bytes(&bytes[..length]).is_err());
        }
        let mut malicious = serde_json::to_value(&request).unwrap();
        malicious["command"] = "delete system data".into();
        assert!(ElevationRequest::from_bytes(&serde_json::to_vec(&malicious).unwrap()).is_err());
        let mut hostile_task = serde_json::to_value(&request).unwrap();
        hostile_task["task"]["command"] = "cmd.exe".into();
        assert!(ElevationRequest::from_bytes(&serde_json::to_vec(&hostile_task).unwrap()).is_err());
        for drive in ["C:", "C:\\", "\\\\.\\PhysicalDrive0", "c", "C;run"] {
            assert!(DriveLetter::parse(drive).is_err());
        }
        let response = ElevationResponse::empty(
            &request,
            ElevationStatus::Unsupported,
            ElevationLimit::MftAccuracyUnvalidated,
        );
        let bytes = serde_json::to_vec(&response).unwrap();
        assert!(ElevationResponse::from_bytes(&bytes, &request).is_ok());
        assert!(
            ElevationResponse::from_bytes(&vec![b' '; MAX_RESPONSE_BYTES + 1], &request).is_err()
        );
        assert!(ElevationResponse::from_bytes(&bytes[..bytes.len() - 1], &request).is_err());
        let mut hostile_response = serde_json::to_value(&response).unwrap();
        hostile_response["output_path"] = "C:\\Windows\\outside-job".into();
        assert!(ElevationResponse::from_bytes(
            &serde_json::to_vec(&hostile_response).unwrap(),
            &request
        )
        .is_err());
        let mut unknown_window_field = serde_json::to_value(&response).unwrap();
        unknown_window_field["window"]["path"] = "C:\\secret".into();
        assert!(ElevationResponse::from_bytes(
            &serde_json::to_vec(&unknown_window_field).unwrap(),
            &request
        )
        .is_err());
        assert_ne!(digest(&bytes), digest(&bytes[..bytes.len() - 1]));
        let mut mismatch = response.clone();
        mismatch.nonce = new_id("nonce");
        assert!(mismatch.validate(&request).is_err());
        assert!(!format!("{request:?} {response:?}").contains(&request.nonce));
        let projected = serde_json::to_string(&super::super::to_inspection(&response)).unwrap();
        assert!(!projected.contains(&request.nonce));
        assert!(!projected.contains(&request.job_id));
        assert!(super::super::to_inspection(&response)
            .observations
            .is_empty());
        assert_eq!(
            super::super::to_inspection(&response).status,
            pcagent_shared::Status::Unsupported
        );
    }
    #[test]
    fn budgets_and_cli_are_strict() {
        assert!(ElevationRequest::new(ElevatedTask::SystemCaches, Duration::ZERO).is_err());
        assert!(ElevationRequest::new(
            ElevatedTask::SystemCaches,
            Duration::from_millis(MAX_BUDGET_MS + 1)
        )
        .is_err());
        for args in [
            vec!["--task", "system_caches"],
            vec![
                "--job-dir",
                ".",
                "--nonce",
                "anything",
                "--request-digest",
                "abc",
            ],
            vec!["--shell", "cmd.exe"],
        ] {
            assert!(WorkerArguments::parse(args.into_iter().map(OsString::from)).is_err());
        }
        let valid =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        let fixture_dir = std::env::temp_dir().join(format!("pcagent-{}", valid.job_id));
        let args = vec![
            OsString::from("--job-dir"),
            fixture_dir.into_os_string(),
            OsString::from("--nonce"),
            valid.nonce.clone().into(),
            OsString::from("--request-digest"),
            "0".repeat(64).into(),
        ];
        assert!(WorkerArguments::parse(args.clone()).is_ok());
        let mut bad_nonce = args.clone();
        bad_nonce[3] = "nonce;cmd.exe".into();
        assert!(WorkerArguments::parse(bad_nonce).is_err());
        let mut bad_digest = args.clone();
        bad_digest[5] = "0".repeat(63).into();
        assert!(WorkerArguments::parse(bad_digest).is_err());
        let mut extra = args;
        extra.push("--output".into());
        assert!(WorkerArguments::parse(extra).is_err());
        let mut request =
            ElevationRequest::new(ElevatedTask::SystemCaches, Duration::from_secs(10)).unwrap();
        assert_eq!(
            request.validate_at(request.expires_at_unix_ms),
            Err(ElevationError::Expired)
        );
        request.protocol_version += 1;
        assert_eq!(
            request.validate_at(unix_ms()),
            Err(ElevationError::InvalidRequest)
        );
    }
}
