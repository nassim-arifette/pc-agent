use super::*;
use crate::watches::{WatchNotification, WatchResource};
use pcagent_shared::{Evidence, Snapshot};

fn cpu(seconds: u64) -> WatchRule {
    WatchRule::ResourceAbove {
        resource: WatchResource::Cpu,
        threshold_basis_points: 8000,
        sustained_seconds: seconds,
    }
}

fn sample(wall: u64, values: &[(Metric, &str, u64)]) -> StoredSample {
    let mut snapshot = Snapshot::empty("watch-test");
    snapshot.origin = Origin::SyntheticFixture;
    snapshot.evidence = values
        .iter()
        .map(|(metric, entity, value)| {
            Evidence::measured(
                *metric,
                entity,
                *value,
                "synthetic-counter",
                Window {
                    started_at_unix_ms: wall.saturating_sub(5000),
                    finished_at_unix_ms: wall,
                    duration_ms: 5000,
                },
            )
        })
        .collect();
    StoredSample::from_snapshot(&snapshot, wall, 5000)
}

fn cpu_sample(wall: u64, value: u64) -> StoredSample {
    sample(wall, &[(Metric::CpuBusyBasisPoints, "system", value)])
}

fn tick(
    engine: &mut WatchEngine,
    store: &Store,
    mono: u64,
    wall: u64,
    value: u64,
) -> Vec<WatchAlert> {
    engine
        .sample(
            store,
            cpu_sample(wall, value),
            ProcessSort::Cpu,
            false,
            mono,
        )
        .unwrap()
}

#[test]
fn zero_duration_includes_threshold_but_volume_is_strictly_below() {
    let mut evaluation = Evaluation::default();
    assert!(matches!(
        evaluation.advance(&cpu(0), Some(7999), false, 0, 5000),
        Decision::Hold
    ));
    assert!(matches!(
        evaluation.advance(&cpu(0), Some(8000), false, 5000, 5000),
        Decision::Open
    ));
    let rule = WatchRule::VolumeFreeBelow {
        root: "C:\\".into(),
        threshold_bytes: 2 << 30,
    };
    assert!(matches!(
        evaluation.advance(&rule, Some(2 << 30), false, 10_000, 60_000),
        Decision::Hold
    ));
    assert!(matches!(
        evaluation.advance(&rule, Some((2 << 30) - 1), false, 70_000, 60_000),
        Decision::Open
    ));
}

#[test]
fn sustained_duration_starts_at_first_qualifying_reading_not_previous_interval() {
    let mut evaluation = Evaluation::default();
    for elapsed in (0..60_000).step_by(5000) {
        assert!(matches!(
            evaluation.advance(&cpu(60), Some(8000), false, elapsed, 5000),
            Decision::Hold
        ));
    }
    assert!(matches!(
        evaluation.advance(&cpu(60), Some(8000), false, 60_000, 5000),
        Decision::Open
    ));
}

#[test]
fn missing_and_long_gaps_restart_sustained_duration() {
    for missing in [true, false] {
        let mut evaluation = Evaluation::default();
        for elapsed in (0..60_000).step_by(5000) {
            evaluation.advance(&cpu(60), Some(9000), false, elapsed, 5000);
        }
        if missing {
            evaluation.advance(&cpu(60), None, false, 60_000, 5000);
        }
        let resumed = if missing { 65_000 } else { 75_000 };
        assert!(matches!(
            evaluation.advance(&cpu(60), Some(9000), false, resumed, 5000),
            Decision::Hold
        ));
        for elapsed in ((resumed + 5000)..(resumed + 60_000)).step_by(5000) {
            assert!(matches!(
                evaluation.advance(&cpu(60), Some(9000), false, elapsed, 5000),
                Decision::Hold
            ));
        }
        assert!(matches!(
            evaluation.advance(&cpu(60), Some(9000), false, resumed + 60_000, 5000),
            Decision::Open
        ));
    }
}

