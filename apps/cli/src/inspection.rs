//! Read-only tool execution and current system observations.
use crate::history::cli_sampler;
use crate::local::write_json;
use crate::{args as options, text, Result};
use pcagent_agent::capabilities::{Capabilities, Probes};
use pcagent_agent::{Sharing, Toolbox};
use pcagent_policy::{Policy, Preferences};
use pcagent_shared::Status;
use pcagent_storage::StorageService;
use pcagent_telemetry::Collector;
use pcagent_tools::Workstation;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// Why a registered tool is not declared here: the capabilities it lacks (`capabilities`).
pub(super) fn unavailable_tool(tool: &str, capabilities: Option<Capabilities>) -> String {
    let needs = capabilities
        .and_then(|capabilities| pcagent_agent::capabilities::missing(tool, &capabilities))
        .unwrap_or_default();
    if needs.is_empty() {
        return "Unknown read-only tool.".into();
    }
    let needs: Vec<&str> = needs.iter().map(|c| c.name()).collect();
    format!(
        "The tool {tool} is unavailable here: it needs {}.",
        needs.join(", ")
    )
}

pub(super) fn inspect(options: options::Inspect) -> Result<u8> {
    let options::Inspect {
        tool,
        arguments,
        anonymize,
        database,
    } = options;
    let mut collector = Collector::new();
    let (policy, prefs) = (Policy::from_env(), Preferences::default());
    let mut workstation = Workstation::new(
        &mut collector,
        Arc::new(StorageService::new()),
        &policy,
        &prefs,
    );
    workstation.set_sharing(if anonymize {
        Sharing::Anonymized
    } else {
        Sharing::Complete
    });
    let spec = *workstation
        .specs()
        .iter()
        .find(|spec| spec.name == tool)
        .ok_or("Unknown read-only tool.")?;
    spec.validate(&arguments)?;
    let sampler = if pcagent_agent::catalog::history::TOOLS
        .iter()
        .any(|spec| spec.name == tool)
    {
        Some(cli_sampler(database)?)
    } else {
        None
    };
    if let Some(sampler) = &sampler {
        workstation.set_history(sampler.clone(), Arc::new(AtomicBool::new(false)));
    }
    // Only what works here, as for the model: no approved scope, helper flow, web
    // lookup or proposal in this command.
    workstation.offer_available(Probes::default());
    let Some(spec) = workstation.specs().iter().find(|s| s.name == tool).copied() else {
        return Err(unavailable_tool(&tool, workstation.capabilities()).into());
    };
    let arguments = spec.validate(&arguments)?;
    let output = workstation.run(&spec, &arguments);
    write_json(&output.cloud)?;
    Ok(if matches!(output.status, Status::Ok | Status::Partial) {
        0
    } else {
        3
    })
}

pub(super) fn snapshot(
    limit: u16,
    sort: pcagent_telemetry::ProcessSort,
    processes_only: bool,
    json: bool,
) -> Result<u8> {
    // One snapshot suffices: its one-second window already gives CPU and I/O rates.
    let mut collector = Collector::new();
    let sample = collector.snapshot_sorted(true, usize::from(limit), sort);
    if json {
        write_json(&sample)?;
    } else {
        text::snapshot(
            &mut io::stdout().lock(),
            &sample,
            processes_only,
            sort.label(),
        )?;
    }
    Ok(if sample.has_observations() { 0 } else { 3 })
}
