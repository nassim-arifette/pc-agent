use super::*;
use std::path::{Path, PathBuf};

fn value(metric: Metric, entity: &str, at: u64, amount: u64, duration: u64) -> SampleValue {
    SampleValue {
        metric,
        entity_ref: entity.into(),
        value: Some(amount),
        min: Some(amount),
        max: Some(amount),
        measured: 1,
        expected: 1,
        status: Status::Ok,
        source: "synthetic history cache fixture".into(),
        observed_from_unix_ms: Some(at.saturating_sub(duration)),
        observed_to_unix_ms: Some(at),
        observed_duration_ms: duration,
        weighted_sum: u128::from(amount) * u128::from(duration),
        weight_ms: duration,
    }
}

fn sample(at: u64, cpu: u64) -> StoredSample {
    let mut values = SYSTEM_CHARTS
        .iter()
        .enumerate()
        .map(|(index, metric)| value(*metric, "system", at, cpu + index as u64, 1_000))
        .collect::<Vec<_>>();
    values.push(value(
        Metric::HardFaultPagesPerSecondX100,
        "system",
        at,
        606,
        1_000,
    ));
    values.push(value(
        Metric::ProcessCpuBasisPoints,
        "proc-fixture",
        at,
        777,
        1_000,
    ));
    StoredSample {
        id: format!("sample-{at}"),
        session_id: "session-one".into(),
        measured_at_unix_ms: at,
        interval_ms: 1_000,
        resolution_ms: 1_000,
        origin: Origin::SyntheticFixture,
        values,
        process_names: BTreeMap::from([("proc-fixture".into(), "fixture.exe".into())]),
    }
}

fn query(metric: Metric, from: u64, to: u64) -> HistoryQuery {
    HistoryQuery {
        metric,
        entity_ref: "system".into(),
        from_unix_ms: from,
        to_unix_ms: to,
    }
}

#[test]
fn chart_uses_one_system_projection_and_never_decodes_process_payloads() {
    let mut store = Store::open_in_memory().unwrap();
    for i in 1..=600 {
        store
            .append_sample(&sample(i * 1_000, if i == 300 { 9_999 } else { 100 }))
            .unwrap();
    }
    store
        .conn
        .execute("UPDATE samples SET payload='not JSON'", [])
        .unwrap();
    let chart = store.history_chart(0, 601_000, 300).unwrap();
    assert_eq!(chart.series.len(), 5);
    assert!(chart.series.iter().all(|s| s.points.len() <= 300));
    let cpu = &chart.histories[0];
    assert_eq!(cpu.points.iter().filter_map(|p| p.max).max(), Some(9_999));
    assert_eq!(
        cpu.points.iter().map(|p| p.measured_samples).sum::<u64>(),
        600
    );
    assert!(chart
        .histories
        .iter()
        .all(|s| s.events.is_empty() && s.process_names.is_empty()));
    let bytes = serde_json::to_vec(&chart).unwrap();
    assert!(bytes.len() < 180_000, "wire size {}", bytes.len());
    assert_eq!(
        bytes,
        serde_json::to_vec(&store.history_chart(0, 601_000, 300).unwrap()).unwrap()
    );
    assert!(
        store.history_chart(0, 601_000, 30).unwrap().series[0]
            .points
            .len()
            <= 30
    );
}

#[test]
fn chart_preserves_disconnected_segments_and_unknowns_even_inside_one_bucket() {
    let store = Store::open_in_memory().unwrap();
    let mut first = sample(1_000, 111);
    insert(&store, &first, false);
    first.id = "unknown".into();
    first.measured_at_unix_ms = 2_000;
    first.values.clear();
    insert(&store, &first, false);
    first = sample(3_000, 333);
    insert(&store, &first, false);
    first = sample(30_000, 999);
    first.session_id = "restart".into();
    insert(&store, &first, false);
    let chart = store.history_chart(0, 60_000, 1).unwrap();
    assert_eq!(chart.series[0].points.len(), 4);
    assert_eq!(chart.series[0].points[1].1, None);
    assert!(chart.gaps.iter().any(|g| g.reason == "collector_restarted"));
    assert_eq!(chart.series[0].points[3].1, Some(999));
}

