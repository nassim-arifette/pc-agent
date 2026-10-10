//! Offline, deterministic history benchmark. No collector, credentials or network are used.
//!
//! cargo run --release --locked -p pcagent-store --example history_benchmark -- \
//!   --db <new-fixture.db> --duration 10m --output <result.json> --max-bytes-per-row 16384
//! Durations: 10m (default), 1h, 24h, 7d. Older days are already minute aggregates,
//! avoiding a multi-gigabyte intermediate raw fixture. --reuse reads only a marked fixture.
//! --baseline compares semantic checksums; optional --min-speedup checks timing improvement.
#![forbid(unsafe_code)]

use pcagent_shared::{Evidence, Metric, Origin, ProcessInstance, Snapshot, Status, Window};
use pcagent_store::{HistoryQuery, HistorySeries, Store, StoredSample};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const END: u64 = 1_800_000_000_000;
const DAY: u64 = 86_400_000;
const CADENCE: u64 = 5_000;
const REPEATS: usize = 5;
const GRAPHS: [Metric; 5] = [
    Metric::CpuBusyBasisPoints,
    Metric::PhysicalAvailableBytes,
    Metric::PhysicalTotalBytes,
    Metric::DiskIdleBasisPoints,
    Metric::GpuBusiestEngineBasisPoints,
];
const SYSTEM: [Metric; 12] = [
    Metric::PhysicalTotalBytes,
    Metric::PhysicalAvailableBytes,
    Metric::SystemCommitBytes,
    Metric::SystemCommitLimitBytes,
    Metric::CpuBusyBasisPoints,
    Metric::CpuBusiestCoreBasisPoints,
    Metric::DiskReadLatencyMicroseconds,
    Metric::DiskWriteLatencyMicroseconds,
    Metric::DiskIdleBasisPoints,
    Metric::DiskQueueLength,
    Metric::HardFaultPagesPerSecondX100,
    Metric::GpuBusiestEngineBasisPoints,
];
const PROCESS: [Metric; 6] = [
    Metric::ProcessWorkingSetBytes,
    Metric::ProcessPrivateCommitBytes,
    Metric::ProcessCpuBasisPoints,
    Metric::ProcessIoReadBytesPerSecond,
    Metric::ProcessIoWriteBytesPerSecond,
    Metric::ProcessGpuBasisPoints,
];

struct Args {
    db: PathBuf,
    output: Option<PathBuf>,
    baseline: Option<PathBuf>,
    duration: u64,
    reuse: bool,
    max_bytes_per_row: Option<f64>,
    max_median_ms: Option<f64>,
    min_speedup: Option<f64>,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut result = Self {
            db: std::env::temp_dir().join(format!(
                "pcagent-history-benchmark-{}.db",
                std::process::id()
            )),
            output: None,
            baseline: None,
            duration: 600_000,
            reuse: false,
            max_bytes_per_row: None,
            max_median_ms: None,
            min_speedup: None,
        };
        while let Some(arg) = args.next() {
            if arg == "--reuse" {
                result.reuse = true;
                continue;
            }
            let value = args.next().ok_or("Expected a value after the option")?;
            match arg.as_str() {
                "--db" => result.db = value.into(),
                "--output" => result.output = Some(value.into()),
                "--baseline" => result.baseline = Some(value.into()),
                "--duration" => {
                    result.duration = match value.as_str() {
                        "10m" => 600_000,
                        "1h" => 3_600_000,
                        "24h" => DAY,
                        "7d" => 7 * DAY,
                        _ => return Err("Duration must be 10m, 1h, 24h or 7d".into()),
                    }
                }
                "--max-bytes-per-row" => result.max_bytes_per_row = Some(value.parse()?),
                "--max-median-ms" => result.max_median_ms = Some(value.parse()?),
                "--min-speedup" => result.min_speedup = Some(value.parse()?),
                _ => return Err(format!("Unknown option: {arg}").into()),
            }
        }
        for threshold in [
            result.max_bytes_per_row,
            result.max_median_ms,
            result.min_speedup,
        ]
        .into_iter()
        .flatten()
        {
            if !threshold.is_finite() || threshold <= 0.0 {
                return Err("Thresholds must be finite and positive".into());
            }
        }
        if result.min_speedup.is_some() && result.baseline.is_none() {
            return Err("--min-speedup requires --baseline".into());
        }
        Ok(result)
    }
}

