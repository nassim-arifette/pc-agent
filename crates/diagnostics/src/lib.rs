//! Conservative observations, not a universal anomaly detector.
#![forbid(unsafe_code)]
mod compare;
pub use compare::{compare, Comparison, Direction, Outcome, WindowSummary};
use pcagent_shared::{ratio_basis_points, Evidence, Metric, Snapshot, SCHEMA_VERSION};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    LowAvailableMemory,
    HighCommitRatio,
    HighCpuSample,
    LargestMemoryContributor,
    NoContentionObserved,
    InsufficientEvidence,
}
/// Evidence support levels (spec 8.3). Not statistical probabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Plausible,
    Insufficient,
}
/// A locally generated statement whose numbers all come from referenced evidence.
#[derive(Debug, Clone, Serialize)]
pub struct Fact {
    pub text: String,
    pub evidence_refs: Vec<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub schema_version: u32,
    pub id: String,
    pub classification: Classification,
    pub support: Support,
    pub support_reason: String,
    pub summary: String,
    pub facts: Vec<Fact>,
    pub hypothesis: Option<String>,
    pub counter_evidence: Vec<String>,
    pub missing: Vec<String>,
    pub evidence_refs: Vec<String>,
    pub next_step: String,
    pub limitations: Vec<String>,
}

const SHORT_SAMPLE: &str = "A single short sample cannot establish sustained contention, a leak or the cause of a slowdown.";
const THRESHOLDS: &str =
    "Initial observation thresholds are prototype triggers, not validated diagnostic rules.";
const OBSERVE_LONGER: &str =
    "Observe a representative workload over a longer period and check resources not yet measured.";
const SYSTEM_METRICS: [(Metric, &str); 5] = [
    (Metric::PhysicalTotalBytes, "physical memory total"),
    (Metric::PhysicalAvailableBytes, "available physical memory"),
    (Metric::SystemCommitBytes, "system commit"),
    (Metric::SystemCommitLimitBytes, "system commit limit"),
    (Metric::CpuBusyBasisPoints, "aggregate CPU"),
];
const NOT_INSTRUMENTED: &str =
    "Disk latency, GPU, hard faults and per-core CPU are not instrumented in this snapshot.";

fn gib(bytes: u64) -> String {
    format!("{:.2} GiB", bytes as f64 / 1_073_741_824.0)
}
fn percent(basis_points: u64) -> String {
    format!("{:.1}%", basis_points as f64 / 100.0)
}
fn fact(text: String, refs: &[&Evidence]) -> Fact {
    Fact {
        text,
        evidence_refs: refs.iter().map(|e| e.id.clone()).collect(),
    }
}

struct Draft {
    class: Classification,
    support: Support,
    support_reason: &'static str,
    summary: &'static str,
    facts: Vec<Fact>,
    hypothesis: Option<&'static str>,
    counter_evidence: Vec<String>,
    next_step: &'static str,
    limitations: Vec<String>,
}
impl Draft {
    fn new(class: Classification, summary: &'static str, facts: Vec<Fact>) -> Self {
        Self {
            class,
            support: Support::Plausible,
            support_reason: SHORT_SAMPLE,
            summary,
            facts,
            hypothesis: None,
            counter_evidence: Vec::new(),
            next_step: OBSERVE_LONGER,
            limitations: vec![THRESHOLDS.into()],
        }
    }
    fn build(self, snapshot: &Snapshot, missing: &[String]) -> Finding {
        let mut evidence_refs: Vec<String> = Vec::new();
        for id in self.facts.iter().flat_map(|f| &f.evidence_refs) {
            if !evidence_refs.contains(id) {
                evidence_refs.push(id.clone());
            }
        }
        Finding {
            schema_version: SCHEMA_VERSION,
            id: format!("{}-{:?}", snapshot.id, self.class),
            classification: self.class,
            support: self.support,
            support_reason: self.support_reason.into(),
            summary: self.summary.into(),
            facts: self.facts,
            hypothesis: self.hypothesis.map(Into::into),
            counter_evidence: self.counter_evidence,
            missing: missing.to_vec(),
            evidence_refs,
            next_step: self.next_step.into(),
            limitations: self.limitations,
        }
    }
}

