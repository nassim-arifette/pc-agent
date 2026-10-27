//! Native macOS inspections; projections contain no raw dictionaries or command output.
use crate::{CoverageStatus as C, Inspection, Query};

mod battery;
#[allow(unsafe_code)]
mod command;
mod events;
mod hardware;
#[allow(unsafe_code)]
mod native;
mod services;
mod startup;
mod system;
mod updates;

pub(super) fn inspect(query: Query<'_>) -> Inspection {
    match query {
        Query::Startup => startup::collect(),
        Query::Battery => live_battery(),
        Query::Power => hardware::power(),
        Query::Hardware => hardware::collect(),
        Query::Services { limit } => services::collect(limit),
        Query::Events { days, limit } => events::collect(days, limit),
        Query::SystemState => system::collect(),
        _ => {
            let mut out = Inspection::new();
            out.coverage(
                "macos_inspection",
                C::Unsupported,
                "macOS native backend",
                "This inspection is not available in this backend.",
            );
            out.finish()
        }
    }
}

pub(super) fn live_battery() -> Inspection {
    use std::{sync::atomic::AtomicBool, time::Duration};
    static LIVE: AtomicBool = AtomicBool::new(false);
    crate::bounded(
        &LIVE,
        "pcagent-iops",
        Duration::from_secs(6),
        battery::collect,
    )
}
