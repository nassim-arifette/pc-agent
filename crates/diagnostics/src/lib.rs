//! Conservative observations, not a universal anomaly detector.
#![forbid(unsafe_code)]
mod compare;
pub use compare::{compare, Comparison, Direction, Outcome, WindowSummary};
use pcagent_shared::{
    gpu_slowdown_reasons, ratio_basis_points, Evidence, Metric, Snapshot,
    NVML_HW_POWER_BRAKE_SLOWDOWN, NVML_HW_SLOWDOWN, NVML_HW_THERMAL_SLOWDOWN,
    NVML_SW_THERMAL_SLOWDOWN, SCHEMA_VERSION,
};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Classification {
    LowAvailableMemory,
    HighCommitRatio,
    HighCpuSample,
    LargestMemoryContributor,
    HighDiskLatency,
    HardFaultPressure,
    SaturatedLogicalProcessor,
    HighGpuEngineSample,
    /// An NVIDIA GPU reduced its clocks for heat or a hardware limit (NVML).
    GpuClockSlowdown,
    /// An NVIDIA GPU's memory was nearly full (NVML).
    GpuMemoryPressure,
    NoContentionObserved,
    InsufficientEvidence,
}
impl Classification {
    /// Short heading for human-readable output.
    pub fn label(self) -> &'static str {
        match self {
            Self::LowAvailableMemory => "Low available memory",
            Self::HighCommitRatio => "System commit close to its limit",
            Self::HighCpuSample => "High CPU use",
            Self::LargestMemoryContributor => "Largest memory contributor",
            Self::HighDiskLatency => "High disk latency",
            Self::HardFaultPressure => "Heavy hard-fault paging",
            Self::SaturatedLogicalProcessor => "One processor core saturated",
            Self::HighGpuEngineSample => "High GPU engine use",
            Self::GpuClockSlowdown => "GPU slowed down by heat or a hardware limit",
            Self::GpuMemoryPressure => "GPU memory nearly full",
            Self::NoContentionObserved => "No contention observed",
            Self::InsufficientEvidence => "Insufficient evidence",
        }
    }
}
/// Evidence support levels (spec 8.3). Not statistical probabilities.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    Supported,
    Plausible,
    Insufficient,
}
impl Support {
    pub fn label(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Plausible => "plausible",
            Self::Insufficient => "insufficient",
        }
    }
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
pub const SYSTEM_METRICS: [(Metric, &str); 12] = [
    (Metric::PhysicalTotalBytes, "physical memory total"),
    (Metric::PhysicalAvailableBytes, "available physical memory"),
    (Metric::SystemCommitBytes, "system commit"),
    (Metric::SystemCommitLimitBytes, "system commit limit"),
    (Metric::CpuBusyBasisPoints, "aggregate CPU"),
    (Metric::CpuBusiestCoreBasisPoints, "per-core CPU"),
    (Metric::DiskReadLatencyMicroseconds, "disk read latency"),
    (Metric::DiskWriteLatencyMicroseconds, "disk write latency"),
    (Metric::DiskIdleBasisPoints, "disk idle time"),
    (Metric::DiskQueueLength, "disk queue length"),
    (Metric::HardFaultPagesPerSecondX100, "hard-fault page reads"),
    (
        Metric::GpuBusiestEngineBasisPoints,
        "GPU engine utilization",
    ),
];
/// Prototype trigger for average read or write latency (50 ms), not a device-independent rule.
const HIGH_DISK_LATENCY_US: u64 = 50_000;
/// Prototype trigger: 1 000 pages read per second to resolve hard faults.
const HIGH_HARD_FAULTS_X100: u64 = 100_000;
const DISK_AGGREGATE: &str = "Disk counters aggregate every physical disk (_Total); the affected device is not identified and one sample cannot separate a slow device from a burst of work.";
/// NVML slowdowns that are not normal operation; a power cap alone is expected under load.
const GPU_SLOWDOWN_BITS: u64 = NVML_HW_SLOWDOWN
    | NVML_SW_THERMAL_SLOWDOWN
    | NVML_HW_THERMAL_SLOWDOWN
    | NVML_HW_POWER_BRAKE_SLOWDOWN;