fn gap(duration: u64) -> (u64, u64) {
    let start = (END - duration + duration / 3) / 60_000 * 60_000;
    (start, start + (duration / 20).min(180_000))
}

fn rows(duration: u64) -> Vec<(u64, u64)> {
    let mut result = Vec::new();
    let (gap_start, gap_end) = gap(duration);
    let mut at = END - duration;
    while at < END {
        let resolution = if at < END - DAY { 60_000 } else { CADENCE };
        if at < gap_start || at >= gap_end {
            result.push((at, resolution));
        }
        at += resolution;
    }
    result
}

fn gauge(metric: Metric) -> bool {
    matches!(
        metric,
        Metric::PhysicalTotalBytes
            | Metric::PhysicalAvailableBytes
            | Metric::SystemCommitBytes
            | Metric::SystemCommitLimitBytes
            | Metric::DiskQueueLength
            | Metric::ProcessWorkingSetBytes
            | Metric::ProcessPrivateCommitBytes
    )
}

// Each minute's twelve deviations sum to zero. Pre-aggregated rows therefore have an
// exact integer weighted mean and retain the same extrema as their underlying 5 s data.
fn reading(metric: Metric, process: u64, at: u64, resolution: u64) -> (Option<u64>, u64, u64) {
    let minute = at / 60_000;
    let wave = minute % 31;
    let mib = 1_048_576;
    let (base, step) = match metric {
        Metric::PhysicalTotalBytes => (16 * 1024 * mib, 0),
        Metric::PhysicalAvailableBytes => ((3 * 1024 + wave * 16) * mib, mib),
        Metric::SystemCommitBytes => ((7 * 1024 + wave * 8) * mib, mib),
        Metric::SystemCommitLimitBytes => (32 * 1024 * mib, 0),
        Metric::CpuBusyBasisPoints => (
            if minute.is_multiple_of(13) {
                9300
            } else {
                1800 + wave * 100
            },
            20,
        ),
        Metric::CpuBusiestCoreBasisPoints => (5000 + wave * 100, 10),
        Metric::DiskReadLatencyMicroseconds => (1200 + wave * 30, 10),
        Metric::DiskWriteLatencyMicroseconds => (1800 + wave * 40, 10),
        Metric::DiskIdleBasisPoints => (7500 + wave * 30, 10),
        Metric::DiskQueueLength => (wave % 3, 0),
        Metric::HardFaultPagesPerSecondX100 => (100 + wave * 20, 1),
        Metric::GpuBusiestEngineBasisPoints => (2000 + wave * 90, 10),
        Metric::ProcessWorkingSetBytes => ((50 + process * 19 + wave) * mib, 4096),
        Metric::ProcessPrivateCommitBytes => ((80 + process * 23 + wave) * mib, 4096),
        Metric::ProcessCpuBasisPoints => (10 + process * 3 + wave, 1),
        Metric::ProcessIoReadBytesPerSecond => (4096 + process * 1024 + wave * 128, 16),
        Metric::ProcessIoWriteBytesPerSecond => (2048 + process * 512 + wave * 64, 16),
        Metric::ProcessGpuBasisPoints => (20 + process * 7 + wave, 1),
        _ => unreachable!("Only fixture metrics are generated"),
    };
    if metric == Metric::GpuBusiestEngineBasisPoints && minute.is_multiple_of(17) {
        return (None, 0, 0);
    }
    if resolution == 60_000 {
        (Some(base), base - 5 * step, base + 5 * step)
    } else {
        let deviation = [-5_i64, -4, -3, -2, -1, 0, 0, 1, 2, 3, 4, 5][(at / CADENCE % 12) as usize];
        let value = (base as i64 + deviation * step as i64) as u64;
        (Some(value), value, value)
    }
}

