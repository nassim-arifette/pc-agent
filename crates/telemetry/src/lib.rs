//! Read-only collectors. Non-Windows targets report unsupported, not fixture data.
#![deny(unsafe_code)]
pub mod counters;
pub mod history;
mod identity;
pub mod tree;
#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

pub use history::{HistoryConfig, MetricHistory, Reading, RecordError, SystemSample};
use identity::{Lookup, Registry};
use pcagent_shared::{
    new_id, unix_ms, HardwareInventory, InstanceState, Metric, MetricSeries, Origin,
    ProcessDetails, ProcessTree, Snapshot, Status, SCHEMA_VERSION,
};
use std::time::{Duration, Instant};

/// Evidence older than this is reported `stale` by `process_details`.
pub const DETAILS_FRESHNESS: Duration = Duration::from_secs(30);

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
            #[cfg(windows)]
            boot_id: windows::boot_id(),
            #[cfg(not(windows))]
            boot_id: None,
            registry: Registry::default(),
            history: MetricHistory::new(config, Origin::Live),
            #[cfg(windows)]
            last_ticks: None,
            #[cfg(windows)]
            pdh: None,
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
        #[cfg(windows)]
        let result = windows::collect(self, include_processes, limit.clamp(1, 50), false);
        #[cfg(not(windows))]
        let result = {
            let _ = (include_processes, limit);
            unsupported_snapshot(&self.session_id)
        };
        self.record(&result);
        result
    }
    /// System-only sample. Reuses the previous CPU tick sample when it is 250 ms to 60 s old,
    /// avoiding the one-second sleep; otherwise behaves like `snapshot(false, _)`.
    pub fn sample_system(&mut self) -> Snapshot {
        #[cfg(windows)]
        let result = windows::collect(self, false, 1, true);
        #[cfg(not(windows))]
        let result = unsupported_snapshot(&self.session_id);
        self.record(&result);
        result
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
        #[cfg(windows)]
        {
            windows::process_tree(self)
        }
        #[cfg(not(windows))]
        {
            ProcessTree {
                schema_version: SCHEMA_VERSION,
                id: new_id("tree"),
                session_id: self.session_id.clone(),
                origin: Origin::Live,
                status: Status::Unsupported,
                window: pcagent_shared::Window::instant(),
                nodes: Vec::new(),
                edges: Vec::new(),
                truncated: false,
                limitations: vec![UNSUPPORTED.into()],
            }
        }
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
                #[cfg(windows)]
                let state = windows::revalidate(key.0, key.1);
                #[cfg(not(windows))]
                let state = InstanceState::Unverified;
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
    /// Local capability inventory without serial numbers or machine identifiers.
    pub fn hardware_inventory(&self) -> HardwareInventory {
        #[cfg(windows)]
        {
            windows::hardware_inventory()
        }
        #[cfg(not(windows))]
        {
            use pcagent_shared::Observed;
            fn m<T>() -> Observed<T> {
                Observed::missing("platform", Status::Unsupported)
            }
            HardwareInventory {
                schema_version: SCHEMA_VERSION,
                id: new_id("inventory"),
                origin: Origin::Live,
                status: Status::Unsupported,
                collected_at_unix_ms: unix_ms(),
                os_version: m(),
                architecture: m(),
                logical_processors: m(),
                installed_memory_bytes: m(),
                volumes: m(),
                gpu_adapters: m(),
                limitations: vec![UNSUPPORTED.into()],
            }
        }
    }
}

#[cfg(not(windows))]
const UNSUPPORTED: &str =
    "Native collection requires Windows; no host or synthetic data substituted.";

#[cfg(not(windows))]
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
    #[cfg(not(windows))]
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
        assert!(s
            .evidence
            .iter()
            .filter(|e| e.metric == Metric::ProcessCpuBasisPoints)
            .all(|e| e.value.is_none() && e.status == Status::Partial));
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