struct Ratios<'a> {
    available: Option<RatioParts<'a>>,
    commit: Option<RatioParts<'a>>,
    cpu: Option<(&'a Evidence, u64)>,
}
type RatioParts<'a> = (&'a Evidence, &'a Evidence, u64, u64, u64);
fn ratio(s: &Snapshot, part: Metric, whole: Metric) -> Option<RatioParts<'_>> {
    let (pe, p) = s.system_value(part)?;
    let (we, w) = s.system_value(whole)?;
    if p > w {
        return None;
    }
    ratio_basis_points(p, w).map(|bp| (pe, we, p, w, bp))
}
fn ratios(s: &Snapshot) -> Ratios<'_> {
    Ratios {
        available: ratio(
            s,
            Metric::PhysicalAvailableBytes,
            Metric::PhysicalTotalBytes,
        ),
        commit: ratio(s, Metric::SystemCommitBytes, Metric::SystemCommitLimitBytes),
        cpu: s
            .system_value(Metric::CpuBusyBasisPoints)
            .filter(|(_, v)| *v <= 10_000),
    }
}

/// Every ratio is computed from one snapshot; findings are ordered most specific first and
/// always end with an explicit abstention.
pub fn analyze(snapshot: &Snapshot) -> Vec<Finding> {
    let r = ratios(snapshot);
    let mut missing: Vec<String> = SYSTEM_METRICS
        .iter()
        .filter(|(m, _)| snapshot.system_value(*m).is_none())
        .map(|(_, label)| (*label).to_string())
        .collect();
    missing.push("representative observation window".into());
    let mut drafts = Vec::new();
    let low_memory = r.available.filter(|a| a.4 < 1_000);
    if let Some((a, t, available, total, bp)) = low_memory {
        let mut d = Draft::new(
            Classification::LowAvailableMemory,
            "Available physical memory is low in this sample. Memory contention is plausible but unproven.",
            vec![fact(
                format!("Available physical memory was {} of {} ({}).", gib(available), gib(total), percent(bp)),
                &[a, t],
            )],
        );
        d.hypothesis = Some("Memory contention may contribute to the reported symptom.");
        if let Some((_, _, _, _, commit_bp)) = r.commit.filter(|c| c.4 < 8_000) {
            d.counter_evidence.push(format!(
                "System commit is {} of its limit, so commit pressure is not also observed.",
                percent(commit_bp)
            ));
        }
        d.next_step =
            "Inspect process memory contributors and observe whether low availability persists.";
        drafts.push(d);
    }
    if let Some((u, l, used, limit, bp)) = r.commit.filter(|c| c.4 >= 9_000) {
        let mut d = Draft::new(
            Classification::HighCommitRatio,
            "System commit is close to its current limit. This does not measure physical RAM occupancy or page-file activity.",
            vec![fact(
                format!("System commit was {} of a {} limit ({}).", gib(used), gib(limit), percent(bp)),
                &[u, l],
            )],
        );
        d.hypothesis =
            Some("Allocation failures or page-file growth may occur if commit keeps rising.");
        d.limitations.push(
            "Commit limit includes page files; it is not a RAM purchase recommendation.".into(),
        );
        drafts.push(d);
    }
    if let Some((e, busy)) = r.cpu.filter(|c| c.1 >= 9_000) {
        let mut d = Draft::new(
            Classification::HighCpuSample,
            "Aggregate CPU activity is high during this short observation. Its connection to the reported symptom needs more evidence.",
            vec![fact(format!("Aggregate CPU busy was {} of total capacity over {} ms.", percent(busy), e.window.duration_ms), &[e])],
        );
        d.hypothesis = Some("CPU contention may contribute to the reported symptom.");
        drafts.push(d);
    }
    if let Some((p, e, bytes)) = largest_private_commit(snapshot) {
        let mut d = Draft::new(
            Classification::LargestMemoryContributor,
            "This readable process instance has the largest private commit observed. Size alone does not indicate a leak, malware or uselessness.",
            vec![fact(format!("Process {p} had {} private commit.", gib(bytes)), &[e])],
        );
        (d.support, d.support_reason) = if low_memory.is_some() {
            (Support::Plausible, "Observed together with low available memory in the same sample; one sample cannot attribute a slowdown.")
        } else {
            (Support::Insufficient, "No memory contention is observed in this sample; a large allocation is not a problem by itself.")
        };
        d.limitations.push(
            "Inaccessible processes may be larger; working sets and commit are not exclusive RAM."
                .into(),
        );
        d.next_step = "Observe whether this instance grows under the same workload before drawing conclusions.";
        drafts.push(d);
    }
    if let (Some(a), Some(c), Some((e, busy))) = (r.available, r.commit, r.cpu) {
        if a.4 >= 2_000 && c.4 < 8_000 && busy < 5_000 {
            let mut d = Draft::new(
                Classification::NoContentionObserved,
                "No memory or aggregate CPU contention was observed in this sample. No software intervention is identified from these measurements.",
                vec![
                    fact(format!("Available physical memory was {}.", percent(a.4)), &[a.0, a.1]),
                    fact(format!("System commit was {} of its limit.", percent(c.4)), &[c.0, c.1]),
                    fact(format!("Aggregate CPU busy was {}.", percent(busy)), &[e]),
                ],
            );
            d.counter_evidence.push(
                "A moderate aggregate CPU value does not exclude a single saturated thread.".into(),
            );
            d.limitations.push(NOT_INSTRUMENTED.into());
            drafts.push(d);
        }
    }
    let mut abstain = Draft::new(
        Classification::InsufficientEvidence,
        "The available measurements do not establish the cause of the symptom. No intervention or hardware purchase is justified by this report alone.",
        snapshot
            .evidence
            .iter()
            .filter(|e| e.entity_ref == "system" && e.usable_value().is_some())
            .map(|e| fact(format!("{:?} was measured by {}.", e.metric, e.source), &[e]))
            .collect(),
    );
    abstain.support = Support::Insufficient;
    abstain.support_reason = "Explicit abstention: causes remain unmeasured or unestablished.";
    abstain.limitations.push(NOT_INSTRUMENTED.into());
    drafts.push(abstain);
    drafts
        .into_iter()
        .map(|d| d.build(snapshot, &missing))
        .collect()
}