fn sample(at: u64, resolution: u64, duration: u64) -> StoredSample {
    let session = u64::from(at >= (END - duration / 2) / 60_000 * 60_000);
    let mut snapshot = Snapshot::empty(&format!("fixture-session-{session}"));
    snapshot.id = format!("fixture-sample-{at}");
    snapshot.origin = Origin::SyntheticFixture;
    let last = at + resolution - CADENCE;
    for (process, metrics) in std::iter::once((None, SYSTEM.as_slice()))
        .chain((0..20).map(|i| (Some(i), PROCESS.as_slice())))
    {
        let entity = process.map_or_else(|| "system".into(), |i| format!("proc-{i:08}"));
        if let Some(i) = process {
            snapshot.processes.push(ProcessInstance {
                entity_ref: entity.clone(),
                pid: 10_000 + i as u32,
                creation_time_filetime: 1,
                boot_id: None,
                executable_name: Some(format!("fixture-s{session}-{i:02}.exe")),
            });
        }
        for &metric in metrics {
            let (value, _, _) = reading(metric, process.unwrap_or(0), at, resolution);
            snapshot.evidence.push(Evidence {
                id: format!("fixture-evidence-{}", snapshot.evidence.len()),
                metric,
                entity_ref: entity.clone(),
                value,
                unit: metric.unit().into(),
                source: "synthetic-history-benchmark-v1".into(),
                window: Window {
                    started_at_unix_ms: if gauge(metric) { at } else { at - CADENCE },
                    finished_at_unix_ms: last,
                    duration_ms: if gauge(metric) { 0 } else { resolution },
                },
                status: if value.is_some() {
                    Status::Ok
                } else {
                    Status::Unsupported
                },
                origin: Origin::SyntheticFixture,
                limitations: vec![],
            });
        }
    }
    let mut sample = StoredSample::from_snapshot(&snapshot, at, resolution);
    sample.interval_ms = CADENCE;
    for value in &mut sample.values {
        let process = value
            .entity_ref
            .strip_prefix("proc-")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let (number, min, max) = reading(value.metric, process, at, resolution);
        value.min = number.map(|_| min);
        value.max = number.map(|_| max);
        value.measured = if number.is_some() {
            resolution / CADENCE
        } else {
            0
        };
        value.expected = resolution / CADENCE;
    }
    sample
}

fn canonical(series: &HistorySeries) -> Result<Value> {
    let aliases: BTreeMap<_, _> = series
        .process_names
        .iter()
        .map(|(key, name)| (key.clone(), name.clone()))
        .collect();
    fn normalize(value: Value, aliases: &BTreeMap<String, String>) -> Value {
        match value {
            Value::String(s) => Value::String(aliases.get(&s).cloned().unwrap_or(s)),
            Value::Array(items) => {
                Value::Array(items.into_iter().map(|v| normalize(v, aliases)).collect())
            }
            Value::Object(items) => Value::Object(
                items
                    .into_iter()
                    .map(|(k, v)| (aliases.get(&k).cloned().unwrap_or(k), normalize(v, aliases)))
                    .collect(),
            ),
            other => other,
        }
    }
    let mut value = normalize(serde_json::to_value(series)?, &aliases);
    if let Some(events) = value["events"].as_array_mut() {
        for event in events.iter_mut() {
            event
                .as_object_mut()
                .expect("serialized event")
                .remove("id");
        }
        events.sort_by_cached_key(Value::to_string);
    }
    Ok(value)
}

fn checksum(value: &Value) -> String {
    // FNV-1a is a stable regression fingerprint, not a cryptographic integrity check.
    let hash = value
        .to_string()
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}

