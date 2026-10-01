//! Locally rendered observation citations and independent report dimensions.
use crate::{ToolClass, ToolOutput, ToolSpec};
use pcagent_diagnostics::SYSTEM_METRICS;
use pcagent_shared::{format_bytes, new_id, Metric, ProcessCoverage, Snapshot, Status};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStatus {
    Completed,
    BudgetExhausted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Complete,
    Partial,
    Unavailable,
    NotCollected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Domain {
    System,
    Processes,
    Hardware,
    Storage,
    History,
}

#[derive(Debug, Clone, Serialize)]
pub struct DomainCoverage {
    pub domain: Domain,
    /// Opaque scope or metric for tools that can inspect several independent targets.
    pub target: Option<String>,
    pub status: CoverageStatus,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensorCoverage {
    pub status: CoverageStatus,
    pub domains: Vec<DomainCoverage>,
}

impl SensorCoverage {
    pub(crate) fn initial(snapshot: &Snapshot) -> Self {
        let mut coverage = Self {
            status: CoverageStatus::Unavailable,
            domains: [
                Domain::System,
                Domain::Processes,
                Domain::Hardware,
                Domain::Storage,
                Domain::History,
            ]
            .into_iter()
            .map(|domain| DomainCoverage {
                domain,
                target: None,
                status: CoverageStatus::NotCollected,
                missing: Vec::new(),
            })
            .collect(),
        };
        coverage.system(&snapshot.evidence);
        coverage.processes(&snapshot.process_coverage);
        coverage
    }

    fn put(&mut self, entry: DomainCoverage) {
        self.domains.retain(|old| {
            old.domain != entry.domain
                || (old.target != entry.target
                    && !(old.target.is_none() && old.status == CoverageStatus::NotCollected))
        });
        self.domains.push(entry);
        self.status = if self
            .domains
            .iter()
            .all(|d| d.status == CoverageStatus::Complete)
        {
            CoverageStatus::Complete
        } else if self
            .domains
            .iter()
            .any(|d| matches!(d.status, CoverageStatus::Complete | CoverageStatus::Partial))
        {
            CoverageStatus::Partial
        } else {
            CoverageStatus::Unavailable
        };
    }

    fn system(&mut self, evidence: &[pcagent_shared::Evidence]) {
        let missing: Vec<String> = SYSTEM_METRICS
            .iter()
            .filter(|(metric, _)| {
                !evidence.iter().any(|e| {
                    e.entity_ref == "system" && e.metric == *metric && e.usable_value().is_some()
                })
            })
            .map(|(_, label)| (*label).into())
            .collect();
        let status = if missing.is_empty() {
            CoverageStatus::Complete
        } else if missing.len() == SYSTEM_METRICS.len() {
            CoverageStatus::Unavailable
        } else {
            CoverageStatus::Partial
        };
        self.put(DomainCoverage {
            domain: Domain::System,
            target: None,
            status,
            missing,
        });
    }

    fn processes(&mut self, p: &ProcessCoverage) {
        let mut missing = Vec::new();
        if p.permission_denied > 0 {
            missing.push(format!("{} processes denied", p.permission_denied));
        }
        if p.unavailable > 0 {
            missing.push(format!("{} processes unavailable", p.unavailable));
        }
        if p.enumeration_truncated {
            missing.push("process enumeration truncated".into());
        }
        if p.returned < p.readable {
            missing.push("only a ranked subset of readable processes was returned".into());
        }
        let status = if p.status == Status::Ok && missing.is_empty() {
            CoverageStatus::Complete
        } else if p.returned > 0 {
            CoverageStatus::Partial
        } else {
            CoverageStatus::Unavailable
        };
        if status == CoverageStatus::Unavailable {
            missing.push("process observations unavailable".into());
        }
        self.put(DomainCoverage {
            domain: Domain::Processes,
            target: None,
            status,
            missing,
        });
    }

    pub(crate) fn record(&mut self, spec: &ToolSpec, output: &ToolOutput) {
        match spec.name {
            "list_storage_scopes" => {
                if let Some(scopes) = output.cloud["scopes"].as_array() {
                    for target in scopes.iter().filter_map(|s| s["scope_id"].as_str()) {
                        if !self.domains.iter().any(|d| {
                            d.domain == Domain::Storage && d.target.as_deref() == Some(target)
                        }) {
                            self.put(DomainCoverage {
                                domain: Domain::Storage,
                                target: Some(target.into()),
                                status: CoverageStatus::NotCollected,
                                missing: vec!["storage scope not inspected".into()],
                            });
                        }
                    }
                }
            }
            "get_system_snapshot" | "get_processes" => {
                self.system(&output.evidence);
                if spec.name == "get_processes" {
                    if let Some(p) = output
                        .cloud
                        .get("process_coverage")
                        .and_then(|v| serde_json::from_value::<ProcessCoverage>(v.clone()).ok())
                    {
                        self.processes(&p);
                    }
                }
            }
            "get_hardware_inventory" => {
                let missing = [
                    "os_version",
                    "architecture",
                    "logical_processors",
                    "installed_memory_bytes",
                    "volumes",
                    "gpu_adapters",
                ]
                .into_iter()
                .filter(|field| {
                    output.cloud[*field]["status"] != "ok"
                        || output.cloud[*field]["value"].is_null()
                })
                .map(str::to_owned)
                .collect::<Vec<_>>();
                self.inspection(Domain::Hardware, None, output, missing);
            }
            "get_storage_overview" | "get_largest_items" => {
                let mut missing = Vec::new();
                if output.status != Status::Ok {
                    missing.push("a complete storage scan".into());
                }
                if spec.name == "get_storage_overview" {
                    if output.cloud["volume"]["status"] != "ok" {
                        missing.push("volume capacity measurements".into());
                    }
                    if output.cloud["scan_running"] == true {
                        missing.push("the currently running scan has not finished".into());
                    }
                    if output.cloud["scan"].is_null() {
                        missing.push("a completed scan".into());
                    }
                } else if output.cloud["total"].as_u64().is_some_and(|n| {
                    n > output.cloud["items"]
                        .as_array()
                        .map_or(0, |items| items.len() as u64)
                }) {
                    missing.push("only one ranked page of items was returned".into());
                }
                self.inspection(
                    Domain::Storage,
                    output.cloud["scope_id"].as_str().map(str::to_owned),
                    output,
                    missing,
                );
            }
            "get_metric_history" => {
                let mut missing = Vec::new();
                if output.status != Status::Ok {
                    missing.push("an uninterrupted requested observation window".into());
                }
                self.inspection(
                    Domain::History,
                    output.cloud["metric"].as_str().map(str::to_owned),
                    output,
                    missing,
                );
            }
            _ => {}
        }
    }

    fn inspection(
        &mut self,
        domain: Domain,
        target: Option<String>,
        output: &ToolOutput,
        missing: Vec<String>,
    ) {
        let status = if !output.inspection_complete {
            CoverageStatus::Unavailable
        } else if output.status == Status::Ok && missing.is_empty() {
            CoverageStatus::Complete
        } else {
            CoverageStatus::Partial
        };
        self.put(DomainCoverage {
            domain,
            target,
            status,
            missing,
        });
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub observation_id: String,
    /// The short reference the model sees and cites (`O1`, `O2`, …).
    pub citation: String,
    pub tool: String,
    pub measured_at_unix_ms: u64,
    pub status: Status,
    pub citable: bool,
    /// These facts are generated by the controller, never by the model.
    pub facts: Vec<String>,
    pub limitations: Vec<String>,
    pub observation: Value,
    /// Real names are restored from trusted local references, never generated by the model.
    pub storage_items: Vec<crate::LocalStorageItem>,
}

impl Observation {
    pub(crate) fn record(spec: &ToolSpec, output: &mut ToolOutput) -> Self {
        if !output.cloud.is_object() {
            output.cloud = serde_json::json!({"payload":output.cloud});
        }
        let id = output.cloud["observation_id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| new_id("obs"));
        // Workstation issues the same ID to action plans, preserving their provenance.
        output.cloud["observation_id"] = id.clone().into();
        let citable = matches!(spec.class, ToolClass::Inspection)
            && output.inspection_complete
            && matches!(output.status, Status::Ok | Status::Partial);
        let mut limitations = output.limitations.clone();
        if let Some(items) = output.cloud["limitations"].as_array() {
            for limit in items.iter().filter_map(Value::as_str) {
                if !limitations.iter().any(|known| known == limit) {
                    limitations.push(limit.into());
                }
            }
        }
        let mut facts = if citable {
            facts(spec.name, &output.cloud)
        } else {
            Vec::new()
        };
        if citable && facts.is_empty() {
            facts.extend(
                output
                    .evidence
                    .iter()
                    .filter_map(|e| {
                        e.usable_value().map(|v| {
                            format!(
                                "{}: {} ({})",
                                e.metric.label(),
                                e.metric.format_value(v),
                                e.entity_ref
                            )
                        })
                    })
                    .take(8),
            );
        }
        if citable && output.status == Status::Partial {
            limitations.push("This observation has partial coverage; its values do not describe everything requested.".into());
        }
        Self {
            observation_id: id,
            citation: String::new(),
            tool: spec.name.into(),
            measured_at_unix_ms: output
                .evidence
                .iter()
                .map(|e| e.window.finished_at_unix_ms)
                .max()
                .unwrap_or_else(pcagent_shared::unix_ms),
            status: output.status,
            citable,
            facts,
            limitations,
            observation: output.cloud.clone(),
            storage_items: output.storage_items.clone(),
        }
    }
}

fn facts(tool: &str, cloud: &Value) -> Vec<String> {
    let mut facts = Vec::new();
    match tool {
        "get_hardware_inventory" => {
            for (field, label, bytes) in [
                ("logical_processors", "Logical processors", false),
                ("installed_memory_bytes", "Installed memory", true),
            ] {
                if cloud[field]["status"] == "ok" {
                    if let Some(value) = cloud[field]["value"].as_u64() {
                        facts.push(format!(
                            "{label}: {}.",
                            if bytes {
                                format_bytes(value)
                            } else {
                                value.to_string()
                            }
                        ));
                    }
                }
            }
            if cloud["architecture"]["status"] == "ok" {
                if let Some(arch) = cloud["architecture"]["value"].as_str() {
                    facts.push(format!("Architecture: {arch}."));
                }
            }
            facts.push("Hardware inventory describes capacity, not the cause of a symptom or an upgrade recommendation.".into());
        }
        "get_storage_overview" => {
            if let (Some(free), Some(total)) = (
                cloud["volume"]["free_bytes"].as_u64(),
                cloud["volume"]["total_bytes"].as_u64(),
            ) {
                facts.push(format!(
                    "Volume free space: {} of {}.",
                    format_bytes(free),
                    format_bytes(total)
                ));
            }
            if let Some(bytes) = cloud["scanned"]["logical_bytes"].as_u64() {
                facts.push(format!(
                    "Scanned logical size: {} (not reclaimable space).",
                    format_bytes(bytes)
                ));
            }
            if let Some(files) = cloud["scan"]["coverage"]["files"].as_u64() {
                facts.push(format!(
                    "The scan indexed {files} files; see its coverage counters for skipped items."
                ));
            }
        }
        "get_largest_items" => {
            if let Some(items) = cloud["items"].as_array() {
                facts.push(format!(
                    "{} ranked items returned; this page is not a complete inventory.",
                    items.len()
                ));
                for item in items.iter().take(3) {
                    if let (Some(id), Some(bytes)) = (
                        item["alias_path"]
                            .as_str()
                            .or_else(|| item["item_ref"].as_str()),
                        item["logical_bytes"].as_u64(),
                    ) {
                        facts.push(format!(
                            "{id}: {} logical size; no deletion recommendation implied.",
                            format_bytes(bytes)
                        ));
                    }
                }
            }
        }
        "get_metric_history" => {
            let metric = cloud
                .get("metric")
                .and_then(|v| serde_json::from_value::<Metric>(v.clone()).ok());
            if let (Some(metric), Some(points)) = (metric, cloud["points"].as_array()) {
                let values: Vec<u64> = points
                    .iter()
                    .filter(|p| p["status"] == "ok")
                    .filter_map(|p| p["value"].as_u64())
                    .collect();
                if let (Some(min), Some(max)) = (values.iter().min(), values.iter().max()) {
                    facts.push(format!(
                        "{}: {} usable samples, observed range {} to {}.",
                        metric.label(),
                        values.len(),
                        metric.format_value(*min),
                        metric.format_value(*max)
                    ));
                }
                if let Some(ms) = cloud["observed_interval_ms"].as_u64() {
                    facts.push(format!("Observed interval: {ms} ms."));
                }
                if let Some(gaps) = cloud["gaps"].as_array() {
                    facts.push(format!(
                        "{} recorded gaps; no interpolation or causal trend inferred.",
                        gaps.len()
                    ));
                }
            }
        }
        _ => {}
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(name: &'static str) -> ToolSpec {
        ToolSpec {
            name,
            class: ToolClass::Inspection,
            description: "",
            params: &[],
        }
    }
    fn output(cloud: Value) -> ToolOutput {
        ToolOutput {
            status: Status::Ok,
            inspection_complete: true,
            cloud,
            evidence: Vec::new(),
            findings: Vec::new(),
            processes: Vec::new(),
            storage_items: Vec::new(),
            limitations: Vec::new(),
        }
    }

    #[test]
    fn coverage_keeps_uninspected_scopes_and_does_not_equate_a_page_with_a_full_inventory() {
        let mut coverage = SensorCoverage::initial(&Snapshot::empty("fixture"));
        coverage.record(
            &spec("list_storage_scopes"),
            &output(json!({"scopes":[{"scope_id":"scope-a"},{"scope_id":"scope-b"}]})),
        );
        coverage.record(&spec("get_storage_overview"), &output(json!({"scope_id":"scope-a","volume":{"status":"ok"},"scan":{"status":"ok"},"scan_running":false})));
        assert!(coverage
            .domains
            .iter()
            .any(|d| d.target.as_deref() == Some("scope-b")
                && d.status == CoverageStatus::NotCollected));
        assert!(coverage.domains.iter().any(
            |d| d.target.as_deref() == Some("scope-a") && d.status == CoverageStatus::Complete
        ));
        coverage.record(
            &spec("get_largest_items"),
            &output(json!({"scope_id":"scope-a","total":40,"items":[{}]})),
        );
        assert!(
            coverage
                .domains
                .iter()
                .any(|d| d.target.as_deref() == Some("scope-a")
                    && d.status == CoverageStatus::Partial)
        );
    }

    #[test]
    fn hardware_coverage_includes_missing_gpu_even_when_the_inventory_dispatch_succeeds() {
        let mut coverage = SensorCoverage::initial(&Snapshot::empty("fixture"));
        let mut cloud = json!({});
        for field in [
            "os_version",
            "architecture",
            "logical_processors",
            "installed_memory_bytes",
            "volumes",
        ] {
            cloud[field] = json!({"status":"ok","value":1});
        }
        cloud["gpu_adapters"] = json!({"status":"unsupported","value":null});
        coverage.record(&spec("get_hardware_inventory"), &output(cloud));
        let hardware = coverage
            .domains
            .iter()
            .find(|d| d.domain == Domain::Hardware)
            .unwrap();
        assert_eq!(hardware.status, CoverageStatus::Partial);
        assert_eq!(hardware.missing, ["gpu_adapters"]);
    }
}
