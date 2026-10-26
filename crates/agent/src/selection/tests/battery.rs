use super::*;
use crate::selection::projection::bounded_battery;
use pcagent_inspect::{CoverageStatus, Inspection};
use pcagent_store::BatteryRecording;

const BATTERY: &str = "battery-00000000-0000-4000-8000-00000000b001";
const DEVICES_DETAIL: &str = "At most eight interfaces and a four-second scan budget. Device paths/tags are used only locally; names, manufacturers, serials and unique IDs are never queried. A stalled driver is subject to its caller's time bound, which does not cancel native I/O.";

#[test]
fn battery_response_omissions_preserve_original_endpoints_and_report_gaps() {
    use pcagent_inspect::{CoverageStatus, Observation};
    use pcagent_store::{BatteryHistory, BatteryPoint, BatteryRecordingCoverage};
    let points: Vec<_> = (0..128)
        .map(|index| {
            let at = 2000 + index * 1000;
            let observations = [
                (
                    "battery_remaining_capacity_mwh",
                    11000 - index * 2,
                    "milliwatt_hours",
                ),
                ("battery_full_charge_capacity_mwh", 15000, "milliwatt_hours"),
            ]
            .into_iter()
            .map(|(metric, value, unit)| Observation {
                observation_id: format!("battery-1-measurement-{index}-{metric}"),
                measured_at_unix_ms: at,
                window: Window {
                    started_at_unix_ms: at - 10,
                    finished_at_unix_ms: at,
                    duration_ms: 10,
                },
                entity_ref: "battery-1".into(),
                metric: metric.into(),
                value: json!(value),
                unit: unit.into(),
                status: CoverageStatus::Ok,
                source: "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS".into(),
                origin: Origin::SyntheticFixture,
            })
            .collect();
            BatteryPoint {
                recorded_at_unix_ms: at,
                recording_elapsed_ms: Some(at - 1000),
                clock_consistent: true,
                status: Status::Ok,
                observations,
            }
        })
        .collect();
    let battery = BatteryRecording {
        status: Status::Ok,
        started_at_unix_ms: 1000,
        ended_at_unix_ms: Some(129000),
        interval_ms: 1000,
        coverage: BatteryRecordingCoverage {
            expected_ticks: 128,
            recorded_ticks: 128,
            ticks_without_battery: 0,
            unavailable_observations: 0,
            rejected_observations: 0,
            points_returned: 128,
            points_omitted_from_response: 0,
            batteries_omitted_from_response: 0,
            battery_tick_points_omitted_during_collection: 0,
        },
        gaps: vec![],
        batteries: vec![BatteryHistory {
            battery_ref: "battery-1".into(),
            status: Status::Ok,
            points,
            gaps: vec![],
            evolution: vec![],
        }],
        limitations: vec![],
    };
    assert!(serde_json::to_vec(&battery).unwrap().len() > 48 * 1024);
    let bounded = bounded_battery(&battery);
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= 48 * 1024);
    assert_eq!(bounded.status, Status::Partial);
    let retained = &bounded.batteries[0].points;
    assert_eq!(
        serde_json::to_value(retained.first()).unwrap(),
        serde_json::to_value(battery.batteries[0].points.first()).unwrap()
    );
    assert_eq!(
        serde_json::to_value(retained.last()).unwrap(),
        serde_json::to_value(battery.batteries[0].points.last()).unwrap()
    );
    assert_eq!(
        bounded.coverage.points_returned + bounded.coverage.points_omitted_from_response,
        128
    );
    assert_eq!(bounded.coverage.recorded_ticks, 128);
    assert_eq!(bounded.gaps[0].reason, "selection_response_limit");
    assert_eq!(bounded.gaps[0].from_unix_ms, 1000);
    assert_eq!(
        battery.batteries[0].points.len(),
        128,
        "the captured source remains immutable"
    );
}

