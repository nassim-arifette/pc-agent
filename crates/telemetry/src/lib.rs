//! Read-only collectors. Non-Windows targets report unsupported, not fixture data.
use pcagent_shared::{new_id, Snapshot};
#[cfg(windows)]
mod windows;
#[cfg(windows)]
use std::collections::HashMap;

pub struct Collector {
    session_id: String,
    #[cfg(windows)]
    identities: HashMap<(u32, u64), String>,
}
impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}
impl Collector {
    pub fn new() -> Self {
        Self {
            session_id: new_id("session"),
            #[cfg(windows)]
            identities: HashMap::new(),
        }
    }
    /// Samples aggregate CPU over one second and memory at the end of that window.
    /// Process inspection is optional; output is bounded to the top 1..=50 readable processes.
    pub fn snapshot(&mut self, include_processes: bool, limit: usize) -> Snapshot {
        #[cfg(windows)]
        {
            windows::collect(self, include_processes, limit.clamp(1, 50))
        }
        #[cfg(not(windows))]
        {
            use pcagent_shared::{Evidence, Metric, Status};
            let _ = (include_processes, limit);
            let mut result = Snapshot::empty(&self.session_id);
            for metric in [
                Metric::PhysicalTotalBytes,
                Metric::PhysicalAvailableBytes,
                Metric::SystemCommitBytes,
                Metric::SystemCommitLimitBytes,
                Metric::CpuBusyBasisPoints,
            ] {
                result.evidence.push(Evidence::missing(
                    metric,
                    "system",
                    "platform",
                    Status::Unsupported,
                    "Native collection requires Windows; no host or synthetic data substituted.",
                ));
            }
            result
                .limitations
                .push("Windows 11 x64 is the initial product target.".into());
            result
        }
    }
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
    #[cfg(not(windows))]
    #[test]
    fn non_windows_never_masquerades_as_a_windows_sample() {
        let s = Collector::new().snapshot(true, 10);
        assert!(!s.has_observations());
        assert!(s
            .evidence
            .iter()
            .all(|e| e.status == pcagent_shared::Status::Unsupported));
    }
    #[cfg(windows)]
    #[test]
    fn native_memory_and_current_process_smoke_test() {
        use pcagent_shared::Metric;
        let mut c = Collector::new();
        let s = c.snapshot(true, 50);
        let (_, total) = s
            .system_value(Metric::PhysicalTotalBytes)
            .expect("physical memory query");
        assert!(total > 0);
        assert!(s.process_coverage.enumerated > 0);
        assert!(s.process_coverage.readable > 0);
        assert!(s.processes.iter().all(|p| p.creation_time_filetime > 0));
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
    }
}
