//! Read a bounded cache of host-approved helper results. No launch or approval path is exposed
//! to the model. Re-reading a result preserves its actual source date and historical nature.
use crate::{Args, Param, ParamKind, PrivacyContext, Sharing, ToolClass, ToolOutput, ToolSpec};
use pcagent_inspect::{elevation::ElevatedTask, Inspection};
use pcagent_shared::{unix_ms, Status};
use serde_json::json;
use std::time::{Duration, Instant};

pub const MAX_APPROVED_INSPECTIONS: usize = 8;
const MAX_INSPECTION_BYTES: usize = 512 * 1024;
const RETENTION: Duration = Duration::from_secs(300);
const TASKS: &[&str] = &[
    "system_caches",
    "mft_inventory",
    "shadow_storage",
    "disk_reliability",
    "process_images",
    "battery_energy",
    "performance_trace",
];
pub const GET_APPROVED_INSPECTION: ToolSpec = ToolSpec {
    name: "get_approved_elevated_inspection", class: ToolClass::Inspection,
    description: "Read already measured results of one locally approved Windows helper task. This never launches UAC, collects fresh data, changes files or grants approval. Cite these historical results as captured measurements, not current state. Missing approval requires the person to review the fixed task locally.",
    params: &[Param { name: "task", kind: ParamKind::OneOf(TASKS), required: true,
        description: "Fixed read-only inspection whose host-approved cached result is requested." }],
};
pub const TOOLS: &[ToolSpec] = &[GET_APPROVED_INSPECTION];

#[derive(Clone)]
pub struct ApprovedInspection {
    task_name: &'static str,
    inspection: Inspection,
    expires_at: Instant,
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
        };
        if serde_json::to_vec(&inspection)
            .map_err(|_| "invalid_inspection")?
            .len()
            > MAX_INSPECTION_BYTES
        {
            return Err("inspection_too_large");
        }
        Ok(Self {
            task_name,
            inspection,
            expires_at: Instant::now() + RETENTION,
        })
    }
    pub fn task_name(&self) -> &'static str {
        self.task_name
    }
    pub fn available(&self) -> bool {
        Instant::now() < self.expires_at
    }
    /// Same privacy projection as execution, for the locally reviewed context.
    pub fn preview(&self, sharing: Sharing, privacy: &PrivacyContext) -> ToolOutput {
        let mut output = crate::system_tools::project(&self.inspection, sharing, privacy);
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
        captured.expires_at = Instant::now();
        assert!(!captured.available());
    }
}
