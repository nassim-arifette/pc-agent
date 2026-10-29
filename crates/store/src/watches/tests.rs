use super::*;
use pcagent_shared::{Origin, Status, Window};

fn rule() -> WatchRule {
    WatchRule::ResourceAbove {
        resource: WatchResource::Cpu,
        threshold_basis_points: 8000,
        sustained_seconds: 60,
    }
}

fn observation(at: u64) -> EventObservation {
    EventObservation {
        entity_ref: "system".into(),
        metric: "cpu_busy_basis_points".into(),
        value: 9000,
        unit: "basis_points_of_total_cpu".into(),
        source: "synthetic CPU counter".into(),
        status: Status::Ok,
        origin: Origin::SyntheticFixture,
        measured_at_unix_ms: Some(at),
        window: Some(Window {
            started_at_unix_ms: at.saturating_sub(5000),
            finished_at_unix_ms: at,
            duration_ms: 5000,
        }),
        derivation: None,
        inputs: vec![],
    }
}

fn open(store: &Store, watch: &Watch, at: u64) -> WatchAlert {
    assert!(store.watch_lease("owner", at).unwrap());
    store
        .open_watch_alert("owner", watch, observation(at), at)
        .unwrap()
        .unwrap()
}

#[test]
fn fresh_and_shipped_image_schema_upgrade_without_rewriting_history() {
    let store = Store::open_in_memory().unwrap();
    let id = store.create("Retain this conversation", 100).unwrap();
    store.conn.execute_batch(
        "DROP TABLE elevated_inspections;
         DROP TABLE system_samples; DROP TABLE history_names; DROP TABLE collection_policy;
         DROP TABLE watch_alerts; DROP TABLE watches; DROP TABLE watch_settings; DROP TABLE watch_lease; PRAGMA user_version=6;",
    ).unwrap();
    let store = Store::init(store.conn).unwrap();
    assert_eq!(
        store
            .conn
            .pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        crate::SCHEMA_VERSION
    );
    assert_eq!(
        store
            .conn
            .query_row::<String, _, _>("SELECT id FROM conversations", [], |r| r.get(0))
            .unwrap(),
        id
    );
    assert!(store
        .conn
        .prepare("SELECT turn_id FROM turn_images")
        .is_ok());
    assert!(store.watches().unwrap().is_empty());
}