#[test]
fn battery_read_targets_only_the_battery_resource_and_explain_freezes_it_without_collecting() {
    let source = SelectionSource::Battery(discharging_battery());
    assert_eq!(source.targets(), vec![battery_target()]);
    assert_eq!(source.measured_at(&battery_target()), Some(4010));
    assert!(!source.is_history());
    let selection = frozen(&source, &battery_target());
    let cloud = &selection.output().cloud;
    assert_eq!(
        cloud["selection"]["target"],
        json!({"kind":"resource","resource":"battery"})
    );
    assert_eq!(cloud["selection"]["fresh_collection"], false);
    assert_eq!(cloud["freshness"], "already_measured");
    assert_eq!(cloud["measured_at_unix_ms"], 4010);
    assert_eq!(cloud["age_ms"], 1990);
    let value = |metric: &str| {
        cloud["observations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|observation| observation["metric"] == metric)
            .map(|observation| observation["value"].clone())
            .unwrap()
    };
    assert_eq!(value("battery_charge_percent"), 78.1);
    assert_eq!(value("battery_power_rate_mw"), -8400);
    assert!(
        value("battery_cycle_count").is_null(),
        "an unknown counter stays null, never 0"
    );
    assert!(selection.snapshot().evidence.is_empty());
    assert!(selection.snapshot().processes.is_empty());
    assert!(selection.output().inspection_complete);
    let mut delegate = SnapshotTools::new(|_, _| panic!("Explain never reads the battery"));
    let mut selected = SelectedTools::new(&mut delegate, &selection);
    assert_eq!(selected.specs().len(), 1);
    let read = selected.run(&GET_SELECTED_EVIDENCE, &Args::default());
    assert_eq!(read.cloud, *cloud);
    let message =
        selection.preview_message(&[], "Explain the selected battery reading.", &privacy());
    assert_eq!(message["initial_observation"]["measured_at_unix_ms"], 4010);
    assert!(checked(
        "During the selected capture, the battery charge was 78.1% [O0].",
        message.clone()
    ));
    assert!(!checked(
        "During the selected capture, the battery charge was 95% [O0].",
        message
    ));
    assert_eq!(
        battery_target().default_question(SelectionIntent::Explain),
        "Explain the selected battery measurements, stating what is measured and what remains uncertain."
    );
}

#[test]
fn battery_reads_without_values_have_no_target_and_snapshots_never_offer_battery() {
    let collector = |scope: &str, status| {
        let mut read = battery_read(vec![]);
        read.coverage[0].scope = scope.into();
        read.coverage[0].status = status;
        SelectionSource::Battery(read)
    };
    // A desktop: Windows lists no battery.
    assert!(collector("battery_devices", CoverageStatus::Unsupported)
        .targets()
        .is_empty());
    // A read refused while the previous one is still running.
    assert!(collector("collector", CoverageStatus::Partial)
        .targets()
        .is_empty());
    let unknown = SelectionSource::Battery(battery_read(vec![battery_observation(
        BATTERY,
        "battery_charge_percent",
        Value::Null,
        "percent",
        CoverageStatus::Unsupported,
    )]));
    assert!(unknown.targets().is_empty());
    let not_a_battery = SelectionSource::Battery(battery_read(vec![battery_observation(
        "system",
        "battery_charge_percent",
        json!(50.0),
        "percent",
        CoverageStatus::Ok,
    )]));
    assert!(not_a_battery.targets().is_empty());
    let snapshot = SelectionSource::Snapshot(capture());
    assert!(!snapshot.targets().contains(&battery_target()));
    assert!(FrozenSelection::freeze(
        &snapshot,
        &battery_target(),
        SelectionIntent::Explain,
        6000,
        Sharing::Anonymized,
        false,
        &privacy(),
    )
    .is_err());
    assert!(!Resource::Battery.contains(Metric::GpuPowerMilliwatts));
}

#[test]
fn battery_resource_deserializes_and_unknown_resources_stay_rejected() {
    let reference: SelectionRef = serde_json::from_value(json!({
        "source_id": "selection-source-00000000-0000-4000-8000-000000000001",
        "target": {"kind": "resource", "resource": "battery"}
    }))
    .unwrap();
    assert!(reference.validate());
    assert_eq!(reference.target, battery_target());
    for bad in ["battery_health", "Battery", "batteries"] {
        assert!(
            serde_json::from_value::<SelectionTarget>(json!({"kind":"resource","resource":bad}))
                .is_err(),
            "{bad}"
        );
    }
    assert!(is_battery_ref(BATTERY));
    for bad in [
        "battery-1",
        "battery-00000000-0000-4000-8000-00000000B001",
        "proc-a1b2c3d4",
        r"\\?\acpi#pnp0c0a#1#{72631e54-78a4-11d0-bcf7-00aa00b7b32a}",
    ] {
        assert!(!is_battery_ref(bad), "{bad}");
    }
}

#[test]
fn battery_selection_never_serializes_device_identities() {
    let device = r"\\?\acpi#pnp0c0a#1#{72631e54-78a4-11d0-bcf7-00aa00b7b32a}";
    assert!(braced_guid(device));
    let mut read = discharging_battery();
    read.names
        .insert(BATTERY.into(), "PRIVATE-BATTERY-MODEL".into());
    // A host bug that kept a non-opaque key must still not reach the model.
    read.observations.push(battery_observation(
        device,
        "battery_charge_percent",
        json!(12.5),
        "percent",
        CoverageStatus::Ok,
    ));
    let source = SelectionSource::Battery(read);
    for sharing in [Sharing::Anonymized, Sharing::Complete] {
        let selection = FrozenSelection::freeze(
            &source,
            &battery_target(),
            SelectionIntent::Explain,
            6000,
            sharing,
            false,
            &privacy(),
        )
        .unwrap();
        let message = selection.preview_message(&[], "Explain the battery.", &privacy());
        let cloud = &message["initial_observation"];
        assert!(cloud.get("shared_names").is_none());
        assert!(cloud.get("names").is_none());
        let observations = cloud["observations"].as_array().unwrap();
        assert_eq!(observations.len(), 4);
        assert!(observations
            .iter()
            .all(|observation| observation["entity_ref"]
                .as_str()
                .is_some_and(is_battery_ref)));
        let mut all = Vec::new();
        strings(&message, &mut all);
        for text in all {
            assert!(!text.contains(r"\\?\"), "device path in {text}");
            assert!(!braced_guid(text), "interface GUID in {text}");
            assert!(!text.contains("PRIVATE-BATTERY-MODEL"));
        }
        // The fixed coverage text may say what is never queried; it carries no identity.
        assert!(cloud["coverage"][0]["detail"]
            .as_str()
            .unwrap()
            .contains("serials"));
    }
}

fn battery_observation(
    entity_ref: &str,
    metric: &str,
    value: Value,
    unit: &str,
    status: CoverageStatus,
) -> pcagent_inspect::Observation {
    pcagent_inspect::Observation {
        observation_id: format!("measurement-{metric}"),
        measured_at_unix_ms: 4010,
        window: Window {
            started_at_unix_ms: 4000,
            finished_at_unix_ms: 4010,
            duration_ms: 10,
        },
        entity_ref: entity_ref.into(),
        metric: metric.into(),
        value,
        unit: unit.into(),
        status,
        source: "DeviceIoControl/IOCTL_BATTERY_QUERY_STATUS".into(),
        origin: Origin::SyntheticFixture,
    }
}

/// A synthetic live battery read, shaped like the collector's (no device is opened).
fn battery_read(observations: Vec<pcagent_inspect::Observation>) -> Inspection {
    Inspection {
        status: Status::Partial,
        measured_at_unix_ms: 4020,
        window: Window {
            started_at_unix_ms: 3990,
            finished_at_unix_ms: 4020,
            duration_ms: 30,
        },
        origin: Origin::SyntheticFixture,
        observations,
        coverage: vec![pcagent_inspect::Coverage {
            scope: "battery_devices".into(),
            status: CoverageStatus::Ok,
            source: "SetupAPI/GUID_DEVICE_BATTERY".into(),
            detail: DEVICES_DETAIL.into(),
        }],
        names: Default::default(),
    }
}

fn discharging_battery() -> Inspection {
    use CoverageStatus::*;
    battery_read(vec![
        battery_observation(BATTERY, "battery_kind", json!("system"), "category", Ok),
        battery_observation(
            BATTERY,
            "battery_charge_percent",
            json!(78.1),
            "percent",
            Ok,
        ),
        battery_observation(
            BATTERY,
            "battery_power_rate_mw",
            json!(-8400),
            "milliwatts",
            Ok,
        ),
        battery_observation(
            BATTERY,
            "battery_cycle_count",
            Value::Null,
            "count",
            Unsupported,
        ),
    ])
}

fn battery_target() -> SelectionTarget {
    SelectionTarget::Resource {
        resource: Resource::Battery,
    }
}

fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
    match value {
        Value::String(text) => out.push(text),
        Value::Array(values) => values.iter().for_each(|value| strings(value, out)),
        Value::Object(values) => values.iter().for_each(|(key, value)| {
            out.push(key);
            strings(value, out);
        }),
        _ => {}
    }
}

/// A `{8-4-4-4-12}` interface GUID such as `{72631e54-78a4-11d0-bcf7-00aa00b7b32a}`.
fn braced_guid(text: &str) -> bool {
    text.match_indices('{').any(|(at, _)| {
        let rest = &text.as_bytes()[at + 1..];
        rest.len() > 36
            && rest[36] == b'}'
            && rest[..36].iter().enumerate().all(|(index, byte)| {
                if matches!(index, 8 | 13 | 18 | 23) {
                    *byte == b'-'
                } else {
                    byte.is_ascii_hexdigit()
                }
            })
    })
}
