//! Portable conversion of performance-counter readings into evidence. Windows supplies the
//! formatted doubles; everything here is deterministic and fixture-tested.
use pcagent_shared::{Evidence, Metric, Status, Window};
use std::time::Duration;

/// Minimum spacing between the two collections of a rate counter (PDH guidance: one second).
pub const MIN_RATE_WINDOW: Duration = Duration::from_secs(1);

pub const SRC_DISK_READ: &str = r"PDH:\PhysicalDisk(_Total)\Avg. Disk sec/Read";
pub const SRC_DISK_WRITE: &str = r"PDH:\PhysicalDisk(_Total)\Avg. Disk sec/Write";
pub const SRC_DISK_IDLE: &str = r"PDH:\PhysicalDisk(_Total)\% Idle Time";
pub const SRC_DISK_QUEUE: &str = r"PDH:\PhysicalDisk(_Total)\Current Disk Queue Length";
pub const SRC_HARD_FAULTS: &str = r"PDH:\Memory\Pages Input/sec";
pub const SRC_CORES: &str = r"PDH:\Processor Information(*)\% Processor Utility";
pub const SRC_GPU: &str = r"PDH:\GPU Engine(*)\Utilization Percentage";

/// A counter that could not be read, with the reason kept for evidence limitations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub status: Status,
    pub reason: String,
}
impl Unavailable {
    pub fn new(status: Status, reason: impl Into<String>) -> Self {
        Self {
            status,
            reason: reason.into(),
        }
    }
}
pub type Sampled<T> = Result<T, Unavailable>;

/// Formatted instances of a wildcard counter. `invalid` counts instances whose data status
/// was not valid (for example instances that appeared between the two collections).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Instances {
    pub valid: Vec<(String, f64)>,
    pub invalid: usize,
}

/// One two-collection reading of every system counter.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterReadings {
    pub disk_read_seconds: Sampled<f64>,
    pub disk_write_seconds: Sampled<f64>,
    pub disk_idle_percent: Sampled<f64>,
    pub disk_queue_length: Sampled<f64>,
    pub pages_input_per_second: Sampled<f64>,
    pub core_utility_percent: Sampled<Instances>,
    pub gpu_engine_percent: Sampled<Instances>,
}
impl CounterReadings {
    /// Every counter unavailable for the same reason.
    pub fn unavailable(u: &Unavailable) -> Self {
        Self {
            disk_read_seconds: Err(u.clone()),
            disk_write_seconds: Err(u.clone()),
            disk_idle_percent: Err(u.clone()),
            disk_queue_length: Err(u.clone()),
            pages_input_per_second: Err(u.clone()),
            core_utility_percent: Err(u.clone()),
            gpu_engine_percent: Err(u.clone()),
        }
    }
}

/// Non-negative finite value times `factor`, rounded; negative, NaN, infinite or overflowing
/// values are unknown.
pub fn scaled(value: f64, factor: u32) -> Option<u64> {
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let v = (value * f64::from(factor)).round();
    // u64::MAX as f64 rounds up to 2^64, so a strict comparison keeps the cast exact-in-range.
    (v < u64::MAX as f64).then_some(v as u64)
}

/// A share in basis points: ≤5 % overshoot (sampling granularity) is clamped, more is unknown.
pub fn clamp_share(basis_points: u64) -> Option<u64> {
    match basis_points {
        v @ 0..=10_000 => Some(v),
        10_001..=10_500 => Some(10_000),
        _ => None,
    }
}

/// Busiest logical processor, excluding `_Total` aggregates such as `_Total` and `0,_Total`.
/// Returns (value, logical processors considered, rejected values).
pub fn busiest_core(instances: &Instances) -> (Option<u64>, usize, usize) {
    let mut max = None;
    let (mut cores, mut rejected) = (0, 0);
    for (name, value) in &instances.valid {
        if name.contains("_Total") {
            continue;
        }
        match scaled(*value, 100) {
            Some(bp) => {
                cores += 1;
                max = max.max(Some(bp));
            }
            None => rejected += 1,
        }
    }
    (max, cores, rejected)
}