#[test]
fn rules_are_closed_and_validate_ranges() {
    let json = serde_json::to_value(rule()).unwrap();
    assert!(serde_json::from_value::<WatchRule>(json.clone()).is_ok());
    let mut extra = json;
    extra["command"] = serde_json::json!("no shell");
    assert!(serde_json::from_value::<WatchRule>(extra).is_err());
    assert!(serde_json::from_str::<WatchRule>(r#"{"kind":"shell"}"#).is_err());
    for threshold in [999, 10001] {
        assert!(WatchRule::ResourceAbove {
            resource: WatchResource::Cpu,
            threshold_basis_points: threshold,
            sustained_seconds: 60
        }
        .validate()
        .is_err());
    }
    assert!(WatchRule::ResourceAbove {
        resource: WatchResource::Cpu,
        threshold_basis_points: 5000,
        sustained_seconds: 61
    }
    .validate()
    .is_err());
    for name in ["", "C:\\secret.exe", "../test.exe", "bad\n.exe"] {
        assert!(WatchRule::AppCpuAbove {
            executable: name.into(),
            threshold_basis_points: 100,
            sustained_seconds: 0
        }
        .validate()
        .is_err());
    }
    for root in ["C:", "\\\\server\\share\\", "C:\\folder", "🦀:\\"] {
        assert!(WatchRule::VolumeFreeBelow {
            root: root.into(),
            threshold_bytes: 1 << 30
        }
        .validate()
        .is_err());
    }
}

#[test]
fn targets_are_unique_case_insensitively_and_rules_have_no_arbitrary_twenty_limit() {
    let store = Store::open_in_memory().unwrap();
    for index in 0..25 {
        store
            .watch_create(
                WatchRule::AppCpuAbove {
                    executable: format!("app{index}.exe"),
                    threshold_basis_points: 100,
                    sustained_seconds: 0,
                },
                100,
            )
            .unwrap();
    }
    assert!(store
        .watch_create(
            WatchRule::AppCpuAbove {
                executable: "APP0.EXE".into(),
                threshold_basis_points: 5000,
                sustained_seconds: 300
            },
            200
        )
        .is_err());
    assert_eq!(store.watches().unwrap().len(), 25);
}

#[test]
fn lease_is_exclusive_recovers_expiry_and_future_clock() {
    let store = Store::open_in_memory().unwrap();
    assert!(store.watch_lease("first", 100_000).unwrap());
    assert!(!store.watch_lease("second", 110_000).unwrap());
    assert!(store.watch_lease("first", 120_000).unwrap());
    assert!(!store.watch_lease("second", 149_999).unwrap());
    assert!(store.watch_lease("second", 150_000).unwrap());
    assert!(!store.owns_watch_lease("first").unwrap());
    assert!(store.watch_lease("third", 100).unwrap());
    assert!(store.owns_watch_lease("third").unwrap());
}

#[test]
fn paused_window_cannot_open_alert_or_close_the_owner_alert() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    let alert = open(&store, &watch, 1000);
    assert!(store
        .open_watch_alert("other", &watch, observation(1000), 1000)
        .unwrap()
        .is_none());
    store.watch_release("other").unwrap();
    store.watch_recover("other").unwrap();
    assert!(store
        .watch_alert(&alert.id)
        .unwrap()
        .unwrap()
        .ended_at_unix_ms
        .is_none());
    store.watch_release("owner").unwrap();
    let alert = store.watch_alert(&alert.id).unwrap().unwrap();
    assert_eq!(alert.ended_at_unix_ms, Some(1000));
    assert_eq!(alert.close_reason, Some(WatchCloseReason::Interrupted));
}

#[test]
fn takeover_closes_at_last_check_not_restart_time() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    let alert = open(&store, &watch, 1000);
    assert!(store.watch_lease("next", 60_000).unwrap());
    store.watch_recover("next").unwrap();
    let ended = store.watch_alert(&alert.id).unwrap().unwrap();
    assert_eq!(ended.ended_at_unix_ms, Some(1000));
    assert_eq!(ended.observation.value, 9000);
}

#[test]
fn edit_keeps_original_alert_rule_and_measurement_disable_closes_delete_cascades() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    let alert = open(&store, &watch, 1000);
    let changed = WatchRule::ResourceAbove {
        resource: WatchResource::Memory,
        threshold_basis_points: 7000,
        sustained_seconds: 0,
    };
    store
        .watch_update(&watch.id, changed.clone(), 2000)
        .unwrap();
    let old = store.watch_alert(&alert.id).unwrap().unwrap();
    assert_eq!(old.rule, rule());
    assert_eq!(old.close_reason, Some(WatchCloseReason::Modified));
    let watch = store.watches().unwrap().remove(0);
    assert_eq!(watch.rule, changed);
    let second = open(&store, &watch, 3000);
    store.watch_set_enabled(&watch.id, false, 4000).unwrap();
    assert_eq!(
        store.watch_alert(&second.id).unwrap().unwrap().close_reason,
        Some(WatchCloseReason::Disabled)
    );
    store.watch_delete(&watch.id).unwrap();
    assert!(store.watch_alert(&alert.id).unwrap().is_none());
    assert!(store.watch_alert(&second.id).unwrap().is_none());
}

#[test]
fn duplicate_edit_rolls_back_and_does_not_close_alert() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    let other_rule = WatchRule::ResourceAbove {
        resource: WatchResource::Memory,
        threshold_basis_points: 7000,
        sustained_seconds: 0,
    };
    store.watch_create(other_rule.clone(), 0).unwrap();
    let alert = open(&store, &watch, 1000);
    assert!(store.watch_update(&watch.id, other_rule, 2000).is_err());
    assert!(store
        .watch_alert(&alert.id)
        .unwrap()
        .unwrap()
        .ended_at_unix_ms
        .is_none());
}

