//! Closed-schema read-only Windows inspections, with an explicit cloud projection.
use crate::{
    tools::{Args, Param, ParamKind, ToolClass, ToolOutput, ToolSpec},
    PrivacyContext, Sharing,
};
use pcagent_inspect::{CoverageStatus, Inspection, Query, WindowsUpdateMode, QUERY_NAMES};
use pcagent_shared::Status;
use serde_json::{json, Map, Value};
use std::collections::BTreeSet;

const MAX_CLOUD_OBSERVATIONS: usize = 500;

const LIMIT: Param = Param {
    name: "limit",
    description: "Maximum returned entries (default 100).",
    kind: ParamKind::Integer { min: 1, max: 500 },
    required: false,
};
pub const TOOLS:&[ToolSpec]=&[
    ToolSpec{name:"get_startup_programs",class:ToolClass::Inspection,description:"Read Run/RunOnce, startup folders, StartupApproved state and logon scheduled tasks. Missing approval states stay unknown; commands and task actions are never read.",params:&[]},
    ToolSpec{name:"get_services",class:ToolClass::Inspection,description:"Read visible Windows services, state, startup type, opaque shared-process groups and recent SCM failure counts. Coverage says which protected services or configuration fields are unavailable.",params:&[LIMIT]},
    ToolSpec{name:"get_recent_events",class:ToolClass::Inspection,description:"Count selected crash, hang, hardware, disk, update and resource exhaustion events; return bounded latest occurrences. No event messages, user IDs, computer names or file contents.",params:&[
        Param{name:"days",description:"Trailing calendar duration in days (default 7, maximum 30).",kind:ParamKind::Integer{min:1,max:30},required:false},
        Param{name:"limit",description:"Maximum recent occurrence timestamps (default 50).",kind:ParamKind::Integer{min:1,max:200},required:false}]},
    ToolSpec{name:"get_system_state",class:ToolClass::Inspection,description:"Read uptime, Windows build, pending-reboot markers, fast-startup configuration, installed servicing hotfixes and pagefile usage. Missing dates and incomplete update coverage are explicit.",params:&[]},
    ToolSpec{name:"get_power_and_thermal",class:ToolClass::Inspection,description:"Read battery design/full/remaining capacities, estimated wear, optional cycles, signed charge/discharge power and separate Windows/current-rate runtime estimates; AC/saver, active plan, CPU frequency/limits and supported thermal/GPU sensors. Unknown values/units stay null; batteries stay separate. Application activity in recordings shows correlations, not measured per-app watts.",params:&[]},
    ToolSpec{name:"get_installed_apps",class:ToolClass::Inspection,description:"Read Uninstall registrations across HKCU/HKLM and registry views: publisher, version, installation date and estimated size. Estimates are not measured disk use; Store/portable app coverage is explicit.",params:&[LIMIT]},
    ToolSpec{name:"get_hardware_inventory",class:ToolClass::Inspection,description:"Read CPU, Windows build, RAM capacity and SMBIOS memory slots, volume space, disk bus/rotational properties and graphics driver versions. No serial numbers or hardware identifiers.",params:&[]},
    ToolSpec{name:"get_drivers",class:ToolClass::Inspection,description:"Read installed display, network, storage, system, audio and Bluetooth drivers and Windows Update offers. The cache is used by default. Version comparisons are unknown unless the offer matches one device unambiguously. Never recommend a downgrade or install a driver.",params:&[
        Param{name:"limit",description:"Maximum installed devices (default 64).",kind:ParamKind::Integer{min:1,max:100},required:false},
        Param{name:"windows_update",description:"none skips offers; offline reads the local update cache (default); online explicitly contacts Microsoft and may time out.",kind:ParamKind::OneOf(&["none","offline","online"]),required:false}]},
    ToolSpec{name:"get_cleanup_opportunities",class:ToolClass::Inspection,description:"Bounded metadata-only inventory of recycle bin, temporary/browser/shader/development caches, WSL/Docker disks, old large downloads, Windows update/rollback files and system-managed files. Categories report review/regenerable/keep, unknown sizes and partial lower bounds. Proposes no deletion and grants no permission.",params:&[]},
    ToolSpec{name:"query_system",class:ToolClass::Inspection,description:"Execute one fixed allowlisted read-only WMI, registry or event-log query. Cannot accept commands, WQL, paths, usernames, remote hosts or arbitrary registry keys.",params:&[
        Param{name:"query",description:"Named fixed inspection.",kind:ParamKind::OneOf(QUERY_NAMES),required:true},
        Param{name:"limit",description:"Maximum WMI rows or event occurrences (default 50).",kind:ParamKind::Integer{min:1,max:100},required:false}]},
];
/// `get_power_and_thermal` on a PC where Windows lists no system battery
/// (`capabilities::VARIANTS`).
pub(crate) const POWER_WITHOUT_BATTERY: ToolSpec = ToolSpec {
    name: "get_power_and_thermal",
    class: ToolClass::Inspection,
    description: "Read AC/saver state, active plan, CPU frequency/limits and supported thermal/GPU sensors. Windows lists no system battery on this PC, so there is no battery capacity, wear, charge or runtime to read. Unknown values/units stay null.",
    params: &[],
};

