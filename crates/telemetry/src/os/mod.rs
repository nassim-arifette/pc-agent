//! Compile-time native backend boundary; common evidence and identity logic stays in the crate.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) mod windows;

use crate::{Collector, ProcessActivity, ProcessSort, ResourceUsage};
use pcagent_shared::{HardwareInventory, InstanceState, ProcessTree, Snapshot};

pub(crate) trait Backend {
    fn boot_id() -> Option<String>;
    fn resource_usage() -> ResourceUsage;
    fn collect(
        collector: &mut Collector,
        processes: bool,
        limit: usize,
        fast: bool,
        sort: ProcessSort,
    ) -> Snapshot;
    fn collect_recording(collector: &mut Collector, limit: usize, sort: ProcessSort) -> Snapshot;
    fn activity(collector: &mut Collector, seconds: u64, limit: usize) -> ProcessActivity;
    fn process_tree(collector: &mut Collector) -> ProcessTree;
    fn revalidate(pid: u32, created: u64) -> InstanceState;
    fn hardware_inventory() -> HardwareInventory;
}

#[cfg(windows)]
pub(crate) use windows::Native as Selected;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(not(any(windows, target_os = "linux")))]
mod unsupported;
#[cfg(target_os = "linux")]
pub(crate) use linux::Native as Selected;
#[cfg(not(any(windows, target_os = "linux")))]
pub(crate) use unsupported::Native as Selected;
