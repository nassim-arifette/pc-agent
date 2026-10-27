use super::*;
use crate::{new_id, unix_ms, unsupported_snapshot, Origin, Status, SCHEMA_VERSION, UNSUPPORTED};
pub(crate) struct Native;
impl Backend for Native {
    fn boot_id() -> Option<String> {
        None
    }
    fn resource_usage() -> ResourceUsage {
        ResourceUsage {
            thread_cpu_100ns: None,
            process_working_set_bytes: None,
            logical_processors: None,
        }
    }
    fn collect(collector: &mut Collector, _: bool, _: usize, _: bool, _: ProcessSort) -> Snapshot {
        unsupported_snapshot(&collector.session_id)
    }
    fn collect_recording(collector: &mut Collector, limit: usize, sort: ProcessSort) -> Snapshot {
        Self::collect(collector, true, limit, true, sort)
    }
    fn activity(collector: &mut Collector, _: u64, _: usize) -> ProcessActivity {
        ProcessActivity::unavailable(&collector.session_id, Status::Unsupported, UNSUPPORTED)
    }
    fn process_tree(collector: &mut Collector) -> ProcessTree {
        ProcessTree {
            schema_version: SCHEMA_VERSION,
            id: new_id("tree"),
            session_id: collector.session_id.clone(),
            origin: Origin::Live,
            status: Status::Unsupported,
            window: pcagent_shared::Window::instant(),
            nodes: Vec::new(),
            edges: Vec::new(),
            truncated: false,
            limitations: vec![UNSUPPORTED.into()],
        }
    }
    fn revalidate(_: u32, _: u64) -> InstanceState {
        InstanceState::Unverified
    }
    fn hardware_inventory() -> HardwareInventory {
        use pcagent_shared::Observed;
        fn m<T>() -> Observed<T> {
            Observed::missing("platform", Status::Unsupported)
        }
        HardwareInventory {
            schema_version: SCHEMA_VERSION,
            id: new_id("inventory"),
            origin: Origin::Live,
            status: Status::Unsupported,
            collected_at_unix_ms: unix_ms(),
            os_version: m(),
            architecture: m(),
            logical_processors: m(),
            installed_memory_bytes: m(),
            volumes: m(),
            gpu_adapters: m(),
            limitations: vec![UNSUPPORTED.into()],
        }
    }
}