#[test]
fn paging_seen_retention_and_summary_do_not_silently_truncate() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    for at in 1..=205 {
        open(&store, &watch, at * 1000);
        store.watch_release("owner").unwrap();
    }
    let first = store.watch_alerts(0, 205_000, 200, 0).unwrap();
    assert_eq!(first.alerts.len(), 200);
    assert_eq!(first.total, 205);
    assert_eq!(first.next_offset, Some(200));
    let last = store.watch_alerts(0, 205_000, 200, 200).unwrap();
    assert_eq!(last.alerts.len(), 5);
    assert_eq!(last.next_offset, None);
    assert_ne!(first.alerts[199].id, last.alerts[0].id);
    store.watch_alerts_seen(200_000).unwrap();
    assert_eq!(store.watch_unseen().unwrap(), 5);
    let summary = store.watch_weekly_summary(205_000).unwrap();
    assert_eq!(summary.total, 205);
    assert_eq!(summary.interrupted, 205);
    assert!(summary.local_only);
    store
        .maintain_watches(SAMPLE_RETENTION_MS + 205_001)
        .unwrap();
    assert_eq!(store.watch_unseen().unwrap(), 0);
    assert_eq!(store.watches().unwrap().len(), 1);
}

#[test]
fn notification_reservation_is_single_use_and_cooldown_is_visible() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    let alert = open(&store, &watch, 1000);
    assert_eq!(
        store
            .watch_notification_reserve(&alert.id, 1000)
            .unwrap()
            .unwrap()
            .notification,
        WatchNotification::Failed
    );
    assert!(store
        .watch_notification_reserve(&alert.id, 1000)
        .unwrap()
        .is_none());
    store
        .watch_notification_result(&alert.id, WatchNotification::Sent)
        .unwrap();
    store.watch_release("owner").unwrap();
    let later = open(&store, &watch, 10_000);
    assert_eq!(
        store
            .watch_notification_reserve(&later.id, 10_000)
            .unwrap()
            .unwrap()
            .notification,
        WatchNotification::RuleCooldown
    );
    assert_eq!(store.watch_unseen().unwrap(), 2);
    store.watch_language_set("fr").unwrap();
    assert_eq!(store.watch_language().unwrap(), "fr");
    assert!(store.watch_language_set("arbitrary").is_err());
}

#[test]
fn expired_lease_cannot_open_and_clear_preserves_rules() {
    let store = Store::open_in_memory().unwrap();
    let watch = store.watch_create(rule(), 0).unwrap();
    store.watch_lease("owner", 1000).unwrap();
    assert!(store
        .open_watch_alert("owner", &watch, observation(31_000), 31_000)
        .unwrap()
        .is_none());
    let alert = open(&store, &watch, 32_000);
    assert_eq!(store.clear_watch_alerts().unwrap(), 1);
    assert!(store.watch_alert(&alert.id).unwrap().is_none());
    assert_eq!(store.watches().unwrap().len(), 1);
}

#[test]
fn hourly_limit_keeps_seventh_alert_and_does_not_report_delivery() {
    let store = Store::open_in_memory().unwrap();
    for n in 0..7 {
        let watch = store
            .watch_create(
                WatchRule::AppCpuAbove {
                    executable: format!("test{n}.exe"),
                    threshold_basis_points: 100,
                    sustained_seconds: 0,
                },
                0,
            )
            .unwrap();
        let alert = open(&store, &watch, 1000);
        let reserved = store
            .watch_notification_reserve(&alert.id, 1000)
            .unwrap()
            .unwrap();
        assert_eq!(
            reserved.notification,
            if n < 6 {
                WatchNotification::Failed
            } else {
                WatchNotification::HourlyLimit
            }
        );
        store
            .watch_notification_result(&alert.id, WatchNotification::Sent)
            .unwrap();
        assert_eq!(
            store.watch_alert(&alert.id).unwrap().unwrap().notification,
            if n < 6 {
                WatchNotification::Sent
            } else {
                WatchNotification::HourlyLimit
            }
        );
    }
    assert_eq!(store.watch_unseen().unwrap(), 7);
}