#[test]
fn exactly_three_intervals_do_not_reset_but_more_do() {
    let mut evaluation = Evaluation::default();
    evaluation.advance(&cpu(60), Some(8000), false, 0, 5000);
    evaluation.advance(&cpu(60), Some(8000), false, 15_000, 5000);
    assert_eq!(evaluation.above_since, Some(0));
    evaluation.advance(&cpu(60), Some(8000), false, 30_001, 5000);
    assert_eq!(evaluation.above_since, Some(30_001));
}

#[test]
fn return_hysteresis_requires_continuous_thirty_seconds_and_missing_is_not_zero() {
    let mut evaluation = Evaluation::default();
    evaluation.advance(&cpu(0), Some(7501), true, 0, 5000);
    assert_eq!(evaluation.return_since, None);
    for elapsed in (5000..35_000).step_by(5000) {
        assert!(matches!(
            evaluation.advance(&cpu(0), Some(7500), true, elapsed, 5000),
            Decision::Hold
        ));
    }
    evaluation.advance(&cpu(0), None, true, 35_000, 5000);
    for elapsed in (40_000..70_000).step_by(5000) {
        assert!(matches!(
            evaluation.advance(&cpu(0), Some(7500), true, elapsed, 5000),
            Decision::Hold
        ));
    }
    assert!(matches!(
        evaluation.advance(&cpu(0), Some(7500), true, 70_000, 5000),
        Decision::Close
    ));
}

#[test]
fn applications_return_after_sixty_seconds_with_small_cpu_threshold_hysteresis() {
    let rule = WatchRule::AppCpuAbove {
        executable: "load.exe".into(),
        threshold_basis_points: 100,
        sustained_seconds: 0,
    };
    let mut evaluation = Evaluation::default();
    assert!(!rule.returned(51));
    for elapsed in (0..60_000).step_by(10_000) {
        assert!(matches!(
            evaluation.advance(&rule, Some(50), true, elapsed, 10_000),
            Decision::Hold
        ));
    }
    assert!(matches!(
        evaluation.advance(&rule, Some(50), true, 60_000, 10_000),
        Decision::Close
    ));
}

#[test]
fn volume_needs_two_return_readings_and_a_missing_read_resets_them() {
    let rule = WatchRule::VolumeFreeBelow {
        root: "C:\\".into(),
        threshold_bytes: 20 << 30,
    };
    let mut evaluation = Evaluation::default();
    assert!(!rule.returned((22 << 30) - 1));
    evaluation.advance(&rule, Some(22 << 30), true, 0, 60_000);
    evaluation.advance(&rule, None, true, 60_000, 60_000);
    assert!(matches!(
        evaluation.advance(&rule, Some(22 << 30), true, 120_000, 60_000),
        Decision::Hold
    ));
    assert!(matches!(
        evaluation.advance(&rule, Some(22 << 30), true, 180_000, 60_000),
        Decision::Close
    ));
}

#[test]
fn recording_freeze_counts_neither_duration_nor_missing_gap() {
    let rule = WatchRule::AppMemoryAbove {
        executable: "load.exe".into(),
        threshold_bytes: 100 << 20,
        sustained_seconds: 60,
    };
    let mut evaluation = Evaluation::default();
    for elapsed in (0..=30_000).step_by(10_000) {
        evaluation.advance(&rule, Some(100 << 20), false, elapsed, 10_000);
    }
    evaluation.freeze(true, 30_000);
    evaluation.freeze(true, 400_000);
    evaluation.freeze(false, 630_000);
    for elapsed in (630_000..660_000).step_by(10_000) {
        assert!(matches!(
            evaluation.advance(&rule, Some(100 << 20), false, elapsed, 10_000),
            Decision::Hold
        ));
    }
    assert!(matches!(
        evaluation.advance(&rule, Some(100 << 20), false, 660_000, 10_000),
        Decision::Open
    ));
}

#[test]
fn gpu_and_memory_return_levels_are_exact() {
    let gpu = WatchRule::GpuTemperatureAbove {
        nvml_index: 0,
        threshold_celsius: 80,
        sustained_seconds: 0,
    };
    assert!(gpu.returned(75));
    assert!(!gpu.returned(76));
    let memory = WatchRule::AppMemoryAbove {
        executable: "load.exe".into(),
        threshold_bytes: 100 << 20,
        sustained_seconds: 0,
    };
    assert!(memory.returned(90 << 20));
    assert!(!memory.returned((90 << 20) + 1));
}