#[test]
fn chart_events_are_exact_beyond_two_hundred_and_only_sent_once() {
    let mut store = Store::open_in_memory().unwrap();
    for i in 1..=260 {
        store.append_sample(&sample(i * 10_000, 9_999)).unwrap();
    }
    let expected: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM activity_events", [], |r| r.get(0))
        .unwrap();
    assert!(expected > 200);
    let chart = store.history_chart(0, 2_610_000, 300).unwrap();
    assert_eq!(chart.events.len(), expected as usize);
    let json = serde_json::to_string(&chart).unwrap();
    assert_eq!(json.matches("\"events\":").count(), 1);
}

#[test]
fn measured_budget_evicts_background_but_keeps_recorded_samples_and_battery_evidence() {
    let mut store = Store::open_in_memory().unwrap();
    let recording = store.begin_recording(30, 1_000).unwrap();
    store.append_sample(&sample(1_000, 900)).unwrap();
    store
        .finish_recording(&recording.id, 2_000, false, 1)
        .unwrap();
    for i in 60..700 {
        let mut s = sample(i * 60_000, 100);
        s.resolution_ms = 60_000;
        for v in &mut s.values {
            v.source = "large synthetic local source".repeat(50);
        }
        insert(&store, &s, false);
    }
    assert!(store.history_collection().unwrap().allocated_bytes > 1024 * 1024);
    store
        .set_collection_budget(1024 * 1024, 42_000_000)
        .unwrap();
    let stats = store.history_collection().unwrap();
    assert!(!stats.over_budget, "{stats:?}");
    assert!(stats.budget_removed_samples > 0);
    assert_eq!(stats.protected_samples, 1);
    store.maintain_samples(2 * SAMPLE_RETENTION_MS).unwrap();
    assert!(store.recording(&recording.id).unwrap().is_some());
    assert_eq!(
        store.history_chart(0, 3_000, 3).unwrap().series[0].points[0].1,
        Some(900)
    );
    let broken: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(broken, 0);
    store.clear_samples().unwrap();
    assert_eq!(store.history_collection().unwrap().samples, 0);
    let count: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM system_samples", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn protected_evidence_reports_budget_overrun_instead_of_deleting_or_compacting_it() {
    let mut store = Store::open_in_memory().unwrap();
    let recording = store.begin_recording(120, 0).unwrap();
    for i in 1..120 {
        let mut s = sample(i * 1_000, 100);
        for v in &mut s.values {
            v.source = "synthetic".repeat(500);
        }
        insert(&store, &s, false);
    }
    store
        .finish_recording(&recording.id, 120_000, false, 119)
        .unwrap();
    store
        .set_collection_budget(1024 * 1024, 2 * SAMPLE_RETENTION_MS)
        .unwrap();
    let stats = store.history_collection().unwrap();
    assert!(stats.over_budget);
    assert_eq!(stats.samples, 119);
    assert_eq!(stats.protected_samples, 119);
    assert_eq!(stats.budget_removed_samples, 0);
    assert!(stats.protected_payload_bytes > stats.budget_bytes);
    assert_eq!(
        store.history_chart(0, 120_000, 120).unwrap().histories[0].points[0].resolution_ms,
        1_000
    );
}