/// Prototype trigger: 90% of an NVIDIA GPU's memory in use (driver reserve included).
const HIGH_GPU_MEMORY_BP: u64 = 9_000;
const NO_PURCHASE: &str =
    "This observation is not a hardware replacement or purchase recommendation.";

/// Names the counters this snapshot could not measure, so gaps are explicit per snapshot.
fn unmeasured(s: &Snapshot) -> Option<String> {
    let absent: Vec<&str> = SYSTEM_METRICS[5..]
        .iter()
        .filter(|(m, _)| s.system_value(*m).is_none())
        .map(|(_, label)| *label)
        .collect();
    (!absent.is_empty()).then(|| format!("Not measured in this snapshot: {}.", absent.join(", ")))
}
fn ms(microseconds: u64) -> String {
    format!("{:.1} ms", microseconds as f64 / 1_000.0)
}
fn rate(x100: u64) -> String {
    format!("{:.1} pages/s", x100 as f64 / 100.0)
}

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
        match snapshot.system_value(Metric::HardFaultPagesPerSecondX100) {
            Some((_, faults)) if faults < HIGH_HARD_FAULTS_X100 / 10 => {
                d.counter_evidence.push(format!(
                    "Hard-fault page reads were {}, so heavy paging is not also observed.",
                    rate(faults)
                ))
            }
            _ => {}
        }
        d.next_step =
            "Inspect process memory contributors and observe whether low availability persists.";
        drafts.push(d);
    }
    if let (Some((a, t, _, _, bp)), Some((f, faults))) = (
        low_memory,
        snapshot
            .system_value(Metric::HardFaultPagesPerSecondX100)
            .filter(|(_, v)| *v >= HIGH_HARD_FAULTS_X100),
    ) {
        let mut d = Draft::new(
            Classification::HardFaultPressure,
            "Many pages were read from disk to resolve hard faults while available memory was low. Paging pressure is plausible but unproven.",
            vec![
                fact(format!("Hard-fault page reads averaged {} over {} ms.", rate(faults), f.window.duration_ms), &[f]),
                fact(format!("Available physical memory was {}.", percent(bp)), &[a, t]),
            ],
        );
        d.hypothesis = Some("Memory pressure may be forcing pages to be read back from disk.");
        if let Some((e, read)) = snapshot
            .system_value(Metric::DiskReadLatencyMicroseconds)
            .filter(|(_, v)| *v < HIGH_DISK_LATENCY_US / 5)
        {
            d.counter_evidence.push(format!(
                "Average disk read latency was {}, so disk reads were not observed to be slow.",
                ms(read)
            ));
            d.facts.push(fact(
                format!("Average disk read latency was {}.", ms(read)),
                &[e],
            ));
        }
        d.limitations.push("Pages Input/sec includes executables and mapped files being loaded; hard faults are not hardware faults.".into());
        d.limitations.push(NO_PURCHASE.into());
        d.next_step = "Observe whether hard faults and low availability persist together under the same workload.";
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
    let slow_disk = [
        (Metric::DiskReadLatencyMicroseconds, "read"),
        (Metric::DiskWriteLatencyMicroseconds, "write"),
    ]
    .into_iter()
    .filter_map(|(m, kind)| {
        snapshot
            .system_value(m)
            .filter(|(_, v)| *v >= HIGH_DISK_LATENCY_US)
            .map(|(e, v)| (e, v, kind))
    })
    .collect::<Vec<_>>();
    if !slow_disk.is_empty() {
        let mut d = Draft::new(
            Classification::HighDiskLatency,
            "Average disk request latency is high in this sample. Storage contention is plausible but unproven.",
            slow_disk
                .iter()
                .map(|(e, v, kind)| {
                    fact(
                        format!("Average disk {kind} latency was {} over {} ms.", ms(*v), e.window.duration_ms),
                        &[e],
                    )
                })
                .collect(),
        );
        d.hypothesis = Some("Slow storage requests may contribute to the reported symptom.");
        if let Some((e, idle)) = snapshot
            .system_value(Metric::DiskIdleBasisPoints)
            .filter(|(_, v)| *v >= 9_000)
        {
            d.counter_evidence.push(format!(
                "Disks were idle {} of the window; averages over few requests are noisy.",
                percent(idle)
            ));
            d.facts
                .push(fact(format!("Disk idle time was {}.", percent(idle)), &[e]));
        }
        if let Some((e, queue)) = snapshot.system_value(Metric::DiskQueueLength) {
            d.facts.push(fact(
                format!("{queue} disk requests were outstanding at the end of the window."),
                &[e],
            ));
        }
        d.limitations.push(DISK_AGGREGATE.into());
        d.limitations.push(NO_PURCHASE.into());
        d.next_step = "Observe disk latency over a longer representative workload and identify which activity issues the requests.";
        drafts.push(d);
    }
    let busiest_core = snapshot
        .system_value(Metric::CpuBusiestCoreBasisPoints)
        .filter(|(_, v)| *v >= 9_500);
    if let (Some((c, core)), Some((e, busy))) = (busiest_core, r.cpu.filter(|c| c.1 < 9_000)) {
        let mut d = Draft::new(
            Classification::SaturatedLogicalProcessor,
            "One logical processor was nearly saturated while aggregate CPU was not. A single busy thread is plausible but unproven.",
            vec![
                fact(format!("The busiest logical processor had {} processor utility.", percent(core)), &[c]),
                fact(format!("Aggregate CPU busy was {} of total capacity.", percent(busy)), &[e]),
            ],
        );
        d.hypothesis = Some("A single-threaded workload may be limited by one processor.");
        d.limitations.push("Processor utility is normalized to nominal frequency and threads migrate between processors; the busy thread is not identified.".into());
        drafts.push(d);
    }
    if let Some((g, gpu)) = snapshot
        .system_value(Metric::GpuBusiestEngineBasisPoints)
        .filter(|(_, v)| *v >= 9_500)
    {
        let mut facts = vec![fact(
            format!(
                "The busiest GPU engine was {} utilized over {} ms.",
                percent(gpu),
                g.window.duration_ms
            ),
            &[g],
        )];
        facts.extend(
            snapshot
                .evidence
                .iter()
                .filter(|e| {
                    e.metric == Metric::GpuEngineBasisPoints && e.usable_value() == Some(gpu)
                })
                .map(|e| {
                    fact(
                        format!("Engine type {} reached that value.", e.entity_ref),
                        &[e],
                    )
                }),
        );
        let mut d = Draft::new(
            Classification::HighGpuEngineSample,
            "A GPU engine was nearly fully busy during this short observation. High GPU use is expected for games and rendering and is not a fault by itself.",
            facts,
        );
        d.limitations.push("GPU engines are aggregated by generic type; dedicated memory, adapter and the using process are not reported.".into());
        d.limitations.push(NO_PURCHASE.into());
        drafts.push(d);
    }
    nvidia_findings(snapshot, &mut drafts);
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
        let contended = drafts.iter().any(|d: &Draft| {
            matches!(
                d.class,
                Classification::HighDiskLatency
                    | Classification::HardFaultPressure
                    | Classification::SaturatedLogicalProcessor
                    | Classification::HighGpuEngineSample
                    | Classification::GpuClockSlowdown
                    | Classification::GpuMemoryPressure
            )
        });
        if a.4 >= 2_000 && c.4 < 8_000 && busy < 5_000 && !contended {
            let mut d = Draft::new(
                Classification::NoContentionObserved,
                "No memory or aggregate CPU contention was observed in this sample. No software intervention is identified from these measurements.",
                vec![
                    fact(format!("Available physical memory was {}.", percent(a.4)), &[a.0, a.1]),
                    fact(format!("System commit was {} of its limit.", percent(c.4)), &[c.0, c.1]),
                    fact(format!("Aggregate CPU busy was {}.", percent(busy)), &[e]),
                ],
            );
            match snapshot.system_value(Metric::CpuBusiestCoreBasisPoints) {
                Some((c, core)) => d.facts.push(fact(
                    format!(
                        "The busiest logical processor had {} processor utility.",
                        percent(core)
                    ),
                    &[c],
                )),
                None => d.counter_evidence.push(
                    "A moderate aggregate CPU value does not exclude a single saturated thread."
                        .into(),
                ),
            }
            for (m, kind) in [
                (Metric::DiskReadLatencyMicroseconds, "read"),
                (Metric::DiskWriteLatencyMicroseconds, "write"),
            ] {
                if let Some((e, v)) = snapshot.system_value(m) {
                    d.facts.push(fact(
                        format!("Average disk {kind} latency was {}.", ms(v)),
                        &[e],
                    ));
                }
            }
            if let Some((f, faults)) = snapshot.system_value(Metric::HardFaultPagesPerSecondX100) {
                d.facts.push(fact(
                    format!("Hard-fault page reads were {}.", rate(faults)),
                    &[f],
                ));
            }
            d.limitations.extend(unmeasured(snapshot));
            drafts.push(d);
        }
    }
    let mut abstain = Draft::new(
        Classification::InsufficientEvidence,
        "The available measurements do not establish the cause of the symptom. No intervention or hardware purchase is justified by this report alone.",
        snapshot
            .evidence
            .iter()
            .filter(|e| e.entity_ref == "system")
            .filter_map(|e| {
                let value = e.usable_value()?;
                let text = format!("{}: {}.", e.metric.label(), e.metric.format_value(value));
                Some(fact(text, &[e]))
            })
            .collect(),
    );
    abstain.support = Support::Insufficient;
    abstain.support_reason = "Explicit abstention: causes remain unmeasured or unestablished.";
    abstain.limitations.extend(unmeasured(snapshot));
    drafts.push(abstain);
    drafts
        .into_iter()
        .map(|d| d.build(snapshot, &missing))
        .collect()
}