fn largest_private_commit(s: &Snapshot) -> Option<(&str, &Evidence, u64)> {
    s.processes
        .iter()
        .filter_map(|p| {
            s.evidence
                .iter()
                .find(|e| {
                    e.entity_ref == p.entity_ref && e.metric == Metric::ProcessPrivateCommitBytes
                })
                .and_then(|e| e.usable_value().map(|v| (p.entity_ref.as_str(), e, v)))
        })
        .max_by_key(|(_, _, v)| *v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{Origin, ProcessInstance, Status, Window};
    fn sample(values: &[(Metric, u64)]) -> Snapshot {
        let mut s = Snapshot::empty("synthetic-test");
        s.origin = Origin::SyntheticFixture;
        s.evidence = values
            .iter()
            .map(|(m, v)| {
                let mut e =
                    Evidence::measured(*m, "system", *v, "fixture:diagnostics", Window::instant());
                e.origin = Origin::SyntheticFixture;
                e
            })
            .collect();
        s
    }
    fn classes(f: &[Finding]) -> Vec<Classification> {
        f.iter().map(|f| f.classification).collect()
    }
    fn with_process(mut s: Snapshot, bytes: u64) -> Snapshot {
        s.processes.push(ProcessInstance {
            entity_ref: "proc-a".into(),
            pid: 1,
            creation_time_filetime: 1,
            boot_id: None,
            executable_name: None,
        });
        s.evidence.push(Evidence::measured(
            Metric::ProcessPrivateCommitBytes,
            "proc-a",
            bytes,
            "fixture",
            Window::instant(),
        ));
        s
    }
    const HEALTHY: [(Metric, u64); 5] = [
        (Metric::PhysicalAvailableBytes, 50),
        (Metric::PhysicalTotalBytes, 100),
        (Metric::SystemCommitBytes, 40),
        (Metric::SystemCommitLimitBytes, 100),
        (Metric::CpuBusyBasisPoints, 1_000),
    ];
    #[test]
    fn absent_metrics_do_not_become_a_healthy_verdict() {
        let f = analyze(&sample(&[]));
        assert_eq!(classes(&f), [Classification::InsufficientEvidence]);
        assert_eq!(f[0].missing.len(), SYSTEM_METRICS.len() + 1);
    }
    #[test]
    fn high_commit_is_not_a_ram_diagnosis() {
        let f = analyze(&sample(&[
            (Metric::SystemCommitBytes, 95),
            (Metric::SystemCommitLimitBytes, 100),
            (Metric::PhysicalAvailableBytes, 80),
            (Metric::PhysicalTotalBytes, 100),
        ]));
        let c = classes(&f);
        assert!(c.contains(&Classification::HighCommitRatio));
        assert!(!c.contains(&Classification::LowAvailableMemory));
        assert!(!c.contains(&Classification::NoContentionObserved));
    }
    #[test]
    fn a_single_sample_is_never_supported() {
        let f = analyze(&sample(&[
            (Metric::PhysicalAvailableBytes, 1),
            (Metric::PhysicalTotalBytes, 100),
            (Metric::CpuBusyBasisPoints, 9_900),
        ]));
        assert_eq!(f[0].classification, Classification::LowAvailableMemory);
        assert!(f.iter().all(|f| f.support != Support::Supported));
        assert!(f[0].hypothesis.is_some());
        assert!(f[0].missing.contains(&"system commit".to_string()));
    }
    #[test]
    fn low_memory_notes_absent_commit_pressure_as_counter_evidence() {
        let f = analyze(&sample(&[
            (Metric::PhysicalAvailableBytes, 1),
            (Metric::PhysicalTotalBytes, 100),
            (Metric::SystemCommitBytes, 10),
            (Metric::SystemCommitLimitBytes, 100),
        ]));
        assert_eq!(f[0].counter_evidence.len(), 1);
    }
    #[test]
    fn coherent_operation_is_a_valid_conclusion_with_limits() {
        let f = analyze(&sample(&HEALTHY));
        assert_eq!(
            classes(&f),
            [
                Classification::NoContentionObserved,
                Classification::InsufficientEvidence
            ]
        );
        assert!(!f[0].counter_evidence.is_empty());
        assert_eq!(f[0].facts.len(), 3);
    }
    #[test]
    fn a_large_process_is_not_accused_without_contention() {
        let f = analyze(&with_process(sample(&HEALTHY), 8 << 30));
        let p = f
            .iter()
            .find(|f| f.classification == Classification::LargestMemoryContributor)
            .unwrap();
        assert_eq!(p.support, Support::Insufficient);
        let low = with_process(
            sample(&[
                (Metric::PhysicalAvailableBytes, 1),
                (Metric::PhysicalTotalBytes, 100),
            ]),
            8 << 30,
        );
        let f = analyze(&low);
        assert_eq!(
            f[1].classification,
            Classification::LargestMemoryContributor
        );
        assert_eq!(f[1].support, Support::Plausible);
    }
    #[test]
    fn zero_denominators_and_inconsistent_counters_are_ignored() {
        for (available, total) in [(0, 0), (200, 100)] {
            let f = analyze(&sample(&[
                (Metric::PhysicalAvailableBytes, available),
                (Metric::PhysicalTotalBytes, total),
            ]));
            assert_eq!(classes(&f), [Classification::InsufficientEvidence]);
        }
        let f = analyze(&sample(&[(Metric::CpuBusyBasisPoints, 20_000)]));
        assert_eq!(classes(&f), [Classification::InsufficientEvidence]);
    }
    #[test]
    fn stale_values_cannot_trigger_a_finding() {
        let mut s = sample(&[
            (Metric::PhysicalAvailableBytes, 1),
            (Metric::PhysicalTotalBytes, 100),
        ]);
        s.evidence[0].status = Status::Stale;
        assert_eq!(
            classes(&analyze(&s)),
            [Classification::InsufficientEvidence]
        );
    }
    #[test]
    fn every_fact_refers_to_real_evidence() {
        let s = with_process(sample(&HEALTHY), 3);
        for f in analyze(&s) {
            for fact in &f.facts {
                assert!(!fact.evidence_refs.is_empty());
            }
            for id in f.evidence_refs {
                assert!(s.evidence.iter().any(|e| e.id == id));
            }
        }
    }
}
