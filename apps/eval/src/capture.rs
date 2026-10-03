//! A bounded sweep of the currently implemented tools, or an explicit validated call list.
use crate::Result;
use pcagent_agent::{
    replay::Recorder, ToolSpec, Toolbox, Workstation, HISTORY_TOOL_METRICS, SYSTEM_QUERY_NAMES,
};
use pcagent_shared::Snapshot;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Call {
    pub tool: String,
    #[serde(default = "empty_object")]
    pub arguments: Value,
}
fn empty_object() -> Value {
    json!({})
}

pub fn validate_calls(calls: &[Call], specs: &[ToolSpec]) -> Result<()> {
    if calls.is_empty() || calls.len() > pcagent_agent::replay::MAX_RECORDED_CALLS {
        return Err("A call list must contain 1 to 4096 calls".into());
    }
    for call in calls {
        let spec = specs
            .iter()
            .find(|s| s.name == call.tool)
            .ok_or("Unknown tool in call list")?;
        spec.validate(&call.arguments.to_string())?;
    }
    Ok(())
}

pub fn run(
    tools: &mut impl Toolbox,
    name: &str,
    arguments: Value,
) -> Result<pcagent_agent::ToolOutput> {
    let spec = *tools
        .specs()
        .iter()
        .find(|s| s.name == name)
        .ok_or("Unknown tool")?;
    let args = spec.validate(&arguments.to_string())?;
    Ok(tools.run(&spec, &args))
}

pub fn sweep(tools: &mut Recorder<Workstation<'_>>, initial: &Snapshot) -> Result<()> {
    run(tools, "get_system_snapshot", json!({}))?;
    let mut processes: BTreeSet<_> = initial
        .processes
        .iter()
        .map(|p| p.entity_ref.clone())
        .collect();
    // Capture both the omitted defaults and explicit variants: replay does not guess defaults.
    let default = run(tools, "get_processes", json!({}))?;
    processes.extend(default.processes.into_iter().map(|p| p.entity_ref));
    for sort_by in [
        "working_set",
        "private_commit",
        "cpu",
        "io_read",
        "io_write",
    ] {
        let out = run(
            tools,
            "get_processes",
            json!({"limit": 10, "sort_by": sort_by}),
        )?;
        processes.extend(out.processes.into_iter().map(|p| p.entity_ref));
    }
    for entity_ref in processes {
        run(
            tools,
            "get_process_details",
            json!({"entity_ref": entity_ref}),
        )?;
    }
    // Activity grouped by application, with services: the default and an explicit variant.
    run(tools, "get_process_activity", json!({}))?;
    run(
        tools,
        "get_process_activity",
        json!({"window_seconds": 3, "limit": 10}),
    )?;
    for tool in [
        "get_process_tree",
        "get_hardware_inventory",
        "get_user_preferences",
        "get_startup_programs",
        "get_system_state",
        "get_power_and_thermal",
        "get_cleanup_opportunities",
    ] {
        run(tools, tool, json!({}))?;
    }
    for tool in ["get_services", "get_installed_apps"] {
        run(tools, tool, json!({}))?;
    }
    run(tools, "get_recent_events", json!({}))?;
    run(tools, "get_recent_events", json!({"days": 7, "limit": 50}))?;
    for query in SYSTEM_QUERY_NAMES {
        run(tools, "query_system", json!({"query": query}))?;
    }
    // Persisted history: the last hour and the last day of each metric, plus anomaly events.
    // Recordings (record_activity) are not part of a sweep: they take 30-120 s on purpose.
    for window in [3_600, 86_400] {
        for metric in HISTORY_TOOL_METRICS {
            run(
                tools,
                "get_metric_history",
                json!({"metric": metric, "window_seconds": window}),
            )?;
        }
        run(
            tools,
            "get_activity_events",
            json!({"window_seconds": window}),
        )?;
    }
    let scopes = run(tools, "list_storage_scopes", json!({}))?;
    if let Some(scopes) = scopes.cloud["scopes"].as_array() {
        for scope in scopes {
            let id = &scope["scope_id"];
            run(tools, "start_storage_scan", json!({"scope_id": id}))?;
            // The caller waits for every scan before a second storage pass below.
        }
    }
    Ok(())
}

pub fn finish_scans(tools: Recorder<Workstation<'_>>) -> Result<Recorder<Workstation<'_>>> {
    let (mut workstation, calls) = tools.into_parts();
    for job in workstation.take_jobs() {
        // Tool coverage reports collection failures; preserve the rest of the capture.
        let _ = job.wait();
    }
    let mut tools = Recorder::new(workstation);
    tools.calls = calls;
    let scopes = run(&mut tools, "list_storage_scopes", json!({}))?;
    if let Some(scopes) = scopes.cloud["scopes"].as_array() {
        for scope in scopes {
            let id = &scope["scope_id"];
            run(&mut tools, "get_storage_overview", json!({"scope_id": id}))?;
            for kind in ["files", "dirs"] {
                for name in ["get_largest_items", "get_cleanup_candidates"] {
                    run(
                        &mut tools,
                        name,
                        json!({"scope_id": id, "kind": kind, "limit": 20}),
                    )?;
                }
            }
        }
    }
    Ok(tools)
}