#[test]
fn overview_counts_overlap_and_pages_every_recording_without_selecting_an_active_one() {
    let mut store = Store::open_in_memory().unwrap();
    for i in 1..=25 {
        let start = i * 100_000;
        let recording = store.begin_recording(30, start).unwrap();
        assert!(store.recording_detail(&recording.id).is_err());
        store.append_sample(&sample(start + 1_000, 9_000)).unwrap();
        store
            .finish_recording(&recording.id, start + 30_000, false, 1)
            .unwrap();
        let detail = store.recording_detail(&recording.id).unwrap().unwrap();
        assert_eq!(detail.recording.id, recording.id);
        assert_eq!(detail.chart.histories[0].points.len(), 1);
        assert!(!detail.minute_summaries);
    }
    let overview = store.history_overview(1_000_000, 3_000_000).unwrap();
    assert_eq!(overview.recordings.len(), 20);
    assert!(overview.more_recordings);
    assert_eq!(overview.latest_sample_unix_ms, Some(2_501_000));
    let events = store
        .activity_events(1_000_000, 3_000_000, usize::MAX)
        .unwrap();
    assert_eq!(overview.periods[1].event_count, events.len() as u64);
    let page2 = store.history_recordings(20).unwrap();
    assert_eq!(page2.recordings.len(), 5);
    assert!(!page2.more);
    assert!(page2
        .recordings
        .iter()
        .all(|r| overview.recordings.iter().all(|other| r.id != other.id)));
    assert!(store.history_overview(3_000_001, 3_000_000).is_err());
    assert!(store.history_overview(0, 26 * 3_600_000).is_err());
    assert!(store.recording_detail("../private").is_err());
    assert!(store
        .recording_detail("recording-unknown")
        .unwrap()
        .is_none());
}

fn drop_history_projection(store: &Store) {
    store
        .conn
        .execute_batch(
            "DROP TABLE system_samples; DROP TABLE history_names; DROP TABLE collection_policy;
         PRAGMA user_version=7;",
        )
        .unwrap();
}

#[test]
fn release_v6_upgrades_through_real_watch_v7_and_history_v8_losslessly() {
    let mut store = Store::open_in_memory().unwrap();
    let conversation = store.create("Private local conversation", 1).unwrap();
    for i in 1..=3 {
        store.append_sample(&sample(i * 5_000, i * 100)).unwrap();
    }
    let before = store.history_chart(0, 20_000, 300).unwrap();
    let serialized = serde_json::to_value(&before).unwrap();
    // Exercise the shipped legacy payload as well as the compact format.
    let compact: String = store
        .conn
        .query_row("SELECT payload FROM samples ORDER BY id LIMIT 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    let legacy = serde_json::to_string(&codec::decode(&compact).unwrap()).unwrap();
    store
        .conn
        .execute(
            "UPDATE samples SET payload=?1 WHERE id=(SELECT MIN(id) FROM samples)",
            [legacy],
        )
        .unwrap();
    drop_history_projection(&store);
    store.conn.execute_batch(
        "DROP TABLE watch_alerts; DROP TABLE watches; DROP TABLE watch_settings; DROP TABLE watch_lease;
         PRAGMA user_version=6;"
    ).unwrap();
    let store = Store::init(store.conn).unwrap();
    assert_eq!(
        store
            .conn
            .pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        8
    );
    assert_eq!(
        serde_json::to_value(store.history_chart(0, 20_000, 300).unwrap()).unwrap(),
        serialized
    );
    assert_eq!(
        store
            .conn
            .query_row::<String, _, _>("SELECT id FROM conversations", [], |r| r.get(0))
            .unwrap(),
        conversation
    );
    assert!(store.watches().unwrap().is_empty());
    assert_eq!(store.history_collection().unwrap().samples, 3);
    assert_eq!(
        store
            .conn
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                .get(0))
            .unwrap(),
        0
    );
    store.clear_samples().unwrap();
    assert_eq!(
        store
            .conn
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM system_samples", [], |r| r.get(0))
            .unwrap(),
        0
    );
}

