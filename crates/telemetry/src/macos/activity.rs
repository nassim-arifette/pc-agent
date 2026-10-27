//! Fresh per-process activity with Darwin semantics and identity-stable deltas.
use super::{collect_processes, native};
use crate::{ActivityProcess, ApplicationActivity, Collector, ProcessActivity, ProcessSort};
use pcagent_shared::{new_id, unix_ms, Evidence, Metric, Snapshot, Status, Window};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

pub(crate) fn collect(collector: &mut Collector, seconds: u64, limit: usize) -> ProcessActivity {
    let mut before = Snapshot::empty(&collector.session_id);
    collect_processes(collector, &mut before, usize::MAX, ProcessSort::Cpu);
    let clock = Instant::now();
    let start = unix_ms();
    std::thread::sleep(Duration::from_secs(seconds));
    let mut after = Snapshot::empty(&collector.session_id);
    collect_processes(collector, &mut after, usize::MAX, ProcessSort::Cpu);
    let mut out = ProcessActivity::unavailable(&collector.session_id, Status::Partial,
        "Darwin libproc disk bytes exclude cache-only and network I/O. Per-process GPU and Windows private commit have no validated source. Application groups are individual processes, not inferred executable families.");
    out.window = Window::finish(start, clock);
    out.coverage.processes = after.process_coverage.clone();
    out.coverage.services_status = Status::Unsupported;
    out.coverage.gpu_status = Status::Unsupported;
    let previous: HashMap<_, _> = before
        .processes
        .iter()
        .map(|p| ((p.pid, p.creation_time_filetime), p))
        .collect();
    let current: HashMap<_, _> = after
        .processes
        .iter()
        .map(|p| ((p.pid, p.creation_time_filetime), p))
        .collect();
    out.coverage.stable_instances = current.keys().filter(|k| previous.contains_key(k)).count();
    out.coverage.new_instances = current.len() - out.coverage.stable_instances;
    out.coverage.exited_instances = previous.keys().filter(|k| !current.contains_key(k)).count();
    let by_pid: HashMap<_, _> = after.processes.iter().map(|p| (p.pid, p)).collect();
    for p in after.processes.iter().take(limit) {
        let metadata = match native::process(p.pid) {
            Ok(row) if row.created == p.creation_time_filetime => row,
            _ => {
                out.coverage.processes.unavailable += 1;
                continue;
            }
        };
        let app = new_id("application");
        let parent = by_pid
            .get(&metadata.parent)
            .filter(|parent| parent.creation_time_filetime <= p.creation_time_filetime);
        out.processes.push(ActivityProcess {
            instance: p.clone(),
            parent_ref: parent.map(|p| p.entity_ref.clone()),
            application_ref: app.clone(),
            role: metadata.role,
            handles: metadata.files,
            threads: metadata.threads,
            session: metadata.session,
            services: vec![],
        });
        out.applications.push(ApplicationActivity {
            entity_ref: app.clone(),
            name: p.executable_name.clone(),
            process_refs: vec![p.entity_ref.clone()],
            process_count: 1,
            members_truncated: false,
        });
        let mut evidence: Vec<_> = after
            .evidence
            .iter()
            .filter(|e| e.entity_ref == p.entity_ref)
            .cloned()
            .collect();
        let old = previous
            .get(&(p.pid, p.creation_time_filetime))
            .and_then(|old| {
                before
                    .evidence
                    .iter()
                    .find(|e| {
                        e.entity_ref == old.entity_ref && e.metric == Metric::ProcessWorkingSetBytes
                    })?
                    .usable_value()
            });
        let current = evidence
            .iter()
            .find(|e| e.metric == Metric::ProcessWorkingSetBytes)
            .and_then(|e| e.usable_value());
        for (metric, value) in [
            (
                Metric::ProcessWorkingSetIncreaseBytes,
                old.zip(current).map(|(a, b)| b.saturating_sub(a)),
            ),
            (
                Metric::ProcessWorkingSetDecreaseBytes,
                old.zip(current).map(|(a, b)| a.saturating_sub(b)),
            ),
        ] {
            evidence.push(match value {
                Some(v) => Evidence::measured(
                    metric,
                    &p.entity_ref,
                    v,
                    "libproc.PROC_PIDTASKINFO",
                    out.window.clone(),
                ),
                None => Evidence::missing(
                    metric,
                    &p.entity_ref,
                    "libproc.PROC_PIDTASKINFO",
                    Status::Partial,
                    "Memory change requires the same instance in both samples.",
                ),
            });
        }
        for metric in [
            Metric::ProcessGpuBasisPoints,
            Metric::ProcessPrivateCommitIncreaseBytes,
            Metric::ProcessPrivateCommitDecreaseBytes,
        ] {
            evidence.push(Evidence::missing(
                metric,
                &p.entity_ref,
                "Darwin",
                Status::Unsupported,
                "No validated equivalent counter.",
            ));
        }
        for (bytes, rate) in [
            (
                Metric::ProcessIoReadBytes,
                Metric::ProcessIoReadBytesPerSecond,
            ),
            (
                Metric::ProcessIoWriteBytes,
                Metric::ProcessIoWriteBytesPerSecond,
            ),
        ] {
            if let Some(source) = evidence.iter().find(|e| e.metric == bytes) {
                let value = source
                    .usable_value()
                    .and_then(|v| v.checked_mul(1000)?.checked_div(source.window.duration_ms));
                evidence.push(match value {
                    Some(v) => Evidence::measured(
                        rate,
                        &p.entity_ref,
                        v,
                        "libproc.RUSAGE_INFO_V2",
                        source.window.clone(),
                    ),
                    None => Evidence::missing(
                        rate,
                        &p.entity_ref,
                        "libproc.RUSAGE_INFO_V2",
                        Status::Partial,
                        "Rate requires a stable instance and nonzero window.",
                    ),
                });
            }
        }
        out.evidence.extend(evidence.iter().cloned().map(|mut e| {
            e.id = new_id("ev");
            e.entity_ref = app.clone();
            e
        }));
        out.evidence.extend(evidence);
    }
    out.coverage.processes.returned = out.processes.len();
    if out.coverage.processes.unavailable > 0 {
        out.coverage.processes.status = Status::Partial;
    }
    out.coverage.applications_total = current.len();
    out.coverage.applications_returned = out.applications.len();
    out.limitations.push("handles denotes open file descriptors and session denotes the POSIX session ID. Role is a local heuristic from application bundle/closed argument markers, falling back to system-vs-user UID; no arguments or UID are retained.".into());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn native_activity_has_real_rates_and_no_invented_commit_or_gpu() {
        let mut collector = Collector::new();
        let result = collector.process_activity(1, usize::MAX);
        assert!(result.window.duration_ms >= 1_000);
        assert!(result.coverage.processes.enumerated > 0);
        let process = result
            .processes
            .iter()
            .find(|p| p.instance.pid == std::process::id())
            .unwrap();
        assert!(process.threads > 0);
        let own = &process.instance.entity_ref;
        assert!(result.evidence.iter().any(|e| e.entity_ref == *own
            && e.metric == Metric::ProcessCpuBasisPoints
            && e.usable_value().is_some()));
        assert!(result
            .evidence
            .iter()
            .filter(|e| matches!(
                e.metric,
                Metric::ProcessPrivateCommitBytes | Metric::ProcessGpuBasisPoints
            ))
            .all(|e| e.value.is_none() && e.status == Status::Unsupported));
        let ids: HashSet<_> = result.evidence.iter().map(|e| &e.id).collect();
        assert_eq!(ids.len(), result.evidence.len());
        assert_eq!(result.coverage.processes.returned, result.processes.len());
        assert!(result.processes.iter().all(|p| result
            .applications
            .iter()
            .any(|app| app.entity_ref == p.application_ref)));
    }
}
