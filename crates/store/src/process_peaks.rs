//! Observed process resource peaks, including spikes below system anomaly thresholds.
use crate::{stored, Store, StoreError, SAMPLE_RETENTION_MS};
use pcagent_shared::{Evidence, Metric, Origin, Status, Window};
use rusqlite::params;
use serde::Serialize;
use std::collections::BTreeMap;

const MAX_ROWS: usize = 100_000;
const MAX_ENTITIES: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct ProcessPeaks {
    pub status: Status,
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub observations: Vec<Evidence>,
    /// Local names; callers must apply the sharing boundary before cloud use.
    pub process_names: BTreeMap<String, String>,
    pub limitations: Vec<String>,
}

impl Store {
    /// Return up to 20 highest observed CPU peaks. These samples identify candidates for further
    /// analysis, not a cause of a slowdown: the sampler retains a bounded selection of processes.
    pub fn process_peaks(
        &self,
        from_unix_ms: u64,
        to_unix_ms: u64,
        limit: usize,
    ) -> Result<ProcessPeaks, StoreError> {
        self.process_peaks_for(
            from_unix_ms,
            to_unix_ms,
            limit,
            &[Metric::ProcessCpuBasisPoints],
        )
    }

    /// Separate GPU and I/O peaks among the retained processes. Different metrics are never
    /// ranked against each other or converted into application energy consumption.
    pub fn process_resource_peaks(
        &self,
        from_unix_ms: u64,
        to_unix_ms: u64,
        limit_per_metric: usize,
    ) -> Result<ProcessPeaks, StoreError> {
        self.process_peaks_for(
            from_unix_ms,
            to_unix_ms,
            limit_per_metric,
            &[
                Metric::ProcessGpuBasisPoints,
                Metric::ProcessIoReadBytesPerSecond,
                Metric::ProcessIoWriteBytesPerSecond,
            ],
        )
    }

