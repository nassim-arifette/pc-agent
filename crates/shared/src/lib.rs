//! Versioned local contracts. Missing measurements are never encoded as zero.
#![forbid(unsafe_code)]
use serde::{Deserialize, Serialize};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const SCHEMA_VERSION: u32 = 1;
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}
/// A short random reference (`proc-3f9a1c2b`): 32 random bits, so a caller cannot guess the
/// references of instances it was never shown. Callers must reject collisions themselves.
pub fn new_short_ref(prefix: &str) -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("{prefix}-{}", &hex[..8])
}
pub fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
/// `YYYY-MM-DD HH:MM UTC` for display (proleptic Gregorian calendar, no leap seconds).
pub fn format_utc_minute(unix_ms: u64) -> String {
    let secs = unix_ms / 1_000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (H. Hinnant's algorithm), in March-based years.
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3_600,
        rem % 3_600 / 60
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Partial,
    Unsupported,
    PermissionDenied,
    Stale,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Live,
    SyntheticFixture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    PhysicalTotalBytes,
    PhysicalAvailableBytes,
    SystemCommitBytes,
    SystemCommitLimitBytes,
    CpuBusyBasisPoints,
    ProcessWorkingSetBytes,
    ProcessPrivateCommitBytes,
    /// Share of all logical processors used by one instance over the evidence window.
    ProcessCpuBasisPoints,
    /// I/O counter deltas over the evidence window; include cached and non-disk I/O.
    ProcessIoReadBytes,
    ProcessIoWriteBytes,
    ProcessIoOtherBytes,
    /// Busiest logical processor over the window, from processor utility: work normalized to
    /// nominal frequency, so turbo can exceed 10 000.
    CpuBusiestCoreBasisPoints,
    /// Average seconds per completed read across all physical disks over the window.
    DiskReadLatencyMicroseconds,
    DiskWriteLatencyMicroseconds,
    /// Share of the window during which the aggregated physical disks were idle.
    DiskIdleBasisPoints,
    /// Instantaneous outstanding requests at the end of the window; not a rate.
    DiskQueueLength,
    /// Pages read from disk to resolve hard faults (includes mapped files, not only paging).
    HardFaultPagesPerSecondX100,
    /// Utilization of one GPU engine type (busiest engine of that type) over the window.
    GpuEngineBasisPoints,
    /// Busiest GPU engine of any type over the window.
    GpuBusiestEngineBasisPoints,
}
impl Metric {
    pub fn unit(self) -> &'static str {
        match self {
            Self::CpuBusyBasisPoints | Self::ProcessCpuBasisPoints => "basis_points_of_total_cpu",
            Self::CpuBusiestCoreBasisPoints => "basis_points_of_one_logical_processor",
            Self::DiskReadLatencyMicroseconds | Self::DiskWriteLatencyMicroseconds => {
                "microseconds"
            }
            Self::DiskIdleBasisPoints => "basis_points_of_window",
            Self::DiskQueueLength => "outstanding_requests",
            Self::HardFaultPagesPerSecondX100 => "pages_per_second_x100",
            Self::GpuEngineBasisPoints | Self::GpuBusiestEngineBasisPoints => {
                "basis_points_of_engine_time"
            }
            Self::PhysicalTotalBytes
            | Self::PhysicalAvailableBytes
            | Self::SystemCommitBytes
            | Self::SystemCommitLimitBytes
            | Self::ProcessWorkingSetBytes
            | Self::ProcessPrivateCommitBytes
            | Self::ProcessIoReadBytes
            | Self::ProcessIoWriteBytes
            | Self::ProcessIoOtherBytes => "bytes",
        }
    }
    /// Short label for human-readable output.
    pub fn label(self) -> &'static str {
        match self {
            Self::PhysicalTotalBytes => "Physical memory",
            Self::PhysicalAvailableBytes => "Available memory",
            Self::SystemCommitBytes => "Commit charge",
            Self::SystemCommitLimitBytes => "Commit limit",
            Self::CpuBusyBasisPoints => "CPU (all cores)",
            Self::ProcessWorkingSetBytes => "Working set",
            Self::ProcessPrivateCommitBytes => "Private commit",
            Self::ProcessCpuBasisPoints => "CPU",
            Self::ProcessIoReadBytes => "I/O read",
            Self::ProcessIoWriteBytes => "I/O write",
            Self::ProcessIoOtherBytes => "I/O other",
            Self::CpuBusiestCoreBasisPoints => "Busiest core",
            Self::DiskReadLatencyMicroseconds => "Disk read latency",
            Self::DiskWriteLatencyMicroseconds => "Disk write latency",
            Self::DiskIdleBasisPoints => "Disk idle time",
            Self::DiskQueueLength => "Disk queue length",
            Self::HardFaultPagesPerSecondX100 => "Hard-fault page reads",
            Self::GpuEngineBasisPoints => "GPU engine",
            Self::GpuBusiestEngineBasisPoints => "Busiest GPU engine",
        }
    }
    /// A value of this metric in human units: binary byte units, percentages, milliseconds.
    pub fn format_value(self, value: u64) -> String {
        match self {
            Self::PhysicalTotalBytes
            | Self::PhysicalAvailableBytes
            | Self::SystemCommitBytes
            | Self::SystemCommitLimitBytes
            | Self::ProcessWorkingSetBytes
            | Self::ProcessPrivateCommitBytes
            | Self::ProcessIoReadBytes
            | Self::ProcessIoWriteBytes
            | Self::ProcessIoOtherBytes => format_bytes(value),
            Self::CpuBusyBasisPoints
            | Self::ProcessCpuBasisPoints
            | Self::CpuBusiestCoreBasisPoints
            | Self::DiskIdleBasisPoints
            | Self::GpuEngineBasisPoints
            | Self::GpuBusiestEngineBasisPoints => format!("{:.1}%", value as f64 / 100.0),
            Self::DiskReadLatencyMicroseconds | Self::DiskWriteLatencyMicroseconds => {
                format!("{:.1} ms", value as f64 / 1_000.0)
            }
            Self::DiskQueueLength => value.to_string(),
            Self::HardFaultPagesPerSecondX100 => format!("{:.1} pages/s", value as f64 / 100.0),
        }
    }
}

