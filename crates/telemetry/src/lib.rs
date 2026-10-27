//! Read-only collectors. Non-Windows targets report unsupported, not fixture data.
#![deny(unsafe_code)]
pub mod activity;
pub mod counters;
pub mod gpu;
pub mod history;
mod identity;
mod os;
mod program_names;
pub mod services;
pub mod tree;
#[cfg(windows)]
use os::windows;

pub use activity::{
    ActivityCoverage, ActivityProcess, ApplicationActivity, ProcessActivity, ProcessRole,
    DEFAULT_ACTIVITY_WINDOW_SECONDS,
};
pub use history::{HistoryConfig, MetricHistory, Reading, RecordError, SystemSample};
use identity::{Lookup, Registry};
use os::Backend;
use pcagent_shared::{
    new_id, unix_ms, HardwareInventory, InstanceState, Metric, MetricSeries, Origin,
    ProcessDetails, ProcessInstance, ProcessTree, Snapshot, Status, SCHEMA_VERSION,
};
pub use program_names::ProgramNames;
pub use services::{service_processes, ServiceProcess, ServiceProcesses};
use std::time::{Duration, Instant};

/// Local measurement of this sampler thread's CPU time and the host process's working set.
/// The working set covers the whole process, not memory attributable to the sampler alone.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ResourceUsage {
    pub thread_cpu_100ns: Option<u64>,
    pub process_working_set_bytes: Option<u64>,
    /// Active logical processors across all processor groups, for total-capacity percentages.
    pub logical_processors: Option<u32>,
}

pub fn current_resource_usage() -> ResourceUsage {
    os::Selected::resource_usage()
}

/// Evidence older than this is reported `stale` by `process_details`.
pub const DETAILS_FRESHNESS: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProcessSort {
    #[default]
    WorkingSet,
    PrivateCommit,
    Cpu,
    IoRead,
    IoWrite,
}
impl ProcessSort {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "working_set" => Some(Self::WorkingSet),
            "private_commit" => Some(Self::PrivateCommit),
            "cpu" => Some(Self::Cpu),
            "io_read" => Some(Self::IoRead),
            "io_write" => Some(Self::IoWrite),
            _ => None,
        }
    }
    pub fn metric(self) -> Metric {
        match self {
            Self::WorkingSet => Metric::ProcessWorkingSetBytes,
            Self::PrivateCommit => Metric::ProcessPrivateCommitBytes,
            Self::Cpu => Metric::ProcessCpuBasisPoints,
            Self::IoRead => Metric::ProcessIoReadBytes,
            Self::IoWrite => Metric::ProcessIoWriteBytes,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::WorkingSet => "working set",
            Self::PrivateCommit => "private commit",
            Self::Cpu => "CPU",
            Self::IoRead => "read I/O rate",
            Self::IoWrite => "write I/O rate",
        }
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    fn rank(self, evidence: &[pcagent_shared::Evidence]) -> Option<u128> {
        let e = evidence.iter().find(|e| e.metric == self.metric())?;
        let value = u128::from(e.usable_value()?);
        if matches!(self, Self::IoRead | Self::IoWrite) {
            (e.window.duration_ms > 0).then(|| value * 1000 / u128::from(e.window.duration_ms))
        } else {
            Some(value)
        }
    }
}

#[cfg(windows)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct TickSample {
    ticks: [u64; 3],
    at: Duration,
    unix_ms: u64,
}

