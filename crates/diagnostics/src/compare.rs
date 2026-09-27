//! Before/after verification (spec 18). The model explains results; it never computes them.
use pcagent_shared::{Evidence, Metric, Origin, SCHEMA_VERSION};
use serde::Serialize;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    LowerIsBetter,
    HigherIsBetter,
}
impl Direction {
    /// Metrics without a direction are not judged: capacities, instantaneous queue depth, disk
    /// idle time and GPU utilization depend on how much work was requested.
    pub fn of(metric: Metric) -> Option<Self> {
        match metric {
            Metric::PhysicalAvailableBytes => Some(Self::HigherIsBetter),
            Metric::SystemCommitBytes
            | Metric::CpuBusyBasisPoints
            | Metric::CpuBusiestCoreBasisPoints
            | Metric::ProcessWorkingSetBytes
            | Metric::ProcessPrivateCommitBytes
            | Metric::DiskReadLatencyMicroseconds
            | Metric::DiskWriteLatencyMicroseconds
            | Metric::HardFaultPagesPerSecondX100 => Some(Self::LowerIsBetter),
            _ => None,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Improved,
    NoNetChange,
    Regressed,
    NotComparable,
    NeedsMoreMeasurement,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WindowSummary {
    pub samples: usize,
    pub median: u64,
    pub min: u64,
    pub max: u64,
    pub first_unix_ms: u64,
    pub last_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub schema_version: u32,
    pub metric: Metric,
    pub entity_ref: String,
    pub direction: Option<Direction>,
    pub before: Option<WindowSummary>,
    pub after: Option<WindowSummary>,
    pub outcome: Outcome,
    pub limitations: Vec<String>,
}

fn select<'a>(set: &'a [Evidence], metric: Metric, entity_ref: &str) -> Vec<&'a Evidence> {
    let mut ids = HashSet::new();
    let mut windows = HashSet::new();
    set.iter()
        .filter(|e| e.metric == metric && e.entity_ref == entity_ref && e.usable_value().is_some())
        .filter(|e| {
            ids.insert(&e.id)
                && windows.insert((e.window.started_at_unix_ms, e.window.finished_at_unix_ms))
        })
        .collect()
}

fn summarize(values: &[&Evidence]) -> Option<WindowSummary> {
    let mut v: Vec<u64> = values.iter().filter_map(|e| e.usable_value()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_unstable();
    let mid = v.len() / 2;
    let median = if v.len().is_multiple_of(2) {
        v[mid - 1] / 2 + v[mid] / 2 + (v[mid - 1] % 2 + v[mid] % 2) / 2
    } else {
        v[mid]
    };
    Some(WindowSummary {
        samples: v.len(),
        median,
        min: v[0],
        max: v[v.len() - 1],
        first_unix_ms: values.iter().map(|e| e.window.started_at_unix_ms).min()?,
        last_unix_ms: values.iter().map(|e| e.window.finished_at_unix_ms).max()?,
    })
}

/// Compares identical metrics of the same entity. An improvement requires the whole after
/// window to be better than the whole before window; overlapping ranges are "no net change".
pub fn compare(
    metric: Metric,
    entity_ref: &str,
    before: &[Evidence],
    after: &[Evidence],
    min_samples: usize,
) -> Comparison {
    let (b, a) = (
        select(before, metric, entity_ref),
        select(after, metric, entity_ref),
    );
    let direction = Direction::of(metric);
    let mut result = Comparison {
        schema_version: SCHEMA_VERSION,
        metric,
        entity_ref: entity_ref.into(),
        direction,
        before: summarize(&b),
        after: summarize(&a),
        outcome: Outcome::NotComparable,
        limitations: vec![
            "Perceived responsiveness, latency and FPS were not measured by this comparison."
                .into(),
            "Workload changes between windows can explain differences.".into(),
        ],
    };
    let origins: Vec<Origin> = b.iter().chain(&a).map(|e| e.origin).collect();
    let mixed = origins.windows(2).any(|w| w[0] != w[1]);
    let units_differ = b.iter().chain(&a).any(|e| e.unit != metric.unit());
    let sources_differ = b.iter().chain(&a).any(|e| {
        b.first()
            .or_else(|| a.first())
            .is_some_and(|first| e.source != first.source)
    });
    let invalid_windows = [&b, &a].iter().any(|set| {
        let mut windows: Vec<_> = set.iter().map(|e| &e.window).collect();
        windows.sort_by_key(|w| w.started_at_unix_ms);
        windows
            .iter()
            .any(|w| w.started_at_unix_ms > w.finished_at_unix_ms)
            || windows
                .windows(2)
                .any(|w| w[0].finished_at_unix_ms > w[1].started_at_unix_ms)
    });
    let reused = b
        .iter()
        .any(|before| a.iter().any(|after| before.id == after.id));
    let (Some(direction), false, false, false) = (
        direction,
        mixed,
        units_differ || sources_differ,
        invalid_windows || reused,
    ) else {
        result
            .limitations
            .push("Metric, unit, source, origin or independent measurement windows do not support a before/after verdict.".into());
        return result;
    };
    let (Some(before), Some(after)) = (&result.before, &result.after) else {
        result.outcome = Outcome::NeedsMoreMeasurement;
        return result;
    };
    if before.samples < min_samples.max(1) || after.samples < min_samples.max(1) {
        result.outcome = Outcome::NeedsMoreMeasurement;
        return result;
    }
    if before.last_unix_ms > after.first_unix_ms {
        result
            .limitations
            .push("Before and after windows overlap in time.".into());
        return result;
    }
    let (better, worse) = match direction {
        Direction::LowerIsBetter => (after.max < before.min, after.min > before.max),
        Direction::HigherIsBetter => (after.min > before.max, after.max < before.min),
    };
    result.outcome = if better {
        Outcome::Improved
    } else if worse {
        Outcome::Regressed
    } else {
        Outcome::NoNetChange
    };
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{Status, Window};
    fn series(metric: Metric, values: &[u64], start: u64) -> Vec<Evidence> {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = start + i as u64 * 1_000;
                let mut e = Evidence::measured(
                    metric,
                    "system",
                    *v,
                    "fixture",
                    Window {
                        started_at_unix_ms: t,
                        finished_at_unix_ms: t,
                        duration_ms: 0,
                    },
                );
                e.origin = Origin::SyntheticFixture;
                e
            })
            .collect()
    }
    const AVAIL: Metric = Metric::PhysicalAvailableBytes;
    #[test]
    fn repeated_or_overlapping_observations_do_not_establish_improvement() {
        let before = series(AVAIL, &[10], 1_000);
        let after = series(AVAIL, &[100], 5_000);
        let r = compare(
            AVAIL,
            "system",
            &vec![before[0].clone(); 3],
            &vec![after[0].clone(); 3],
            3,
        );
        assert_eq!(r.outcome, Outcome::NeedsMoreMeasurement);
        assert_eq!(r.before.unwrap().samples, 1);
        let mut before = series(AVAIL, &[10, 12, 11], 2_000);
        let mut after = series(AVAIL, &[100, 101, 102], 5_000);
        for e in before.iter_mut().chain(&mut after) {
            e.window.started_at_unix_ms = e.window.finished_at_unix_ms - 1_500;
            e.window.duration_ms = 1_500;
        }
        assert_eq!(
            compare(AVAIL, "system", &before, &after, 3).outcome,
            Outcome::NotComparable
        );
        // Individually disjoint series, but the first after interval overlaps the last before one.
        for e in &mut before {
            e.window.started_at_unix_ms = e.window.finished_at_unix_ms;
        }
        for e in &mut after {
            e.window.started_at_unix_ms = e.window.finished_at_unix_ms;
        }
        after[0].window.started_at_unix_ms = 3_500;
        assert_eq!(
            compare(AVAIL, "system", &before, &after, 3).outcome,
            Outcome::NotComparable
        );
    }
    #[test]
    fn non_overlapping_windows_are_required_for_a_verdict() {
        let before = series(AVAIL, &[10, 12, 11], 0);
        assert_eq!(
            compare(
                AVAIL,
                "system",
                &before,
                &series(AVAIL, &[20, 25, 22], 10_000),
                3
            )
            .outcome,
            Outcome::Improved
        );
        assert_eq!(
            compare(
                AVAIL,
                "system",
                &before,
                &series(AVAIL, &[11, 25, 22], 10_000),
                3
            )
            .outcome,
            Outcome::NoNetChange
        );
        assert_eq!(
            compare(
                AVAIL,
                "system",
                &before,
                &series(AVAIL, &[1, 2, 3], 10_000),
                3
            )
            .outcome,
            Outcome::Regressed
        );
        let cpu = Metric::CpuBusyBasisPoints;
        assert_eq!(
            compare(
                cpu,
                "system",
                &series(cpu, &[9_000, 9_500], 0),
                &series(cpu, &[100, 200], 5_000),
                2
            )
            .outcome,
            Outcome::Improved
        );
    }
    #[test]
    fn a_single_favourable_snapshot_is_not_enough() {
        let r = compare(
            AVAIL,
            "system",
            &series(AVAIL, &[1, 1, 1], 0),
            &series(AVAIL, &[99], 10_000),
            3,
        );
        assert_eq!(r.outcome, Outcome::NeedsMoreMeasurement);
        let r = compare(
            AVAIL,
            "system",
            &[],
            &series(AVAIL, &[99, 98, 97], 10_000),
            3,
        );
        assert_eq!(r.outcome, Outcome::NeedsMoreMeasurement);
    }
    #[test]
    fn missing_values_and_other_entities_are_ignored() {
        let mut before = series(AVAIL, &[10, 10, 10], 0);
        before[0].status = Status::Stale;
        before[1].entity_ref = "proc-x".into();
        let r = compare(
            AVAIL,
            "system",
            &before,
            &series(AVAIL, &[20, 20, 20], 10_000),
            2,
        );
        assert_eq!(r.outcome, Outcome::NeedsMoreMeasurement);
        assert_eq!(r.before.unwrap().samples, 1);
    }
    #[test]
    fn capacity_metrics_mixed_origins_and_overlaps_are_not_comparable() {
        let total = Metric::PhysicalTotalBytes;
        assert_eq!(
            compare(
                total,
                "system",
                &series(total, &[1], 0),
                &series(total, &[2], 5_000),
                1
            )
            .outcome,
            Outcome::NotComparable
        );
        let mut after = series(AVAIL, &[20], 5_000);
        after[0].origin = Origin::Live;
        assert_eq!(
            compare(AVAIL, "system", &series(AVAIL, &[1], 0), &after, 1).outcome,
            Outcome::NotComparable
        );
        assert_eq!(
            compare(
                AVAIL,
                "system",
                &series(AVAIL, &[1, 1], 0),
                &series(AVAIL, &[5], 500),
                1
            )
            .outcome,
            Outcome::NotComparable
        );
    }
    #[test]
    fn new_counters_have_deliberate_directions() {
        for m in [
            Metric::DiskReadLatencyMicroseconds,
            Metric::DiskWriteLatencyMicroseconds,
            Metric::HardFaultPagesPerSecondX100,
            Metric::CpuBusiestCoreBasisPoints,
        ] {
            assert_eq!(Direction::of(m), Some(Direction::LowerIsBetter), "{m:?}");
        }
        for m in [
            Metric::DiskIdleBasisPoints,
            Metric::DiskQueueLength,
            Metric::GpuEngineBasisPoints,
            Metric::GpuBusiestEngineBasisPoints,
        ] {
            assert_eq!(Direction::of(m), None, "{m:?}");
        }
        let lat = Metric::DiskReadLatencyMicroseconds;
        assert_eq!(
            compare(
                lat,
                "system",
                &series(lat, &[40_000, 42_000, 41_000], 0),
                &series(lat, &[4_000, 5_000, 4_500], 10_000),
                3
            )
            .outcome,
            Outcome::Improved
        );
    }
    #[test]
    fn median_does_not_overflow() {
        let s = summarize(
            &series(AVAIL, &[u64::MAX, u64::MAX - 1], 0)
                .iter()
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert_eq!(s.median, u64::MAX - 1);
    }
}