/// Binary (IEC) units, labelled as such: 1 KiB = 1024 bytes.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let decimals = if unit >= 2 { 2 } else { 1 };
    format!("{value:.decimals$} {}", UNITS[unit])
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub started_at_unix_ms: u64,
    pub finished_at_unix_ms: u64,
    /// Monotonic elapsed time, not a subtraction of wall-clock timestamps.
    pub duration_ms: u64,
}
impl Window {
    pub fn finish(started_at_unix_ms: u64, start: Instant) -> Self {
        Self {
            started_at_unix_ms,
            finished_at_unix_ms: unix_ms(),
            duration_ms: start.elapsed().as_millis() as u64,
        }
    }
    pub fn instant() -> Self {
        let now = unix_ms();
        Self {
            started_at_unix_ms: now,
            finished_at_unix_ms: now,
            duration_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    pub metric: Metric,
    pub entity_ref: String,
    pub value: Option<u64>,
    pub unit: String,
    pub source: String,
    pub window: Window,
    pub status: Status,
    pub origin: Origin,
    pub limitations: Vec<String>,
}
impl Evidence {
    pub fn measured(
        metric: Metric,
        entity: &str,
        value: u64,
        source: &str,
        window: Window,
    ) -> Self {
        Self {
            id: new_id("ev"),
            metric,
            entity_ref: entity.into(),
            value: Some(value),
            unit: metric.unit().into(),
            source: source.into(),
            window,
            status: Status::Ok,
            origin: Origin::Live,
            limitations: Vec::new(),
        }
    }
    pub fn missing(
        metric: Metric,
        entity: &str,
        source: &str,
        status: Status,
        reason: &str,
    ) -> Self {
        Self {
            id: new_id("ev"),
            metric,
            entity_ref: entity.into(),
            value: None,
            unit: metric.unit().into(),
            source: source.into(),
            window: Window::instant(),
            status,
            origin: Origin::Live,
            limitations: vec![reason.into()],
        }
    }
    pub fn usable_value(&self) -> Option<u64> {
        if self.status == Status::Ok {
            self.value
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInstance {
    pub entity_ref: String,
    pub pid: u32,
    pub creation_time_filetime: u64,
    /// Collector-provided boot identity; its source and precision are stated in limitations.
    pub boot_id: Option<String>,
    pub executable_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessCoverage {
    pub status: Status,
    pub enumerated: usize,
    pub readable: usize,
    pub permission_denied: usize,
    pub unavailable: usize,
    pub returned: usize,
    pub enumeration_truncated: bool,
}
impl Default for ProcessCoverage {
    fn default() -> Self {
        Self {
            status: Status::Unsupported,
            enumerated: 0,
            readable: 0,
            permission_denied: 0,
            unavailable: 0,
            returned: 0,
            enumeration_truncated: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub origin: Origin,
    pub evidence: Vec<Evidence>,
    pub processes: Vec<ProcessInstance>,
    pub process_coverage: ProcessCoverage,
    pub limitations: Vec<String>,
}
impl Snapshot {
    pub fn empty(session_id: &str) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            id: new_id("snapshot"),
            session_id: session_id.into(),
            origin: Origin::Live,
            evidence: Vec::new(),
            processes: Vec::new(),
            process_coverage: ProcessCoverage::default(),
            limitations: Vec::new(),
        }
    }
    pub fn system_value(&self, metric: Metric) -> Option<(&Evidence, u64)> {
        self.evidence
            .iter()
            .find(|e| e.entity_ref == "system" && e.metric == metric)
            .and_then(|e| e.usable_value().map(|v| (e, v)))
    }
    pub fn has_observations(&self) -> bool {
        self.evidence.iter().any(|e| e.usable_value().is_some())
    }
}

/// A single inventory observation with its own provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Observed<T> {
    pub value: Option<T>,
    pub source: String,
    pub status: Status,
}
impl<T> Observed<T> {
    pub fn ok(value: T, source: &str) -> Self {
        Self {
            value: Some(value),
            source: source.into(),
            status: Status::Ok,
        }
    }
    pub fn missing(source: &str, status: Status) -> Self {
        Self {
            value: None,
            source: source.into(),
            status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OsVersion {
    pub major: u32,
    pub minor: u32,
    pub build: u32,
    pub update_build_revision: Option<u32>,
    pub display_version: Option<String>,
    /// Inferred from major version and build number, not from the marketing product name.
    pub family: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    NoRootDir,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeInfo {
    pub root: String,
    pub kind: VolumeKind,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
    pub available_to_caller_bytes: Option<u64>,
    pub source: String,
    pub status: Status,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuAdapter {
    pub description: String,
    pub dedicated_video_memory_bytes: Option<u64>,
}

/// Local capability inventory. Contains no serial numbers or machine identifiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareInventory {
    pub schema_version: u32,
    pub id: String,
    pub origin: Origin,
    /// Aggregate of OS, architecture, processors, memory and volumes; GPU is reported separately.
    pub status: Status,
    pub collected_at_unix_ms: u64,
    pub os_version: Observed<OsVersion>,
    pub architecture: Observed<String>,
    pub logical_processors: Observed<u32>,
    pub installed_memory_bytes: Observed<u64>,
    pub volumes: Observed<Vec<VolumeInfo>>,
    pub gpu_adapters: Observed<Vec<GpuAdapter>>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentRelation {
    /// Parent PID was observed and its instance predates the child.
    Observed,
    /// No process with the recorded parent PID was observed; the parent likely exited.
    ParentNotObserved,
    /// The recorded parent PID now belongs to an instance created after the child.
    PidReused,
    /// A creation time needed for validation was unreadable.
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessTreeNode {
    pub pid: u32,
    pub parent_pid: u32,
    pub entity_ref: Option<String>,
    pub creation_time_filetime: Option<u64>,
    pub executable_name: Option<String>,
    pub status: Status,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessEdge {
    pub parent_pid: u32,
    pub child_pid: u32,
    /// Set only when `relation` is `observed`; a reused PID never inherits a parent identity.
    pub parent_ref: Option<String>,
    pub child_ref: Option<String>,
    pub relation: ParentRelation,
}

/// Observed parent relationships. Observation only: never an authorization to act on descendants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessTree {
    pub schema_version: u32,
    pub id: String,
    pub session_id: String,
    pub origin: Origin,
    pub status: Status,
    pub window: Window,
    pub nodes: Vec<ProcessTreeNode>,
    pub edges: Vec<ProcessEdge>,
    pub truncated: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    /// Re-opened and matched on PID plus creation time at lookup.
    Running,
    /// Known to this session but could not be re-validated at lookup.
    Unverified,
    Exited,
    /// The PID now belongs to a different instance.
    PidReused,
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessDetails {
    pub schema_version: u32,
    pub entity_ref: String,
    pub status: Status,
    pub state: InstanceState,
    pub instance: Option<ProcessInstance>,
    /// Latest evidence recorded for this instance; never transferred to a reused PID.
    pub evidence: Vec<Evidence>,
    pub observed_at_unix_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPoint {
    pub unix_ms: u64,
    /// Monotonic milliseconds since the recording collector started.
    pub monotonic_ms: u64,
    pub value: Option<u64>,
    pub status: Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// Nothing was recorded (collector not sampling, or before its first sample).
    NotRecorded,
    /// Samples existed but were evicted by the retention bounds.
    Evicted,
    /// Consecutive samples are farther apart than twice the expected interval.
    MissedSamples,
    BootChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryGap {
    pub from_monotonic_ms: u64,
    pub to_monotonic_ms: u64,
    pub reason: GapReason,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootChange {
    pub at_unix_ms: u64,
    pub previous_boot_id: Option<String>,
    pub boot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retention {
    pub max_samples: usize,
    pub max_age_ms: u64,
    pub expected_interval_ms: u64,
}

/// Samples actually recorded in a bounded window. Values are never interpolated across gaps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricSeries {
    pub schema_version: u32,
    pub id: String,
    pub metric: Metric,
    pub entity_ref: String,
    pub unit: String,
    pub origin: Origin,
    pub status: Status,
    pub requested_window_ms: u64,
    pub window_start_monotonic_ms: u64,
    pub window_end_monotonic_ms: u64,
    pub observed_interval_ms: Option<u64>,
    pub retention: Retention,
    pub points: Vec<MetricPoint>,
    pub gaps: Vec<HistoryGap>,
    pub boot_changes: Vec<BootChange>,
    pub limitations: Vec<String>,
}

/// Calculate an integer ratio without overflowing a 64-bit byte counter.
pub fn ratio_basis_points(numerator: u64, denominator: u64) -> Option<u64> {
    if denominator == 0 {
        return None;
    }
    u64::try_from(u128::from(numerator) * 10_000 / u128::from(denominator)).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_is_null_not_zero() {
        let e = Evidence::missing(
            Metric::PhysicalAvailableBytes,
            "system",
            "test",
            Status::Unsupported,
            "not collected",
        );
        let json = serde_json::to_value(&e).unwrap();
        assert!(json["value"].is_null());
        assert_eq!(json["status"], "unsupported");
    }
    #[test]
    fn zero_is_a_real_measurement() {
        let e = Evidence::measured(
            Metric::SystemCommitBytes,
            "system",
            0,
            "test",
            Window::instant(),
        );
        assert_eq!(e.usable_value(), Some(0));
    }
    #[test]
    fn stale_values_are_not_used() {
        let mut e = Evidence::measured(
            Metric::SystemCommitBytes,
            "system",
            10,
            "test",
            Window::instant(),
        );
        e.status = Status::Stale;
        assert_eq!(e.usable_value(), None);
    }
    #[test]
    fn metric_units_are_explicit() {
        assert_eq!(
            Metric::ProcessCpuBasisPoints.unit(),
            "basis_points_of_total_cpu"
        );
        assert_eq!(Metric::ProcessIoOtherBytes.unit(), "bytes");
        assert_eq!(Metric::DiskReadLatencyMicroseconds.unit(), "microseconds");
        assert_eq!(
            Metric::HardFaultPagesPerSecondX100.unit(),
            "pages_per_second_x100"
        );
        assert_eq!(
            serde_json::to_value(Metric::HardFaultPagesPerSecondX100).unwrap(),
            "hard_fault_pages_per_second_x100"
        );
        let json = serde_json::to_value(Metric::ProcessIoReadBytes).unwrap();
        assert_eq!(json, "process_io_read_bytes");
    }
    #[test]
    fn missing_inventory_fields_are_null() {
        let o: Observed<u64> = Observed::missing("platform", Status::Unsupported);
        let json = serde_json::to_value(&o).unwrap();
        assert!(json["value"].is_null());
        assert_eq!(json["status"], "unsupported");
    }
    #[test]
    fn ratios_are_checked() {
        assert_eq!(ratio_basis_points(1, 0), None);
        assert_eq!(ratio_basis_points(u64::MAX, u64::MAX), Some(10_000));
        assert_eq!(ratio_basis_points(1, 4), Some(2_500));
    }
    #[test]
    fn bytes_round_trip_above_javascript_integer_precision() {
        let e = Evidence::measured(
            Metric::PhysicalTotalBytes,
            "system",
            u64::MAX,
            "test",
            Window::instant(),
        );
        let encoded = serde_json::to_string(&e).unwrap();
        let decoded: Evidence = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded.value, Some(u64::MAX));
    }
    #[test]
    fn utc_dates_are_civil_dates() {
        assert_eq!(format_utc_minute(0), "1970-01-01 00:00 UTC");
        assert_eq!(format_utc_minute(951_782_400_000), "2000-02-29 00:00 UTC");
        assert_eq!(format_utc_minute(1_790_527_240_472), "2026-09-27 16:40 UTC");
        assert_eq!(format_utc_minute(4_102_444_799_000), "2099-12-31 23:59 UTC");
    }
    #[test]
    fn display_values_use_each_metric_unit() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(12_612_919_296), "11.75 GiB");
        assert_eq!(format_bytes(u64::MAX), "16384.00 PiB");
        assert_eq!(Metric::CpuBusyBasisPoints.format_value(1124), "11.2%");
        assert_eq!(Metric::DiskIdleBasisPoints.format_value(9806), "98.1%");
        assert_eq!(
            Metric::DiskWriteLatencyMicroseconds.format_value(191),
            "0.2 ms"
        );
        assert_eq!(Metric::DiskQueueLength.format_value(3), "3");
        assert_eq!(
            Metric::HardFaultPagesPerSecondX100.format_value(1597),
            "16.0 pages/s"
        );
        assert_eq!(
            Metric::ProcessPrivateCommitBytes.format_value(1_536_155_648),
            "1.43 GiB"
        );
    }
}