/// Generic engine type from a `GPU Engine` instance name such as
/// `pid_1234_luid_0x0_0x1_phys_0_eng_0_engtype_3D`: lowercase ASCII letters and digits without
/// a trailing index (`Compute_0` → `compute`). Only known generic types are kept; anything else
/// (driver-specific names) is `other`, so no device naming reaches the evidence.
pub fn gpu_engine_type(instance: &str) -> String {
    let raw = instance.rsplit_once("_engtype_").map_or("", |(_, t)| t);
    let mut t: String = raw
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if t.chars().any(|c| c.is_ascii_alphabetic()) {
        while t.ends_with(|c: char| c.is_ascii_digit()) {
            t.pop();
        }
    }
    if GPU_ENGINE_TYPES.contains(&t.as_str()) {
        t
    } else {
        "other".into()
    }
}

const GPU_ENGINE_TYPES: [&str; 12] = [
    "3d",
    "compute",
    "copy",
    "videodecode",
    "videoencode",
    "videoprocessing",
    "overlay",
    "legacyoverlay",
    "security",
    "crypto",
    "vr",
    "graphics",
];

/// Physical engine identity (adapter LUID, physical adapter and engine index) without the PID.
fn gpu_engine_key(instance: &str) -> Option<&str> {
    let start = instance.find("luid_")?;
    let end = instance.rfind("_engtype_")?;
    instance.get(start..end)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuSummary {
    /// Busiest engine of each generic type, sorted by type.
    pub by_type: Vec<(String, u64)>,
    pub engines: usize,
    pub rejected: usize,
}

/// Sums per-process instances of each physical engine, then keeps the busiest engine per type.
pub fn gpu_engines(instances: &Instances) -> GpuSummary {
    use std::collections::BTreeMap;
    let mut engines: BTreeMap<&str, (String, Option<u64>)> = BTreeMap::new();
    let mut rejected = 0;
    for (name, value) in &instances.valid {
        let Some(key) = gpu_engine_key(name) else {
            rejected += 1;
            continue;
        };
        let entry = engines
            .entry(key)
            .or_insert_with(|| (gpu_engine_type(name), Some(0)));
        entry.1 = match (entry.1, scaled(*value, 100)) {
            (Some(sum), Some(v)) => sum.checked_add(v),
            _ => None,
        };
    }
    let mut by_type: BTreeMap<String, u64> = BTreeMap::new();
    let mut counted = 0;
    for (kind, sum) in engines.into_values() {
        match sum.and_then(clamp_share) {
            Some(v) => {
                counted += 1;
                let slot = by_type.entry(kind).or_insert(0);
                *slot = (*slot).max(v);
            }
            None => rejected += 1,
        }
    }
    GpuSummary {
        by_type: by_type.into_iter().collect(),
        engines: counted,
        rejected,
    }
}

const DISK_TOTAL: &str = "_Total aggregates every physical disk; one slow device can be diluted or dominate, and the device is not identified.";
const WINDOW_AVERAGE: &str = "Average over the window; short spikes are not visible.";
const HARD_FAULTS: &str = "Pages Input/sec counts pages read to resolve hard faults, including executables and mapped files; it does not prove RAM shortage by itself.";
const UTILITY: &str = "Processor Utility normalizes work to nominal frequency and can exceed 10 000 with turbo; it may differ from % Processor Time.";
const GPU: &str = "GPU Engine instance names are provided by the graphics driver stack; engines are grouped by generic type and the busiest engine of each type is reported. Adapter and process identities are not kept.";

fn one(
    metric: Metric,
    source: &str,
    reading: &Sampled<f64>,
    window: &Window,
    convert: impl Fn(f64) -> Option<u64>,
    notes: &[&str],
) -> Evidence {
    let mut e = match reading {
        Ok(v) => match convert(*v) {
            Some(value) => Evidence::measured(metric, "system", value, source, window.clone()),
            None => Evidence::missing(
                metric,
                "system",
                source,
                Status::Partial,
                "Counter returned a negative, non-finite or out-of-range value.",
            ),
        },
        Err(u) => Evidence::missing(metric, "system", source, u.status, &u.reason),
    };
    e.limitations.extend(notes.iter().map(|n| (*n).to_string()));
    e
}

/// Converts one reading into system (and generic GPU engine type) evidence over `window`.
pub fn counter_evidence(r: &CounterReadings, window: &Window) -> Vec<Evidence> {
    let end = Window {
        started_at_unix_ms: window.finished_at_unix_ms,
        finished_at_unix_ms: window.finished_at_unix_ms,
        duration_ms: 0,
    };
    let mut out = vec![
        one(
            Metric::DiskReadLatencyMicroseconds,
            SRC_DISK_READ,
            &r.disk_read_seconds,
            window,
            |s| scaled(s, 1_000_000),
            &[DISK_TOTAL, WINDOW_AVERAGE],
        ),
        one(
            Metric::DiskWriteLatencyMicroseconds,
            SRC_DISK_WRITE,
            &r.disk_write_seconds,
            window,
            |s| scaled(s, 1_000_000),
            &[DISK_TOTAL, WINDOW_AVERAGE],
        ),
        one(
            Metric::DiskIdleBasisPoints,
            SRC_DISK_IDLE,
            &r.disk_idle_percent,
            window,
            |p| scaled(p, 100).and_then(clamp_share),
            &[DISK_TOTAL],
        ),
        one(
            Metric::DiskQueueLength,
            SRC_DISK_QUEUE,
            &r.disk_queue_length,
            &end,
            |q| scaled(q, 1),
            &[DISK_TOTAL, "Instantaneous value at the end of the window; no universal queue threshold applies to every device."],
        ),
        one(
            Metric::HardFaultPagesPerSecondX100,
            SRC_HARD_FAULTS,
            &r.pages_input_per_second,
            window,
            |p| scaled(p, 100),
            &[HARD_FAULTS],
        ),
    ];
    out.push(match &r.core_utility_percent {
        Ok(instances) => {
            let (value, cores, rejected) = busiest_core(instances);
            let mut e = match value {
                Some(v) => Evidence::measured(
                    Metric::CpuBusiestCoreBasisPoints,
                    "system",
                    v,
                    SRC_CORES,
                    window.clone(),
                ),
                None => Evidence::missing(
                    Metric::CpuBusiestCoreBasisPoints,
                    "system",
                    SRC_CORES,
                    Status::Partial,
                    "No valid per-processor instance was reported.",
                ),
            };
            if value.is_some() && (rejected > 0 || instances.invalid > 0) {
                e.status = Status::Partial;
            }
            e.limitations.push(format!(
                "Maximum over {cores} logical processors; {} instances were invalid or rejected.",
                rejected + instances.invalid
            ));
            e.limitations.push(UTILITY.into());
            e
        }
        Err(u) => {
            let mut e = Evidence::missing(
                Metric::CpuBusiestCoreBasisPoints,
                "system",
                SRC_CORES,
                u.status,
                &u.reason,
            );
            e.limitations.push(UTILITY.into());
            e
        }
    });
    out.extend(gpu_evidence(&r.gpu_engine_percent, window));
    out
}

fn gpu_evidence(reading: &Sampled<Instances>, window: &Window) -> Vec<Evidence> {
    let busiest = Metric::GpuBusiestEngineBasisPoints;
    let instances = match reading {
        Ok(i) => i,
        Err(u) => {
            let mut e = Evidence::missing(busiest, "system", SRC_GPU, u.status, &u.reason);
            e.limitations.push(GPU.into());
            return vec![e];
        }
    };
    let summary = gpu_engines(instances);
    let Some(max) = summary.by_type.iter().map(|(_, v)| *v).max() else {
        let (status, reason) = if instances.valid.is_empty() && instances.invalid == 0 {
            (
                Status::Unsupported,
                "No GPU engine instances were reported (no WDDM GPU counters, e.g. basic display adapter or VM).",
            )
        } else {
            (
                Status::Partial,
                "GPU engine instances were reported but none had a usable value.",
            )
        };
        let mut e = Evidence::missing(busiest, "system", SRC_GPU, status, reason);
        e.limitations.push(GPU.into());
        return vec![e];
    };
    let partial = summary.rejected > 0;
    let coverage = format!(
        "{} physical engines aggregated; {} per-process instances invalid between collections, {} rejected.",
        summary.engines, instances.invalid, summary.rejected
    );
    let mut out = Vec::with_capacity(summary.by_type.len() + 1);
    for (metric, entity, value) in std::iter::once((busiest, "system".to_string(), max)).chain(
        summary
            .by_type
            .iter()
            .map(|(t, v)| (Metric::GpuEngineBasisPoints, format!("gpu-engine-{t}"), *v)),
    ) {
        let mut e = Evidence::measured(metric, &entity, value, SRC_GPU, window.clone());
        if partial {
            e.status = Status::Partial;
        }
        e.limitations.push(coverage.clone());
        e.limitations.push(GPU.into());
        out.push(e);
    }
    out
}

/// Every counter-derived system metric, in the order `counter_evidence` emits them.
pub const COUNTER_METRICS: [Metric; 7] = [
    Metric::DiskReadLatencyMicroseconds,
    Metric::DiskWriteLatencyMicroseconds,
    Metric::DiskIdleBasisPoints,
    Metric::DiskQueueLength,
    Metric::HardFaultPagesPerSecondX100,
    Metric::CpuBusiestCoreBasisPoints,
    Metric::GpuBusiestEngineBasisPoints,
];

#[cfg(test)]
mod tests {
    use super::*;
    fn window() -> Window {
        Window {
            started_at_unix_ms: 1_000,
            finished_at_unix_ms: 2_000,
            duration_ms: 1_000,
        }
    }
    fn inst(items: &[(&str, f64)]) -> Instances {
        Instances {
            valid: items.iter().map(|(n, v)| ((*n).into(), *v)).collect(),
            invalid: 0,
        }
    }
    fn readings() -> CounterReadings {
        CounterReadings {
            disk_read_seconds: Ok(0.0125),
            disk_write_seconds: Ok(0.0),
            disk_idle_percent: Ok(100.2),
            disk_queue_length: Ok(3.0),
            pages_input_per_second: Ok(12.345),
            core_utility_percent: Ok(inst(&[
                ("_Total", 20.0),
                ("0,_Total", 20.0),
                ("0,0", 97.5),
                ("0,1", 3.0),
            ])),
            gpu_engine_percent: Ok(inst(&[
                ("pid_4_luid_0x0_0xA_phys_0_eng_0_engtype_3D", 30.0),
                ("pid_8_luid_0x0_0xA_phys_0_eng_0_engtype_3D", 25.5),
                ("pid_8_luid_0x0_0xA_phys_0_eng_3_engtype_VideoDecode", 4.0),
                ("pid_8_luid_0x0_0xA_phys_0_eng_5_engtype_Compute_0", 1.0),
            ])),
        }
    }
    fn value(ev: &[Evidence], m: Metric, entity: &str) -> Option<u64> {
        ev.iter()
            .find(|e| e.metric == m && e.entity_ref == entity)
            .and_then(Evidence::usable_value)
    }
    #[test]
    fn scaling_is_checked() {
        assert_eq!(scaled(0.0125, 1_000_000), Some(12_500));
        assert_eq!(scaled(0.0, 100), Some(0));
        assert_eq!(scaled(-0.1, 100), None);
        assert_eq!(scaled(f64::NAN, 100), None);
        assert_eq!(scaled(f64::INFINITY, 1), None);
        assert_eq!(scaled(1e30, 1_000_000), None);
        assert_eq!(clamp_share(10_400), Some(10_000));
        assert_eq!(clamp_share(12_000), None);
    }
    #[test]
    fn readings_become_explicit_units_over_the_window() {
        let ev = counter_evidence(&readings(), &window());
        assert_eq!(
            value(&ev, Metric::DiskReadLatencyMicroseconds, "system"),
            Some(12_500)
        );
        assert_eq!(
            value(&ev, Metric::DiskWriteLatencyMicroseconds, "system"),
            Some(0)
        );
        assert_eq!(
            value(&ev, Metric::DiskIdleBasisPoints, "system"),
            Some(10_000)
        );
        assert_eq!(value(&ev, Metric::DiskQueueLength, "system"), Some(3));
        assert_eq!(
            value(&ev, Metric::HardFaultPagesPerSecondX100, "system"),
            Some(1_235)
        );
        assert_eq!(
            value(&ev, Metric::CpuBusiestCoreBasisPoints, "system"),
            Some(9_750)
        );
        assert_eq!(
            value(&ev, Metric::GpuBusiestEngineBasisPoints, "system"),
            Some(5_550)
        );
        assert_eq!(
            value(&ev, Metric::GpuEngineBasisPoints, "gpu-engine-3d"),
            Some(5_550)
        );
        assert_eq!(
            value(&ev, Metric::GpuEngineBasisPoints, "gpu-engine-videodecode"),
            Some(400)
        );
        assert_eq!(
            value(&ev, Metric::GpuEngineBasisPoints, "gpu-engine-compute"),
            Some(100)
        );
        for e in &ev {
            assert_eq!(e.unit, e.metric.unit());
            if e.metric == Metric::DiskQueueLength {
                assert_eq!(e.window.duration_ms, 0);
            } else {
                assert_eq!(e.window.duration_ms, 1_000);
            }
        }
        for m in COUNTER_METRICS {
            assert!(ev.iter().any(|e| e.metric == m && e.entity_ref == "system"));
        }
    }
    #[test]
    fn cloud_visible_fields_carry_no_instance_identity() {
        let ev = counter_evidence(&readings(), &window());
        let text = serde_json::to_string(&ev).unwrap();
        assert!(!text.contains("pid_"));
        assert!(!text.contains("luid"));
        assert!(!text.contains("0xA"));
    }
    #[test]
    fn missing_counters_stay_unknown_with_their_status() {
        let u = Unavailable::new(Status::Unsupported, "counter absent");
        let ev = counter_evidence(&CounterReadings::unavailable(&u), &window());
        assert_eq!(ev.len(), COUNTER_METRICS.len());
        assert!(ev
            .iter()
            .all(|e| e.value.is_none() && e.status == Status::Unsupported));
        let mut r = readings();
        r.gpu_engine_percent = Ok(Instances::default());
        r.disk_read_seconds = Ok(-1.0);
        let ev = counter_evidence(&r, &window());
        let gpu = ev
            .iter()
            .find(|e| e.metric == Metric::GpuBusiestEngineBasisPoints)
            .unwrap();
        assert_eq!(gpu.status, Status::Unsupported);
        assert!(gpu.value.is_none());
        assert!(!ev.iter().any(|e| e.metric == Metric::GpuEngineBasisPoints));
        let read = ev
            .iter()
            .find(|e| e.metric == Metric::DiskReadLatencyMicroseconds)
            .unwrap();
        assert_eq!((read.status, read.value), (Status::Partial, None));
    }
    #[test]
    fn per_core_excludes_totals_and_flags_rejects() {
        let (v, cores, rejected) = busiest_core(&inst(&[
            ("_Total", 99.0),
            ("0,_Total", 99.0),
            ("0,0", 10.0),
            ("0,1", f64::NAN),
            ("1,0", 110.0),
        ]));
        assert_eq!((v, cores, rejected), (Some(11_000), 2, 1));
        assert_eq!(busiest_core(&inst(&[("_Total", 5.0)])), (None, 0, 0));
    }
    #[test]
    fn gpu_types_are_generic_and_overflow_is_rejected() {
        assert_eq!(
            gpu_engine_type("pid_1_luid_x_phys_0_eng_0_engtype_3D"),
            "3d"
        );
        assert_eq!(
            gpu_engine_type("pid_1_luid_x_phys_0_eng_0_engtype_Compute_1"),
            "compute"
        );
        assert_eq!(gpu_engine_type("garbage"), "other");
        assert_eq!(gpu_engine_type("x_engtype_\\\\secret\\path"), "other");
        let s = gpu_engines(&inst(&[
            ("pid_1_luid_a_phys_0_eng_0_engtype_3D", 80.0),
            ("pid_2_luid_a_phys_0_eng_0_engtype_3D", 80.0),
            ("pid_2_luid_a_phys_0_eng_1_engtype_Copy", 2.0),
            ("no-luid-here", 5.0),
        ]));
        assert_eq!(s.by_type, [("copy".to_string(), 200)]);
        assert_eq!((s.engines, s.rejected), (1, 2));
    }
}
