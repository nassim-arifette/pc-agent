use super::*;

const B1: &str = "battery-00000000-0000-4000-8000-00000000b001";

const B2: &str = "battery-00000000-0000-4000-8000-00000000b002";

const UPS: &str = "battery-00000000-0000-4000-8000-00000000c001";

/// Synthetic observations of one battery: `(metric, value)`, ok unless null; no device is
/// opened.
fn cells(reference: &str, values: &[(&str, Value)]) -> Vec<Observation> {
    values
        .iter()
        .map(|(metric, value)| Observation {
            observation_id: format!("measurement-{reference}-{metric}"),
            measured_at_unix_ms: 5000,
            window: Window {
                started_at_unix_ms: 4990,
                finished_at_unix_ms: 5000,
                duration_ms: 10,
            },
            entity_ref: reference.into(),
            metric: (*metric).into(),
            value: value.clone(),
            unit: "fixture".into(),
            status: if value.is_null() {
                CoverageStatus::Unsupported
            } else {
                CoverageStatus::Ok
            },
            source: "fixture".into(),
            origin: pcagent_shared::Origin::SyntheticFixture,
        })
        .collect()
}

fn inspection(observations: Vec<Observation>, scope: &str, status: CoverageStatus) -> Inspection {
    Inspection {
        status: Status::Partial,
        measured_at_unix_ms: 5010,
        window: Window {
            started_at_unix_ms: 4980,
            finished_at_unix_ms: 5010,
            duration_ms: 30,
        },
        origin: pcagent_shared::Origin::SyntheticFixture,
        observations,
        coverage: vec![pcagent_inspect::Coverage {
            scope: scope.into(),
            status,
            source: "fixture".into(),
            detail: "Synthetic.".into(),
        }],
        names: Default::default(),
    }
}

/// The dev laptop of docs/battery.md, plugged in and full.
fn plugged_in(reference: &str) -> Vec<Observation> {
    use serde_json::json;
    cells(
        reference,
        &[
            ("battery_kind", json!("system")),
            ("battery_capacity_units", json!("milliwatt_hours")),
            ("battery_rechargeable", json!(true)),
            ("battery_design_capacity_mwh", json!(47310)),
            ("battery_full_charge_capacity_mwh", json!(39900)),
            ("battery_critical_bias_mwh", json!(0)),
            ("battery_cycle_count", Value::Null),
            ("battery_capacities_consistent", json!(true)),
            (
                "battery_capacity_retention_percent",
                json!(39900.0 / 47310.0 * 100.0),
            ),
            (
                "battery_estimated_wear_percent",
                json!(100.0 - 39900.0 / 47310.0 * 100.0),
            ),
            ("battery_remaining_capacity_mwh", json!(39900)),
            ("battery_charge_percent", json!(100.0)),
            ("battery_power_rate_mw", Value::Null),
            ("battery_discharge_power_mw", Value::Null),
            ("battery_charge_power_mw", Value::Null),
            ("battery_voltage_mv", json!(12900)),
            ("battery_charge_state", json!("idle")),
            ("battery_on_ac", json!(true)),
            ("battery_critical", json!(false)),
            ("battery_state_consistent", json!(true)),
            ("battery_estimated_runtime_windows_seconds", Value::Null),
            (
                "battery_estimated_runtime_at_current_rate_seconds",
                Value::Null,
            ),
        ],
    )
}

fn discharging(reference: &str, charge: f64) -> Vec<Observation> {
    use serde_json::json;
    cells(
        reference,
        &[
            ("battery_kind", json!("system")),
            ("battery_capacity_units", json!("milliwatt_hours")),
            ("battery_design_capacity_mwh", json!(57000)),
            ("battery_full_charge_capacity_mwh", json!(52800)),
            ("battery_remaining_capacity_mwh", json!(41250)),
            ("battery_charge_percent", json!(charge)),
            ("battery_power_rate_mw", json!(-8400)),
            ("battery_discharge_power_mw", json!(8400)),
            ("battery_voltage_mv", json!(11400)),
            ("battery_charge_state", json!("discharging")),
            ("battery_on_ac", json!(false)),
            ("battery_critical", json!(false)),
            ("battery_state_consistent", json!(true)),
            ("battery_capacities_consistent", json!(true)),
            ("battery_cycle_count", json!(212)),
            ("battery_estimated_runtime_windows_seconds", json!(17400)),
            (
                "battery_estimated_runtime_at_current_rate_seconds",
                json!(17678.6),
            ),
        ],
    )
}