pub struct Collector {
    session_id: String,
    started: Instant,
    boot_id: Option<String>,
    registry: Registry,
    history: MetricHistory,
    #[cfg(windows)]
    last_ticks: Option<TickSample>,
    #[cfg(windows)]
    pdh: Option<Result<windows::pdh::Pdh, counters::Unavailable>>,
    #[cfg(windows)]
    recording_gpu_baseline: Option<windows::RecordingGpuBaseline>,
    /// NVML, opened on the first snapshot and shut down with the collector.
    #[cfg(windows)]
    nvml: Option<Result<windows::nvml::Nvml, windows::nvml::OpenError>>,
    #[cfg(target_os = "linux")]
    linux: os::linux::State,
}
impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}
impl Collector {
    pub fn new() -> Self {
        Self::with_history(HistoryConfig::default())
    }
    pub fn with_history(config: HistoryConfig) -> Self {
        Self {
            session_id: new_id("session"),
            started: Instant::now(),
            boot_id: os::Selected::boot_id(),
            registry: Registry::default(),
            history: MetricHistory::new(config, Origin::Live),
            #[cfg(windows)]
            last_ticks: None,
            #[cfg(windows)]
            pdh: None,
            #[cfg(windows)]
            recording_gpu_baseline: None,
            #[cfg(windows)]
            nvml: None,
            #[cfg(target_os = "linux")]
            linux: os::linux::State::default(),
        }
    }
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    /// Estimated boot identity, fixed for the collector's lifetime; see `docs/telemetry.md`.
    pub fn boot_id(&self) -> Option<&str> {
        self.boot_id.as_deref()
    }
    /// Samples aggregate CPU over one second and memory at the end of that window.
    /// Process inspection is optional; output is bounded to the top 1..=50 readable processes.
    /// Per-process CPU and I/O are deltas since this collector's previous observation of the same instance.
    pub fn snapshot(&mut self, include_processes: bool, limit: usize) -> Snapshot {
        self.snapshot_sorted(include_processes, limit, ProcessSort::WorkingSet)
    }
    pub fn snapshot_sorted(
        &mut self,
        include_processes: bool,
        limit: usize,
        sort: ProcessSort,
    ) -> Snapshot {
        let result =
            os::Selected::collect(self, include_processes, limit.clamp(1, 50), false, sort);
        self.record(&result);
        result
    }
    /// System-only sample. Reuses the previous CPU tick sample when it is 250 ms to 60 s old,
    /// avoiding the one-second sleep; otherwise behaves like `snapshot(false, _)`.
    pub fn sample_system(&mut self) -> Snapshot {
        let result = os::Selected::collect(self, false, 1, true, ProcessSort::WorkingSet);
        self.record(&result);
        result
    }
    /// Background sample with bounded process rows. The first call warms up CPU/PDH counters;
    /// later calls reuse their previous readings and do not sleep for a measurement window.
    pub fn sample_processes(&mut self, limit: usize, sort: ProcessSort) -> Snapshot {
        let result = os::Selected::collect(self, true, limit.clamp(1, 50), true, sort);
        self.record(&result);
        result
    }
    /// Recording sample that reuses the existing PDH window for bounded process GPU evidence.
    /// Warm-up and unverifiable identities stay unknown; there is no extra measurement sleep.
    pub fn sample_recording_processes(&mut self, limit: usize, sort: ProcessSort) -> Snapshot {
        let result = os::Selected::collect_recording(self, limit.clamp(1, 50), sort);
        self.record(&result);
        result
    }
    /// Start a recording without lending its first GPU window a prior recording's identities.
    /// Existing CPU/PDH counters remain warm; only the local attribution anchor is cleared.
    pub fn reset_recording_gpu(&mut self) {
        #[cfg(windows)]
        {
            self.recording_gpu_baseline = None;
        }
    }
    /// Fresh CPU, memory growth, I/O and GPU observations for processes and applications.
    /// The requested window is bounded to 1..=30 seconds; callers default to 3 seconds.
    pub fn process_activity(&mut self, window_seconds: u64, limit: usize) -> ProcessActivity {
        os::Selected::activity(
            self,
            window_seconds.clamp(1, activity::MAX_ACTIVITY_WINDOW_SECONDS),
            limit.clamp(1, 50),
        )
    }
    fn record(&mut self, snapshot: &Snapshot) {
        let sample = SystemSample::from_snapshot(
            snapshot,
            unix_ms(),
            self.monotonic_ms(),
            self.boot_id.clone(),
        );
        let _ = self.history.record(sample);
    }
    fn monotonic_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
    pub fn history(&self) -> &MetricHistory {
        &self.history
    }
    /// System metric samples recorded by this collector over a bounded trailing window.
    pub fn metric_history(&self, metric: Metric, window: Duration) -> MetricSeries {
        self.history.query(metric, window, self.monotonic_ms())
    }
    /// Observed parent relations, validated by creation time. Observation, never authorization.
    pub fn process_tree(&mut self) -> ProcessTree {
        os::Selected::process_tree(self)
    }
    /// Looks up an instance by an opaque `entity_ref` issued by this collector; bare PIDs never match.
    pub fn process_details(&mut self, entity_ref: &str) -> ProcessDetails {
        let (state, tracked) = match self.registry.lookup(entity_ref) {
            Lookup::NotFound => {
                return ProcessDetails {
                    schema_version: SCHEMA_VERSION,
                    entity_ref: entity_ref.into(),
                    status: Status::Error,
                    state: InstanceState::NotFound,
                    instance: None,
                    evidence: Vec::new(),
                    observed_at_unix_ms: None,
                    age_ms: None,
                    limitations: vec!["Unknown entity_ref: only references issued by this collector session are accepted, never bare PIDs.".into()],
                }
            }
            Lookup::Retired(r) => (r.state, r.tracked.clone()),
            Lookup::Live(key, t) => {
                let tracked = t.clone();
                let state = os::Selected::revalidate(key.0, key.1);
                if matches!(state, InstanceState::Exited | InstanceState::PidReused) {
                    self.registry.retire(key, state);
                }
                (state, tracked)
            }
        };
        let age_ms = tracked
            .observed_unix_ms
            .map(|t| unix_ms().saturating_sub(t));
        let fresh = age_ms.is_some_and(|a| u128::from(a) <= DETAILS_FRESHNESS.as_millis());
        let (status, note) = match state {
            InstanceState::Running if fresh => (Status::Ok, None),
            InstanceState::Running => (
                Status::Stale,
                Some("Instance is running but its evidence is older than the freshness bound; take a new snapshot."),
            ),
            InstanceState::Unverified => (
                Status::Partial,
                Some("Instance could not be re-validated at lookup; evidence may be stale."),
            ),
            InstanceState::Exited => (
                Status::Stale,
                Some("Instance is no longer running; evidence is historical."),
            ),
            _ => (
                Status::Stale,
                Some("PID now belongs to a different instance; evidence belongs only to the original instance."),
            ),
        };
        let mut limitations: Vec<String> = note.into_iter().map(str::to_owned).collect();
        limitations.push("Details are an observation and never authorize an action.".into());
        ProcessDetails {
            schema_version: SCHEMA_VERSION,
            entity_ref: entity_ref.into(),
            status,
            state,
            instance: Some(tracked.instance),
            evidence: tracked.evidence,
            observed_at_unix_ms: tracked.observed_unix_ms,
            age_ms,
            limitations,
        }
    }
    /// The instance behind a reference this collector issued, live or retired, straight from
    /// its registry: no system call and no revalidation. For local display lookups only.
    pub fn instance(&self, entity_ref: &str) -> Option<&ProcessInstance> {
        match self.registry.lookup(entity_ref) {
            Lookup::Live(_, tracked) => Some(&tracked.instance),
            Lookup::Retired(retired) => Some(&retired.tracked.instance),
            Lookup::NotFound => None,
        }
    }
    /// Local capability inventory without serial numbers or machine identifiers.
    pub fn hardware_inventory(&self) -> HardwareInventory {
        os::Selected::hardware_inventory()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
const UNSUPPORTED: &str =
    "Native collection requires Windows; no host or synthetic data substituted.";

#[cfg(not(any(windows, target_os = "linux")))]
fn unsupported_snapshot(session_id: &str) -> Snapshot {
    use pcagent_shared::Evidence;
    let mut result = Snapshot::empty(session_id);
    for metric in [
        Metric::PhysicalTotalBytes,
        Metric::PhysicalAvailableBytes,
        Metric::SystemCommitBytes,
        Metric::SystemCommitLimitBytes,
        Metric::CpuBusyBasisPoints,
    ]
    .into_iter()
    .chain(counters::COUNTER_METRICS)
    {
        result.evidence.push(Evidence::missing(
            metric,
            "system",
            "platform",
            Status::Unsupported,
            UNSUPPORTED,
        ));
    }
    result
        .limitations
        .push("Windows 11 x64 is the initial product target.".into());
    result
}

/// GetSystemTimes includes idle time in kernel time. Counter regressions are unknown.
pub fn cpu_busy_basis_points(before: [u64; 3], after: [u64; 3]) -> Option<u64> {
    let idle = after[0].checked_sub(before[0])?;
    let kernel = after[1].checked_sub(before[1])?;
    let user = after[2].checked_sub(before[2])?;
    let total = kernel.checked_add(user)?;
    let busy = total.checked_sub(idle)?;
    pcagent_shared::ratio_basis_points(busy, total)
}

/// Process kernel+user time (100 ns units) as basis points of all logical processors over
/// `elapsed`. Timer granularity may overshoot capacity slightly (≤5 %, clamped); larger
/// overshoots, regressions and empty windows are unknown.
pub fn process_cpu_basis_points(
    before_100ns: u64,
    after_100ns: u64,
    elapsed: Duration,
    logical_processors: u32,
) -> Option<u64> {
    let busy = after_100ns.checked_sub(before_100ns)?;
    let elapsed_100ns = u64::try_from(elapsed.as_nanos() / 100).ok()?;
    let capacity = elapsed_100ns.checked_mul(u64::from(logical_processors))?;
    match pcagent_shared::ratio_basis_points(busy, capacity)? {
        v @ 0..=10_000 => Some(v),
        10_001..=10_500 => Some(10_000),
        _ => None,
    }
}

/// Inferred from the kernel version, not the registry product name (which says Windows 10 on 11).
pub fn windows_family(major: u32, build: u32) -> Option<&'static str> {
    match (major, build) {
        (10, 22_000..) => Some("windows_11"),
        (10, _) => Some("windows_10"),
        _ => None,
    }
}

/// Wall-clock boot estimate rounded to one minute: clock changes shift it, so it is labelled.
pub fn boot_estimate_id(now_unix_ms: u64, uptime_ms: u64) -> Option<String> {
    let boot = now_unix_ms.checked_sub(uptime_ms)?;
    Some(format!("boot-estimate-unix-s-{}", boot / 60_000 * 60))
}

/// Splits a double-NUL-terminated UTF-16 list such as `GetLogicalDriveStringsW` output.
pub fn parse_multi_sz(buffer: &[u16]) -> Vec<String> {
    buffer
        .split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resource_ranking_keeps_cpu_and_commit_contributors_and_normalizes_io() {
        use pcagent_shared::{Evidence, Window};
        let row = |working_set, commit, cpu, io, duration_ms| {
            vec![
                Evidence::measured(
                    Metric::ProcessWorkingSetBytes,
                    "proc-fixture",
                    working_set,
                    "fixture",
                    Window::instant(),
                ),
                Evidence::measured(
                    Metric::ProcessPrivateCommitBytes,
                    "proc-fixture",
                    commit,
                    "fixture",
                    Window::instant(),
                ),
                Evidence::measured(
                    Metric::ProcessCpuBasisPoints,
                    "proc-fixture",
                    cpu,
                    "fixture",
                    Window::instant(),
                ),
                Evidence::measured(
                    Metric::ProcessIoReadBytes,
                    "proc-fixture",
                    io,
                    "fixture",
                    Window {
                        started_at_unix_ms: 0,
                        finished_at_unix_ms: duration_ms,
                        duration_ms,
                    },
                ),
            ]
        };
        let memory_heavy = row(10_000, 2_000, 10, 2_000, 10_000);
        let cpu_heavy = row(100, 20_000, 9_000, 1_000, 1_000);
        assert!(
            ProcessSort::WorkingSet.rank(&memory_heavy) > ProcessSort::WorkingSet.rank(&cpu_heavy)
        );
        for sort in [
            ProcessSort::PrivateCommit,
            ProcessSort::Cpu,
            ProcessSort::IoRead,
        ] {
            assert!(sort.rank(&cpu_heavy) > sort.rank(&memory_heavy));
            assert!(sort.rank(&memory_heavy) > sort.rank(&[]));
        }
        assert_eq!(ProcessSort::IoRead.rank(&row(0, 0, 0, 100, 0)), None);
    }
    #[test]
    fn idle_is_not_double_counted() {
        assert_eq!(cpu_busy_basis_points([0, 0, 0], [50, 80, 20]), Some(5_000));
    }
    #[test]
    fn counter_regressions_and_zero_windows_are_unknown() {
        assert_eq!(cpu_busy_basis_points([2, 2, 2], [1, 3, 3]), None);
        assert_eq!(cpu_busy_basis_points([1, 1, 1], [1, 1, 1]), None);
        assert_eq!(cpu_busy_basis_points([0, 0, 0], [10, 1, 1]), None);
    }
    #[test]
    fn process_cpu_is_a_share_of_total_capacity() {
        let s = Duration::from_secs(1);
        assert_eq!(process_cpu_basis_points(0, 10_000_000, s, 4), Some(2_500));
        assert_eq!(process_cpu_basis_points(0, 40_000_000, s, 4), Some(10_000));
        assert_eq!(process_cpu_basis_points(5, 5, s, 8), Some(0));
        assert_eq!(process_cpu_basis_points(0, 41_000_000, s, 4), Some(10_000));
    }
    #[test]
    fn process_cpu_regressions_empty_windows_and_overshoot_are_unknown() {
        let s = Duration::from_secs(1);
        assert_eq!(process_cpu_basis_points(10, 9, s, 4), None);
        assert_eq!(process_cpu_basis_points(0, 1, Duration::ZERO, 4), None);
        assert_eq!(process_cpu_basis_points(0, 1, s, 0), None);
        assert_eq!(process_cpu_basis_points(0, 50_000_000, s, 4), None);
        assert_eq!(
            process_cpu_basis_points(0, u64::MAX, Duration::MAX, u32::MAX),
            None
        );
    }
    #[test]
    fn windows_family_uses_build_numbers() {
        assert_eq!(windows_family(10, 22_631), Some("windows_11"));
        assert_eq!(windows_family(10, 19_045), Some("windows_10"));
        assert_eq!(windows_family(6, 9_600), None);
    }
    #[test]
    fn boot_estimate_is_rounded_and_checked() {
        assert_eq!(
            boot_estimate_id(1_700_000_059_999, 59_999).as_deref(),
            Some("boot-estimate-unix-s-1699999980")
        );
        assert_eq!(boot_estimate_id(5, 10), None);
    }
    #[test]
    fn multi_sz_parsing_stops_at_double_nul() {
        let buf: Vec<u16> = "C:\\\0D:\\\0\0garbage\0".encode_utf16().collect();
        assert_eq!(parse_multi_sz(&buf), ["C:\\", "D:\\"]);
        assert!(parse_multi_sz(&[0, 0]).is_empty());
    }
    #[test]
    fn details_reject_bare_pids_and_unknown_refs() {
        let mut c = Collector::new();
        let d = c.process_details("4");
        assert_eq!(d.state, InstanceState::NotFound);
        assert_eq!(d.status, Status::Error);
        assert!(d.instance.is_none());
    }
    #[test]
    fn retired_details_are_stale_and_never_transferred() {
        let mut c = Collector::new();
        let old = c.registry.entry(10, 100, None).instance.entity_ref.clone();
        c.registry.entry(10, 200, None);
        c.registry.sweep(
            &[(10, 200)].into_iter().collect(),
            &[10].into_iter().collect(),
        );
        let d = c.process_details(&old);
        assert_eq!(d.state, InstanceState::PidReused);
        assert_eq!(d.status, Status::Stale);
        assert_eq!(d.instance.unwrap().creation_time_filetime, 100);
    }
    #[test]
    fn history_records_every_snapshot() {
        let mut c = Collector::new();
        c.sample_system();
        c.sample_system();
        assert_eq!(c.history().len(), 2);
        let s = c.metric_history(Metric::PhysicalTotalBytes, Duration::from_secs(60));
        assert_eq!(s.points.len(), 2);
        assert_eq!(s.origin, Origin::Live);
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    #[test]
    fn non_windows_never_masquerades_as_a_windows_sample() {
        let mut c = Collector::new();
        let s = c.snapshot(true, 10);
        assert!(!s.has_observations());
        assert!(s.evidence.iter().all(|e| e.status == Status::Unsupported));
        assert_eq!(c.process_tree().status, Status::Unsupported);
        let inv = c.hardware_inventory();
        assert_eq!(inv.status, Status::Unsupported);
        assert!(inv.installed_memory_bytes.value.is_none());
        assert_eq!(
            c.metric_history(Metric::CpuBusyBasisPoints, Duration::from_secs(60))
                .status,
            Status::Unsupported
        );
        assert!(c.boot_id().is_none());
    }
    #[cfg(windows)]
    #[test]
    fn native_memory_and_current_process_smoke_test() {
        let mut c = Collector::new();
        let s = c.snapshot(true, 50);
        let (_, total) = s
            .system_value(Metric::PhysicalTotalBytes)
            .expect("physical memory query");
        assert!(total > 0);
        assert!(s.process_coverage.enumerated > 0);
        assert!(s.process_coverage.readable > 0);
        assert!(s.processes.iter().all(|p| p.creation_time_filetime > 0));
        // Every process comes from the system list: none is refused, even without admin.
        assert_eq!(s.process_coverage.permission_denied, 0);
        assert_eq!(s.process_coverage.readable, s.process_coverage.enumerated);
        // The readings taken at the start of the one-second window give rates immediately.
        let first_rates: Vec<_> = s
            .evidence
            .iter()
            .filter(|e| e.metric == Metric::ProcessCpuBasisPoints)
            .filter_map(|e| e.usable_value().map(|v| (v, e.window.duration_ms)))
            .collect();
        assert!(!first_rates.is_empty());
        assert!(first_rates.iter().all(|(v, ms)| *v <= 10_000 && *ms >= 900));
        let s2 = c.snapshot(true, 50);
        for p in &s.processes {
            if let Some(q) = s2
                .processes
                .iter()
                .find(|q| q.pid == p.pid && q.creation_time_filetime == p.creation_time_filetime)
            {
                assert_eq!(p.entity_ref, q.entity_ref);
            }
        }
        assert!(s2
            .evidence
            .iter()
            .any(|e| e.metric == Metric::ProcessCpuBasisPoints && e.usable_value().is_some()));
        assert!(s2
            .evidence
            .iter()
            .filter_map(|e| (e.metric == Metric::ProcessCpuBasisPoints).then_some(e.value))
            .flatten()
            .all(|v| v <= 10_000));
    }
    #[cfg(windows)]
    #[test]
    fn native_counters_smoke_test_tolerates_unsupported() {
        let mut c = Collector::new();
        let s = c.snapshot(false, 1);
        for m in counters::COUNTER_METRICS {
            let e = s
                .evidence
                .iter()
                .find(|e| e.metric == m && e.entity_ref == "system")
                .unwrap_or_else(|| panic!("{m:?} reported"));
            assert_eq!(e.unit, m.unit());
            match e.status {
                Status::Ok | Status::Partial if e.value.is_some() => {
                    assert!(e.window.duration_ms >= 900 || m == Metric::DiskQueueLength);
                }
                Status::Ok => panic!("{m:?} ok without a value"),
                _ => assert!(e.value.is_none(), "{m:?}"),
            }
            assert_ne!(e.status, Status::Error, "{m:?}: {:?}", e.limitations);
        }
        assert!(s
            .evidence
            .iter()
            .filter(|e| e.metric == Metric::GpuEngineBasisPoints)
            .all(
                |e| e.entity_ref.starts_with("gpu-engine-") && e.value.is_some_and(|v| v <= 10_000)
            ));
        let text = serde_json::to_string(&s.evidence).unwrap();
        assert!(!text.contains("luid") && !text.contains("pid_"));
    }
    #[cfg(windows)]
    #[test]
    fn native_nvml_reports_each_nvidia_metric_or_nothing_without_the_driver() {
        use pcagent_shared::NVML_SLOWDOWN_MASK;
        let mut c = Collector::new();
        let s = c.snapshot(false, 1);
        let nvidia: Vec<_> = s
            .evidence
            .iter()
            .filter(|e| e.entity_ref.starts_with("gpu-nvidia-"))
            .collect();
        let windows = std::env::var_os("WINDIR").expect("WINDIR");
        if !std::path::Path::new(&windows)
            .join(r"System32\nvml.dll")
            .exists()
        {
            assert!(nvidia.is_empty() && s.gpus.is_empty());
            assert!(!s.limitations.iter().any(|l| l.contains("NVML")));
            return;
        }
        if s.gpus.is_empty() {
            // Library present but no usable device: the reason is stated, nothing invented.
            assert!(nvidia.is_empty());
            return;
        }
        for gpu in &s.gpus {
            assert!(gpu.entity_ref.starts_with("gpu-nvidia-"));
            assert_eq!(gpu.source, "NVML");
            let value = |metric: Metric| {
                let found: Vec<_> = nvidia
                    .iter()
                    .filter(|e| e.metric == metric && e.entity_ref == gpu.entity_ref)
                    .collect();
                assert_eq!(found.len(), 1, "{metric:?} once per device");
                assert_eq!(found[0].unit, metric.unit());
                found[0].usable_value()
            };
            if let Some(total) = value(Metric::GpuMemoryTotalBytes) {
                assert!(total > 0);
                assert!(value(Metric::GpuMemoryUsedBytes).is_none_or(|used| used <= total));
            }
            assert!(value(Metric::GpuTemperatureCelsius).is_none_or(|t| t <= 150));
            assert!(value(Metric::GpuUtilizationBasisPoints).is_none_or(|u| u <= 10_000));
            assert!(value(Metric::GpuSlowdownReasons).is_none_or(|b| b & !NVML_SLOWDOWN_MASK == 0));
            value(Metric::GpuSmClockMegahertz);
            value(Metric::GpuPowerMilliwatts);
        }
        assert!(s.limitations.iter().any(|l| l.contains("NVML")));
        // The second snapshot reuses the open library.
        assert_eq!(c.snapshot(false, 1).gpus.len(), s.gpus.len());
    }
    #[cfg(windows)]
    #[test]
    fn native_details_tree_inventory_and_fast_sample_smoke_test() {
        let mut c = Collector::new();
        assert!(c.boot_id().is_some_and(|b| b.starts_with("boot-estimate-")));
        let s = c.snapshot(true, 50);
        let me = std::process::id();
        let tree = c.process_tree();
        assert!(!tree.nodes.is_empty());
        let node = tree
            .nodes
            .iter()
            .find(|n| n.pid == me)
            .expect("self in tree");
        let entity = node.entity_ref.clone().expect("own creation time readable");
        let d = c.process_details(&entity);
        assert_eq!(d.state, InstanceState::Running);
        assert_eq!(d.instance.unwrap().pid, me);
        if let Some(p) = s.processes.first() {
            assert_ne!(
                c.process_details(&p.pid.to_string()).state,
                InstanceState::Running
            );
        }
        let inv = c.hardware_inventory();
        assert!(inv.logical_processors.value.is_some_and(|n| n > 0));
        assert!(inv.os_version.value.is_some_and(|v| v.build > 0));
        assert!(inv.architecture.value.is_some());
        assert_eq!(inv.gpu_adapters.status, Status::Unsupported);
        let volumes = inv.volumes.value.expect("volumes");
        assert!(volumes
            .iter()
            .any(|v| v.kind == pcagent_shared::VolumeKind::Fixed && v.total_bytes.is_some()));
        std::thread::sleep(Duration::from_millis(300));
        let clock = Instant::now();
        let fast = c.sample_system();
        assert!(clock.elapsed() < Duration::from_millis(900));
        assert!(fast.system_value(Metric::CpuBusyBasisPoints).is_some());
        assert!(
            c.metric_history(Metric::PhysicalTotalBytes, Duration::from_secs(60))
                .points
                .len()
                >= 2
        );
    }
}
