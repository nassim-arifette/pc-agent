//! Read a bounded cache of host-approved helper results. No launch or approval path is exposed
//! to the model. Re-reading a result preserves its actual source date and historical nature.
pub use pcagent_agent::catalog::elevated::{GET_APPROVED_INSPECTION, TOOLS};
use pcagent_agent::{Args, PrivacyContext, Sharing, ToolOutput, ToolSpec};
use pcagent_inspect::{elevation::ElevatedTask, Inspection};
use pcagent_shared::{unix_ms, Status};
use serde_json::json;

#[derive(Clone)]
pub struct ApprovedInspection {
    task_name: &'static str,
    inspection: Inspection,
}
impl ApprovedInspection {
    pub fn new(task: &ElevatedTask, inspection: Inspection) -> Result<Self, &'static str> {
        let task_name = match task {
            ElevatedTask::SystemCaches => "system_caches",
            ElevatedTask::MftInventory { .. } => "mft_inventory",
            ElevatedTask::ShadowStorage => "shadow_storage",
            ElevatedTask::DiskReliability => "disk_reliability",
            ElevatedTask::ProcessImages { .. } => "process_images",
            ElevatedTask::BatteryEnergy => "battery_energy",
            ElevatedTask::PerformanceTrace { .. } => "performance_trace",
            ElevatedTask::DeepAnalysis { .. } => "deep_analysis",
        };
        if serde_json::to_vec(&inspection)
            .map_err(|_| "invalid_inspection")?
            .len()
            > pcagent_store::MAX_ELEVATED_RECORD_BYTES
        {
            return Err("inspection_too_large");
        }
        Ok(Self {
            task_name,
            inspection,
        })
    }
    pub fn task_name(&self) -> &'static str {
        self.task_name
    }
    pub fn available(&self) -> bool {
        unix_ms()
            .checked_sub(self.inspection.measured_at_unix_ms)
            .is_some_and(|age| age < pcagent_store::ELEVATED_RETENTION_MS)
    }
    /// Same privacy projection as execution, for the locally reviewed context.
    pub fn preview(&self, sharing: Sharing, privacy: &PrivacyContext) -> ToolOutput {
        let mut output =
            pcagent_agent::projections::system::project(&self.inspection, sharing, privacy);
        output.cloud["historical"] = json!(true);
        output.cloud["freshness"] = json!("already_measured");
        output.cloud["approved_task"] = json!(self.task_name);
        output.cloud["age_ms"] = json!(unix_ms().checked_sub(self.inspection.measured_at_unix_ms));
        output
    }
}
pub fn run(
    spec: &ToolSpec,
    args: &Args,
    approved: &[ApprovedInspection],
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> Option<ToolOutput> {
    if spec.name == pcagent_agent::catalog::elevated::PROPOSE_ELEVATED_CHECK.name {
        let tasks = args.texts("tasks")?;
        return Some(ToolOutput {
            status: Status::Ok,
            cloud: json!({"status":"ok", "plan":{
                "plan_id":pcagent_shared::new_id("plan"),
                "kind":"elevated_check", "tasks":tasks,
                "requires_local_review":true, "executed":false
            }}),
            evidence: Vec::new(),
            processes: Vec::new(),
            findings: Vec::new(),
            storage_items: Vec::new(),
            limitations: Vec::new(),
            inspection_complete: false,
        });
    }
    if spec.name != GET_APPROVED_INSPECTION.name {
        return None;
    }
    let task = args.text("task").unwrap_or("");
    if let Some(captured) = approved
        .iter()
        .rev()
        .find(|r| r.task_name == task && r.available())
    {
        return Some(captured.preview(sharing, privacy));
    }
    Some(ToolOutput {
        status: Status::Unsupported,
        inspection_complete: false,
        cloud: json!({"status":"unsupported","coverage":[{"status":"needs_admin",
            "source":"Local helper approval","detail":"No still-valid locally approved cached result. The model cannot request or grant elevation."}],
            "task":task,"observations":[]}),
        evidence: Vec::new(),
        findings: Vec::new(),
        processes: Vec::new(),
        storage_items: Vec::new(),
        limitations: vec!["No locally approved helper result is available.".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proposal_is_closed_unique_local_review_only_and_never_an_inspection() {
        use pcagent_agent::catalog::elevated::PROPOSE_ELEVATED_CHECK as spec;
        for invalid in [
            r#"{"tasks":[]}"#,
            r#"{"tasks":["system_caches","system_caches"]}"#,
            r#"{"tasks":["performance_trace"]}"#,
            r#"{"tasks":["cmd.exe"]}"#,
            r#"{"tasks":["deep_analysis"]}"#,
            r#"{"tasks":[{"kind":"system_caches"}]}"#,
            r#"{"tasks":["system_caches"],"approve":true}"#,
            r#"{"tasks":["system_caches"],"budget_ms":1}"#,
            r#"{"tasks":"system_caches"}"#,
            r#"{"tasks":[null]}"#,
        ] {
            assert!(spec.validate(invalid).is_err(), "{invalid}");
        }
        let args = spec
            .validate(r#"{"tasks":["system_caches","process_images","mft_inventory"]}"#)
            .unwrap();
        let output = run(
            &spec,
            &args,
            &[],
            Sharing::Anonymized,
            &PrivacyContext::local(),
        )
        .unwrap();
        assert_eq!(args.to_value()["tasks"].as_array().unwrap().len(), 3);
        assert_eq!(output.cloud["plan"]["tasks"], args.to_value()["tasks"]);
        assert_eq!(output.cloud["plan"]["requires_local_review"], true);
        assert_eq!(output.cloud["plan"]["executed"], false);
        assert!(output.cloud["plan"]["plan_id"]
            .as_str()
            .unwrap()
            .starts_with("plan-"));
        assert!(!output.inspection_complete);
        assert!(
            output.evidence.is_empty() && output.processes.is_empty() && output.findings.is_empty()
        );
    }
    #[test]
    fn missing_approval_is_coverage_only_and_schema_cannot_launch_commands() {
        let args = GET_APPROVED_INSPECTION
            .validate(r#"{"task":"battery_energy"}"#)
            .unwrap();
        let result = run(
            &GET_APPROVED_INSPECTION,
            &args,
            &[],
            Sharing::Complete,
            &PrivacyContext::local(),
        )
        .unwrap();
        assert!(!result.inspection_complete);
        assert!(result.cloud["observations"].as_array().unwrap().is_empty());
        assert!(GET_APPROVED_INSPECTION
            .validate(r#"{"task":"battery_energy","approve":true}"#)
            .is_err());
        assert!(GET_APPROVED_INSPECTION
            .validate(r#"{"task":"cmd.exe"}"#)
            .is_err());
    }
    #[test]
    fn cache_does_not_rebase_dates_and_partial_measurements_are_usable() {
        let mut inspection: Inspection = serde_json::from_value(json!({
            "status":"partial", "measured_at_unix_ms":1,
            "window":{"started_at_unix_ms":1,"finished_at_unix_ms":1,"duration_ms":0},
            "origin":"synthetic_fixture", "observations":[], "coverage":[]
        }))
        .unwrap();
        inspection.measured_at_unix_ms = 1;
        inspection.window = pcagent_shared::Window {
            started_at_unix_ms: 1,
            finished_at_unix_ms: 1,
            duration_ms: 0,
        };
        let mut measured: pcagent_inspect::Observation = serde_json::from_value(json!({
            "observation_id":"local-fixture","measured_at_unix_ms":1,"window":inspection.window,
            "entity_ref":"battery-app-1","metric":"battery_app_energy_estimated","value":12,
            "unit":"mJ","status":"partial","source":"synthetic fixture","origin":"synthetic_fixture"})).unwrap();
        measured.status = pcagent_inspect::CoverageStatus::Partial;
        inspection.observations.push(measured);
        let mut captured =
            ApprovedInspection::new(&ElevatedTask::BatteryEnergy, inspection).unwrap();
        let output = captured.preview(Sharing::Anonymized, &PrivacyContext::local());
        assert!(output.inspection_complete);
        assert_eq!(output.cloud["measured_at_unix_ms"], 1);
        assert_eq!(output.cloud["historical"], true);
        captured.inspection.measured_at_unix_ms = unix_ms();
        assert!(captured.available());
        captured.inspection.measured_at_unix_ms = unix_ms() - pcagent_store::ELEVATED_RETENTION_MS;
        assert!(!captured.available());
    }
}