/// NVIDIA devices read through NVML (`gpu-nvidia-<index>` entities).
fn nvidia_findings(snapshot: &Snapshot, drafts: &mut Vec<Draft>) {
    let mut devices: Vec<&str> = Vec::new();
    for e in &snapshot.evidence {
        if e.entity_ref.starts_with("gpu-nvidia-") && !devices.contains(&e.entity_ref.as_str()) {
            devices.push(&e.entity_ref);
        }
    }
    let value = |entity: &str, metric: Metric| {
        snapshot
            .evidence
            .iter()
            .find(|e| e.entity_ref == entity && e.metric == metric)
            .and_then(|e| e.usable_value().map(|v| (e, v)))
    };
    let (mut slow, mut full) = (Vec::new(), Vec::new());
    for entity in devices {
        let name = format!("NVIDIA GPU {}", entity.trim_start_matches("gpu-nvidia-"));
        if let Some((e, bits)) = value(entity, Metric::GpuSlowdownReasons)
            .filter(|(_, bits)| bits & GPU_SLOWDOWN_BITS != 0)
        {
            slow.push(fact(
                format!(
                    "{name} reported slowdown reasons: {}.",
                    gpu_slowdown_reasons(bits).join(", ")
                ),
                &[e],
            ));
            if let Some((t, celsius)) = value(entity, Metric::GpuTemperatureCelsius) {
                slow.push(fact(format!("{name} was at {celsius} °C."), &[t]));
            }
        }
        if let (Some((u, used)), Some((t, total))) = (
            value(entity, Metric::GpuMemoryUsedBytes),
            value(entity, Metric::GpuMemoryTotalBytes),
        ) {
            if let Some(bp) = ratio_basis_points(used, total).filter(|bp| *bp >= HIGH_GPU_MEMORY_BP)
            {
                full.push(fact(
                    format!(
                        "{name} had {} of {} memory in use ({}).",
                        gib(used),
                        gib(total),
                        percent(bp)
                    ),
                    &[u, t],
                ));
            }
        }
    }
    if !slow.is_empty() {
        let mut d = Draft::new(
            Classification::GpuClockSlowdown,
            "An NVIDIA GPU was reducing its clocks because of heat or a hardware limit during this sample. Laptops do this near their limits; a sustained slowdown under load makes games and rendering slower.",
            slow,
        );
        d.hypothesis = Some("If it persists under the same workload, cooling (dust, blocked vents, a soft surface under a laptop) or the power supply may be limiting the GPU.");
        d.next_step = "Check the vents and airflow, then observe the GPU temperature and slowdown reasons again under the same workload.";
        d.limitations.push(
            "NVML reports the driver's state at one instant; a short spike can set a reason."
                .into(),
        );
        d.limitations.push(NO_PURCHASE.into());
        drafts.push(d);
    }
    if !full.is_empty() {
        let mut d = Draft::new(
            Classification::GpuMemoryPressure,
            "An NVIDIA GPU's memory was nearly full during this sample. Games and applications that need more video memory can stutter when it is full.",
            full,
        );
        d.next_step = "Close other GPU-heavy applications or lower texture and resolution settings, then observe GPU memory again under the same workload.";
        d.limitations.push("Memory in use includes the driver's reserve; the process holding it is not reported on Windows.".into());
        d.limitations.push(NO_PURCHASE.into());
        drafts.push(d);
    }
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
    fn healthy_with(extra: &[(Metric, u64)]) -> Snapshot {
        let mut v = HEALTHY.to_vec();
        v.extend_from_slice(extra);
        sample(&v)
    }
    fn finding(f: &[Finding], c: Classification) -> Option<&Finding> {
        f.iter().find(|f| f.classification == c)
    }
    #[test]
    fn high_disk_latency_is_reported_with_refs_counter_evidence_and_limits() {
        let s = healthy_with(&[
            (Metric::DiskReadLatencyMicroseconds, 80_000),
            (Metric::DiskWriteLatencyMicroseconds, 2_000),
            (Metric::DiskIdleBasisPoints, 9_500),
            (Metric::DiskQueueLength, 0),
        ]);
        let f = analyze(&s);
        let d = finding(&f, Classification::HighDiskLatency).unwrap();
        assert_eq!(d.support, Support::Plausible);
        assert_eq!(d.facts.len(), 3);
        assert!(d.facts[0].text.contains("read latency was 80.0 ms"));
        assert_eq!(d.counter_evidence.len(), 1);
        assert!(d.limitations.iter().any(|l| l.contains("_Total")));
        assert!(d.limitations.iter().any(|l| l.contains("not a hardware")));
        assert!(finding(&f, Classification::NoContentionObserved).is_none());
        let fast = analyze(&healthy_with(&[(
            Metric::DiskReadLatencyMicroseconds,
            49_999,
        )]));
        assert!(finding(&fast, Classification::HighDiskLatency).is_none());
    }
    #[test]
    fn hard_faults_need_low_available_memory() {
        let faults = (Metric::HardFaultPagesPerSecondX100, 250_000);
        let f = analyze(&healthy_with(&[faults]));
        assert!(finding(&f, Classification::HardFaultPressure).is_none());
        let s = sample(&[
            (Metric::PhysicalAvailableBytes, 3),
            (Metric::PhysicalTotalBytes, 100),
            faults,
            (Metric::DiskReadLatencyMicroseconds, 1_000),
        ]);
        let f = analyze(&s);
        let h = finding(&f, Classification::HardFaultPressure).unwrap();
        assert_eq!(h.support, Support::Plausible);
        assert_eq!(h.evidence_refs.len(), 4);
        assert_eq!(h.counter_evidence.len(), 1);
        assert!(h.facts[0].text.contains("2500.0 pages/s"));
        let quiet = analyze(&sample(&[
            (Metric::PhysicalAvailableBytes, 3),
            (Metric::PhysicalTotalBytes, 100),
            (Metric::HardFaultPagesPerSecondX100, 500),
        ]));
        let low = finding(&quiet, Classification::LowAvailableMemory).unwrap();
        assert!(low.counter_evidence[0].contains("5.0 pages/s"));
    }
    #[test]
    fn a_saturated_core_is_distinguished_from_aggregate_cpu() {
        let f = analyze(&healthy_with(&[(Metric::CpuBusiestCoreBasisPoints, 9_900)]));
        assert!(finding(&f, Classification::SaturatedLogicalProcessor).is_some());
        assert!(finding(&f, Classification::NoContentionObserved).is_none());
        let calm = analyze(&healthy_with(&[(Metric::CpuBusiestCoreBasisPoints, 3_000)]));
        let n = finding(&calm, Classification::NoContentionObserved).unwrap();
        assert_eq!(n.facts.len(), 4);
        assert!(n.counter_evidence.is_empty());
    }
    #[test]
    fn busy_gpu_names_only_a_generic_engine_type() {
        let mut s = healthy_with(&[(Metric::GpuBusiestEngineBasisPoints, 9_800)]);
        s.evidence.push(Evidence::measured(
            Metric::GpuEngineBasisPoints,
            "gpu-engine-3d",
            9_800,
            "fixture",
            Window::instant(),
        ));
        let f = analyze(&s);
        let g = finding(&f, Classification::HighGpuEngineSample).unwrap();
        assert_eq!(g.facts.len(), 2);
        assert!(g.facts[1].text.contains("gpu-engine-3d"));
        assert!(g.hypothesis.is_none());
    }
    #[test]
    fn unsupported_counters_stay_listed_as_unmeasured() {
        let mut s = healthy_with(&[]);
        s.evidence.push(Evidence::missing(
            Metric::GpuBusiestEngineBasisPoints,
            "system",
            "fixture",
            Status::Unsupported,
            "no GPU counters",
        ));
        let f = analyze(&s);
        let abstain = f.last().unwrap();
        let note = abstain
            .limitations
            .iter()
            .find(|l| l.starts_with("Not measured"))
            .unwrap();
        assert!(note.contains("GPU engine utilization") && note.contains("disk read latency"));
        assert!(abstain
            .missing
            .contains(&"GPU engine utilization".to_string()));
        let all = healthy_with(&[
            (Metric::CpuBusiestCoreBasisPoints, 1_000),
            (Metric::DiskReadLatencyMicroseconds, 1_000),
            (Metric::DiskWriteLatencyMicroseconds, 1_000),
            (Metric::DiskIdleBasisPoints, 9_000),
            (Metric::DiskQueueLength, 0),
            (Metric::HardFaultPagesPerSecondX100, 0),
            (Metric::GpuBusiestEngineBasisPoints, 100),
        ]);
        for f in analyze(&all) {
            assert!(!f.limitations.iter().any(|l| l.starts_with("Not measured")));
            assert!(!f.limitations.iter().any(|l| l.contains("not instrumented")));
        }
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
    #[test]
    fn abstention_facts_state_measured_values() {
        let s = sample(&[
            (Metric::CpuBusyBasisPoints, 1_124),
            (Metric::DiskIdleBasisPoints, 9_806),
        ]);
        let findings = analyze(&s);
        let abstain = findings
            .iter()
            .find(|f| f.classification == Classification::InsufficientEvidence)
            .unwrap();
        let texts: Vec<&str> = abstain.facts.iter().map(|f| f.text.as_str()).collect();
        assert_eq!(texts, ["CPU (all cores): 11.2%.", "Disk idle time: 98.1%."]);
    }
    #[test]
    fn nvidia_slowdowns_and_full_memory_are_findings_but_a_power_cap_is_not() {
        use pcagent_shared::{NVML_HW_THERMAL_SLOWDOWN, NVML_SW_POWER_CAP};
        let gpu = |bits: u64, used: u64| {
            let mut s = sample(&[]);
            for (metric, value) in [
                (Metric::GpuSlowdownReasons, bits),
                (Metric::GpuMemoryUsedBytes, used),
                (Metric::GpuMemoryTotalBytes, 2 << 30),
                (Metric::GpuTemperatureCelsius, 87),
            ] {
                s.evidence.push(Evidence::measured(
                    metric,
                    "gpu-nvidia-0",
                    value,
                    "fixture:nvml",
                    Window::instant(),
                ));
            }
            analyze(&s)
        };
        let hot = gpu(NVML_HW_THERMAL_SLOWDOWN, 1 << 30);
        let slow = finding(&hot, Classification::GpuClockSlowdown).unwrap();
        assert_eq!(slow.support, Support::Plausible);
        assert_eq!(
            slow.facts[0].text,
            "NVIDIA GPU 0 reported slowdown reasons: thermal (hardware)."
        );
        assert_eq!(slow.facts[1].text, "NVIDIA GPU 0 was at 87 °C.");
        assert!(slow.limitations.iter().any(|l| l.contains("purchase")));
        assert!(finding(&hot, Classification::GpuMemoryPressure).is_none());

        let capped = gpu(NVML_SW_POWER_CAP, (2 << 30) / 100 * 95);
        assert!(finding(&capped, Classification::GpuClockSlowdown).is_none());
        let full = finding(&capped, Classification::GpuMemoryPressure).unwrap();
        assert_eq!(
            full.facts[0].text,
            "NVIDIA GPU 0 had 1.90 GiB of 2.00 GiB memory in use (95.0%)."
        );
        assert_eq!(full.evidence_refs.len(), 2);
    }
}
