//! Execute a bounded process activity measurement and project it through the agent contract.
pub use pcagent_agent::catalog::activity::TOOLS;
use pcagent_agent::projections::activity::project;
use pcagent_agent::{Args, PrivacyContext, Sharing, ToolOutput, ToolSpec};
use pcagent_telemetry::{Collector, DEFAULT_ACTIVITY_WINDOW_SECONDS};

pub fn run(
    spec: &ToolSpec,
    args: &Args,
    collector: &mut Collector,
    sharing: Sharing,
) -> Option<ToolOutput> {
    if spec.name != "get_process_activity" {
        return None;
    }
    let activity = collector.process_activity(
        args.integer("window_seconds")
            .unwrap_or(DEFAULT_ACTIVITY_WINDOW_SECONDS),
        args.integer("limit").unwrap_or(10) as usize,
    );
    Some(project(activity, sharing, &PrivacyContext::local()))
}
