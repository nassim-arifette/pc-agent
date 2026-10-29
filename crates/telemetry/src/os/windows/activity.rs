//! Fresh, synchronous two-sample activity window, bounded by the public collector.
use super::{pdh, processor_count, sysproc};
use crate::{
    activity::{assemble, Collection, ProcessReading},
    counters::{Instances, Sampled, Unavailable},
    service_processes, Collector, ProcessActivity,
};
use pcagent_shared::{unix_ms, Status, Window};
use std::time::{Duration, Instant};

fn processes() -> Result<Vec<ProcessReading>, Status> {
    sysproc::query().map(|rows| {
        rows.into_iter()
            .map(|p| ProcessReading {
                browser: Default::default(),
                pid: p.pid,
                parent_pid: p.parent_pid,
                created: p.created,
                cpu_100ns: p.cpu_100ns,
                working_set: p.working_set,
                private_commit: p.private_commit,
                io: p.io,
                handles: p.handles,
                threads: p.threads,
                session: p.session,
                name: p.name,
            })
            .collect()
    })
}

pub(crate) fn collect(collector: &mut Collector, seconds: u64, limit: usize) -> ProcessActivity {
    let pdh = collector.pdh.get_or_insert_with(pdh::Pdh::open);
    if let Ok(query) = pdh {
        query.start(collector.started.elapsed());
    }
    let before = processes();
    let started = unix_ms();
    let clock = Instant::now();
    std::thread::sleep(Duration::from_secs(seconds));
    // Bracket SCM's PID-only attribution with two identity-bearing process lists.
    let services = service_processes();
    let mut after = processes();
    let counters_at = collector.started.elapsed();
    let window = Window::finish(started, clock);
    let (gpu, gpu_window): (Sampled<Instances>, Window) = match collector.pdh.as_mut() {
        Some(Ok(query)) => match query.read(collector.started.elapsed()) {
            Ok((readings, window)) => (readings.gpu_engine_percent, window),
            Err(why) => (Err(why), window.clone()),
        },
        Some(Err(why)) => (Err(why.clone()), window.clone()),
        None => (
            Err(Unavailable::new(
                Status::Unsupported,
                "GPU counter query was unavailable.",
            )),
            window.clone(),
        ),
    };
    // Classification happens after the counter window: slow/inaccessible providers must not
    // dilute the measured CPU and I/O rates or lengthen the GPU sampling interval.
    if let Ok(rows) = &mut after {
        let mut classifier = super::browser::Classifier::default();
        for row in rows.iter_mut() {
            row.browser =
                classifier.classify(row.pid, row.created, row.name.as_deref().unwrap_or(""));
        }
        crate::browser::attribute_embedded_main(rows);
    }
    assemble(
        collector,
        Collection {
            before,
            after,
            window,
            counters_at,
            gpu,
            gpu_window,
            services,
            logical_processors: processor_count(),
        },
        limit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::Metric;

    #[test]
    fn native_activity_keeps_query_only_classification_separate_from_counter_coverage() {
        let mut collector = Collector::new();
        let result = collector.process_activity(1, 50);
        assert!(result.window.duration_ms >= 1000);
        assert!(result.coverage.processes.enumerated > 0);
        assert_eq!(
            result.coverage.processes.readable,
            result.coverage.processes.enumerated
        );
        assert_eq!(result.coverage.processes.permission_denied, 0);
        assert!(result
            .evidence
            .iter()
            .any(|e| e.metric == Metric::ProcessCpuBasisPoints && e.usable_value().is_some()));
        assert!(result.coverage.services_attributed > 0);
        assert!(!result.applications.is_empty());
        assert!(result
            .evidence
            .iter()
            .filter_map(|e| (e.metric == Metric::ProcessGpuBasisPoints)
                .then_some(e.value)
                .flatten())
            .all(|value| value <= 10000));
        let text = serde_json::to_string(&result.evidence).unwrap();
        assert!(!text.contains("luid_") && !text.contains("pid_"));
    }
}
