//! Raw values, missing-reading codes and safe numeric presentation.
use pcagent_shared::{Evidence, Metric, Snapshot, Status};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measure {
    /// Raw value in the metric's unit; `None` unless the measurement is usable.
    pub value: Option<u64>,
    pub text: Option<String>,
    /// `None` when the metric was not collected at all.
    pub status: Option<Status>,
    /// Why the value is missing, when it is.
    pub note: Option<String>,
    /// The same reason as a closed code ([`MISSING_REASONS`]) the window words in its own
    /// language; `None` for any other reason, which the window names by `status`.
    pub note_code: Option<&'static str>,
}
impl Measure {
    pub(super) fn of(evidence: Option<&Evidence>) -> Self {
        let Some(e) = evidence else {
            return Self {
                value: None,
                text: None,
                status: None,
                note: None,
                note_code: None,
            };
        };
        match e.usable_value() {
            Some(v) => Self {
                value: Some(v),
                text: Some(e.metric.format_value(v)),
                status: Some(e.status),
                note: None,
                note_code: None,
            },
            None => Self {
                value: None,
                text: None,
                status: Some(e.status),
                note: e.limitations.first().cloned(),
                note_code: e.limitations.first().and_then(|note| missing_reason(note)),
            },
        }
    }
}
/// Why a live value is missing, as the telemetry words it (`crates/telemetry`), with the closed
/// code the window words it by. A reason starting with one of these texts gets its code: NVML
/// errors end with their own number. `missing_reasons_match_their_producers` checks the texts.
pub(super) const MISSING_REASONS: [(&str, &str); 15] = [
    ("No earlier reading of this instance in this window; a rate needs two readings of the same instance.", "first_reading"),
    ("Counter query failed.", "query_failed"),
    ("CPU counter query failed.", "query_failed"),
    ("System memory query failed.", "query_failed"),
    ("Process memory query failed.", "query_failed"),
    ("Counter regression, zero interval or inconsistent timing.", "inconsistent"),
    ("Counter reset, inconsistent values or zero elapsed counter interval.", "inconsistent"),
    ("Counter returned a negative, non-finite or out-of-range value.", "inconsistent"),
    ("No valid GPU engine was attributed to this unchanged process instance; absence is unknown, not zero.", "gpu_unattributed"),
    ("Application GPU is unknown when no valid physical engine can be attributed to its unchanged members.", "gpu_unattributed"),
    ("No full-window GPU measurement for this retained, unchanged and revalidated process; absence is unknown, not zero.", "gpu_unattributed"),
    ("No GPU engine instances were reported (no WDDM GPU counters, e.g. basic display adapter or VM).", "no_gpu_counters"),
    ("GPU engine instances were reported but none had a usable value.", "gpu_counters_unusable"),
    ("Not exported by this NVML version.", "nvml_unsupported"),
    ("NVML returned ", "nvml_error"),
];
pub(super) fn missing_reason(note: &str) -> Option<&'static str> {
    MISSING_REASONS
        .iter()
        .find(|(text, _)| note.starts_with(text))
        .map(|(_, code)| *code)
}
pub(super) fn find<'a>(s: &'a Snapshot, entity: &str, metric: Metric) -> Option<&'a Evidence> {
    s.evidence
        .iter()
        .find(|e| e.entity_ref == entity && e.metric == metric)
}
pub(super) fn measure(s: &Snapshot, entity: &str, metric: Metric) -> Measure {
    Measure::of(find(s, entity, metric))
}
/// `part / whole` in 0..=1 when both are usable and `whole` is positive.
pub(super) fn ratio(part: Option<u64>, whole: Option<u64>) -> Option<f64> {
    match (part, whole) {
        (Some(p), Some(w)) if w > 0 => Some((p as f64 / w as f64).clamp(0.0, 1.0)),
        _ => None,
    }
}