fn validate(series: &HistorySeries, duration: u64) -> Result<()> {
    if series.points.is_empty()
        || series.points.len() > 2000
        || series.origin != Some(Origin::SyntheticFixture)
    {
        return Err("Invalid history point bound or origin".into());
    }
    let mut expected_min = None;
    let mut expected_max = None;
    let mut expected_measured = 0;
    let mut expected_count = 0;
    for (at, resolution) in rows(duration) {
        if at < series.from_unix_ms || at > series.to_unix_ms {
            continue;
        }
        let (value, min, max) = reading(series.metric, 0, at, resolution);
        expected_count += resolution / CADENCE;
        if value.is_some() {
            expected_min = Some(expected_min.map_or(min, |old: u64| old.min(min)));
            expected_max = Some(expected_max.map_or(max, |old: u64| old.max(max)));
            expected_measured += resolution / CADENCE;
        }
    }
    if series.points.iter().filter_map(|p| p.min).min() != expected_min
        || series.points.iter().filter_map(|p| p.max).max() != expected_max
        || series
            .points
            .iter()
            .map(|p| p.measured_samples)
            .sum::<u64>()
            != expected_measured
        || series
            .points
            .iter()
            .map(|p| p.expected_samples)
            .sum::<u64>()
            != expected_count
    {
        return Err("History lost extrema or measured/expected sample counts".into());
    }
    for (index, point) in series.points.iter().enumerate() {
        if point.unix_ms < series.from_unix_ms
            || point.unix_ms > series.to_unix_ms
            || index > 0 && point.unix_ms < series.points[index - 1].unix_ms
            || point.measured_samples > point.expected_samples
            || point.value.is_some_and(|v| {
                point.min.is_none_or(|min| min > v) || point.max.is_none_or(|max| max < v)
            })
            || point.value.is_none()
                && (point.min.is_some() || point.max.is_some() || point.measured_samples != 0)
            || point
                .observed_from_unix_ms
                .zip(point.observed_to_unix_ms)
                .is_some_and(|(from, to)| from > to)
        {
            return Err(
                "History point bounds, missing values or observation windows changed".into(),
            );
        }
    }
    let (gap_start, gap_end) = gap(duration);
    if gap_start > series.from_unix_ms
        && gap_end < series.to_unix_ms
        && !series.gaps.iter().any(|g| {
            g.reason == "not_recorded" && g.from_unix_ms <= gap_start && g.to_unix_ms >= gap_end
        })
    {
        return Err("Intentional collection gap was lost".into());
    }
    let restart = (END - duration / 2) / 60_000 * 60_000;
    if restart > series.from_unix_ms
        && restart < series.to_unix_ms
        && !series
            .gaps
            .iter()
            .any(|g| g.reason == "collector_restarted" && g.to_unix_ms == restart)
    {
        return Err("Collector restart was lost".into());
    }
    Ok(())
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// A tiny precision probe is separate from the realistic performance fixture. Its deliberately
/// large values make a u128 weighted numerator exceed u64, catching lossy JSON/REAL projections.
fn precision_probe() -> Result<Value> {
    let mut store = Store::open_in_memory()?;
    let start = END - DAY - 180_000;
    let values: Vec<u64> = (0..24).map(|i| u64::MAX - 4096 + i * i % 101).collect();
    for (index, &value) in values.iter().enumerate() {
        let at = start + index as u64 * CADENCE;
        let mut snapshot = Snapshot::empty("fixture-u128");
        snapshot.id = format!("fixture-u128-{index}");
        snapshot.origin = Origin::SyntheticFixture;
        snapshot.evidence.push(Evidence {
            id: format!("fixture-u128-value-{index}"),
            metric: Metric::SystemCommitLimitBytes,
            entity_ref: "system".into(),
            value: Some(value),
            unit: "bytes".into(),
            source: "synthetic-u128-precision-probe".into(),
            window: Window {
                started_at_unix_ms: at,
                finished_at_unix_ms: at,
                duration_ms: 0,
            },
            status: Status::Ok,
            origin: Origin::SyntheticFixture,
            limitations: vec![],
        });
        store.append_sample(&StoredSample::from_snapshot(&snapshot, at, CADENCE))?;
    }
    let query = HistoryQuery {
        metric: Metric::SystemCommitLimitBytes,
        entity_ref: "system".into(),
        from_unix_ms: start,
        to_unix_ms: start + 24 * CADENCE,
    };
    let mut raw = store.metric_history(&query)?;
    raw.limit_points(1);
    store.maintain_samples(END)?;
    let mut aggregated = store.metric_history(&query)?;
    aggregated.limit_points(1);
    let mean = (values.iter().map(|&v| u128::from(v)).sum::<u128>() / 24) as u64;
    for series in [&raw, &aggregated] {
        if series.points.len() != 1 {
            return Err("Precision probe did not reduce to one connected point".into());
        }
        let point = &series.points[0];
        if point.value != Some(mean)
            || point.min != values.iter().copied().min()
            || point.max != values.iter().copied().max()
            || point.measured_samples != 24
            || point.expected_samples != 24
            || point.observed_duration_ms != 0
            || point.observed_from_unix_ms != Some(start)
            || point.observed_to_unix_ms != Some(start + 23 * CADENCE)
        {
            return Err(
                "u128 precision, extrema, counts or real observation windows were lost".into(),
            );
        }
    }
    Ok(
        json!({"weighted_numerator_exceeds_u64":true,"raw_and_minute_means_equal":true,
        "mean":mean,"samples":24,"observed_duration_ms":0}),
    )
}

fn benchmark(path: &Path, duration: u64, window: u64) -> Result<Value> {
    let mut fingerprints = Vec::new();
    let mut details = Vec::new();
    let mut times = [const { Vec::new() }; 5];
    let mut warm_times = [const { Vec::new() }; 5];
    let mut batches = Vec::new();
    let mut warm_batches = Vec::new();
    for repeat in 0..REPEATS {
        // Cold means an empty Store cache, not an OS disk-cache flush. Opening/migration and
        // validation are outside the timer. The UI's five metrics share the same Store.
        let store = Store::open(path)?;
        for warm in [false, true] {
            let mut total_ms = 0.0;
            for (index, metric) in GRAPHS.iter().copied().enumerate() {
                let start = Instant::now();
                let series = store.metric_history(&HistoryQuery {
                    metric,
                    entity_ref: "system".into(),
                    from_unix_ms: END - window,
                    to_unix_ms: END,
                })?;
                let elapsed = start.elapsed().as_secs_f64() * 1000.0;
                total_ms += elapsed;
                if warm {
                    warm_times[index].push(elapsed);
                } else {
                    times[index].push(elapsed);
                }
                validate(&series, duration)?;
                let fingerprint = checksum(&canonical(&series)?);
                if repeat == 0 && !warm {
                    fingerprints.push(fingerprint);
                    details.push(json!({"metric": metric, "points": series.points.len(), "gaps": series.gaps.len(),
                    "min": series.points.iter().filter_map(|p|p.min).min(), "max": series.points.iter().filter_map(|p|p.max).max(),
                    "measured": series.points.iter().map(|p|p.measured_samples).sum::<u64>(),
                    "expected": series.points.iter().map(|p|p.expected_samples).sum::<u64>(),
                    "process_names": series.process_names.len(), "events": series.events.len()}));
                } else if fingerprints[index] != fingerprint {
                    return Err(
                        "Cold/warm/repeated history reads are not semantically identical".into(),
                    );
                }
            }
            if warm {
                warm_batches.push(total_ms);
            } else {
                batches.push(total_ms);
            }
        }
    }
    for (index, time) in times.into_iter().enumerate() {
        details[index]["median_ms"] = json!(median(time));
        details[index]["checksum"] = json!(fingerprints[index]);
    }
    for (index, time) in warm_times.into_iter().enumerate() {
        details[index]["warm_median_ms"] = json!(median(time));
    }
    Ok(
        json!({"window_ms": window, "five_queries_median_ms": median(batches.clone()),
        "five_queries_ms": batches, "warm_five_queries_median_ms":median(warm_batches.clone()),
        "warm_five_queries_ms":warm_batches,
        "cache_mode":"Fresh Store per repeat; five sequential UI metrics, then a second warm batch. OS disk cache is not flushed.",
        "metrics": details, "checksum": checksum(&json!(fingerprints))}),
    )
}

fn run(args: Args) -> Result<()> {
    let precision = precision_probe()?;
    let marker = args.db.with_extension("fixture.json");
    let fixture = json!({"fixture_version":1,"duration_ms":args.duration,"end_unix_ms":END,"values_per_sample":132});
    let mut write_ms = None;
    if args.reuse {
        if serde_json::from_slice::<Value>(&fs::read(&marker)?)? != fixture {
            return Err("Reuse requires the matching synthetic fixture marker".into());
        }
    } else {
        if args.db.exists() || marker.exists() {
            return Err(
                "Refusing to overwrite an existing fixture; use --reuse or a new path".into(),
            );
        }
        let start = Instant::now();
        let mut store = Store::open(&args.db)?;
        for (at, resolution) in rows(args.duration) {
            if !store.append_sample(&sample(at, resolution, args.duration))? {
                return Err("Duplicate fixture sample".into());
            }
        }
        drop(store); // SQLite checkpoints the owned WAL before file size is measured.
        write_ms = Some(start.elapsed().as_secs_f64() * 1000.0);
        fs::write(&marker, serde_json::to_vec_pretty(&fixture)?)?;
    }
    let mut windows = vec![args.duration.min(3_600_000)];
    if args.duration >= DAY {
        windows.push(DAY);
    }
    if args.duration >= 7 * DAY {
        windows.push(7 * DAY);
    }
    let results = windows
        .into_iter()
        .map(|window| benchmark(&args.db, args.duration, window))
        .collect::<Result<Vec<_>>>()?;
    let connection =
        Connection::open_with_flags(&args.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (count, payload_bytes): (i64, i64) = connection.query_row(
        "SELECT COUNT(*),COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM samples",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    drop(connection);
    let count = u64::try_from(count)?;
    let payload_bytes = u64::try_from(payload_bytes)?;
    if count != rows(args.duration).len() as u64 {
        return Err("Unexpected fixture row count".into());
    }
    let db_bytes = fs::metadata(&args.db)?.len();
    let bytes_per_row = db_bytes as f64 / count as f64;
    let mut failures = Vec::new();
    if args
        .max_bytes_per_row
        .is_some_and(|limit| bytes_per_row > limit)
    {
        failures.push("database_bytes_per_row exceeds threshold".to_owned());
    }
    let mut comparisons = Vec::new();
    let baseline: Option<Value> = args
        .baseline
        .as_ref()
        .map(fs::read)
        .transpose()?
        .map(|bytes| serde_json::from_slice(&bytes))
        .transpose()?;
    if let Some(baseline) = &baseline {
        if baseline["fixture"] != fixture {
            return Err("Baseline uses a different fixture".into());
        }
    }
    for result in &results {
        let elapsed = result["five_queries_median_ms"]
            .as_f64()
            .expect("benchmark time");
        if args.max_median_ms.is_some_and(|limit| elapsed > limit) {
            failures.push(format!(
                "five-query latency exceeds threshold for {} ms",
                result["window_ms"]
            ));
        }
        if let Some(baseline) = &baseline {
            let old = baseline["windows"]
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .find(|item| item["window_ms"] == result["window_ms"])
                })
                .ok_or("Baseline window missing")?;
            if old["checksum"] != result["checksum"] {
                failures.push(format!(
                    "semantic checksum differs for {} ms",
                    result["window_ms"]
                ));
            }
            let speedup = old["five_queries_median_ms"]
                .as_f64()
                .ok_or("Baseline latency missing")?
                / elapsed;
            if args.min_speedup.is_some_and(|limit| speedup < limit) {
                failures.push(format!(
                    "speedup below threshold for {} ms",
                    result["window_ms"]
                ));
            }
            comparisons.push(json!({"window_ms":result["window_ms"],"speedup":speedup,"same_checksum":old["checksum"] == result["checksum"]}));
        }
    }
    let report = json!({"fixture":fixture,"precision_probe":precision,"repeats":REPEATS,"rows":count,"db_bytes":db_bytes,
        "thresholds":{"max_bytes_per_row":args.max_bytes_per_row,"max_median_ms":args.max_median_ms,"min_speedup":args.min_speedup},
        "database_bytes_per_row":bytes_per_row,"payload_bytes":payload_bytes,"payload_bytes_per_row":payload_bytes as f64/count as f64,
        "write_ms":write_ms,"windows":results,"comparisons":comparisons,
        "checksum_scope":"Full serialized HistorySeries; random event IDs removed and process aliases replaced with deterministic synthetic names.",
        "validation":"Bounds, ordering, gaps, session restart, origin, min/max and measured/expected counts checked against generated data on every read.",
        "ok":failures.is_empty(),"failures":failures});
    if let Some(path) = &args.output {
        if path == &args.db || path == &marker || args.baseline.as_ref() == Some(path) {
            return Err("Output must not overwrite the fixture or baseline".into());
        }
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(&report)?)?;
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !failures.is_empty() {
        return Err("History benchmark thresholds or parity failed (see JSON failures)".into());
    }
    Ok(())
}

fn main() -> Result<()> {
    run(Args::parse()?)
}