#[test]
fn v8_migration_failure_rolls_back_and_v7_watch_rows_survive_retry_and_clear() {
    use crate::watches::{WatchResource, WatchRule};
    let mut store = Store::open_in_memory().unwrap();
    let capture = sample(5_000, 9_000);
    store.append_sample(&capture).unwrap();
    let watch = store
        .watch_create(
            WatchRule::ResourceAbove {
                resource: WatchResource::Cpu,
                threshold_basis_points: 8_000,
                sustained_seconds: 0,
            },
            0,
        )
        .unwrap();
    assert!(store.watch_lease("migration-test", 5_000).unwrap());
    let alert = store
        .open_watch_alert(
            "migration-test",
            &watch,
            EventObservation::measured(&capture.values[0], capture.origin).unwrap(),
            5_000,
        )
        .unwrap()
        .unwrap();
    let payload: String = store
        .conn
        .query_row("SELECT payload FROM samples", [], |r| r.get(0))
        .unwrap();
    drop_history_projection(&store);
    store
        .conn
        .execute("UPDATE samples SET payload='broken'", [])
        .unwrap();
    assert!(super::super::system::migrate(&store.conn).is_err());
    assert_eq!(
        store
            .conn
            .pragma_query_value::<i64, _>(None, "user_version", |r| r.get(0))
            .unwrap(),
        7
    );
    assert_eq!(store.conn.query_row::<i64, _, _>(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('system_samples','history_names','collection_policy')",
        [], |r| r.get(0)).unwrap(), 0);
    store
        .conn
        .execute("UPDATE samples SET payload=?1", [payload])
        .unwrap();
    let store = Store::init(store.conn).unwrap();
    assert!(store.watch_alert(&alert.id).unwrap().is_some());
    store.conn.execute_batch(
        "CREATE TRIGGER fail_alert_delete BEFORE DELETE ON watch_alerts BEGIN SELECT RAISE(ABORT,'fixture'); END;"
    ).unwrap();
    assert!(store.clear_samples().is_err());
    assert_eq!(store.history_collection().unwrap().samples, 1);
    assert!(store.watch_alert(&alert.id).unwrap().is_some());
    store
        .conn
        .execute_batch("DROP TRIGGER fail_alert_delete;")
        .unwrap();
    store.clear_samples().unwrap();
    assert!(store.watch_alert(&alert.id).unwrap().is_none());
    assert_eq!(store.watches().unwrap()[0].id, watch.id);
    assert!(store.owns_watch_lease("migration-test").unwrap());
    assert_eq!(
        store
            .conn
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r
                .get(0))
            .unwrap(),
        0
    );
}

fn json(series: &HistorySeries) -> String {
    serde_json::to_string(series).unwrap()
}

/// These are already stored aliases, so direct fixture insertion avoids random alias differences
/// when comparing an old-format database with a compact or mixed-format database.
fn insert(store: &Store, sample: &StoredSample, legacy: bool) {
    let text = if legacy {
        serde_json::to_string(sample).unwrap()
    } else {
        codec::encode(sample).unwrap()
    };
    store.conn.execute(
        "INSERT INTO samples(sample_key,session_id,measured_at,resolution_ms,payload) VALUES(?1,?2,?3,?4,?5)",
        params![sample.id, sample.session_id, stored(sample.measured_at_unix_ms), stored(sample.resolution_ms), text],
    ).unwrap();
    super::super::system::insert(&store.conn, sample).unwrap();
}

struct Database(PathBuf);

impl Database {
    fn new() -> Self {
        let directory = std::env::temp_dir().join(new_id("pcagent-history-cache"));
        std::fs::create_dir(&directory).unwrap();
        Self(directory)
    }

