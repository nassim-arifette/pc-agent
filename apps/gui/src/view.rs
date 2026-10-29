//! Plain data for the desktop interface, built from the core's own types. Every number keeps its
//! raw value next to the text the core formats; a missing measurement carries its status and
//! reason and is never shown as zero. This is a local window, so local names (process names,
//! full paths) may appear here; nothing in this module is sent to a provider.
use serde::Serialize;

mod actions;
mod activity;
mod battery;
mod dashboard;
mod labels;
mod measurement;
mod report;
mod settings;
mod storage;
mod usage;

pub use actions::{outcome, plan, ActionOutcome, PlanView};
pub(crate) use activity::application_key;
pub use activity::{activity, ActivityView};
pub use battery::{battery_read, BatteryView, LiveBattery};
pub use dashboard::{dashboard, ntfs_volumes, volumes, Dashboard, VolumeView};
pub use labels::{
    finding, localize, no_readable_names, process_name, FindingView, ProcessLabels, Readable,
};
pub use measurement::Measure;
pub use report::{error_code, report, ErrorView, Report, LOCAL_EVIDENCE_NOTE};
pub use settings::{app_info, capabilities, cloud, AppInfo, Cloud, MODELS};
pub use storage::{page, progress, storage, Page};
pub use usage::{price, usage, PriceView, Usage};

fn snake(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
