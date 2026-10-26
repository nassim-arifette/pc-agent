use super::*;

#[test]
fn dashboard_keeps_raw_values_and_never_turns_a_missing_value_into_zero() {
    let d = dashboard(&fixture());
    assert_eq!(d.memory.used_ratio, Some(0.75));
    assert_eq!(d.memory.used_text.as_deref(), Some("12.00 GiB"));
    assert_eq!(d.memory.total.text.as_deref(), Some("16.00 GiB"));
    // CPU was observed once: its status and reason are kept, not a zero.
    assert_eq!(d.cpu.busy.value, None);
    assert_eq!(d.cpu.busy.status, Some(Status::Partial));
    assert!(d.cpu.busy.note.as_deref().unwrap().contains("two samples"));
    // Never collected at all: no status.
    assert_eq!(d.cpu.busiest_core.status, None);
    assert_eq!(d.disk.busy_ratio, Some(0.25));
    assert_eq!(d.gpus[0].name, "NVIDIA Fixture");
    assert_eq!(d.gpus[0].memory_ratio, Some(0.25));
    assert_eq!(d.gpus[0].slowdown_reasons, ["thermal (driver)"]);
    assert_eq!(d.gpus[0].slowdown_codes, ["thermal_driver"]);
    assert_eq!(d.gpus[0].temperature.status, None);
    assert_eq!(d.gpu_engines[0].kind, "3d");
    assert_eq!(d.processes[0].name, "editor.exe");
    assert_eq!(d.processes[0].working_set.value, Some(3 << 30));
    // The battery comes from its own read, filled by the caller: none until then.
    assert!(d.batteries.is_empty());
    assert!(d.battery_source_id.is_none());
    assert!(!d.battery_unavailable);
}

#[test]
fn volumes_keep_fixed_drives_with_both_sizes() {
    use pcagent_shared::{new_id, Observed, Origin, VolumeInfo, SCHEMA_VERSION};
    let volume = |root: &str, kind, total, free| VolumeInfo {
        root: root.into(),
        kind,
        total_bytes: total,
        free_bytes: free,
        available_to_caller_bytes: free,
        source: "fixture".into(),
        status: Status::Ok,
        limitations: Vec::new(),
    };
    fn missing<T>() -> Observed<T> {
        Observed::missing("fixture", Status::Unsupported)
    }
    let inventory = HardwareInventory {
        schema_version: SCHEMA_VERSION,
        id: new_id("inventory"),
        origin: Origin::Live,
        status: Status::Ok,
        collected_at_unix_ms: 0,
        os_version: missing(),
        architecture: missing(),
        logical_processors: missing(),
        installed_memory_bytes: missing(),
        volumes: Observed::ok(
            vec![
                volume("C:\\", VolumeKind::Fixed, Some(1000), Some(250)),
                volume("D:\\", VolumeKind::Fixed, Some(1000), None),
                volume("E:\\", VolumeKind::Removable, Some(1000), Some(900)),
            ],
            "fixture",
        ),
        gpu_adapters: missing(),
        limitations: Vec::new(),
    };
    let list = volumes(&inventory, |root| (root == "C:\\").then(|| "NTFS".into()));
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].root, "C:\\");
    assert_eq!(list[0].free_ratio, 0.25);
    assert_eq!(list[0].kind, VolumeKind::Fixed);
    assert_eq!(list[0].filesystem.as_deref(), Some("NTFS"));
    let shown = serde_json::to_value(&list[0]).unwrap();
    assert_eq!(shown["kind"], "fixed");
    assert_eq!(shown["filesystem"], "NTFS");
    // No inventory at all: nothing, never an empty drive.
    let unknown = HardwareInventory {
        volumes: missing(),
        ..inventory
    };
    assert!(volumes(&unknown, |_| None).is_empty());
}

#[test]
fn ntfs_volumes_need_a_fixed_ntfs_drive_and_stay_unknown_while_one_is_unread() {
    let volume = |root: &str, kind, filesystem: Option<&str>| VolumeView {
        root: root.into(),
        kind,
        filesystem: filesystem.map(str::to_owned),
        total_bytes: 1000,
        free_bytes: 500,
        free_ratio: 0.5,
    };
    let ntfs = volume("C:\\", VolumeKind::Fixed, Some("NTFS"));
    let refs = volume("D:\\", VolumeKind::Fixed, Some("ReFS"));
    let unread = volume("E:\\", VolumeKind::Fixed, None);
    // A RAM disk is never read by the helper, whatever its file system.
    let ram = volume("R:\\", VolumeKind::RamDisk, Some("NTFS"));
    assert_eq!(ntfs_volumes(&[refs.clone(), ntfs.clone()]), Some(true));
    assert_eq!(ntfs_volumes(&[unread.clone(), ntfs]), Some(true));
    assert_eq!(ntfs_volumes(&[refs.clone(), ram.clone()]), Some(false));
    assert_eq!(ntfs_volumes(&[refs, unread]), None);
    assert_eq!(ntfs_volumes(&[ram]), Some(false));
    assert_eq!(ntfs_volumes(&[]), Some(false));
}

#[test]
fn a_missing_live_value_names_its_reason_by_a_closed_code() {
    let reason = |note: &str| {
        let mut e = Evidence::missing(
            Metric::ProcessCpuBasisPoints,
            "proc-1",
            "fixture",
            Status::Partial,
            note,
        );
        e.limitations.push("A second note.".into());
        Measure::of(Some(&e)).note_code
    };
    assert_eq!(
        reason("No earlier reading of this instance in this window; a rate needs two readings of the same instance."),
        Some("first_reading")
    );
    assert_eq!(
        reason("NVML returned 999 (unknown error)."),
        Some("nvml_error")
    );
    assert_eq!(reason("Something else."), None);
    // A usable value, or no evidence at all, has no reason.
    let ok = Evidence::measured(
        Metric::CpuBusyBasisPoints,
        "system",
        7,
        "fixture",
        Window::instant(),
    );
    assert_eq!(Measure::of(Some(&ok)).note_code, None);
    assert_eq!(Measure::of(None).note_code, None);
}