    fn path(&self) -> PathBuf {
        self.0.join("history.sqlite")
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // Only the three files of this test-owned database are removed; never recurse.
        for name in ["history.sqlite", "history.sqlite-wal", "history.sqlite-shm"] {
            let _ = std::fs::remove_file(self.0.join(name));
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn same_as_new_reader(store: &Store, path: &Path, query: &HistoryQuery) {
    assert_eq!(
        json(&store.metric_history(query).unwrap()),
        json(&Store::open(path).unwrap().metric_history(query).unwrap()),
    );
}

#[test]
fn one_cached_window_contains_five_independent_exact_system_series() {
    let store = Store::open_in_memory().unwrap();
    insert(&store, &sample(1_000, 100), false);
    insert(&store, &sample(2_000, 200), false);
    for (index, metric) in SYSTEM_CHARTS.iter().enumerate() {
        let query = query(*metric, 0, 3_000);
        let cached = store.metric_history(&query).unwrap();
        let uncached = store
            .read_history(&query, &[*metric], None)
            .unwrap()
            .remove(0);
        assert_eq!(json(&cached), json(&uncached));
        assert_eq!(cached.points[0].value, Some(100 + index as u64));
        let cache = store.history_cache.borrow();
        let cache = cache.as_ref().unwrap();
        assert_eq!(cache.series.len(), SYSTEM_CHARTS.len());
        assert!(cache
            .series
            .iter()
            .all(|series| series.points.len() <= MAX_POINTS));
        assert_eq!(cache.revision, store.history_revision().unwrap());
    }
}

#[test]
fn own_append_and_clear_invalidate_values_names_and_events() {
    let mut store = Store::open_in_memory().unwrap();
    store.append_sample(&sample(1_000, 9_100)).unwrap();
    let query = query(Metric::CpuBusyBasisPoints, 0, 4_000);
    let first = store.metric_history(&query).unwrap();
    assert_eq!(first.points.len(), 1);
    assert!(!first.events.is_empty());
    assert!(!first.process_names.is_empty());
    let revision = store.history_cache.borrow().as_ref().unwrap().revision;
    store.append_sample(&sample(2_000, 9_500)).unwrap();
    let updated = store.metric_history(&query).unwrap();
    assert_eq!(updated.points.len(), 2);
    assert_eq!(updated.points[1].value, Some(9_500));
    assert_ne!(
        revision,
        store.history_cache.borrow().as_ref().unwrap().revision
    );
    store.clear_samples().unwrap();
    let empty = store.metric_history(&query).unwrap();
    assert!(empty.points.is_empty());
    assert!(empty.process_names.is_empty());
    assert!(empty.events.is_empty());
}

#[test]
fn own_maintenance_replaces_cached_raw_points_with_exact_minute_measurements() {
    let mut store = Store::open_in_memory().unwrap();
    let now = 10 * SAMPLE_RETENTION_MS;
    let minute = (now - RAW_RETENTION_MS - 120_000) / 60_000 * 60_000;
    store.append_sample(&sample(minute + 1_000, 1_000)).unwrap();
    store.append_sample(&sample(minute + 2_000, 9_000)).unwrap();
    let query = query(Metric::CpuBusyBasisPoints, minute, minute + 60_000);
    assert_eq!(store.metric_history(&query).unwrap().points.len(), 2);
    store.maintain_samples(now).unwrap();
    let history = store.metric_history(&query).unwrap();
    assert_eq!(history.points.len(), 1);
    let point = &history.points[0];
    assert_eq!(point.value, Some(5_000));
    assert_eq!(point.min, Some(1_000));
    assert_eq!(point.max, Some(9_000));
    assert_eq!(point.measured_samples, 2);
    assert_eq!(point.observed_duration_ms, 2_000);
    assert_eq!(point.observed_from_unix_ms, Some(minute));
    assert_eq!(point.observed_to_unix_ms, Some(minute + 2_000));
}

#[test]
fn another_connection_append_event_only_update_and_delete_revoke_the_cache() {
    let database = Database::new();
    let path = database.path();
    let reader = Store::open(&path).unwrap();
    let mut writer = Store::open(&path).unwrap();
    writer.append_sample(&sample(1_000, 9_100)).unwrap();
    let query = query(Metric::CpuBusyBasisPoints, 0, 4_000);
    assert_eq!(reader.metric_history(&query).unwrap().points.len(), 1);
    writer.append_sample(&sample(2_000, 9_500)).unwrap();
    same_as_new_reader(&reader, &path, &query);
    assert_eq!(reader.metric_history(&query).unwrap().points.len(), 2);
    writer
        .conn
        .execute("UPDATE activity_events SET active=0", [])
        .unwrap();
    let changed = reader.metric_history(&query).unwrap();
    assert!(!changed.events.is_empty());
    assert!(changed.events.iter().all(|event| !event.active));
    same_as_new_reader(&reader, &path, &query);
    writer.clear_samples().unwrap();
    let empty = reader.metric_history(&query).unwrap();
    assert!(empty.points.is_empty());
    assert!(empty.events.is_empty());
    assert!(empty.process_names.is_empty());
    // Row IDs and row count can repeat after deletion. data_version must still detect it.
    writer.append_sample(&sample(1_000, 300)).unwrap();
    assert_eq!(
        reader.metric_history(&query).unwrap().points[0].value,
        Some(300)
    );
    same_as_new_reader(&reader, &path, &query);
}

#[test]
fn another_connection_maintenance_replaces_cached_resolution() {
    let database = Database::new();
    let path = database.path();
    let reader = Store::open(&path).unwrap();
    let mut writer = Store::open(&path).unwrap();
    let now = 10 * SAMPLE_RETENTION_MS;
    let minute = (now - RAW_RETENTION_MS - 120_000) / 60_000 * 60_000;
    writer.append_sample(&sample(minute + 1_000, 100)).unwrap();
    writer.append_sample(&sample(minute + 2_000, 300)).unwrap();
    let query = query(Metric::CpuBusyBasisPoints, minute, minute + 60_000);
    assert_eq!(reader.metric_history(&query).unwrap().points.len(), 2);
    writer.maintain_samples(now).unwrap();
    let history = reader.metric_history(&query).unwrap();
    assert_eq!(history.points.len(), 1);
    assert_eq!(history.points[0].resolution_ms, 60_000);
    same_as_new_reader(&reader, &path, &query);
}

#[test]
fn cache_never_supplies_a_different_window_metric_or_entity() {
    let store = Store::open_in_memory().unwrap();
    insert(&store, &sample(1_000, 100), false);
    insert(&store, &sample(2_000, 200), false);
    let whole = query(Metric::CpuBusyBasisPoints, 0, 3_000);
    assert_eq!(store.metric_history(&whole).unwrap().points.len(), 2);
    let narrow = query(Metric::CpuBusyBasisPoints, 1_500, 3_000);
    let history = store.metric_history(&narrow).unwrap();
    assert_eq!(history.points.len(), 1);
    assert_eq!(history.points[0].value, Some(200));
    let other = query(Metric::HardFaultPagesPerSecondX100, 0, 3_000);
    assert_eq!(
        store.metric_history(&other).unwrap().points[0].value,
        Some(606)
    );
    let mut process = query(Metric::ProcessCpuBasisPoints, 0, 3_000);
    process.entity_ref = "proc-fixture".into();
    assert_eq!(
        store.metric_history(&process).unwrap().points[0].value,
        Some(777)
    );
    process.metric = Metric::CpuBusyBasisPoints;
    assert!(store
        .metric_history(&process)
        .unwrap()
        .points
        .iter()
        .all(|point| point.value.is_none()));
    assert_eq!(store.metric_history(&whole).unwrap().points.len(), 2);
}

#[test]
fn legacy_compact_and_mixed_rows_have_identical_public_history() {
    let stores = [
        Store::open_in_memory().unwrap(),
        Store::open_in_memory().unwrap(),
        Store::open_in_memory().unwrap(),
    ];
    for (index, at) in [1_000, 2_000, 30_000, 31_000, 32_000]
        .into_iter()
        .enumerate()
    {
        let mut sample = sample(at, 100 + index as u64);
        if index == 1 {
            let gpu = sample
                .values
                .iter_mut()
                .find(|value| value.metric == Metric::GpuBusiestEngineBasisPoints)
                .unwrap();
            gpu.value = None;
            gpu.min = None;
            gpu.max = None;
            gpu.measured = 0;
            gpu.status = Status::Unsupported;
            gpu.weight_ms = 0;
            gpu.weighted_sum = 0;
        }
        if index >= 3 {
            sample.session_id = "session-two".into();
        }
        if index == 4 {
            sample.origin = Origin::Live;
        }
        for (kind, store) in stores.iter().enumerate() {
            insert(store, &sample, kind == 0 || (kind == 2 && index % 2 == 0));
        }
    }
    for metric in SYSTEM_CHARTS
        .into_iter()
        .chain([Metric::HardFaultPagesPerSecondX100])
    {
        let query = query(metric, 0, 34_000);
        let expected = stores[0].metric_history(&query).unwrap();
        for store in &stores[1..] {
            assert_eq!(
                json(&expected),
                json(&store.metric_history(&query).unwrap())
            );
        }
        assert!(expected.gaps.iter().any(|gap| gap.reason == "not_recorded"));
        assert!(expected
            .gaps
            .iter()
            .any(|gap| gap.reason == "collector_restarted"));
        assert!(expected
            .gaps
            .iter()
            .any(|gap| gap.reason == "origin_changed"));
        assert!(expected.process_names.is_empty());
    }
    let mut query = query(Metric::ProcessCpuBasisPoints, 0, 34_000);
    query.entity_ref = "proc-fixture".into();
    let expected = json(&stores[0].metric_history(&query).unwrap());
    for store in &stores[1..] {
        assert_eq!(expected, json(&store.metric_history(&query).unwrap()));
    }
}

#[test]
fn streaming_reduction_retains_large_exact_weights_extrema_and_observation_windows() {
    let store = Store::open_in_memory().unwrap();
    let low = u64::MAX - 5;
    let high = u64::MAX - 1;
    for index in 0..=MAX_POINTS {
        let at = 5_000 + index as u64 * 1_000;
        let mut sample = sample(at, 100);
        sample.values = vec![value(
            Metric::PhysicalAvailableBytes,
            "system",
            at,
            if index % 2 == 0 { low } else { high },
            if index % 2 == 0 { 5_000 } else { 1_000 },
        )];
        insert(&store, &sample, index % 2 == 0);
    }
    let query = query(
        Metric::PhysicalAvailableBytes,
        4_000,
        6_000 + MAX_POINTS as u64 * 1_000,
    );
    let history = store.metric_history(&query).unwrap();
    assert_eq!(history.points.len(), 1_001);
    let first = &history.points[0];
    let sum = u128::from(low) * 5_000 + u128::from(high) * 1_000;
    assert!(sum > u128::from(u64::MAX));
    assert_eq!(first.weighted_sum, sum);
    assert_eq!(first.weight_ms, 6_000);
    assert_eq!(first.value, Some((sum / 6_000) as u64));
    assert_eq!(first.min, Some(low));
    assert_eq!(first.max, Some(high));
    assert_eq!(first.measured_samples, 2);
    assert_eq!(first.expected_samples, 2);
    assert_eq!(first.observed_from_unix_ms, Some(0));
    assert_eq!(first.observed_to_unix_ms, Some(6_000));
    assert_eq!(first.observed_duration_ms, 6_000);
    assert_eq!(history.points.last().unwrap().measured_samples, 1);
}

#[test]
fn streaming_limit_keeps_disconnected_segments_separate_and_reports_omission() {
    let store = Store::open_in_memory().unwrap();
    for index in 0..MAX_POINTS + 5 {
        let mut sample = sample(1_000 + index as u64 * 1_000, index as u64);
        sample.session_id = format!("session-{index}");
        insert(&store, &sample, false);
    }
    let history = store
        .metric_history(&query(Metric::CpuBusyBasisPoints, 0, 3_000_000))
        .unwrap();
    assert_eq!(history.points.len(), MAX_POINTS);
    assert_eq!(history.points[0].value, Some(5));
    assert_eq!(history.points[0].measured_samples, 1);
    assert_eq!(history.status, Status::Partial);
    assert!(history
        .gaps
        .iter()
        .any(|gap| gap.reason == "response_limit" && gap.to_unix_ms == 6_000));
    assert!(history
        .limitations
        .iter()
        .any(|limitation| limitation.contains("disconnected segments")));
}

#[test]
fn an_overlapping_minute_is_kept_but_earlier_raw_samples_are_not() {
    let store = Store::open_in_memory().unwrap();
    let mut minute = sample(0, 100);
    minute.resolution_ms = 60_000;
    insert(&store, &minute, true);
    insert(&store, &sample(25_000, 200), false);
    insert(&store, &sample(40_000, 300), false);
    let history = store
        .metric_history(&query(Metric::CpuBusyBasisPoints, 30_000, 50_000))
        .unwrap();
    assert_eq!(
        history
            .points
            .iter()
            .map(|point| point.unix_ms)
            .collect::<Vec<_>>(),
        [0, 40_000]
    );
    assert_eq!(
        history
            .points
            .iter()
            .map(|point| point.value)
            .collect::<Vec<_>>(),
        [Some(100), Some(300)]
    );
}