#[test]
fn wall_clock_jumps_do_not_trigger_or_extend_duration() {
    for forward in [true, false] {
        let store = Store::open_in_memory().unwrap();
        store.watch_create(cpu(60), 1).unwrap();
        let mut engine = WatchEngine::new();
        for mono in (0..60_000).step_by(5000) {
            let wall = if mono < 30_000 {
                1_000_000 + mono
            } else if forward {
                9_000_000 + mono
            } else {
                100_000 + mono
            };
            assert!(tick(&mut engine, &store, mono, wall, 8000).is_empty());
        }
        let wall = if forward { 9_060_000 } else { 160_000 };
        let alerts = tick(&mut engine, &store, 60_000, wall, 8000);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].started_at_unix_ms, wall);
        assert_eq!(alerts[0].observation.origin, Origin::SyntheticFixture);
    }
}

#[test]
fn only_one_engine_can_evaluate_and_takeover_closes_at_last_check() {
    let store = Store::open_in_memory().unwrap();
    store.watch_create(cpu(0), 1).unwrap();
    let mut first = WatchEngine::new();
    let alert = tick(&mut first, &store, 0, 1000, 9000).remove(0);
    let mut second = WatchEngine::new();
    assert!(tick(&mut second, &store, 0, 5000, 9000).is_empty());
    assert_eq!(
        second.status(&store).unwrap().rules[0].phase,
        WatchPhase::Paused
    );
    assert_eq!(tick(&mut second, &store, 40_000, 41_000, 9000).len(), 1);
    let interrupted = store.watch_alert(&alert.id).unwrap().unwrap();
    assert_eq!(interrupted.ended_at_unix_ms, Some(1000));
    assert_eq!(
        interrupted.close_reason,
        Some(WatchCloseReason::Interrupted)
    );
    first.stop(&store).unwrap();
    assert!(store.owns_watch_lease(&second.owner).unwrap());
}

#[test]
fn return_and_extreme_updates_keep_notification_and_seen_state() {
    let store = Store::open_in_memory().unwrap();
    store.watch_create(cpu(0), 1).unwrap();
    let mut engine = WatchEngine::new();
    let alert = tick(&mut engine, &store, 0, 1000, 8000).remove(0);
    store.watch_notification_reserve(&alert.id, 1000).unwrap();
    store
        .watch_notification_result(&alert.id, WatchNotification::Sent)
        .unwrap();
    store.watch_alerts_seen(1000).unwrap();
    tick(&mut engine, &store, 5000, 6000, 9500);
    for mono in (10_000..=40_000).step_by(5000) {
        assert!(tick(&mut engine, &store, mono, mono + 1000, 7500).is_empty());
    }
    let closed = store.watch_alert(&alert.id).unwrap().unwrap();
    assert_eq!(closed.extreme_observation.value, 9500);
    assert_eq!(closed.observation.value, 8000);
    assert_eq!(closed.close_reason, Some(WatchCloseReason::Returned));
    assert_eq!(closed.ended_at_unix_ms, Some(41_000));
    assert_eq!(closed.notification, WatchNotification::Sent);
    assert!(closed.seen);
}

#[test]
fn deleted_cleared_and_disabled_alerts_are_never_resurrected() {
    for change in ["clear", "delete", "disable", "edit"] {
        let store = Store::open_in_memory().unwrap();
        let watch = store.watch_create(cpu(0), 1).unwrap();
        let mut engine = WatchEngine::new();
        let alert = tick(&mut engine, &store, 0, 1000, 8000).remove(0);
        match change {
            "clear" => {
                store.clear_watch_alerts().unwrap();
            }
            "delete" => store.watch_delete(&watch.id).unwrap(),
            "disable" => store.watch_set_enabled(&watch.id, false, 2000).unwrap(),
            _ => store.watch_update(&watch.id, cpu(60), 2000).unwrap(),
        }
        tick(&mut engine, &store, 5000, 6000, 8000);
        assert!(store
            .watch_alert(&alert.id)
            .unwrap()
            .is_none_or(|a| a.ended_at_unix_ms.is_some()));
    }
}

