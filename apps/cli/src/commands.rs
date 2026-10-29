//! Dispatch parsed commands after validating the shared routing configuration.
use crate::args::{Args, Command};
use crate::{actions, elevation, history, inspection, investigation, provider, storage};
use crate::{model_routing, report, schedule, Result};

pub(super) fn run(args: Args) -> Result<u8> {
    let routing = model_routing::load(
        &args.routing,
        args.super_model.clone(),
        args.ultra_model.clone(),
        args.routing_calibration.as_deref(),
    )?;
    match args.command {
        Command::Report(options) => report::run(options),
        Command::Schedule { action } => schedule::run(action),
        Command::RestoreStartup(options) => actions::restore_startup(options, args.json),
        Command::ElevatedInspect(options) => elevation::inspect(options),
        Command::Models(options) => provider::models(options, args.json),
        Command::Inspect(options) => inspection::inspect(options),
        Command::History(options) => history::history(options),
        Command::Record(options) => history::record(options),
        Command::Sample(options) => history::sample(options),
        Command::Snapshot(options) => inspection::snapshot(
            options.limit,
            pcagent_telemetry::ProcessSort::WorkingSet,
            false,
            args.json,
        ),
        Command::Processes(options) => inspection::snapshot(
            options.limit,
            pcagent_telemetry::ProcessSort::parse(&options.sort_by).unwrap_or_default(),
            true,
            args.json,
        ),
        Command::Storage(options) => storage::scan(options, args.json),
        Command::Doctor(options) => provider::doctor(options, args.json),
        Command::DemoCache(options) => actions::demo_cache(options, args.json),
        Command::Diagnose(options) => investigation::diagnose(options, args.json, &routing),
        Command::Chat(options) => investigation::chat(options, args.json, &routing),
    }
}
