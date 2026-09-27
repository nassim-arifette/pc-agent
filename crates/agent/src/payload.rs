//! Model-facing payloads. The model gets what it needs to choose tools and findings: usable
//! values grouped by entity, each unit once, unmeasured metrics summarized once per reason, and
//! findings under short session aliases. Evidence IDs, sources, timestamps and per-value
//! limitations stay in the local report, which keeps every field.
use pcagent_diagnostics::{Classification, Finding, Support};
use pcagent_shared::{Evidence, Metric, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Measurements {
    /// Entity → metric → value, for evidence with a usable value (status `ok`).
    pub values: BTreeMap<String, BTreeMap<String, u64>>,
    /// Metric → unit, for every metric named in `values` or `not_measured`.
    pub units: BTreeMap<String, String>,
    /// Metric → longest observation window in milliseconds, for values measured over a window.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub windows_ms: BTreeMap<String, u64>,
    /// Missing or unusable values, one entry per metric, status and reason.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub not_measured: Vec<NotMeasured>,
    /// Distinct limitations attached to usable values.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotMeasured {
    pub metric: String,
    pub status: Status,
    /// How many entities lack this metric for this reason.
    pub entities: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub fn metric_name(metric: Metric) -> String {
    match serde_json::to_value(metric) {
        Ok(Value::String(name)) => name,
        _ => format!("{metric:?}"),
    }
}

/// Groups evidence for the model. Callers filter entities first (cloud allowlist).
pub fn measurements<'a>(evidence: impl IntoIterator<Item = &'a Evidence>) -> Measurements {
    let mut m = Measurements::default();
    let mut missing: BTreeMap<(String, String, Option<String>), NotMeasured> = BTreeMap::new();
    for e in evidence {
        let name = metric_name(e.metric);
        m.units.insert(name.clone(), e.unit.clone());
        match e.usable_value() {
            Some(value) => {
                m.values
                    .entry(e.entity_ref.clone())
                    .or_default()
                    .insert(name.clone(), value);
                if e.window.duration_ms > 0 {
                    let w = m.windows_ms.entry(name).or_default();
                    *w = (*w).max(e.window.duration_ms);
                }
                for note in &e.limitations {
                    if !m.notes.contains(note) {
                        m.notes.push(note.clone());
                    }
                }
            }
            None => {
                let reason = (!e.limitations.is_empty()).then(|| e.limitations.join(" "));
                let status = serde_json::to_value(e.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                missing
                    .entry((name.clone(), status, reason.clone()))
                    .or_insert_with(|| NotMeasured {
                        metric: name,
                        status: e.status,
                        entities: 0,
                        reason,
                    })
                    .entities += 1;
            }
        }
    }
    m.not_measured = missing.into_values().collect();
    m
}

/// A finding as the model sees it: no local evidence IDs, no next step, no schema field.
#[derive(Debug, Serialize)]
pub struct ModelFinding<'a> {
    pub id: String,
    pub classification: Classification,
    pub support: Support,
    pub support_reason: &'a str,
    pub summary: &'a str,
    pub facts: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hypothesis: Option<&'a str>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub counter_evidence: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub missing: &'a [String],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    pub limitations: &'a [String],
}

/// Short finding IDs (`F1`, `F2`, …) for one investigation, resolved back locally. They are
/// cheaper to send and harder to confuse with scope or item references than local IDs.
#[derive(Debug, Default)]
pub struct FindingAliases {
    by_id: HashMap<String, String>,
    by_alias: HashMap<String, String>,
}
impl FindingAliases {
    pub fn alias(&mut self, id: &str) -> String {
        if let Some(alias) = self.by_id.get(id) {
            return alias.clone();
        }
        let alias = format!("F{}", self.by_id.len() + 1);
        self.by_id.insert(id.to_owned(), alias.clone());
        self.by_alias.insert(alias.clone(), id.to_owned());
        alias
    }
    pub fn resolve(&self, alias: &str) -> Option<&str> {
        self.by_alias.get(alias).map(String::as_str)
    }
    pub fn model_finding<'a>(&mut self, f: &'a Finding) -> ModelFinding<'a> {
        ModelFinding {
            id: self.alias(&f.id),
            classification: f.classification,
            support: f.support,
            support_reason: &f.support_reason,
            summary: &f.summary,
            facts: f.facts.iter().map(|fact| fact.text.as_str()).collect(),
            hypothesis: f.hypothesis.as_deref(),
            counter_evidence: &f.counter_evidence,
            missing: &f.missing,
            limitations: &f.limitations,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::Window;
    fn window(ms: u64) -> Window {
        Window {
            started_at_unix_ms: 1_000,
            finished_at_unix_ms: 1_000 + ms,
            duration_ms: ms,
        }
    }
    #[test]
    fn usable_values_are_grouped_and_gaps_summarized_once() {
        let mut evidence = vec![
            Evidence::measured(
                Metric::CpuBusyBasisPoints,
                "system",
                3_535,
                "s",
                window(1_000),
            ),
            Evidence::measured(Metric::ProcessWorkingSetBytes, "proc-1", 10, "s", window(0)),
            Evidence::measured(Metric::ProcessWorkingSetBytes, "proc-2", 20, "s", window(0)),
        ];
        for entity in ["proc-1", "proc-2"] {
            evidence.push(Evidence::missing(
                Metric::ProcessCpuBasisPoints,
                entity,
                "s",
                Status::Partial,
                "First observation of this instance.",
            ));
        }
        let m = measurements(&evidence);
        assert_eq!(m.values["system"]["cpu_busy_basis_points"], 3_535);
        assert_eq!(m.values["proc-2"]["process_working_set_bytes"], 20);
        assert_eq!(m.units["process_working_set_bytes"], "bytes");
        assert_eq!(
            m.units["process_cpu_basis_points"],
            "basis_points_of_total_cpu"
        );
        assert_eq!(m.windows_ms.len(), 1);
        assert_eq!(m.windows_ms["cpu_busy_basis_points"], 1_000);
        assert_eq!(
            m.not_measured,
            [NotMeasured {
                metric: "process_cpu_basis_points".into(),
                status: Status::Partial,
                entities: 2,
                reason: Some("First observation of this instance.".into()),
            }]
        );
        let text = serde_json::to_string(&m).unwrap();
        for local in ["ev-", "\"source\"", "finished_at"] {
            assert!(!text.contains(local), "{local}");
        }
    }
    #[test]
    fn aliases_are_stable_short_and_resolve_back() {
        let mut a = FindingAliases::default();
        assert_eq!(a.alias("snapshot-1-LowAvailableMemory"), "F1");
        assert_eq!(a.alias("snapshot-1-HighCpuSample"), "F2");
        assert_eq!(a.alias("snapshot-1-LowAvailableMemory"), "F1");
        assert_eq!(a.resolve("F2"), Some("snapshot-1-HighCpuSample"));
        assert_eq!(a.resolve("snapshot-1-HighCpuSample"), None);
        assert_eq!(a.resolve("F3"), None);
    }
}