#[test]
fn rule_refresh_observes_other_window_changes_within_ten_seconds() {
    let store = Store::open_in_memory().unwrap();
    let mut engine = WatchEngine::new();
    tick(&mut engine, &store, 0, 1000, 8000);
    store.watch_create(cpu(0), 2000).unwrap();
    assert!(tick(&mut engine, &store, 5000, 6000, 8000).is_empty());
    assert_eq!(tick(&mut engine, &store, 10_000, 11_000, 8000).len(), 1);
}

#[test]
fn reenable_already_enabled_rule_does_not_duplicate_or_orphan_open_alert() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(cpu(0), 1).unwrap();
    let mut engine = WatchEngine::new();
    let alert = tick(&mut engine, &store, 0, 1000, 8000).remove(0);
    store.watch_set_enabled(&watch.id, true, 2000).unwrap();
    engine.refresh(&store, 5000, 6000, true).unwrap();
    assert!(tick(&mut engine, &store, 5000, 6000, 9000).is_empty());
    assert_eq!(
        engine.status(&store).unwrap().rules[0].alert_id.as_ref(),
        Some(&alert.id)
    );
}

#[test]
fn exact_basename_sum_is_a_lower_bound_and_excludes_children_with_other_names() {
    let mut sample = sample(
        1000,
        &[
            (Metric::ProcessCpuBasisPoints, "p1", 2000),
            (Metric::ProcessCpuBasisPoints, "p2", 3000),
            (Metric::ProcessCpuBasisPoints, "p3", 4500),
            (Metric::ProcessCpuBasisPoints, "p4", 100),
        ],
    );
    sample.process_names.extend([
        ("p1".into(), "App.EXE".into()),
        ("p2".into(), "app.exe".into()),
        ("p3".into(), "app-helper.exe".into()),
        ("p4".into(), "myapp.exe".into()),
    ]);
    let result = app_observation(&sample, "APP.exe", Metric::ProcessCpuBasisPoints).unwrap();
    assert_eq!(result.value, 5000);
    assert_eq!(result.inputs.len(), 2);
    assert_eq!(result.status, Status::Partial);
    assert!(app_observation(&sample, "unknown.exe", Metric::ProcessCpuBasisPoints).is_err());
}

#[test]
fn application_rules_use_only_matching_sort_and_memory_freezes_while_recording() {
    let store = Store::open_in_memory().unwrap();
    let rule = WatchRule::AppMemoryAbove {
        executable: "app.exe".into(),
        threshold_bytes: 64 << 20,
        sustained_seconds: 0,
    };
    store.watch_create(rule, 1).unwrap();
    let mut engine = WatchEngine::new();
    let reading = |at| {
        let mut sample = sample(at, &[(Metric::ProcessWorkingSetBytes, "p1", 64 << 20)]);
        sample.process_names.insert("p1".into(), "app.exe".into());
        sample
    };
    assert!(engine
        .sample(&store, reading(1000), ProcessSort::Cpu, false, 0)
        .unwrap()
        .is_empty());
    assert!(engine
        .sample(&store, reading(6000), ProcessSort::Cpu, true, 5000)
        .unwrap()
        .is_empty());
    assert_eq!(
        engine.status(&store).unwrap().rules[0].phase,
        WatchPhase::Paused
    );
    let alerts = engine
        .sample(
            &store,
            reading(11_000),
            ProcessSort::WorkingSet,
            false,
            10_000,
        )
        .unwrap();
    assert_eq!(alerts.len(), 1);
    assert!(alerts[0].lower_bound);
}