pub fn run(spec: &ToolSpec, args: &Args, sharing: Sharing) -> Option<ToolOutput> {
    run_with_inspection(spec, args, sharing, |_| {})
}

/// Retain locally resolved action targets before projecting the read-only inspection for C5.
/// The callback cannot change collector output or authorize any action.
pub fn run_with_inspection(
    spec: &ToolSpec,
    args: &Args,
    sharing: Sharing,
    retain_local: impl FnOnce(&Inspection),
) -> Option<ToolOutput> {
    let limit = args.integer("limit").unwrap_or(100) as usize;
    let query = match spec.name {
        "get_startup_programs" => Query::Startup,
        "get_services" => Query::Services { limit },
        "get_recent_events" => Query::Events {
            days: args.integer("days").unwrap_or(7) as u32,
            limit: args.integer("limit").unwrap_or(50) as usize,
        },
        "get_system_state" => Query::SystemState,
        "get_power_and_thermal" => Query::Power,
        "get_installed_apps" => Query::Apps { limit },
        "get_hardware_inventory" => Query::Hardware,
        "get_drivers" => Query::Drivers {
            limit: args.integer("limit").unwrap_or(64) as usize,
            windows_update: match args.text("windows_update").unwrap_or("offline") {
                "none" => WindowsUpdateMode::None,
                "online" => WindowsUpdateMode::Online,
                _ => WindowsUpdateMode::Offline,
            },
        },
        "get_cleanup_opportunities" => Query::Cleanup,
        "query_system" => Query::Allowlisted {
            name: args.text("query").unwrap_or(""),
            limit: args.integer("limit").unwrap_or(50) as usize,
        },
        _ => return None,
    };
    let inspection = pcagent_inspect::inspect(query);
    retain_local(&inspection);
    let mut output = project(&inspection, sharing, &PrivacyContext::local());
    if spec.name == "get_hardware_inventory" {
        let inventory = pcagent_telemetry::Collector::new().hardware_inventory();
        let volumes=inventory.volumes.value.as_ref().map(|rows|rows.iter().map(|v|json!({"kind":v.kind,"total_bytes":v.total_bytes,"free_bytes":v.free_bytes,"available_to_caller_bytes":v.available_to_caller_bytes,"status":v.status})).collect::<Vec<_>>());
        for (key, value) in [
            ("os_version", json!(inventory.os_version)),
            ("architecture", json!(inventory.architecture)),
            ("logical_processors", json!(inventory.logical_processors)),
            (
                "installed_memory_bytes",
                json!(inventory.installed_memory_bytes),
            ),
            (
                "volumes",
                json!({"value":volumes,"status":inventory.volumes.status,"source":inventory.volumes.source}),
            ),
        ] {
            output.cloud[key] = value;
        }
        output.inspection_complete |= inventory.logical_processors.value.is_some()
            || inventory.installed_memory_bytes.value.is_some();
        if inventory.status != Status::Ok && output.status == Status::Ok {
            output.status = Status::Partial;
            output.cloud["status"] = json!(Status::Partial);
        }
    }
    Some(output)
}

