use pcagent_agent::{
    capabilities::{catalog, Probes},
    routing::CalibrationRuntime,
    Toolbox,
};
use pcagent_policy::{Policy, Preferences};
use pcagent_storage::StorageService;
use pcagent_telemetry::Collector;
use pcagent_tools::Workstation;
use std::sync::Arc;

#[test]
fn host_filtering_uses_the_static_catalog_without_changing_calibration_identity() {
    let canonical = pcagent_agent::catalog::inspection_tools();
    let global = CalibrationRuntime::current().registered_tool_registry_sha256;
    let (policy, prefs) = (Policy::default(), Preferences::default());
    for probes in [
        Probes::default(),
        Probes {
            elevated_helper: true,
            nvidia_gpu: Some(false),
            battery: Some(false),
            ntfs_volumes: Some(false),
        },
    ] {
        let mut collector = Collector::new();
        let storage = Arc::new(StorageService::new());
        let mut tools = Workstation::new(&mut collector, storage, &policy, &prefs);
        tools.offer_available(probes);
        let declared = tools.specs();
        assert_ne!(declared.len(), canonical.len());
        let expected = catalog(&canonical, &tools.capabilities().unwrap());
        assert_eq!(
            declared
                .iter()
                .map(|spec| spec.definition())
                .collect::<Vec<_>>(),
            expected
                .iter()
                .map(|spec| spec.definition())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            CalibrationRuntime::current().registered_tool_registry_sha256,
            global
        );
    }
}
