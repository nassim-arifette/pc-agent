//! Bounded in-memory session history of system samples. Portable and deterministic.
use pcagent_shared::{
    new_id, BootChange, GapReason, HistoryGap, Metric, MetricPoint, MetricSeries, Origin,
    Retention, Snapshot, Status, SCHEMA_VERSION,
};
use std::{collections::VecDeque, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryConfig {
    pub max_samples: usize,
    pub max_age: Duration,
    pub expected_interval: Duration,
}
impl Default for HistoryConfig {
    /// One hour of detail at the spec's two-second investigation cadence.
    fn default() -> Self {
        Self {
            max_samples: 1800,
            max_age: Duration::from_secs(3600),
            expected_interval: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub metric: Metric,
    pub value: Option<u64>,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemSample {
    pub unix_ms: u64,
    pub monotonic_ms: u64,
    pub boot_id: Option<String>,
    pub readings: Vec<Reading>,
}
impl SystemSample {
    /// Keeps only system-scope evidence; unusable values stay `None` with their status.
    pub fn from_snapshot(
        snapshot: &Snapshot,
        unix_ms: u64,
        monotonic_ms: u64,
        boot_id: Option<String>,
    ) -> Self {
        let readings = snapshot
            .evidence
            .iter()
            .filter(|e| e.entity_ref == "system")
            .map(|e| Reading {
                metric: e.metric,
                value: e.usable_value(),
                status: e.status,
            })
            .collect();
        Self {
            unix_ms,
            monotonic_ms,
            boot_id,
            readings,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordError {
    /// Monotonic time went backwards relative to the newest retained sample.
    NonMonotonic,
    ZeroCapacity,
}

#[derive(Debug, Clone)]
pub struct MetricHistory {
    config: HistoryConfig,
    origin: Origin,
    samples: VecDeque<SystemSample>,
    newest_evicted_ms: Option<u64>,
}
impl MetricHistory {
    pub fn new(config: HistoryConfig, origin: Origin) -> Self {
        Self {
            config,
            origin,
            samples: VecDeque::with_capacity(config.max_samples.min(4096)),
            newest_evicted_ms: None,
        }
    }
    pub fn config(&self) -> HistoryConfig {
        self.config
    }
    pub fn len(&self) -> usize {
        self.samples.len()
    }
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
    pub fn record(&mut self, sample: SystemSample) -> Result<(), RecordError> {
        if self.config.max_samples == 0 {
            return Err(RecordError::ZeroCapacity);
        }
        if self
            .samples
            .back()
            .is_some_and(|last| sample.monotonic_ms < last.monotonic_ms)
        {
            return Err(RecordError::NonMonotonic);
        }
        let newest = sample.monotonic_ms;
        self.samples.push_back(sample);
        let max_age = ms(self.config.max_age);
        while self.samples.len() > self.config.max_samples
            || self
                .samples
                .front()
                .is_some_and(|s| newest.saturating_sub(s.monotonic_ms) > max_age)
        {
            if let Some(old) = self.samples.pop_front() {
                self.newest_evicted_ms = Some(old.monotonic_ms);
            }
        }
        Ok(())
    }
    /// Returns recorded points for `metric` in `[now - window, now]`; the window is capped at `max_age`.
    pub fn query(&self, metric: Metric, window: Duration, now_monotonic_ms: u64) -> MetricSeries {
        let requested = ms(window);
        let span = requested.min(ms(self.config.max_age));
        let start = now_monotonic_ms.saturating_sub(span);
        let tolerance = ms(self.config.expected_interval).saturating_mul(2);
        let selected: Vec<&SystemSample> = self
            .samples
            .iter()
            .filter(|s| s.monotonic_ms >= start && s.monotonic_ms <= now_monotonic_ms)
            .collect();
        let mut points = Vec::with_capacity(selected.len());
        let mut gaps = Vec::new();
        let mut boot_changes = Vec::new();
        let mut intervals = Vec::new();
        let mut limitations = Vec::new();
        let leading_to = selected
            .first()
            .map_or(now_monotonic_ms, |s| s.monotonic_ms);
        if leading_to.saturating_sub(start) > tolerance {
            let evicted = self.newest_evicted_ms.is_some_and(|e| e >= start);
            gaps.push(HistoryGap {
                from_monotonic_ms: start,
                to_monotonic_ms: leading_to,
                reason: if evicted {
                    GapReason::Evicted
                } else {
                    GapReason::NotRecorded
                },
            });
        }
        for (i, s) in selected.iter().enumerate() {
            if let Some(prev) = i.checked_sub(1).map(|j| selected[j]) {
                let delta = s.monotonic_ms - prev.monotonic_ms;
                if prev.boot_id != s.boot_id {
                    boot_changes.push(BootChange {
                        at_unix_ms: s.unix_ms,
                        previous_boot_id: prev.boot_id.clone(),
                        boot_id: s.boot_id.clone(),
                    });
                    gaps.push(HistoryGap {
                        from_monotonic_ms: prev.monotonic_ms,
                        to_monotonic_ms: s.monotonic_ms,
                        reason: GapReason::BootChanged,
                    });
                } else {
                    intervals.push(delta);
                    if delta > tolerance {
                        gaps.push(HistoryGap {
                            from_monotonic_ms: prev.monotonic_ms,
                            to_monotonic_ms: s.monotonic_ms,
                            reason: GapReason::MissedSamples,
                        });
                    }
                }
            }
            let reading = s.readings.iter().find(|r| r.metric == metric);
            points.push(MetricPoint {
                unix_ms: s.unix_ms,
                monotonic_ms: s.monotonic_ms,
                value: reading.and_then(|r| r.value),
                status: reading.map_or(Status::Unsupported, |r| r.status),
            });
        }
        if let Some(last) = selected.last() {
            if now_monotonic_ms.saturating_sub(last.monotonic_ms) > tolerance {
                gaps.push(HistoryGap {
                    from_monotonic_ms: last.monotonic_ms,
                    to_monotonic_ms: now_monotonic_ms,
                    reason: GapReason::NotRecorded,
                });
            }
        }
        intervals.sort_unstable();
        let observed_interval_ms = intervals.get(intervals.len() / 2).copied();
        let usable = points.iter().filter(|p| p.value.is_some()).count();
        let status = if points.is_empty() {
            limitations.push("No samples were recorded in the requested window.".into());
            Status::Partial
        } else if usable == 0 && points.iter().all(|p| p.status == Status::Unsupported) {
            Status::Unsupported
        } else if usable == points.len() && gaps.is_empty() {
            Status::Ok
        } else {
            Status::Partial
        };
        if requested > span {
            limitations.push(format!(
                "Requested window exceeds retention; capped to {span} ms."
            ));
        }
        limitations.push("In-memory session history only; values are never interpolated across gaps and are lost when the collector is dropped.".into());
        MetricSeries {
            schema_version: SCHEMA_VERSION,
            id: new_id("series"),
            metric,
            entity_ref: "system".into(),
            unit: metric.unit().into(),
            origin: self.origin,
            status,
            requested_window_ms: requested,
            window_start_monotonic_ms: start,
            window_end_monotonic_ms: now_monotonic_ms,
            observed_interval_ms,
            retention: Retention {
                max_samples: self.config.max_samples,
                max_age_ms: ms(self.config.max_age),
                expected_interval_ms: ms(self.config.expected_interval),
            },
            points,
            gaps,
            boot_changes,
            limitations,
        }
    }
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    const M: Metric = Metric::PhysicalAvailableBytes;
    fn config(max_samples: usize, max_age_s: u64) -> HistoryConfig {
        HistoryConfig {
            max_samples,
            max_age: Duration::from_secs(max_age_s),
            expected_interval: Duration::from_secs(2),
        }
    }
    fn sample(t_ms: u64, value: Option<u64>, boot: &str) -> SystemSample {
        SystemSample {
            unix_ms: 1_700_000_000_000 + t_ms,
            monotonic_ms: t_ms,
            boot_id: Some(boot.into()),
            readings: vec![Reading {
                metric: M,
                value,
                status: if value.is_some() {
                    Status::Ok
                } else {
                    Status::Error
                },
            }],
        }
    }
    fn filled(config: HistoryConfig, times: &[u64]) -> MetricHistory {
        let mut h = MetricHistory::new(config, Origin::SyntheticFixture);
        for &t in times {
            h.record(sample(t, Some(t), "b1")).unwrap();
        }
        h
    }
    #[test]
    fn regular_series_is_ok_and_synthetic() {
        let h = filled(config(100, 3600), &[0, 2_000, 4_000, 6_000]);
        let s = h.query(M, Duration::from_secs(6), 6_000);
        assert_eq!(s.status, Status::Ok);
        assert_eq!(s.origin, Origin::SyntheticFixture);
        assert_eq!(s.points.len(), 4);
        assert!(s.gaps.is_empty());
        assert_eq!(s.observed_interval_ms, Some(2_000));
        assert_eq!(s.unit, "bytes");
    }
    #[test]
    fn sample_count_bound_evicts_oldest_and_reports_it() {
        let times: Vec<u64> = (0..10).map(|i| i * 2_000).collect();
        let h = filled(config(3, 3600), &times);
        assert_eq!(h.len(), 3);
        let s = h.query(M, Duration::from_secs(60), 18_000);
        assert_eq!(s.points.first().unwrap().monotonic_ms, 14_000);
        assert_eq!(s.gaps[0].reason, GapReason::Evicted);
        assert_eq!(s.status, Status::Partial);
    }
    #[test]
    fn age_bound_evicts_and_caps_window() {
        let h = filled(config(1000, 10), &[0, 2_000, 12_000, 14_000]);
        assert_eq!(h.len(), 2);
        let s = h.query(M, Duration::from_secs(3600), 14_000);
        assert_eq!(s.window_start_monotonic_ms, 4_000);
        assert!(s.limitations.iter().any(|l| l.contains("capped")));
    }
    #[test]
    fn missed_samples_are_explicit_gaps() {
        let h = filled(config(100, 3600), &[0, 2_000, 20_000, 22_000]);
        let s = h.query(M, Duration::from_secs(22), 22_000);
        assert_eq!(s.gaps.len(), 1);
        assert_eq!(s.gaps[0].reason, GapReason::MissedSamples);
        assert_eq!(
            (s.gaps[0].from_monotonic_ms, s.gaps[0].to_monotonic_ms),
            (2_000, 20_000)
        );
        assert_eq!(s.points.len(), 4);
    }
    #[test]
    fn leading_and_trailing_silence_are_not_recorded_gaps() {
        let h = filled(config(100, 3600), &[10_000, 12_000]);
        let s = h.query(M, Duration::from_secs(30), 30_000);
        let reasons: Vec<_> = s.gaps.iter().map(|g| g.reason).collect();
        assert_eq!(reasons, [GapReason::NotRecorded, GapReason::NotRecorded]);
    }
    #[test]
    fn boot_changes_are_recorded_and_break_continuity() {
        let mut h = MetricHistory::new(config(100, 3600), Origin::SyntheticFixture);
        h.record(sample(0, Some(1), "b1")).unwrap();
        h.record(sample(2_000, Some(2), "b2")).unwrap();
        let s = h.query(M, Duration::from_secs(2), 2_000);
        assert_eq!(s.boot_changes.len(), 1);
        assert_eq!(s.gaps[0].reason, GapReason::BootChanged);
        assert_eq!(s.observed_interval_ms, None);
    }
    #[test]
    fn missing_values_stay_null_and_absent_metrics_are_unsupported() {
        let mut h = MetricHistory::new(config(100, 3600), Origin::SyntheticFixture);
        h.record(sample(0, None, "b1")).unwrap();
        h.record(sample(2_000, Some(0), "b1")).unwrap();
        let s = h.query(M, Duration::from_secs(2), 2_000);
        assert_eq!(s.points[0].value, None);
        assert_eq!(s.points[1].value, Some(0));
        assert_eq!(s.status, Status::Partial);
        let other = h.query(Metric::CpuBusyBasisPoints, Duration::from_secs(2), 2_000);
        assert_eq!(other.status, Status::Unsupported);
        assert!(other.points.iter().all(|p| p.value.is_none()));
    }
    #[test]
    fn empty_history_and_bad_input_are_explicit() {
        let mut h = MetricHistory::new(config(100, 3600), Origin::Live);
        let s = h.query(M, Duration::from_secs(10), 10_000);
        assert!(s.points.is_empty());
        assert_eq!(s.status, Status::Partial);
        h.record(sample(5_000, Some(1), "b1")).unwrap();
        assert_eq!(
            h.record(sample(4_000, Some(1), "b1")),
            Err(RecordError::NonMonotonic)
        );
        let mut zero = MetricHistory::new(config(0, 10), Origin::Live);
        assert_eq!(
            zero.record(sample(0, Some(1), "b1")),
            Err(RecordError::ZeroCapacity)
        );
    }
    #[test]
    fn snapshot_conversion_keeps_only_system_scope() {
        use pcagent_shared::{Evidence, Window};
        let mut snap = Snapshot::empty("s");
        snap.evidence
            .push(Evidence::measured(M, "system", 7, "t", Window::instant()));
        snap.evidence.push(Evidence::measured(
            Metric::ProcessWorkingSetBytes,
            "proc-1",
            9,
            "t",
            Window::instant(),
        ));
        let s = SystemSample::from_snapshot(&snap, 1, 2, None);
        assert_eq!(s.readings.len(), 1);
        assert_eq!(s.readings[0].value, Some(7));
    }
}