pub(crate) fn project(
    inspection: &Inspection,
    sharing: Sharing,
    privacy: &PrivacyContext,
) -> ToolOutput {
    let retained =
        &inspection.observations[..inspection.observations.len().min(MAX_CLOUD_OBSERVATIONS)];
    let omitted = inspection.observations.len() - retained.len();
    let status = if omitted > 0 {
        Status::Partial
    } else {
        inspection.status
    };
    // Bound the projection before serialization; keep the native inspection intact for local use.
    let mut cloud = json!({
        "status": status,
        "measured_at_unix_ms": inspection.measured_at_unix_ms,
        "window": inspection.window,
        "origin": inspection.origin,
        "observations": retained,
        "coverage": inspection.coverage,
    });
    // Names are never embedded in records. This map can be removed in full when a later turn
    // changes from complete to anonymized sharing.
    if sharing == Sharing::Complete {
        let mut references: BTreeSet<&str> =
            retained.iter().map(|o| o.entity_ref.as_str()).collect();
        for observation in retained {
            if observation.unit == "opaque_ref" {
                if let Some(reference) = observation.value.as_str() {
                    references.insert(reference);
                }
            }
        }
        let names: Map<String, Value> = inspection
            .names
            .iter()
            .filter(|(id, _)| {
                references.contains(id.as_str())
                    || id
                        .strip_suffix("-display")
                        .is_some_and(|entity| references.contains(entity))
            })
            .filter_map(|(id, name)| Some((id.clone(), Value::from(privacy.shared_name(name)?))))
            .collect();
        if !names.is_empty() {
            cloud["shared_names"] = Value::Object(names);
        }
    }
    let mut limitations: Vec<String> = inspection
        .coverage
        .iter()
        .filter(|c| c.status != CoverageStatus::Ok)
        .map(|c| format!("{}: {}", c.scope, c.detail))
        .collect();
    if omitted > 0 {
        let detail = format!(
            "Cloud result limited to {} observations; {omitted} observations omitted. The local inspection remains complete at its native coverage level.",
            retained.len()
        );
        cloud["projection"] = json!({
            "truncated": true,
            "observation_limit": MAX_CLOUD_OBSERVATIONS,
            "total_observations": inspection.observations.len(),
            "returned_observations": retained.len(),
            "omitted_observations": omitted,
        });
        cloud["coverage"]
            .as_array_mut()
            .expect("coverage array")
            .push(json!({
                "scope": "cloud_projection",
                "status": CoverageStatus::Partial,
                "source": "Agent bounded projection",
                "detail": detail,
            }));
        limitations.push(format!("cloud_projection: {detail}"));
    }
    privacy.sanitize_value(&mut cloud);
    ToolOutput {
        status,
        inspection_complete: retained.iter().any(|o| {
            matches!(o.status, CoverageStatus::Ok | CoverageStatus::Partial) && !o.value.is_null()
        }),
        cloud,
        evidence: Vec::new(),
        findings: Vec::new(),
        processes: Vec::new(),
        storage_items: Vec::new(),
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(count: usize) -> Inspection {
        let window = json!({"started_at_unix_ms":1,"finished_at_unix_ms":1,"duration_ms":0});
        serde_json::from_value(json!({
            "status":"ok", "measured_at_unix_ms":1, "window":window,
            "origin":"synthetic_fixture", "coverage":[],
            "observations": (0..count).map(|index| json!({
                "observation_id": format!("measurement-{index}"),
                "measured_at_unix_ms": 1, "window":window,
                "entity_ref": format!("entity-{index}"),
                "metric":"enabled", "value":true, "unit":"boolean", "status":"ok",
                "source":"fixture", "origin":"synthetic_fixture",
            })).collect::<Vec<_>>(),
        }))
        .unwrap()
    }
    #[test]
    fn all_schemas_are_closed_and_queries_cannot_be_injected() {
        for spec in TOOLS {
            assert_eq!(
                spec.definition()["function"]["parameters"]["additionalProperties"],
                false
            );
            assert!(spec.validate(r#"{"command":"whoami"}"#).is_err());
        }
        let query = TOOLS.iter().find(|t| t.name == "query_system").unwrap();
        for bad in [
            r#"{"query":"SELECT * FROM Win32_Process"}"#,
            r#"{"query":"HKEY_USERS"}"#,
            r#"{"query":"pagefile","limit":101}"#,
        ] {
            assert!(query.validate(bad).is_err());
        }
        assert!(query.validate(r#"{"query":"pagefile","limit":10}"#).is_ok());
    }
    #[test]
    fn names_are_consent_gated_and_sanitized() {
        let mut inspection = fixture(1);
        inspection
            .names
            .insert("entity-0".into(), "Example Service".into());
        let privacy =
            PrivacyContext::from_identifiers("C:\\Users\\Alice", "Alice", "EXAMPLE-PC", &[]);
        let anonymized = project(&inspection, Sharing::Anonymized, &privacy);
        assert!(anonymized.cloud.get("shared_names").is_none());
        let complete = project(&inspection, Sharing::Complete, &privacy);
        assert_eq!(
            complete.cloud["shared_names"]["entity-0"],
            "Example Service"
        );
    }

    #[test]
    fn cloud_limit_preserves_local_data_and_only_retains_referenced_names() {
        let mut inspection = fixture(MAX_CLOUD_OBSERVATIONS + 1);
        for index in 0..inspection.observations.len() {
            inspection
                .names
                .insert(format!("entity-{index}"), format!("Program {index}"));
        }
        inspection
            .names
            .insert("entity-0-display".into(), "Display name".into());
        inspection
            .names
            .insert("entity-0-publisher".into(), "Publisher".into());
        inspection
            .names
            .insert("orphan".into(), "Unused name".into());
        inspection.observations[0].unit = "opaque_ref".into();
        inspection.observations[0].value = Value::from("entity-0-publisher");
        let before = serde_json::to_value(&inspection).unwrap();
        let names_before = inspection.names.clone();
        let privacy = PrivacyContext::from_identifiers("", "", "", &[]);
        let output = project(&inspection, Sharing::Complete, &privacy);
        assert_eq!(
            output.cloud["observations"].as_array().unwrap().len(),
            MAX_CLOUD_OBSERVATIONS
        );
        assert_eq!(output.status, Status::Partial);
        assert_eq!(output.cloud["status"], "partial");
        assert_eq!(output.cloud["projection"]["omitted_observations"], 1);
        assert_eq!(output.cloud["coverage"][0]["status"], "partial");
        assert_eq!(output.limitations.len(), 1);
        let names = output.cloud["shared_names"].as_object().unwrap();
        assert_eq!(names.len(), MAX_CLOUD_OBSERVATIONS + 2);
        assert!(names.contains_key("entity-0-display"));
        assert!(names.contains_key("entity-0-publisher"));
        assert!(!names.contains_key("entity-500"));
        assert!(!names.contains_key("orphan"));
        assert_eq!(serde_json::to_value(&inspection).unwrap(), before);
        assert_eq!(inspection.names, names_before);

        inspection.observations.truncate(MAX_CLOUD_OBSERVATIONS);
        let exact = project(&inspection, Sharing::Anonymized, &privacy);
        assert_eq!(exact.status, Status::Ok);
        assert!(exact.cloud.get("projection").is_none());
        assert!(exact.cloud.get("shared_names").is_none());
    }
}