#[test]
fn battery_unknowns_stay_none_and_never_zero() {
    let read = battery_read(inspection(
        plugged_in(B1),
        "battery_devices",
        CoverageStatus::Ok,
    ));
    assert!(!read.busy && !read.unavailable);
    let b = &read.batteries[0];
    assert_eq!(b.battery_ref, B1);
    assert_eq!(b.kind.as_deref(), Some("system"));
    assert_eq!(b.charge_percent, Some(100.0));
    assert_eq!(b.charge_state.as_deref(), Some("idle"));
    assert_eq!(b.on_ac, Some(true));
    assert_eq!(b.critical, Some(false));
    assert_eq!(b.full_charge_capacity_mwh, Some(39900));
    assert_eq!(b.design_capacity_mwh, Some(47310));
    assert_eq!(b.remaining_capacity_mwh, Some(39900));
    assert_eq!(b.power_rate_mw, None);
    assert_eq!(b.cycle_count, None);
    assert_eq!(b.estimated_runtime_windows_seconds, None);
    assert_eq!(b.estimated_runtime_at_current_rate_seconds, None);
    assert!((b.estimated_wear_percent.unwrap() - 15.6627).abs() < 1e-4);
    assert!((b.capacity_retention_percent.unwrap() - 84.3373).abs() < 1e-4);
    assert!(!b.inconsistent);
    assert_eq!(b.measured_at_unix_ms, 5000);
    let json = serde_json::to_string(b).unwrap();
    assert!(json.contains(r#""cycle_count":null"#));
    for field in [
        "power_rate_mw",
        "cycle_count",
        "estimated_runtime_windows_seconds",
        "estimated_runtime_at_current_rate_seconds",
    ] {
        assert!(json.contains(&format!(r#""{field}":null"#)), "{field}");
        assert!(!json.contains(&format!(r#""{field}":0"#)), "{field}");
    }
}

#[test]
fn discharging_battery_keeps_signed_power_and_both_runtimes_apart() {
    let read = battery_read(inspection(
        discharging(B1, 78.1),
        "battery_devices",
        CoverageStatus::Ok,
    ));
    let b = &read.batteries[0];
    assert_eq!(b.power_rate_mw, Some(-8400));
    assert_eq!(b.estimated_runtime_windows_seconds, Some(17400));
    assert_eq!(b.estimated_runtime_at_current_rate_seconds, Some(17678.6));
    assert_eq!(b.charge_state.as_deref(), Some("discharging"));
    assert_eq!(b.on_ac, Some(false));
    assert_eq!(b.cycle_count, Some(212));
    assert_eq!(b.charge_percent, Some(78.1));
}

#[test]
fn partial_or_contradicting_values_are_not_shown_as_measurements() {
    let mut observations = discharging(B1, 64.0);
    for o in &mut observations {
        match o.metric.as_str() {
            "battery_power_rate_mw" => o.status = CoverageStatus::Partial,
            "battery_state_consistent" => o.value = Value::Bool(false),
            _ => {}
        }
    }
    let read = battery_read(inspection(
        observations,
        "battery_devices",
        CoverageStatus::Ok,
    ));
    let b = &read.batteries[0];
    assert_eq!(b.power_rate_mw, None);
    assert!(b.inconsistent);
    assert_eq!(b.charge_percent, Some(64.0));
    // The contradicting reading still reaches the preview with its partial status.
    let source = read.source.unwrap();
    let power = source
        .observations
        .iter()
        .find(|o| o.metric == "battery_power_rate_mw")
        .unwrap();
    assert_eq!(power.status, CoverageStatus::Partial);
}

#[test]
fn desktop_other_systems_and_busy_reads_show_no_battery() {
    // A desktop: Windows lists no battery.
    let desktop = battery_read(inspection(
        vec![],
        "battery_devices",
        CoverageStatus::Unsupported,
    ));
    assert!(desktop.batteries.is_empty() && desktop.source.is_none());
    assert!(!desktop.busy && !desktop.unavailable);
    // Other systems.
    let other = battery_read(inspection(vec![], "windows", CoverageStatus::Unsupported));
    assert!(other.batteries.is_empty() && !other.unavailable && !other.busy);
    // A read refused or timed out keeps the caller's previous values.
    let busy = battery_read(inspection(vec![], "collector", CoverageStatus::Partial));
    assert!(busy.busy && busy.batteries.is_empty() && busy.source.is_none());
    assert!(!busy.unavailable);
    // Windows lists a battery that could not be read: say so instead of hiding it.
    let unreadable = battery_read(inspection(
        vec![],
        "battery_devices",
        CoverageStatus::Partial,
    ));
    assert!(unreadable.unavailable && unreadable.batteries.is_empty());
    let mut silent = plugged_in(B1);
    silent.retain(|o| o.metric == "battery_kind" || o.metric == "battery_voltage_mv");
    let silent = battery_read(inspection(silent, "battery_devices", CoverageStatus::Ok));
    assert!(silent.unavailable && silent.batteries.is_empty() && silent.source.is_none());
    // The battery capability: listed even when unreadable, unknown after a busy read.
    assert_eq!(desktop.battery_listed(), Some(false));
    assert_eq!(other.battery_listed(), Some(false));
    assert_eq!(busy.battery_listed(), None);
    assert_eq!(unreadable.battery_listed(), Some(true));
    assert_eq!(silent.battery_listed(), Some(true));
    let laptop = battery_read(inspection(
        plugged_in(B1),
        "battery_devices",
        CoverageStatus::Ok,
    ));
    assert_eq!(laptop.battery_listed(), Some(true));
}

#[test]
fn ups_and_non_system_batteries_are_left_out_and_batteries_are_never_summed() {
    use serde_json::json;
    let mut observations = discharging(B1, 80.0);
    observations.extend(cells(
        UPS,
        &[
            ("battery_kind", json!("short_term")),
            ("battery_charge_percent", json!(100.0)),
            ("battery_on_ac", json!(true)),
        ],
    ));
    observations.extend(cells(
        "battery-00000000-0000-4000-8000-00000000c002",
        &[
            ("battery_kind", json!("non_system")),
            ("battery_charge_percent", json!(55.0)),
        ],
    ));
    observations.extend(discharging(B2, 40.0));
    let read = battery_read(inspection(
        observations,
        "battery_devices",
        CoverageStatus::Ok,
    ));
    assert!(!read.unavailable);
    let shown: Vec<_> = read
        .batteries
        .iter()
        .map(|b| (b.battery_ref.as_str(), b.charge_percent))
        .collect();
    assert_eq!(shown, [(B1, Some(80.0)), (B2, Some(40.0))]);
    let source = read.source.unwrap();
    assert!(source
        .observations
        .iter()
        .all(|o| o.entity_ref == B1 || o.entity_ref == B2));
    // Only what the window displays leaves in an Explain preview.
    for hidden in [
        "battery_voltage_mv",
        "battery_discharge_power_mw",
        "battery_charge_power_mw",
        "battery_rechargeable",
        "battery_critical_bias_mwh",
    ] {
        assert!(
            source.observations.iter().all(|o| o.metric != hidden),
            "{hidden}"
        );
    }
    assert!(source
        .observations
        .iter()
        .all(|o| BATTERY_METRICS.contains(&o.metric.as_str())));
    assert_eq!(source.coverage[0].scope, "battery_devices");
    assert_eq!(source.window.duration_ms, 30);
    // The registry accepts it as the battery resource.
    let source = pcagent_agent::selection::SelectionSource::Battery(source);
    assert_eq!(source.targets().len(), 1);
    // A desktop with only a UPS has no system battery: no battery energy to check.
    let ups_only = battery_read(inspection(
        cells(
            UPS,
            &[
                ("battery_kind", json!("short_term")),
                ("battery_charge_percent", json!(100.0)),
                ("battery_on_ac", json!(true)),
            ],
        ),
        "battery_devices",
        CoverageStatus::Ok,
    ));
    assert_eq!(ups_only.battery_listed(), Some(false));
}

#[test]
fn relative_units_never_show_energy_or_power() {
    use serde_json::json;
    let mut observations = discharging(B1, 64.0);
    for o in &mut observations {
        if o.metric == "battery_capacity_units" {
            o.value = json!("relative");
        }
    }
    let read = battery_read(inspection(
        observations,
        "battery_devices",
        CoverageStatus::Ok,
    ));
    let b = &read.batteries[0];
    assert_eq!(b.capacity_units.as_deref(), Some("relative"));
    assert_eq!(b.charge_percent, Some(64.0));
    assert_eq!(b.charge_state.as_deref(), Some("discharging"));
    assert_eq!(b.estimated_runtime_windows_seconds, Some(17400));
    for value in [
        b.remaining_capacity_mwh,
        b.full_charge_capacity_mwh,
        b.design_capacity_mwh,
    ] {
        assert_eq!(value, None);
    }
    assert_eq!(b.power_rate_mw, None);
    assert_eq!(b.estimated_runtime_at_current_rate_seconds, None);
    assert_eq!(b.capacity_retention_percent, None);
    assert_eq!(b.estimated_wear_percent, None);
}

fn shown(live: &LiveBattery, now: Instant) -> Dashboard {
    let mut d = dashboard(&fixture());
    live.show(&mut d, now);
    d
}

fn one_battery() -> BatteryRead {
    battery_read(inspection(
        discharging(B1, 78.1),
        "battery_devices",
        CoverageStatus::Ok,
    ))
}

fn nothing() -> BatteryRead {
    battery_read(inspection(
        vec![],
        "battery_devices",
        CoverageStatus::Unsupported,
    ))
}

fn busy() -> BatteryRead {
    battery_read(inspection(vec![], "collector", CoverageStatus::Partial))
}

#[test]
fn live_battery_cadence_follows_what_the_read_shows() {
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    let mut live = LiveBattery::default();
    assert!(live.due(start), "the first dashboard asks at once");
    // A laptop: every second 5-second tick, stamped when asked.
    live.requested(at(0));
    live.record(one_battery(), Some("selection-source-1".into()), at(1));
    assert!(!live.due(at(6)));
    assert!(live.due(at(10)));
    let d = shown(&live, at(6));
    assert_eq!(d.batteries.len(), 1);
    assert_eq!(d.battery_source_id.as_deref(), Some("selection-source-1"));
    assert!(!d.battery_unavailable);
    // A desktop: a complete read that shows nothing is repeated every five minutes only.
    live.requested(at(12));
    live.record(nothing(), None, at(12));
    assert!(!live.due(at(30)));
    assert!(!live.due(at(311)));
    assert!(live.due(at(312)));
    let d = shown(&live, at(13));
    assert!(d.batteries.is_empty() && !d.battery_unavailable);
    // A battery Windows lists but cannot read is retried at the normal pace and said so.
    let mut unreadable = LiveBattery::default();
    unreadable.requested(at(0));
    unreadable.record(
        battery_read(inspection(
            vec![],
            "battery_devices",
            CoverageStatus::Partial,
        )),
        None,
        at(0),
    );
    assert!(unreadable.due(at(10)));
    assert!(shown(&unreadable, at(1)).battery_unavailable);
    // A busy read retries soon and keeps the previous values while they are recent.
    let mut stalled = LiveBattery::default();
    stalled.requested(at(0));
    stalled.record(one_battery(), Some("selection-source-2".into()), at(0));
    stalled.requested(at(10));
    stalled.record(busy(), None, at(10));
    assert!(stalled.due(at(15)));
    let d = shown(&stalled, at(15));
    assert_eq!(d.batteries.len(), 1);
    assert_eq!(d.battery_source_id.as_deref(), Some("selection-source-2"));
}

#[test]
fn stale_or_stalled_battery_values_are_not_shown_as_current() {
    let start = Instant::now();
    let at = |seconds: u64| start + Duration::from_secs(seconds);
    let mut live = LiveBattery::default();
    live.requested(at(0));
    live.record(one_battery(), Some("selection-source-1".into()), at(0));
    assert_eq!(shown(&live, at(30)).batteries.len(), 1);
    // Old values (the window was hidden) are neither shown nor a failure.
    let d = shown(&live, at(31));
    assert!(d.batteries.is_empty() && d.battery_source_id.is_none());
    assert!(!d.battery_unavailable);
    // Reads that stop finishing while a battery was known: no stale charge, no expired
    // source, and an explicit "not reported".
    live.record(busy(), None, at(20));
    let d = shown(&live, at(25));
    assert_eq!(d.batteries.len(), 1, "recent values survive a busy read");
    let d = shown(&live, at(31));
    assert!(d.batteries.is_empty() && d.battery_source_id.is_none());
    assert!(d.battery_unavailable);
    // The next read that finishes shows the battery again.
    live.record(one_battery(), Some("selection-source-3".into()), at(40));
    let d = shown(&live, at(41));
    assert_eq!(d.batteries.len(), 1);
    assert_eq!(d.battery_source_id.as_deref(), Some("selection-source-3"));
    assert!(!d.battery_unavailable);
    // The dashboard JSON carries the three battery fields.
    let json = serde_json::to_value(&d).unwrap();
    assert!(json["batteries"].is_array());
    assert_eq!(json["battery_source_id"], "selection-source-3");
    assert_eq!(json["battery_unavailable"], false);
}