#[test]
fn memory_and_disk_use_exact_history_derivations_and_gpu_is_not_discarded() {
    let store = Store::open_in_memory().unwrap();
    for resource in [WatchResource::Memory, WatchResource::DiskBusy] {
        store
            .watch_create(
                WatchRule::ResourceAbove {
                    resource,
                    threshold_basis_points: 8000,
                    sustained_seconds: 0,
                },
                1,
            )
            .unwrap();
    }
    store
        .watch_create(
            WatchRule::GpuTemperatureAbove {
                nvml_index: 0,
                threshold_celsius: 80,
                sustained_seconds: 0,
            },
            1,
        )
        .unwrap();
    let sample = sample(
        1000,
        &[
            (Metric::PhysicalAvailableBytes, "system", 10),
            (Metric::PhysicalTotalBytes, "system", 100),
            (Metric::DiskIdleBasisPoints, "system", 1000),
            (Metric::GpuTemperatureCelsius, "gpu-nvidia-0", 80),
        ],
    );
    let expected = system_observations(&sample);
    let alerts = WatchEngine::new()
        .sample(&store, sample, ProcessSort::Cpu, false, 0)
        .unwrap();
    assert_eq!(alerts.len(), 3);
    for observation in expected.into_iter().flatten() {
        assert!(alerts
            .iter()
            .any(|a| serde_json::to_value(&a.observation).unwrap()
                == serde_json::to_value(&observation).unwrap()));
    }
}

fn volume(kind: VolumeKind, free: u64) -> VolumeInfo {
    VolumeInfo {
        root: "C:\\".into(),
        kind,
        total_bytes: Some(100 << 30),
        free_bytes: Some(free),
        available_to_caller_bytes: Some(free),
        source: "synthetic-drive".into(),
        status: Status::Ok,
        limitations: vec![],
    }
}

#[test]
fn volume_and_application_target_validation_rejects_unobserved_or_nonfixed_targets() {
    let mut engine = WatchEngine::new();
    let rule = WatchRule::VolumeFreeBelow {
        root: "c:\\".into(),
        threshold_bytes: 10 << 30,
    };
    for kind in [
        VolumeKind::Removable,
        VolumeKind::Network,
        VolumeKind::RamDisk,
        VolumeKind::Unknown,
    ] {
        assert!(engine
            .validate_target(&rule, &[volume(kind, 5 << 30)])
            .is_err());
    }
    assert!(engine
        .validate_target(&rule, &[volume(VolumeKind::Fixed, 5 << 30)])
        .is_ok());
    let too_large = WatchRule::VolumeFreeBelow {
        root: "C:\\".into(),
        threshold_bytes: 101 << 30,
    };
    assert!(engine
        .validate_target(&too_large, &[volume(VolumeKind::Fixed, 5 << 30)])
        .is_err());
    let app = WatchRule::AppCpuAbove {
        executable: "app.exe".into(),
        threshold_basis_points: 1000,
        sustained_seconds: 0,
    };
    assert!(engine.validate_target(&app, &[]).is_err());
    let mut reading = cpu_sample(1000, 8000);
    reading.process_names.insert("p1".into(), "APP.exe".into());
    engine.latest = Some(reading);
    assert!(engine.validate_target(&app, &[]).is_ok());
}

#[test]
fn fixed_volume_opens_once_and_missing_volume_does_not_close_it() {
    let store = Store::open_in_memory().unwrap();
    store
        .watch_create(
            WatchRule::VolumeFreeBelow {
                root: "C:\\".into(),
                threshold_bytes: 10 << 30,
            },
            1,
        )
        .unwrap();
    let mut engine = WatchEngine::new();
    engine.refresh(&store, 0, 1000, true).unwrap();
    let alert = engine
        .volumes(&store, &[volume(VolumeKind::Fixed, 5 << 30)], 0, 1000)
        .unwrap()
        .remove(0);
    engine.refresh(&store, 60_000, 61_000, true).unwrap();
    assert!(engine
        .volumes(&store, &[], 60_000, 61_000)
        .unwrap()
        .is_empty());
    assert_eq!(
        engine.status(&store).unwrap().rules[0].phase,
        WatchPhase::Missing
    );
    assert!(store
        .watch_alert(&alert.id)
        .unwrap()
        .unwrap()
        .ended_at_unix_ms
        .is_none());
    engine.stop(&store).unwrap();
    assert_eq!(
        store
            .watch_alert(&alert.id)
            .unwrap()
            .unwrap()
            .ended_at_unix_ms,
        Some(61_000)
    );
}
