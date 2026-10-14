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
        let uncached = store.read_history(&query, &[*metric]).unwrap().remove(0);
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
        assert_eq!(
            expected
                .process_names
                .get("proc-fixture")
                .map(String::as_str),
            Some("fixture.exe")
        );
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
