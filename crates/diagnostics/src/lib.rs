//! Conservative observations, not a universal anomaly detector.
#![forbid(unsafe_code)]
use pcagent_shared::{ratio_basis_points, Metric, Snapshot};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification { LowAvailableMemory, HighCommitRatio, HighCpuSample, InsufficientEvidence }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support { Plausible, Insufficient }
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub id: String,
    pub classification: Classification,
    pub support: Support,
    pub summary: String,
    pub evidence_refs: Vec<String>,
    pub next_step: String,
    pub limitations: Vec<String>,
}
fn finding(snapshot: &Snapshot, class: Classification, refs: Vec<String>, summary: &str) -> Finding {
    Finding {
        id: format!("{}-{class:?}", snapshot.id), classification: class,
        support: if class == Classification::InsufficientEvidence { Support::Insufficient } else { Support::Plausible },
        summary: summary.into(), evidence_refs: refs,
        next_step: "Observe a representative workload over a longer period and check resources not yet measured.".into(),
        limitations: vec!["These are initial observation thresholds, not validated diagnostic rules. A short sample does not establish sustained contention, a leak or the cause of a slowdown.".into()],
    }
}

/// Each ratio is calculated from the same snapshot, never from different investigations.
pub fn analyze(snapshot: &Snapshot) -> Vec<Finding> {
    let mut findings = Vec::new();
    if let (Some((a, available)), Some((t, total))) = (
        snapshot.system_value(Metric::PhysicalAvailableBytes), snapshot.system_value(Metric::PhysicalTotalBytes)) {
        if available <= total && ratio_basis_points(available, total).is_some_and(|p| p < 1_000) {
            findings.push(finding(snapshot, Classification::LowAvailableMemory, vec![a.id.clone(), t.id.clone()],
                "Available physical memory is low in this sample. Memory contention remains unproven."));
        }
    }
    if let (Some((used_e, used)), Some((limit_e, limit))) = (
        snapshot.system_value(Metric::SystemCommitBytes), snapshot.system_value(Metric::SystemCommitLimitBytes)) {
        if used <= limit && ratio_basis_points(used, limit).is_some_and(|p| p >= 9_000) {
            findings.push(finding(snapshot, Classification::HighCommitRatio, vec![used_e.id.clone(), limit_e.id.clone()],
                "System commit is close to its current limit. This does not measure physical RAM occupancy or page-file activity."));
        }
    }
    if let Some((e, busy)) = snapshot.system_value(Metric::CpuBusyBasisPoints) {
        if (9_000..=10_000).contains(&busy) {
            findings.push(finding(snapshot, Classification::HighCpuSample, vec![e.id.clone()],
                "Aggregate CPU activity is high during this short observation. Its connection to the reported symptom needs more evidence."));
        }
    }
    findings.push(finding(snapshot, Classification::InsufficientEvidence,
        snapshot.evidence.iter().filter(|e| e.usable_value().is_some()).map(|e| e.id.clone()).collect(),
        "The available measurements do not establish the cause of the symptom. No intervention or hardware purchase is justified by this report alone."));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{Evidence, Origin, Status, Window};
    fn sample(values: &[(Metric, u64)]) -> Snapshot {
        let mut s = Snapshot::empty("synthetic-test"); s.origin = Origin::SyntheticFixture;
        s.evidence = values.iter().map(|(m, v)| {
            let mut e = Evidence::measured(*m, "system", *v, "fixture:diagnostics", Window::instant());
            e.origin = Origin::SyntheticFixture; e
        }).collect(); s
    }
    #[test]
    fn absent_metrics_do_not_become_a_healthy_verdict() {
        let f = analyze(&sample(&[]));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].classification, Classification::InsufficientEvidence);
    }
    #[test]
    fn high_commit_is_not_a_ram_diagnosis() {
        let f = analyze(&sample(&[(Metric::SystemCommitBytes, 95), (Metric::SystemCommitLimitBytes, 100),
            (Metric::PhysicalAvailableBytes, 80), (Metric::PhysicalTotalBytes, 100)]));
        assert!(f.iter().any(|f| f.classification == Classification::HighCommitRatio));
        assert!(!f.iter().any(|f| f.classification == Classification::LowAvailableMemory));
    }
    #[test]
    fn a_single_memory_sample_is_not_a_proven_cause() {
        let f = analyze(&sample(&[(Metric::PhysicalAvailableBytes, 1), (Metric::PhysicalTotalBytes, 100)]));
        assert_eq!(f[0].support, Support::Plausible);
        assert!(!f[0].limitations.is_empty());
    }
    #[test]
    fn zero_denominators_and_inconsistent_counters_are_ignored() {
        for (available, total) in [(0, 0), (200, 100)] {
            assert_eq!(analyze(&sample(&[(Metric::PhysicalAvailableBytes, available), (Metric::PhysicalTotalBytes, total)])).len(), 1);
        }
    }
    #[test]
    fn stale_values_cannot_trigger_a_finding() {
        let mut s = sample(&[(Metric::PhysicalAvailableBytes, 1), (Metric::PhysicalTotalBytes, 100)]);
        s.evidence[0].status = Status::Stale;
        assert_eq!(analyze(&s).len(), 1);
    }
    #[test]
    fn every_finding_refers_to_real_evidence() {
        let s = sample(&[(Metric::CpuBusyBasisPoints, 9_900)]);
        for f in analyze(&s) {
            for id in f.evidence_refs { assert!(s.evidence.iter().any(|e| e.id == id)); }
        }
    }
}