    fn process_peaks_for(
        &self,
        from_unix_ms: u64,
        to_unix_ms: u64,
        limit: usize,
        metrics: &[Metric],
    ) -> Result<ProcessPeaks, StoreError> {
        if from_unix_ms >= to_unix_ms
            || to_unix_ms - from_unix_ms > SAMPLE_RETENTION_MS
            || to_unix_ms > i64::MAX as u64
            || !(1..=20).contains(&limit)
        {
            return Err(StoreError::InvalidQuery);
        }
        let mut statement = self.conn.prepare("SELECT payload FROM samples WHERE measured_at>=?1 AND measured_at<=?2 ORDER BY measured_at,id LIMIT ?3")?;
        let rows = statement.query_map(
            params![
                stored(from_unix_ms.saturating_sub(60_000)),
                stored(to_unix_ms),
                (MAX_ROWS + 1) as i64
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut peaks: BTreeMap<(usize, String, u8), Evidence> = BTreeMap::new();
        let mut names = BTreeMap::new();
        let mut limitations = vec![
            "Observed peaks among the sampled processes, not proof of causality or a complete process ranking; absent processes and intervals are unknown.".into(),
            "The sampler retains at most 20 processes per snapshot; CPU values are shares of total logical processor capacity.".into(),
        ];
        let mut bounded = false;
        for (index, row) in rows.enumerate() {
            if index == MAX_ROWS {
                limitations.push(
                    "The query reached its sample read limit; narrow the requested window.".into(),
                );
                break;
            }
            let sample = super::samples::decode_sample(&row?)?;
            if sample.measured_at_unix_ms < from_unix_ms
                && (sample.resolution_ms < 60_000
                    || sample
                        .measured_at_unix_ms
                        .saturating_add(sample.resolution_ms)
                        <= from_unix_ms)
            {
                continue;
            }
            for value in &sample.values {
                let Some(metric_index) = metrics.iter().position(|metric| *metric == value.metric)
                else {
                    continue;
                };
                if !value.entity_ref.starts_with("proc-")
                    || value.measured == 0
                    || !matches!(value.status, Status::Ok | Status::Partial)
                {
                    continue;
                }
                let Some(peak) = value.max else { continue };
                let key = (
                    metric_index,
                    value.entity_ref.clone(),
                    u8::from(sample.origin != Origin::Live),
                );
                if peaks
                    .get(&key)
                    .is_some_and(|before| before.value.is_some_and(|n| n >= peak))
                {
                    continue;
                }
                if peaks.len() >= MAX_ENTITIES && !peaks.contains_key(&key) {
                    bounded = true;
                    continue;
                }
                // Older serialized rows without observation windows cannot establish C4 evidence.
                let (Some(start), Some(end)) =
                    (value.observed_from_unix_ms, value.observed_to_unix_ms)
                else {
                    continue;
                };
                let mut evidence = Evidence::measured(
                    value.metric,
                    &value.entity_ref,
                    peak,
                    &value.source,
                    Window {
                        started_at_unix_ms: start,
                        finished_at_unix_ms: end,
                        duration_ms: value.observed_duration_ms,
                    },
                );
                evidence.status = value.status;
                evidence.origin = sample.origin;
                evidence.limitations.push("Highest retained measurement of this resource in the requested window; not its average across that window or measured application energy.".into());
                if sample.resolution_ms >= 60_000 {
                    evidence.status = Status::Partial;
                    evidence.limitations.push("Peak retained in a minute aggregate; its exact subinterval is unavailable. The observation window encloses that aggregate and may overlap the query boundary.".into());
                }
                if let Some(name) = sample.process_names.get(&value.entity_ref) {
                    names.insert(value.entity_ref.clone(), name.clone());
                }
                peaks.insert(key, evidence);
            }
        }
        if bounded {
            limitations.push(
                "The query reached its process identity limit; some processes were omitted.".into(),
            );
        }
        let mut ordered: Vec<_> = peaks
            .into_iter()
            .map(|((metric_index, _, _), evidence)| (metric_index, evidence))
            .collect();
        ordered.sort_by(|(a_index, a), (b_index, b)| {
            a_index
                .cmp(b_index)
                .then_with(|| b.value.cmp(&a.value))
                .then_with(|| a.entity_ref.cmp(&b.entity_ref))
        });
        let count = ordered.len();
        let mut per_metric = vec![0; metrics.len()];
        let observations: Vec<_> = ordered
            .into_iter()
            .filter_map(|(index, evidence)| {
                if per_metric[index] == limit {
                    return None;
                }
                per_metric[index] += 1;
                Some(evidence)
            })
            .collect();
        let omitted = count - observations.len();
        if omitted != 0 {
            limitations.push(format!("The response keeps the highest {limit} observed process peaks per metric; {omitted} lower peaks were omitted."));
        }
        for (index, metric) in metrics.iter().enumerate() {
            if per_metric[index] == 0 {
                limitations.push(format!(
                    "No usable retained measurement for {metric:?}; this resource use is unknown."
                ));
            }
        }
        names.retain(|reference, _| observations.iter().any(|e| &e.entity_ref == reference));
        Ok(ProcessPeaks {
            status: if observations.is_empty() {
                Status::Unsupported
            } else {
                Status::Partial
            },
            from_unix_ms,
            to_unix_ms,
            observations,
            process_names: names,
            limitations,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StoredSample;
    use pcagent_shared::Snapshot;

    fn sample(session: &str, at: u64, cpu: Option<u64>) -> StoredSample {
        let mut snapshot = Snapshot::empty(session);
        snapshot.origin = Origin::SyntheticFixture;
        snapshot.evidence.push(match cpu {
            Some(cpu) => Evidence::measured(
                Metric::ProcessCpuBasisPoints,
                "proc-aabbccdd",
                cpu,
                "fixture",
                Window {
                    started_at_unix_ms: at - 1000,
                    finished_at_unix_ms: at,
                    duration_ms: 1000,
                },
            ),
            None => Evidence::missing(
                Metric::ProcessCpuBasisPoints,
                "proc-aabbccdd",
                "fixture",
                Status::Unsupported,
                "not measured",
            ),
        });
        let mut sample = StoredSample::from_snapshot(&snapshot, at, 1000);
        sample
            .process_names
            .insert("proc-aabbccdd".into(), "fixture.exe".into());
        sample
    }

    #[test]
    fn peaks_find_short_process_spikes_without_system_anomaly_events() {
        let mut store = Store::open_in_memory().unwrap();
        for (at, cpu) in [
            (1000, Some(100)),
            (2000, Some(1200)),
            (3000, None),
            (4000, Some(200)),
        ] {
            store.append_sample(&sample("session", at, cpu)).unwrap();
        }
        let peaks = store.process_peaks(1, 5000, 10).unwrap();
        assert_eq!(peaks.observations.len(), 1);
        let peak = &peaks.observations[0];
        assert_eq!(peak.value, Some(1200));
        assert_eq!(peak.window.finished_at_unix_ms, 2000);
        assert_eq!(peak.window.duration_ms, 1000);
        assert_eq!(peak.origin, Origin::SyntheticFixture);
        assert_eq!(peaks.status, Status::Partial);
        assert_eq!(
            peaks.process_names.get(&peak.entity_ref).unwrap(),
            "fixture.exe"
        );
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM activity_events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn missing_values_and_other_windows_are_not_zero_peaks() {
        let mut store = Store::open_in_memory().unwrap();
        store.append_sample(&sample("session", 1000, None)).unwrap();
        store
            .append_sample(&sample("session", 9000, Some(100)))
            .unwrap();
        let peaks = store.process_peaks(1, 5000, 10).unwrap();
        assert!(peaks.observations.is_empty());
        assert_eq!(peaks.status, Status::Unsupported);
        assert!(peaks.process_names.is_empty());
        assert!(store.process_peaks(1, 5000, 21).is_err());
    }

    #[test]
    fn collector_restarts_keep_distinct_identities_and_bounded_names() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .append_sample(&sample("first", 1000, Some(800)))
            .unwrap();
        store
            .append_sample(&sample("second", 2000, Some(1000)))
            .unwrap();
        let peaks = store.process_peaks(1, 5000, 10).unwrap();
        assert_eq!(peaks.observations.len(), 2);
        assert_ne!(
            peaks.observations[0].entity_ref,
            peaks.observations[1].entity_ref
        );
        let peaks = store.process_peaks(1, 5000, 1).unwrap();
        assert_eq!(peaks.observations[0].value, Some(1000));
        assert_eq!(peaks.process_names.len(), 1);
        assert!(peaks
            .limitations
            .iter()
            .any(|text| text.contains("1 lower peaks")));
    }

    #[test]
    fn resource_peaks_keep_separate_rankings_windows_and_unknown_gpu() {
        let mut store = Store::open_in_memory().unwrap();
        for at in [1000, 2000] {
            let mut snapshot = Snapshot::empty("resource-session");
            snapshot.origin = Origin::SyntheticFixture;
            let window = Window {
                started_at_unix_ms: at - 500,
                finished_at_unix_ms: at,
                duration_ms: 500,
            };
            for (reference, gpu, read, write) in [
                ("proc-aabbccdd", 7000, 100, 8000),
                ("proc-eeff0011", 1000, 9000, 50),
            ] {
                for (metric, value) in [
                    (Metric::ProcessCpuBasisPoints, 400),
                    (Metric::ProcessGpuBasisPoints, gpu),
                    (Metric::ProcessIoReadBytesPerSecond, read),
                    (Metric::ProcessIoWriteBytesPerSecond, write),
                ] {
                    let evidence = if at == 2000 && metric == Metric::ProcessGpuBasisPoints {
                        Evidence::missing(
                            metric,
                            reference,
                            "fixture",
                            Status::PermissionDenied,
                            "identity not verified",
                        )
                    } else {
                        Evidence::measured(metric, reference, value, "fixture", window.clone())
                    };
                    snapshot.evidence.push(evidence);
                }
            }
            store
                .append_sample(&StoredSample::from_snapshot(&snapshot, at, 1000))
                .unwrap();
        }
        let peaks = store.process_resource_peaks(1, 3000, 1).unwrap();
        assert_eq!(peaks.observations.len(), 3);
        let gpu = &peaks.observations[0];
        let read = &peaks.observations[1];
        let write = &peaks.observations[2];
        assert_eq!(gpu.metric, Metric::ProcessGpuBasisPoints);
        assert_eq!(gpu.value, Some(7000));
        assert_eq!(gpu.window.finished_at_unix_ms, 1000);
        assert_eq!(gpu.window.duration_ms, 500);
        assert_eq!(read.metric, Metric::ProcessIoReadBytesPerSecond);
        assert_eq!(read.value, Some(9000));
        assert_eq!(write.metric, Metric::ProcessIoWriteBytesPerSecond);
        assert_eq!(write.value, Some(8000));
        assert_eq!(gpu.entity_ref, write.entity_ref);
        assert_ne!(gpu.entity_ref, read.entity_ref);
        assert!(store
            .process_peaks(1, 3000, 1)
            .unwrap()
            .observations
            .iter()
            .all(|evidence| evidence.metric == Metric::ProcessCpuBasisPoints));
        assert!(store.process_resource_peaks(1, 3000, 0).is_err());
        assert!(store.process_resource_peaks(1, 3000, 21).is_err());
    }

    #[test]
    fn unknown_resource_measurements_do_not_create_zero_peaks() {
        let mut store = Store::open_in_memory().unwrap();
        store
            .append_sample(&sample("cpu-only", 1000, Some(1000)))
            .unwrap();
        let resources = store.process_resource_peaks(1, 2000, 10).unwrap();
        assert_eq!(resources.status, Status::Unsupported);
        assert!(resources.observations.is_empty());
        assert!(resources.process_names.is_empty());
    }
}
