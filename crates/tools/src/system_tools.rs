//! Dispatch closed read-only inspection queries to the local Windows collectors.
pub use pcagent_agent::catalog::system::TOOLS;
use pcagent_agent::projections::system::project;
use pcagent_agent::{Args, PrivacyContext, Sharing, ToolOutput, ToolSpec};
use pcagent_inspect::{Inspection, Query, WindowsUpdateMode};
use pcagent_shared::Status;
use serde_json::json;

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
