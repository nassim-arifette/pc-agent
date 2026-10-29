use super::*;
use pcagent_store::{
    watches::{WatchAlert, WatchNotification, WatchResource, WatchRule},
    EventObservation,
};

fn alert() -> WatchAlert {
    let observation = EventObservation {
        entity_ref: "system".into(),
        metric: "cpu_busy_basis_points".into(),
        value: 9300,
        unit: "basis_points".into(),
        source: "fixture::GetSystemTimes".into(),
        status: Status::Ok,
        origin: Origin::SyntheticFixture,
        measured_at_unix_ms: Some(2000),
        window: Some(Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 2000,
            duration_ms: 1000,
        }),
        derivation: None,
        inputs: vec![],
    };
    WatchAlert {
        id: "alert-private".into(),
        watch_id: "watch-private".into(),
        rule: WatchRule::ResourceAbove {
            resource: WatchResource::Cpu,
            threshold_basis_points: 8500,
            sustained_seconds: 0,
        },
        started_at_unix_ms: 2000,
        last_checked_unix_ms: 2000,
        ended_at_unix_ms: None,
        close_reason: None,
        observation: observation.clone(),
        extreme_observation: observation,
        lower_bound: false,
        notification: WatchNotification::Sent,
        seen: false,
    }
}

#[test]
fn watch_alert_projection_is_frozen_dated_and_citable_not_current_or_configured() {
    let target = SelectionTarget::WatchAlert {
        alert_id: "alert-private".into(),
    };
    let source = SelectionSource::WatchAlert(alert());
    let selected = frozen(&source, &target);
    let message = selected.preview_message(&[], "Explain the stored alert.", &privacy());
    let text = message.to_string();
    assert!(!text.contains("alert-private"));
    assert!(!text.contains("watch-private"));
    assert_eq!(message["initial_observation"]["measured_at_unix_ms"], 2000);
    assert_eq!(
        message["initial_observation"]["alerts"][0]["rule_at_trigger"]["threshold_basis_points"],
        8500
    );
    assert_eq!(selected.output().evidence.len(), 2);
    assert!(checked(
        "During the recorded alert, CPU usage was 93% [O0].",
        message.clone()
    ));
    assert!(!checked(
        "Currently CPU usage is 93% [O0].",
        message.clone()
    ));
    assert!(!checked(
        "During the recorded alert, CPU usage was 85% [O0].",
        message
    ));
    assert!(FrozenSelection::freeze(
        &source,
        &SelectionTarget::WatchAlert {
            alert_id: "alert-other".into()
        },
        SelectionIntent::Explain,
        6000,
        Sharing::Complete,
        false,
        &privacy()
    )
    .is_err());
}

#[test]
fn watch_alert_targets_never_disclose_roots_device_indexes_or_unapproved_names() {
    for rule in [
        WatchRule::VolumeFreeBelow {
            root: "Z:\\".into(),
            threshold_bytes: 1_000_000,
        },
        WatchRule::GpuTemperatureAbove {
            nvml_index: 777,
            threshold_celsius: 90,
            sustained_seconds: 10,
        },
        WatchRule::AppCpuAbove {
            executable: "browser.exe".into(),
            threshold_basis_points: 7000,
            sustained_seconds: 10,
        },
    ] {
        let mut capture = alert();
        capture.rule = rule;
        capture.observation.entity_ref = "Z:\\".into();
        capture.extreme_observation.entity_ref = "gpu-777".into();
        let output = crate::projections::watches::project(
            &[capture.clone()],
            1,
            0,
            Sharing::Anonymized,
            &privacy(),
        );
        let encoded = output.cloud.to_string();
        for private in [
            "Z:",
            "gpu-777",
            "nvml_index",
            "browser.exe",
            "watch-private",
            "alert-private",
        ] {
            assert!(!encoded.contains(private), "{private} leaked: {encoded}");
        }
        let complete = crate::projections::watches::project(
            &[capture.clone()],
            1,
            0,
            Sharing::Complete,
            &privacy(),
        );
        assert_eq!(
            complete.cloud.to_string().contains("browser.exe"),
            capture.rule.is_application()
        );
        if capture.rule.is_application() {
            let names = &complete.cloud["alerts"][0]["shared_names"];
            assert_eq!(names["watch-target-1"], "browser.exe");
        }
    }
}

#[test]
fn watch_pages_report_next_offset_without_truncating_total() {
    let output = crate::projections::watches::project(
        &[alert(), alert()],
        50,
        10,
        Sharing::Complete,
        &privacy(),
    );
    assert_eq!(output.cloud["coverage"]["total"], 50);
    assert_eq!(output.cloud["coverage"]["next_offset"], 12);
    assert_eq!(output.cloud["coverage"]["returned"], 2);
    assert_eq!(output.status, Status::Partial);
}

#[test]
fn watch_tool_schema_has_only_bounded_read_only_arguments() {
    let spec = crate::catalog::inspection_tools()
        .into_iter()
        .find(|s| s.name == "get_watch_alerts")
        .unwrap();
    assert!(spec
        .validate(r#"{"from_unix_ms":0,"to_unix_ms":1000,"limit":200,"offset":0}"#)
        .is_ok());
    for payload in [
        r#"{"from_unix_ms":0,"to_unix_ms":1000,"limit":201,"offset":0}"#,
        r#"{"from_unix_ms":0,"to_unix_ms":1000,"limit":1,"offset":0,"rule":{}}"#,
        r#"{"from_unix_ms":0,"to_unix_ms":1000,"limit":1,"offset":0,"path":"C:\\"}"#,
        r#"{"from_unix_ms":0,"to_unix_ms":1000,"limit":1,"offset":-1}"#,
    ] {
        assert!(spec.validate(payload).is_err());
    }
}

#[test]
fn incomplete_alert_provenance_never_certifies_a_resource_number() {
    let mut capture = alert();
    capture.observation.window = None;
    capture.extreme_observation.measured_at_unix_ms = None;
    let output =
        crate::projections::watches::project(&[capture], 1, 0, Sharing::Complete, &privacy());
    assert!(output.cloud["alerts"][0]["initial_observation"]["value"].is_null());
    assert!(output.cloud["alerts"][0]["extreme_observation"]["value"].is_null());
    assert!(!checked(
        "During the recorded alert, CPU usage was 93% [O0].",
        output.cloud
    ));
}

#[test]
fn alert_input_provenance_is_bounded_but_counted_without_losing_the_page() {
    let mut capture = alert();
    capture.observation.inputs = vec![capture.extreme_observation.clone(); 2000];
    let output =
        crate::projections::watches::project(&[capture], 1, 0, Sharing::Complete, &privacy());
    assert!(output.cloud.to_string().len() < 128 * 1024);
    assert_eq!(output.cloud["coverage"]["returned"], 1);
    assert_eq!(
        output.cloud["alerts"][0]["initial_observation"]["input_coverage"]["total"],
        2000
    );
    assert_eq!(
        output.cloud["alerts"][0]["initial_observation"]["input_coverage"]["returned"],
        63
    );
    assert_eq!(output.status, Status::Partial);
}
