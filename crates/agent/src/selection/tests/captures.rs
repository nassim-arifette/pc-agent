use super::*;
use crate::{ToolClass, ToolSpec};

#[test]
fn frozen_measurements_keep_the_original_window_and_minimize_identities() {
    let source = SelectionSource::Snapshot(capture());
    let target = SelectionTarget::Process {
        entity_ref: "proc-a1b2c3d4".into(),
    };
    let selection = frozen(&source, &target);
    let message = selection.preview_message(
        &[],
        "Alice fixture-secret-token browser.exe uses memory",
        &privacy(),
    );
    let wire = message.to_string();
    for private in [
        "Alice",
        "fixture-secret-token",
        "browser.exe",
        "777777",
        "987654321",
        "private-boot",
        "private-selection-session",
        "private-selection-snapshot",
    ] {
        assert!(!wire.contains(private), "leaked {private}");
    }
    assert_eq!(message["initial_observation"]["measured_at_unix_ms"], 2000);
    assert_eq!(message["initial_observation"]["age_ms"], 4000);
    assert_eq!(selection.output.evidence[0].window.started_at_unix_ms, 1000);
    assert_eq!(selection.output.evidence[0].window.duration_ms, 1000);
    assert_eq!(selection.output.evidence.len(), 1);
    assert!(checked(
        "During the selected capture, proc-a1b2c3d4 used 200 MB [O0].",
        message.clone()
    ));
    assert!(!checked("proc-a1b2c3d4 uses 900 MB [O0].", message));
    let named = FrozenSelection::freeze(
        &source,
        &target,
        SelectionIntent::Explain,
        6000,
        Sharing::Complete,
        false,
        &privacy(),
    )
    .unwrap();
    assert_eq!(
        named.output.cloud["process_names"]["proc-a1b2c3d4"],
        "browser.exe"
    );
}

#[test]
fn resource_selection_excludes_unrelated_measurements_and_explain_never_collects() {
    let source = SelectionSource::Snapshot(capture());
    let selection = frozen(
        &source,
        &SelectionTarget::Resource {
            resource: Resource::Cpu,
        },
    );
    let mut delegate = SnapshotTools::new(|_, _| panic!("Explain must never collect"));
    let mut selected = SelectedTools::new(&mut delegate, &selection);
    assert_eq!(selected.specs().len(), 1);
    assert_eq!(selected.specs()[0].name, GET_SELECTED_EVIDENCE.name);
    assert!(GET_SELECTED_EVIDENCE.validate(r#"{"fresh":true}"#).is_err());
    let result = selected.run(&GET_SELECTED_EVIDENCE, &Args::default());
    assert_eq!(result.evidence.len(), 1);
    assert_eq!(result.evidence[0].metric, Metric::CpuBusyBasisPoints);
    let forbidden = ToolSpec {
        name: "get_system_snapshot",
        class: ToolClass::Inspection,
        description: "",
        params: &[],
    };
    assert_eq!(
        selected.run(&forbidden, &Args::default()).status,
        Status::PermissionDenied
    );
    let message = selection.preview_message(&[], "Explain the CPU capture.", &privacy());
    assert!(checked(
        "During the selected capture, the CPU used 93% [O0].",
        message.clone()
    ));
    assert!(!checked("The process used 200 MB [O0].", message));
}

#[test]
fn investigate_delegates_fresh_tools_after_the_local_selection_boundary() {
    let source = SelectionSource::Snapshot(capture());
    let selection = FrozenSelection::freeze(
        &source,
        &SelectionTarget::Resource {
            resource: Resource::Cpu,
        },
        SelectionIntent::Investigate,
        6000,
        Sharing::Anonymized,
        false,
        &privacy(),
    )
    .unwrap();
    let mut calls = 0;
    {
        let mut delegate = SnapshotTools::new(|_, _| {
            calls += 1;
            capture()
        });
        let mut selected = SelectedTools::new(&mut delegate, &selection);
        let spec = *selected
            .specs()
            .iter()
            .find(|spec| spec.name == "get_system_snapshot")
            .unwrap();
        selected.run(&spec, &Args::default());
    }
    assert_eq!(calls, 1);
}

#[test]
fn local_references_and_tool_arguments_are_closed_and_wall_clock_rollback_does_not_invent_age() {
    for bad in [
        json!({"source_id":"selection-source-valid","target":{"kind":"resource","resource":"cpu","path":"C:/private"}}),
        json!({"source_id":"selection-source-valid","target":{"kind":"process","entity_ref":"proc-a1b2c3d4"},"payload":{}}),
    ] {
        assert!(serde_json::from_value::<SelectionRef>(bad).is_err());
    }
    let source = SelectionSource::Snapshot(capture());
    let selection = FrozenSelection::freeze(
        &source,
        &SelectionTarget::Resource {
            resource: Resource::Cpu,
        },
        SelectionIntent::Explain,
        100,
        Sharing::Anonymized,
        false,
        &privacy(),
    )
    .unwrap();
    assert!(selection.output.cloud["age_ms"].is_null());
    assert_eq!(
        selection.output.cloud["clock_consistency"],
        "capture_after_preview_wall_clock"
    );
    assert_eq!(selection.output.cloud["measured_at_unix_ms"], 2000);
}
