//! Persistent, bounded measurement history. Raw samples expire into minute aggregates after a
//! day; extrema and coverage are retained. A missing sample never becomes a zero reading.
mod codec;
mod collection;
mod history;
mod overview;
pub use collection::{HistoryCollection, DEFAULT_COLLECTION_BUDGET_BYTES};
pub use history::{ChartPoint, ChartSeries, HistoryChart};
pub use overview::{HistoryOverview, HistoryPeriod, HistoryRecordings, RecordingDetail};
pub(crate) mod system;
use super::{stored, Store, StoreError};
pub(super) use history::HistoryCache;
use pcagent_shared::{
    new_id, new_short_ref, ratio_basis_points, Metric, Origin, Snapshot, Status, Window,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SAMPLE_RETENTION_MS: u64 = 7 * 24 * 3_600_000;
const RAW_RETENTION_MS: u64 = 24 * 3_600_000;
const MAX_POINTS: usize = 2000;
const MAX_QUERY_ROWS: usize = 100_000;

pub(super) fn decode_sample(text: &str) -> Result<StoredSample, StoreError> {
    codec::decode(text)
}
pub(super) const MIGRATION: &str = "BEGIN;
CREATE TABLE samples (
 id INTEGER PRIMARY KEY, sample_key TEXT NOT NULL UNIQUE, session_id TEXT NOT NULL,
 measured_at INTEGER NOT NULL, resolution_ms INTEGER NOT NULL, payload TEXT NOT NULL
);
CREATE INDEX samples_by_time ON samples(measured_at);
CREATE TABLE sample_entities (
 session_id TEXT NOT NULL, local_ref TEXT NOT NULL, entity_ref TEXT NOT NULL UNIQUE,
 last_seen INTEGER NOT NULL, PRIMARY KEY(session_id,local_ref)
);
CREATE INDEX sample_entities_by_time ON sample_entities(last_seen);
CREATE TABLE activity_events (
 id TEXT PRIMARY KEY, kind TEXT NOT NULL, entity_ref TEXT, session_id TEXT NOT NULL,
 started_at INTEGER NOT NULL, ended_at INTEGER NOT NULL, peak INTEGER NOT NULL,
 metric TEXT NOT NULL, unit TEXT NOT NULL, active INTEGER NOT NULL,
 contributor_ref TEXT, contributor_value INTEGER, contributor_metric TEXT, contributor_unit TEXT,
 origin TEXT NOT NULL
);
CREATE INDEX events_by_time ON activity_events(ended_at);
CREATE TABLE recordings (
 id TEXT PRIMARY KEY, started_at INTEGER NOT NULL, ended_at INTEGER,
 duration_seconds INTEGER NOT NULL, status TEXT NOT NULL, samples INTEGER NOT NULL DEFAULT 0,
 markers TEXT NOT NULL DEFAULT '[]'
);
CREATE UNIQUE INDEX recordings_one_active ON recordings(status) WHERE status='recording';
PRAGMA user_version = 2;
COMMIT;";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SampleValue {
    pub metric: Metric,
    pub entity_ref: String,
    pub value: Option<u64>,
    pub min: Option<u64>,
    pub max: Option<u64>,
    pub measured: u64,
    pub expected: u64,
    pub status: Status,
    pub source: String,
    #[serde(default)]
    pub observed_from_unix_ms: Option<u64>,
    #[serde(default)]
    pub observed_to_unix_ms: Option<u64>,
    /// Sum of the actual monotonic observation windows; instantaneous gauges have zero.
    #[serde(default)]
    pub observed_duration_ms: u64,
    /// Preserve the unrounded weighted numerator across repeated minute/query aggregation.
    #[serde(default)]
    weighted_sum: u128,
    #[serde(default)]
    weight_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSample {
    pub id: String,
    pub session_id: String,
    pub measured_at_unix_ms: u64,
    pub interval_ms: u64,
    pub resolution_ms: u64,
    pub origin: Origin,
    pub values: Vec<SampleValue>,
    /// Local basenames, projected by the agent's sharing boundary before cloud use.
    pub process_names: BTreeMap<String, String>,
}
impl StoredSample {
    pub fn from_snapshot(snapshot: &Snapshot, at: u64, interval_ms: u64) -> Self {
        let keep = |m: Metric| {
            matches!(
                m,
                Metric::ProcessWorkingSetBytes
                    | Metric::ProcessPrivateCommitBytes
                    | Metric::ProcessCpuBasisPoints
                    | Metric::ProcessIoReadBytes
                    | Metric::ProcessIoWriteBytes
                    | Metric::ProcessIoOtherBytes
                    | Metric::ProcessGpuBasisPoints
                    | Metric::ProcessIoReadBytesPerSecond
                    | Metric::ProcessIoWriteBytesPerSecond
                    | Metric::ProcessIoOtherBytesPerSecond
                    | Metric::GpuTemperatureCelsius
            )
        };
        let values = snapshot
            .evidence
            .iter()
            .filter(|e| e.entity_ref == "system" || keep(e.metric))
            .map(|e| {
                let metric = match e.metric {
                    Metric::ProcessIoReadBytes => Metric::ProcessIoReadBytesPerSecond,
                    Metric::ProcessIoWriteBytes => Metric::ProcessIoWriteBytesPerSecond,
                    Metric::ProcessIoOtherBytes => Metric::ProcessIoOtherBytesPerSecond,
                    metric => metric,
                };
                let value = e.usable_value().and_then(|value| {
                    if metric == e.metric {
                        Some(value)
                    } else if e.window.duration_ms == 0 {
                        None
                    } else {
                        u64::try_from(u128::from(value) * 1000 / u128::from(e.window.duration_ms))
                            .ok()
                    }
                });
                // Rates use their actual measured window; instantaneous gauges use the nominal
                // sampling interval only as averaging weight, never as an observation window.
                let weight_ms = value.map_or(0, |_| {
                    if e.window.duration_ms == 0 {
                        interval_ms
                    } else {
                        e.window.duration_ms
                    }
                });
                SampleValue {
                    metric,
                    entity_ref: e.entity_ref.clone(),
                    value,
                    min: value,
                    max: value,
                    measured: u64::from(value.is_some()),
                    expected: 1,
                    status: if value.is_none() && e.status == Status::Ok {
                        Status::Partial
                    } else {
                        e.status
                    },
                    source: e.source.clone(),
                    observed_from_unix_ms: Some(e.window.started_at_unix_ms),
                    observed_to_unix_ms: Some(e.window.finished_at_unix_ms),
                    observed_duration_ms: e.window.duration_ms,
                    weighted_sum: u128::from(value.unwrap_or(0)) * u128::from(weight_ms),
                    weight_ms,
                }
            })
            .collect();
        let process_names = snapshot
            .processes
            .iter()
            .take(20)
            .filter_map(|p| {
                p.executable_name
                    .as_ref()
                    .map(|name| (p.entity_ref.clone(), name.clone()))
            })
            .collect();
        Self {
            id: snapshot.id.clone(),
            session_id: snapshot.session_id.clone(),
            measured_at_unix_ms: at,
            interval_ms,
            resolution_ms: interval_ms,
            origin: snapshot.origin,
            values,
            process_names,
        }
    }
}

#[derive(Debug, Clone)]
pub struct HistoryQuery {
    pub metric: Metric,
    pub entity_ref: String,
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPoint {
    pub unix_ms: u64,
    pub value: Option<u64>,
    pub min: Option<u64>,
    pub max: Option<u64>,
    pub status: Status,
    pub measured_samples: u64,
    pub expected_samples: u64,
    pub resolution_ms: u64,
    pub observed_from_unix_ms: Option<u64>,
    pub observed_to_unix_ms: Option<u64>,
    pub observed_duration_ms: u64,
    pub origin: Origin,
    #[serde(skip)]
    session_id: String,
    #[serde(skip)]
    weighted_sum: u128,
    #[serde(skip)]
    weight_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryGap {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityEvent {
    pub id: String,
    pub kind: String,
    pub entity_ref: Option<String>,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: u64,
    pub peak: u64,
    pub metric: String,
    pub unit: String,
    pub active: bool,
    /// A separately measured contributor at the system peak, never the system peak itself.
    pub contributor_ref: Option<String>,
    pub contributor_value: Option<u64>,
    pub contributor_metric: Option<String>,
    pub contributor_unit: Option<String>,
    pub origin: Origin,
    /// Metadata saved with the peak itself, never reconstructed from episode boundaries.
    /// Older database rows retain None rather than a fabricated measurement window or source.
    #[serde(default)]
    pub peak_observation: Option<EventObservation>,
    #[serde(default)]
    pub contributor_observation: Option<EventObservation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventObservation {
    pub entity_ref: String,
    pub metric: String,
    pub value: u64,
    pub unit: String,
    pub source: String,
    pub status: Status,
    pub origin: Origin,
    pub measured_at_unix_ms: Option<u64>,
    /// None when the source did not retain its window or derived inputs have different windows.
    pub window: Option<Window>,
    pub derivation: Option<String>,
    /// Original measured operands retain their individual source and window.
    pub inputs: Vec<EventObservation>,
}

impl EventObservation {
    pub(crate) fn measured(value: &SampleValue, origin: Origin) -> Option<Self> {
        let measured = value.value?;
        let window = value
            .observed_from_unix_ms
            .zip(value.observed_to_unix_ms)
            .map(|(started_at_unix_ms, finished_at_unix_ms)| Window {
                started_at_unix_ms,
                finished_at_unix_ms,
                duration_ms: value.observed_duration_ms,
            });
        Some(Self {
            entity_ref: value.entity_ref.clone(),
            metric: metric_key(value.metric),
            value: measured,
            unit: value.metric.unit().into(),
            source: value.source.clone(),
            status: if value.status == Status::Ok && (window.is_none() || value.source.is_empty()) {
                Status::Partial
            } else {
                value.status
            },
            origin,
            measured_at_unix_ms: window.as_ref().map(|w| w.finished_at_unix_ms),
            window,
            derivation: None,
            inputs: vec![],
        })
    }

    pub(crate) fn derived(metric: &str, value: u64, derivation: &str, inputs: Vec<Self>) -> Self {
        let first = &inputs[0];
        let window = first.window.clone().filter(|window| {
            inputs.iter().all(|input| {
                input.window.as_ref().is_some_and(|other| {
                    window.started_at_unix_ms == other.started_at_unix_ms
                        && window.finished_at_unix_ms == other.finished_at_unix_ms
                        && window.duration_ms == other.duration_ms
                })
            })
        });
        let mut sources: Vec<_> = inputs.iter().map(|input| input.source.clone()).collect();
        sources.sort();
        sources.dedup();
        Self {
            entity_ref: "system".into(),
            metric: metric.into(),
            value,
            unit: "basis_points".into(),
            source: sources.join("; "),
            status: if window.is_some() && inputs.iter().all(|input| input.status == Status::Ok) {
                Status::Ok
            } else {
                Status::Partial
            },
            origin: first.origin,
            measured_at_unix_ms: window.as_ref().map(|w| w.finished_at_unix_ms),
            window,
            derivation: Some(derivation.into()),
            inputs,
        }
    }
}

pub(crate) fn system_observations(sample: &StoredSample) -> [Option<EventObservation>; 3] {
    let measured = |metric| {
        sample
            .values
            .iter()
            .find(|v| v.metric == metric && v.entity_ref == "system" && v.measured > 0)
            .and_then(|v| EventObservation::measured(v, sample.origin))
    };
    let cpu = measured(Metric::CpuBusyBasisPoints);
    let memory = measured(Metric::PhysicalAvailableBytes)
        .zip(measured(Metric::PhysicalTotalBytes))
        .and_then(|(free, total)| {
            let used = total.value.checked_sub(free.value)?;
            let value = ratio_basis_points(used, total.value)?;
            Some(EventObservation::derived(
                "physical_used_basis_points", value,
                "10000 * (physical_total_bytes - physical_available_bytes) / physical_total_bytes; integer division",
                vec![total, free],
            ))
        });
    let disk_busy = measured(Metric::DiskIdleBasisPoints).and_then(|idle| {
        let value = 10_000_u64.checked_sub(idle.value)?;
        Some(EventObservation::derived(
            "disk_busy_basis_points",
            value,
            "10000 - disk_idle_basis_points",
            vec![idle],
        ))
    });
    [cpu, memory, disk_busy]
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recording {
    pub id: String,
    pub started_at_unix_ms: u64,
    pub ended_at_unix_ms: Option<u64>,
    pub duration_seconds: u64,
    pub status: String,
    pub samples: u64,
    pub markers: Vec<Marker>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marker {
    pub at_unix_ms: u64,
    pub kind: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistorySeries {
    pub metric: Metric,
    pub entity_ref: String,
    pub unit: String,
    pub status: Status,
    /// None for no observations or mixed origins; individual points retain their own origin.
    pub origin: Option<Origin>,
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub points: Vec<HistoryPoint>,
    pub gaps: Vec<HistoryGap>,
    pub events: Vec<ActivityEvent>,
    pub process_names: BTreeMap<String, String>,
    pub sources: Vec<String>,
    pub limitations: Vec<String>,
}

impl HistorySeries {
    /// Bound a projection without rounding means again or joining disconnected observations.
    /// The persistent history is unchanged; callers can request a narrower window for detail.
    pub fn limit_points(&mut self, max_points: usize) {
        let max_points = max_points.max(1);
        if self.points.len() <= max_points {
            return;
        }
        let width = self.points.len().div_ceil(max_points);
        let discontinuities: std::collections::BTreeSet<_> =
            self.gaps.iter().map(|gap| gap.to_unix_ms).collect();
        let mut reduced: Vec<HistoryPoint> = Vec::new();
        let mut chunk_len = 0;
        let mut previous_at = 0;
        let mut previous_resolution = 0;
        for point in std::mem::take(&mut self.points) {
            let compatible = reduced.last().is_some_and(|last| {
                last.origin == point.origin
                    && last.session_id == point.session_id
                    && last.status == point.status
                    && last.value.is_some() == point.value.is_some()
                    && point.unix_ms >= previous_at
                    && point.unix_ms.saturating_sub(previous_at)
                        <= point
                            .resolution_ms
                            .max(previous_resolution)
                            .saturating_mul(2)
                    && discontinuities
                        .range((
                            std::ops::Bound::Excluded(previous_at),
                            std::ops::Bound::Included(point.unix_ms),
                        ))
                        .next()
                        .is_none()
            });
            previous_at = point.unix_ms;
            previous_resolution = point.resolution_ms;
            if chunk_len < width && compatible {
                let last = reduced.last_mut().expect("compatible history point");
                let missing_status = last.status;
                merge_points(last, &point);
                if last.value.is_none() {
                    last.status = missing_status;
                }
                chunk_len += 1;
            } else {
                reduced.push(point);
                chunk_len = 1;
            }
        }
        self.points = reduced;
        if self.points.len() > max_points {
            let omitted = self.points.len() - max_points;
            self.points.drain(..omitted);
            self.gaps.push(HistoryGap {
                from_unix_ms: self.from_unix_ms,
                to_unix_ms: self.points[0].unix_ms,
                reason: "response_limit".into(),
            });
            self.status = Status::Partial;
            self.limitations.push(format!(
                "Cloud projection omitted {omitted} older disconnected segments; only the latest {max_points} remain. Narrow the requested window for their measurements."
            ));
        }
        self.limitations.push(format!(
            "Response limited to {max_points} points using weighted means, measured minima/maxima and sample counts. Collection gaps, sessions, origins and coverage changes are not merged."
        ));
    }
}

fn metric_key(metric: Metric) -> String {
    serde_json::to_value(metric)
        .expect("metric serializes")
        .as_str()
        .unwrap()
        .into()
}
fn merge_values(target: &mut SampleValue, next: &SampleValue) {
    let count = target.measured.saturating_add(next.measured);
    let (a, wa) = mean_parts(target);
    let (b, wb) = mean_parts(next);
    target.weight_ms = wa.saturating_add(wb);
    target.weighted_sum = a.saturating_add(b);
    target.value =
        (target.weight_ms > 0).then(|| (target.weighted_sum / u128::from(target.weight_ms)) as u64);
    target.min = target.min.into_iter().chain(next.min).min();
    target.max = target.max.into_iter().chain(next.max).max();
    target.measured = count;
    target.expected = target.expected.saturating_add(next.expected);
    target.status = if count == target.expected {
        Status::Ok
    } else {
        Status::Partial
    };
    target.observed_from_unix_ms = target
        .observed_from_unix_ms
        .into_iter()
        .chain(next.observed_from_unix_ms)
        .min();
    target.observed_to_unix_ms = target
        .observed_to_unix_ms
        .into_iter()
        .chain(next.observed_to_unix_ms)
        .max();
    target.observed_duration_ms = target
        .observed_duration_ms
        .saturating_add(next.observed_duration_ms);
}

fn mean_parts(value: &SampleValue) -> (u128, u64) {
    if value.weight_ms > 0 {
        (value.weighted_sum, value.weight_ms)
    } else {
        (
            u128::from(value.value.unwrap_or(0)) * u128::from(value.measured),
            value.measured,
        )
    }
}

/// Count scheduled positions across the whole minute, including leading/trailing silence.
/// The next sample's interval describes cadence changes (e.g. 5 s background to 1 s recording).
fn expected_in_minute(samples: &[StoredSample], minute: u64) -> u64 {
    let first = &samples[0];
    let last = samples.last().expect("nonempty minute");
    let mut expected = samples.len() as u64;
    expected = expected.saturating_add(
        first.measured_at_unix_ms.saturating_sub(minute) / first.interval_ms.max(1),
    );
    for pair in samples.windows(2) {
        let interval = pair[1].interval_ms.max(1);
        let elapsed = pair[1]
            .measured_at_unix_ms
            .saturating_sub(pair[0].measured_at_unix_ms);
        let positions = elapsed.saturating_add(interval / 2) / interval;
        expected = expected.saturating_add(positions.saturating_sub(1));
    }
    expected.saturating_add(
        minute
            .saturating_add(59_999)
            .saturating_sub(last.measured_at_unix_ms)
            / last.interval_ms.max(1),
    )
}

fn merge_points(target: &mut HistoryPoint, next: &HistoryPoint) {
    target.weighted_sum = target.weighted_sum.saturating_add(next.weighted_sum);
    target.weight_ms = target.weight_ms.saturating_add(next.weight_ms);
    target.value =
        (target.weight_ms > 0).then(|| (target.weighted_sum / u128::from(target.weight_ms)) as u64);
    target.min = target.min.into_iter().chain(next.min).min();
    target.max = target.max.into_iter().chain(next.max).max();
    target.measured_samples = target
        .measured_samples
        .saturating_add(next.measured_samples);
    target.expected_samples = target
        .expected_samples
        .saturating_add(next.expected_samples);
    target.status = if target.status == Status::Ok
        && next.status == Status::Ok
        && target.measured_samples == target.expected_samples
    {
        Status::Ok
    } else {
        Status::Partial
    };
    target.resolution_ms = next
        .unix_ms
        .saturating_sub(target.unix_ms)
        .saturating_add(next.resolution_ms);
    target.observed_from_unix_ms = target
        .observed_from_unix_ms
        .into_iter()
        .chain(next.observed_from_unix_ms)
        .min();
    target.observed_to_unix_ms = target
        .observed_to_unix_ms
        .into_iter()
        .chain(next.observed_to_unix_ms)
        .max();
    target.observed_duration_ms = target
        .observed_duration_ms
        .saturating_add(next.observed_duration_ms);
}

fn origin_key(origin: Origin) -> &'static str {
    match origin {
        Origin::Live => "live",
        Origin::SyntheticFixture => "synthetic_fixture",
    }
}

fn event_observation_column(
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<Option<EventObservation>> {
    row.get::<_, Option<String>>(column)?
        .map(|text| {
            serde_json::from_str(&text).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    column,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })
        })
        .transpose()
}

impl Store {
    pub fn append_sample(&mut self, sample: &StoredSample) -> Result<bool, StoreError> {
        if !(100..=60_000).contains(&sample.interval_ms)
            || sample.resolution_ms < sample.interval_ms
            || sample.resolution_ms > 60_000
            || sample.measured_at_unix_ms > i64::MAX as u64
            || sample.values.len() > 512
            || sample.process_names.len() > 20
            || sample.id.len() > 128
            || sample.session_id.len() > 128
            || sample
                .values
                .iter()
                .any(|value| value.entity_ref.len() > 128 || value.source.len() > 512)
            || sample.process_names.values().any(|name| name.len() > 1024)
            || sample
                .process_names
                .keys()
                .any(|reference| reference.len() > 128)
        {
            return Err(StoreError::InvalidQuery);
        }
        let tx = self.conn.transaction()?;
        let mut stored_sample = sample.clone();
        // Collector refs are unique only in that collector. Persist separately issued aliases
        // without disclosing the collector UUID or allowing two sessions to share a history key.
        let local_refs: std::collections::BTreeSet<_> = sample
            .values
            .iter()
            .map(|value| value.entity_ref.as_str())
            .chain(sample.process_names.keys().map(String::as_str))
            .filter(|reference| reference.starts_with("proc-"))
            .collect();
        let mut aliases = BTreeMap::new();
        for local_ref in local_refs {
            let existing: Option<String> = tx
                .query_row(
                    "SELECT entity_ref FROM sample_entities WHERE session_id=?1 AND local_ref=?2",
                    params![sample.session_id, local_ref],
                    |row| row.get(0),
                )
                .optional()?;
            let alias = if let Some(alias) = existing {
                tx.execute(
                    "UPDATE sample_entities SET last_seen=MAX(last_seen,?3) WHERE session_id=?1 AND local_ref=?2",
                    params![sample.session_id, local_ref, stored(sample.measured_at_unix_ms)],
                )?;
                alias
            } else {
                loop {
                    let alias = new_short_ref("proc");
                    let inserted = tx.execute(
                        "INSERT OR IGNORE INTO sample_entities(session_id,local_ref,entity_ref,last_seen) VALUES(?1,?2,?3,?4)",
                        params![sample.session_id,local_ref,alias,stored(sample.measured_at_unix_ms)],
                    )?;
                    if inserted != 0 {
                        break alias;
                    }
                }
            };
            aliases.insert(local_ref, alias);
        }
        for value in &mut stored_sample.values {
            if let Some(alias) = aliases.get(value.entity_ref.as_str()) {
                value.entity_ref = alias.clone();
            }
        }
        stored_sample.process_names = sample
            .process_names
            .iter()
            .map(|(reference, name)| {
                (
                    aliases
                        .get(reference.as_str())
                        .cloned()
                        .unwrap_or_else(|| reference.clone()),
                    name.clone(),
                )
            })
            .collect();
        let text = codec::encode(&stored_sample)?;
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO samples(sample_key,session_id,measured_at,resolution_ms,payload) VALUES(?1,?2,?3,?4,?5)",
            params![sample.id, sample.session_id, stored(sample.measured_at_unix_ms), stored(sample.resolution_ms), text])? > 0;
        if inserted {
            system::insert(&tx, &stored_sample)?;
            Self::update_events(&tx, &stored_sample)?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Minute buckets never merge process identities from different collector sessions.
    pub fn maintain_samples(&mut self, now: u64) -> Result<usize, StoreError> {
        let cutoff = now.saturating_sub(SAMPLE_RETENTION_MS);
        let removed = self.conn.execute(
            &format!(
                "DELETE FROM samples WHERE id IN (SELECT s.id FROM samples s
             WHERE s.measured_at < ?1 AND {})",
                collection::UNPROTECTED
            ),
            params![stored(cutoff)],
        )?;
        self.conn.execute(
            "DELETE FROM activity_events WHERE ended_at < ?1 AND NOT EXISTS
             (SELECT 1 FROM recordings r WHERE activity_events.ended_at>=r.started_at
              AND (r.ended_at IS NULL OR activity_events.started_at<=r.ended_at))",
            params![stored(cutoff)],
        )?;
        self.conn.execute(
            "DELETE FROM sample_entities WHERE last_seen < ?1",
            params![stored(cutoff)],
        )?;
        self.conn.execute(
            "DELETE FROM history_names WHERE last_seen < ?1 AND entity_ref NOT IN
             (SELECT contributor_ref FROM activity_events WHERE contributor_ref IS NOT NULL)",
            params![stored(cutoff)],
        )?;
        let under_pressure = self.collection_bytes()? > self.collection_budget()?;
        let aggregate_before = if under_pressure {
            now
        } else {
            now.saturating_sub(RAW_RETENTION_MS)
        } / 60_000
            * 60_000;
        let groups: Vec<(String, i64)> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT s.session_id,s.measured_at/60000 AS bucket
             FROM samples s WHERE s.resolution_ms < 60000 AND s.measured_at < ?1
             AND NOT EXISTS (SELECT 1 FROM recordings r
              WHERE (s.measured_at/60000+1)*60000>=r.started_at
              AND (r.ended_at IS NULL OR s.measured_at/60000*60000<=r.ended_at))
             GROUP BY s.session_id,bucket ORDER BY bucket LIMIT {}",
                if under_pressure { i64::MAX } else { 120 }
            ))?;
            let rows = stmt.query_map(params![stored(aggregate_before)], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        for (session, minute) in groups {
            let samples: Vec<StoredSample> = {
                let mut stmt = self.conn.prepare("SELECT payload FROM samples WHERE session_id=?1 AND measured_at>=?2 AND measured_at<?3 ORDER BY measured_at")?;
                let rows = stmt.query_map(
                    params![session, minute * 60_000, (minute + 1) * 60_000],
                    |r| r.get::<_, String>(0),
                )?;
                rows.map(|row| codec::decode(&row?))
                    .collect::<Result<_, _>>()?
            };
            if samples.is_empty() {
                continue;
            }
            // A synthetic import can share a session spelling with live collection; never
            // blend their values or silently relabel synthetic observations as live.
            let mut origins: BTreeMap<u8, Vec<StoredSample>> = BTreeMap::new();
            for sample in samples {
                origins
                    .entry(if sample.origin == Origin::Live { 0 } else { 1 })
                    .or_default()
                    .push(sample);
            }
            let mut merged_rows = Vec::new();
            for (origin, mut samples) in origins {
                let expected = expected_in_minute(&samples, minute as u64 * 60_000);
                let mut merged = samples.remove(0);
                let mut values: BTreeMap<(String, String), SampleValue> = merged
                    .values
                    .drain(..)
                    .map(|v| ((v.entity_ref.clone(), metric_key(v.metric)), v))
                    .collect();
                for sample in samples {
                    for value in sample.values {
                        values
                            .entry((value.entity_ref.clone(), metric_key(value.metric)))
                            .and_modify(|current| merge_values(current, &value))
                            .or_insert(value);
                    }
                    merged.process_names.extend(sample.process_names);
                }
                merged.id = format!("minute-{session}-{minute}-{origin}");
                merged.measured_at_unix_ms = minute as u64 * 60_000;
                merged.resolution_ms = 60_000;
                for value in values.values_mut() {
                    value.expected = value.expected.max(expected);
                    if value.measured != value.expected {
                        value.status = Status::Partial;
                    }
                }
                merged.values = values.into_values().collect();
                let text = codec::encode(&merged)?;
                merged_rows.push((merged, text));
            }
            let tx = self.conn.transaction()?;
            tx.execute(
                "DELETE FROM samples WHERE session_id=?1 AND measured_at>=?2 AND measured_at<?3",
                params![session, minute * 60_000, (minute + 1) * 60_000],
            )?;
            for (sample, text) in merged_rows {
                tx.execute("INSERT INTO samples(sample_key,session_id,measured_at,resolution_ms,payload) VALUES(?1,?2,?3,60000,?4)", params![sample.id,session,minute*60_000,text])?;
                system::insert(&tx, &sample)?;
            }
            tx.commit()?;
        }
        Ok(removed + self.enforce_collection_budget(now)?)
    }

    fn update_events(conn: &rusqlite::Connection, sample: &StoredSample) -> Result<(), StoreError> {
        let system = |metric| {
            sample
                .values
                .iter()
                .find(|v| v.metric == metric && v.entity_ref == "system")
                .filter(|v| v.value.is_some() && v.measured > 0)
        };
        let [cpu, memory, disk_busy] = system_observations(sample);
        let disk = system(Metric::DiskReadLatencyMicroseconds)
            .into_iter()
            .chain(system(Metric::DiskWriteLatencyMicroseconds))
            .max_by_key(|value| value.value)
            .and_then(|v| EventObservation::measured(v, sample.origin))
            .map(|mut observation| {
                observation.derivation = Some(
                    "Highest available measured read/write latency in this sample; metric identifies the selected counter.".into(),
                );
                observation
            });
        let events = [
            (
                "cpu_busy",
                cpu,
                9000,
                "cpu_busy_basis_points",
                "basis_points_of_total_cpu",
                Metric::ProcessCpuBasisPoints,
            ),
            (
                "memory_pressure",
                memory,
                9000,
                "physical_used_basis_points",
                "basis_points",
                Metric::ProcessWorkingSetBytes,
            ),
            (
                "disk_latency",
                disk,
                50_000,
                "disk_latency_microseconds",
                "microseconds",
                Metric::ProcessIoReadBytesPerSecond,
            ),
            (
                "disk_busy",
                disk_busy,
                9500,
                "disk_busy_basis_points",
                "basis_points",
                Metric::ProcessIoReadBytesPerSecond,
            ),
        ];
        for (kind, peak_observation, threshold, metric, unit, contribution) in events {
            // Missing readings break an observed episode rather than extending it across an
            // unobserved period. Recovery closes the episode; it creates no new event.
            let Some(peak_observation) =
                peak_observation.filter(|reading| reading.value >= threshold)
            else {
                conn.execute(
                    "UPDATE activity_events SET active=0 WHERE kind=?1 AND origin=?2 AND active=1",
                    params![kind, origin_key(sample.origin)],
                )?;
                continue;
            };
            let value = peak_observation.value;
            // Attribution is only a measured contributor, never a causal accusation. Disk I/O
            // bytes do not attribute a system latency spike to one process.
            let contributor = if kind.starts_with("disk_") {
                None
            } else {
                sample
                    .values
                    .iter()
                    .filter(|v| {
                        v.metric == contribution
                            && v.value.is_some()
                            && v.status == Status::Ok
                            && v.entity_ref.starts_with("proc-")
                    })
                    .max_by_key(|v| v.value)
            };
            let contributor_ref = contributor.map(|v| v.entity_ref.as_str());
            let contributor_value = contributor.and_then(|v| v.value).map(stored);
            let contributor_metric = contributor.map(|v| metric_key(v.metric));
            let contributor_unit = contributor.map(|v| v.metric.unit());
            let contributor_observation = contributor
                .and_then(|v| EventObservation::measured(v, sample.origin))
                .map(|observation| serde_json::to_string(&observation))
                .transpose()
                .map_err(|_| StoreError::Corrupt)?;
            let peak_observation =
                serde_json::to_string(&peak_observation).map_err(|_| StoreError::Corrupt)?;
            let recent:Option<String>=conn.query_row("SELECT id FROM activity_events WHERE kind=?1 AND session_id=?2 AND origin=?3 AND active=1 AND ended_at>=?4 AND ended_at<=?5 ORDER BY ended_at DESC LIMIT 1",params![kind,sample.session_id,origin_key(sample.origin),stored(sample.measured_at_unix_ms.saturating_sub(sample.interval_ms*2)),stored(sample.measured_at_unix_ms)],|r|r.get(0)).optional()?;
            if let Some(id) = recent {
                conn.execute("UPDATE activity_events SET ended_at=?2,peak=MAX(peak,?3),contributor_ref=CASE WHEN ?3>=peak THEN ?4 ELSE contributor_ref END,contributor_value=CASE WHEN ?3>=peak THEN ?5 ELSE contributor_value END,contributor_metric=CASE WHEN ?3>=peak THEN ?6 ELSE contributor_metric END,contributor_unit=CASE WHEN ?3>=peak THEN ?7 ELSE contributor_unit END,peak_observation=CASE WHEN ?3>=peak THEN ?8 ELSE peak_observation END,contributor_observation=CASE WHEN ?3>=peak THEN ?9 ELSE contributor_observation END WHERE id=?1",params![id,stored(sample.measured_at_unix_ms),stored(value),contributor_ref,contributor_value,contributor_metric,contributor_unit,peak_observation,contributor_observation])?;
            } else {
                conn.execute(
                    "UPDATE activity_events SET active=0 WHERE kind=?1 AND origin=?2 AND active=1",
                    params![kind, origin_key(sample.origin)],
                )?;
                conn.execute("INSERT INTO activity_events(id,kind,entity_ref,session_id,started_at,ended_at,peak,metric,unit,active,contributor_ref,contributor_value,contributor_metric,contributor_unit,origin,peak_observation,contributor_observation) VALUES(?1,?2,'system',?3,?4,?4,?5,?6,?7,1,?8,?9,?10,?11,?12,?13,?14)",params![new_id("event"),kind,sample.session_id,stored(sample.measured_at_unix_ms),stored(value),metric,unit,contributor_ref,contributor_value,contributor_metric,contributor_unit,origin_key(sample.origin),peak_observation,contributor_observation])?;
            }
        }
        Ok(())
    }

    pub fn activity_events(
        &self,
        from: u64,
        to: u64,
        limit: usize,
    ) -> Result<Vec<ActivityEvent>, StoreError> {
        if from > to || to.saturating_sub(from) > SAMPLE_RETENTION_MS {
            return Err(StoreError::InvalidQuery);
        }
        let mut stmt=self.conn.prepare("SELECT id,kind,entity_ref,started_at,ended_at,peak,metric,unit,active,contributor_ref,contributor_value,contributor_metric,contributor_unit,origin,peak_observation,contributor_observation FROM activity_events WHERE ended_at>=?1 AND started_at<=?2 ORDER BY started_at DESC LIMIT ?3")?;
        let rows = stmt.query_map(
            params![
                stored(from),
                stored(to),
                i64::try_from(limit).unwrap_or(i64::MAX)
            ],
            |r| {
                Ok(ActivityEvent {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    entity_ref: r.get(2)?,
                    started_at_unix_ms: r.get::<_, i64>(3)? as u64,
                    ended_at_unix_ms: r.get::<_, i64>(4)? as u64,
                    peak: r.get::<_, i64>(5)? as u64,
                    metric: r.get(6)?,
                    unit: r.get(7)?,
                    active: r.get(8)?,
                    contributor_ref: r.get(9)?,
                    contributor_value: r.get::<_, Option<i64>>(10)?.map(|v| v as u64),
                    contributor_metric: r.get(11)?,
                    contributor_unit: r.get(12)?,
                    origin: match r.get::<_, String>(13)?.as_str() {
                        "live" => Origin::Live,
                        "synthetic_fixture" => Origin::SyntheticFixture,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    },
                    peak_observation: event_observation_column(r, 14)?,
                    contributor_observation: event_observation_column(r, 15)?,
                })
            },
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn begin_recording(&self, seconds: u64, now: u64) -> Result<Recording, StoreError> {
        if !(30..=120).contains(&seconds) {
            return Err(StoreError::InvalidQuery);
        }
        let active: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM recordings WHERE status='recording')",
            [],
            |r| r.get(0),
        )?;
        if active {
            return Err(StoreError::Database(
                "A problem recording is already running".into(),
            ));
        }
        let id = new_id("recording");
        let markers = vec![Marker {
            at_unix_ms: now,
            kind: "start".into(),
        }];
        self.conn.execute("INSERT INTO recordings(id,started_at,duration_seconds,status,markers) VALUES(?1,?2,?3,'recording',?4)",params![id,stored(now),stored(seconds),serde_json::to_string(&markers).map_err(|_|StoreError::Corrupt)?])?;
        self.recording(&id)?.ok_or(StoreError::Corrupt)
    }
    pub fn finish_recording(
        &self,
        id: &str,
        now: u64,
        cancelled: bool,
        samples: u64,
    ) -> Result<(), StoreError> {
        let Some(mut recording) = self.recording(id)? else {
            return Err(StoreError::InvalidQuery);
        };
        recording.markers.push(Marker {
            at_unix_ms: now,
            kind: if cancelled { "cancelled" } else { "end" }.into(),
        });
        self.conn.execute("UPDATE recordings SET ended_at=?2,status=?3,samples=?4,markers=?5 WHERE id=?1 AND status='recording'",params![id,stored(now),if cancelled {"cancelled"} else {"completed"},stored(samples),serde_json::to_string(&recording.markers).map_err(|_|StoreError::Corrupt)?])?;
        Ok(())
    }
    /// Persist progress after each committed sample so a crash keeps its real count.
    pub fn recording_progress(&self, id: &str, samples: u64) -> Result<(), StoreError> {
        if id.len() > 128 || samples > i64::MAX as u64 {
            return Err(StoreError::InvalidQuery);
        }
        let changed = self.conn.execute(
            "UPDATE recordings SET samples=MAX(samples,?2) WHERE id=?1 AND status='recording'",
            params![id, samples as i64],
        )?;
        if changed == 0 {
            return Err(StoreError::InvalidQuery);
        }
        Ok(())
    }
    pub fn recording(&self, id: &str) -> Result<Option<Recording>, StoreError> {
        let row=self.conn.query_row("SELECT id,started_at,ended_at,duration_seconds,status,samples,markers FROM recordings WHERE id=?1",params![id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,Option<i64>>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,String>(6)?))).optional()?;
        row.map(|(id, start, end, duration, status, samples, markers)| {
            Ok(Recording {
                id,
                started_at_unix_ms: start as u64,
                ended_at_unix_ms: end.map(|n| n as u64),
                duration_seconds: duration as u64,
                status,
                samples: samples as u64,
                markers: serde_json::from_str(&markers).map_err(|_| StoreError::Corrupt)?,
            })
        })
        .transpose()
    }
    pub fn interrupt_old_recordings(&self, now: u64) -> Result<usize, StoreError> {
        // The unique active-recording index bounds this recovery to at most one row. Recheck its
        // state in the update in case another sampler finished or cleared it after the read.
        let cutoff = stored(now.saturating_sub(180_000));
        let id: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM recordings WHERE status='recording' AND started_at<?1 LIMIT 1",
                params![cutoff],
                |row| row.get(0),
            )
            .optional()?;
        let Some(id) = id else {
            return Ok(0);
        };
        let Some(mut recording) = self.recording(&id)? else {
            return Ok(0);
        };
        recording.markers.push(Marker {
            at_unix_ms: now,
            kind: "interrupted".into(),
        });
        let markers = serde_json::to_string(&recording.markers).map_err(|_| StoreError::Corrupt)?;
        Ok(self.conn.execute(
            "UPDATE recordings SET status='interrupted',ended_at=?2,markers=?4 WHERE id=?1 AND status='recording' AND started_at<?3",
            params![id, stored(now), cutoff, markers],
        )?)
    }
    pub fn clear_samples(&self) -> Result<usize, StoreError> {
        self.history_cache.borrow_mut().take();
        let tx = self.conn.unchecked_transaction()?;
        let removed = self.conn.execute("DELETE FROM samples", [])?;
        self.conn.execute("DELETE FROM sample_entities", [])?;
        self.conn.execute("DELETE FROM history_names", [])?;
        self.conn.execute("DELETE FROM activity_events", [])?;
        self.conn.execute("DELETE FROM recordings", [])?;
        self.clear_watch_alerts()?;
        tx.commit()?;
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pcagent_shared::{Evidence, Window};
    fn sample(at: u64, cpu: Option<u64>) -> StoredSample {
        let mut snapshot = Snapshot::empty("fixture");
        snapshot.origin = Origin::SyntheticFixture;
        let window = Window {
            started_at_unix_ms: at.saturating_sub(5000),
            finished_at_unix_ms: at,
            duration_ms: 5000,
        };
        snapshot.evidence.push(match cpu {
            Some(n) => {
                Evidence::measured(Metric::CpuBusyBasisPoints, "system", n, "fixture", window)
            }
            None => Evidence::missing(
                Metric::CpuBusyBasisPoints,
                "system",
                "fixture",
                Status::Unsupported,
                "not measured",
            ),
        });
        StoredSample::from_snapshot(&snapshot, at, 5000)
    }
    fn query(from: u64, to: u64) -> HistoryQuery {
        HistoryQuery {
            metric: Metric::CpuBusyBasisPoints,
            entity_ref: "system".into(),
            from_unix_ms: from,
            to_unix_ms: to,
        }
    }
    fn history_ref(store: &Store, session: &str, local_ref: &str) -> String {
        store
            .conn
            .query_row(
                "SELECT entity_ref FROM sample_entities WHERE session_id=?1 AND local_ref=?2",
                params![session, local_ref],
                |row| row.get(0),
            )
            .unwrap()
    }
    #[test]
    fn cloud_point_limit_keeps_exact_weights_extrema_and_windows() {
        let mut store = Store::open_in_memory().unwrap();
        for i in 0..240 {
            store
                .append_sample(&sample((i + 1) * 5000, Some(i)))
                .unwrap();
        }
        let mut history = store.metric_history(&query(0, 1_200_001)).unwrap();
        history.limit_points(120);
        assert_eq!(history.points.len(), 120);
        assert_eq!(history.points[0].value, Some(0));
        assert_eq!(history.points[0].min, Some(0));
        assert_eq!(history.points[0].max, Some(1));
        assert_eq!(history.points[0].measured_samples, 2);
        assert_eq!(history.points[0].expected_samples, 2);
        assert_eq!(history.points[0].observed_from_unix_ms, Some(0));
        assert_eq!(history.points[0].observed_to_unix_ms, Some(10_000));
        assert_eq!(history.points[0].observed_duration_ms, 10_000);
        // A second reduction must use the original numerator, not the rounded pair means.
        history.limit_points(1);
        assert_eq!(history.points[0].value, Some(119));
        assert_eq!(history.points[0].min, Some(0));
        assert_eq!(history.points[0].max, Some(239));
        assert_eq!(history.points[0].measured_samples, 240);
        assert_eq!(history.points[0].observed_duration_ms, 1_200_000);
    }

    #[test]
    fn cloud_point_limit_never_blends_sessions_origins_or_missing_values() {
        let mut store = Store::open_in_memory().unwrap();
        for i in 0..8 {
            let mut item = sample(
                (i + 1) * 5000,
                Some(if i < 4 { i * 2 } else { 100 + (i - 4) * 2 }),
            );
            if i >= 4 {
                item.session_id = "another-session".into();
                item.origin = Origin::Live;
            }
            store.append_sample(&item).unwrap();
        }
        let mut history = store.metric_history(&query(0, 40_001)).unwrap();
        history.limit_points(2);
        assert_eq!(
            history.points.iter().map(|p| p.value).collect::<Vec<_>>(),
            vec![Some(3), Some(103)]
        );
        assert_ne!(history.points[0].origin, history.points[1].origin);
        assert_eq!(history.points[0].max, Some(6));
        assert_eq!(history.points[1].min, Some(100));

        let mut store = Store::open_in_memory().unwrap();
        for (at, value) in [
            (5000, Some(100)),
            (10_000, None),
            (15_000, None),
            (20_000, Some(200)),
        ] {
            store.append_sample(&sample(at, value)).unwrap();
        }
        let mut history = store.metric_history(&query(0, 20_001)).unwrap();
        history.limit_points(3);
        assert_eq!(history.points.len(), 3);
        assert_eq!(history.points[1].value, None);
        assert_eq!(history.points[1].status, Status::Unsupported);
        assert_eq!(history.points[1].measured_samples, 0);
        assert_eq!(history.points[1].expected_samples, 2);
    }

    #[test]
    fn cloud_point_limit_reports_disconnected_segments_that_cannot_fit() {
        let mut store = Store::open_in_memory().unwrap();
        for i in 0..6 {
            store
                .append_sample(&sample((i + 1) * 100_000, Some(i)))
                .unwrap();
        }
        let mut history = store.metric_history(&query(0, 600_001)).unwrap();
        history.limit_points(2);
        assert_eq!(
            history.points.iter().map(|p| p.value).collect::<Vec<_>>(),
            vec![Some(4), Some(5)]
        );
        assert_eq!(history.status, Status::Partial);
        assert!(history
            .gaps
            .iter()
            .any(|gap| gap.reason == "response_limit" && gap.to_unix_ms == 500_000));
        assert!(history
            .limitations
            .iter()
            .any(|text| text.contains("omitted 4")));
        history.limit_points(0);
        assert_eq!(history.points.len(), 1);
        assert_eq!(history.points[0].value, Some(5));
    }

    #[test]
    fn persistent_history_retains_values_gaps_and_missing_readings() {
        let dir = std::env::temp_dir().join(new_id("pcagent-history"));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("history.sqlite");
        {
            let mut store = Store::open(&path).unwrap();
            store.append_sample(&sample(10_000, Some(1200))).unwrap();
            store.append_sample(&sample(15_000, None)).unwrap();
            store.append_sample(&sample(60_000, Some(2000))).unwrap();
        }
        let store = Store::open(&path).unwrap();
        let history = store.metric_history(&query(1, 70_000)).unwrap();
        assert_eq!(history.points.len(), 3);
        assert_eq!(history.points[1].value, None);
        assert!(!history.gaps.is_empty());
        assert_eq!(history.status, Status::Partial);
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn retention_aggregates_old_minutes_and_keeps_extrema_without_inventing_zero() {
        let mut store = Store::open_in_memory().unwrap();
        let now = 10 * SAMPLE_RETENTION_MS;
        let old = now - RAW_RETENTION_MS - 120_000;
        let minute = old / 60_000 * 60_000;
        store
            .append_sample(&sample(minute + 1000, Some(1000)))
            .unwrap();
        store
            .append_sample(&sample(minute + 6000, Some(9000)))
            .unwrap();
        store.append_sample(&sample(minute + 11000, None)).unwrap();
        store
            .append_sample(&sample(now - SAMPLE_RETENTION_MS - 1, Some(500)))
            .unwrap();
        assert_eq!(store.maintain_samples(now).unwrap(), 1);
        let history = store
            .metric_history(&query(minute, minute + 60_000))
            .unwrap();
        assert_eq!(history.points.len(), 1);
        let point = &history.points[0];
        assert_eq!(point.value, Some(5000));
        assert_eq!(point.min, Some(1000));
        assert_eq!(point.max, Some(9000));
        assert_eq!(point.measured_samples, 2);
        assert_eq!(point.expected_samples, 12);
        assert_eq!(point.status, Status::Partial);
    }
    #[test]
    fn anomalies_merge_consecutive_samples_and_recovery_is_not_an_event() {
        let mut store = Store::open_in_memory().unwrap();
        for (at, cpu) in [(1000, 9500), (6000, 9800), (11000, 1000)] {
            store.append_sample(&sample(at, Some(cpu))).unwrap();
        }
        let events = store.activity_events(0, 20000, 100).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].peak, 9800);
        assert!(!events[0].active);
        assert_eq!(events[0].started_at_unix_ms, 1000);
        assert_eq!(events[0].ended_at_unix_ms, 6000);
    }
    #[test]
    fn recordings_are_bounded_exclusive_and_end_with_explicit_markers() {
        let store = Store::open_in_memory().unwrap();
        assert!(store.begin_recording(29, 1000).is_err());
        let r = store.begin_recording(30, 1000).unwrap();
        assert!(store.begin_recording(30, 2000).is_err());
        store.finish_recording(&r.id, 11000, true, 10).unwrap();
        let r = store.recording(&r.id).unwrap().unwrap();
        assert_eq!(r.status, "cancelled");
        assert_eq!(r.markers[1].kind, "cancelled");
        assert_eq!(r.samples, 10);
    }

    #[test]
    fn recording_progress_is_persistent_monotonic_and_kept_after_interruption() {
        let store = Store::open_in_memory().unwrap();
        let recording = store.begin_recording(120, 1000).unwrap();
        store.recording_progress(&recording.id, 8).unwrap();
        assert_eq!(store.recording(&recording.id).unwrap().unwrap().samples, 8);
        store.recording_progress(&recording.id, 3).unwrap();
        assert_eq!(store.recording(&recording.id).unwrap().unwrap().samples, 8);
        assert_eq!(store.interrupt_old_recordings(200_000).unwrap(), 1);
        let interrupted = store.recording(&recording.id).unwrap().unwrap();
        assert_eq!(interrupted.samples, 8);
        assert_eq!(interrupted.status, "interrupted");
        assert!(store.recording_progress(&recording.id, 9).is_err());
        assert_eq!(store.recording(&recording.id).unwrap().unwrap().samples, 8);
        assert!(store.recording_progress("missing", 1).is_err());
    }

    #[test]
    fn restarted_recording_expires_after_its_grace_period_and_releases_the_slot() {
        let dir = std::env::temp_dir().join(new_id("pcagent-recording-restart"));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("history.sqlite");
        let old_id = {
            let store = Store::open(&path).unwrap();
            let recording = store.begin_recording(120, 1000).unwrap();
            store.recording_progress(&recording.id, 8).unwrap();
            recording.id
        };
        let store = Store::open(&path).unwrap();
        assert_eq!(store.interrupt_old_recordings(2000).unwrap(), 0);
        assert!(store.begin_recording(30, 2000).is_err());
        assert_eq!(store.interrupt_old_recordings(181_000).unwrap(), 0);
        assert_eq!(store.interrupt_old_recordings(181_001).unwrap(), 1);
        let previous = store.recording(&old_id).unwrap().unwrap();
        assert_eq!(previous.status, "interrupted");
        assert_eq!(previous.samples, 8);
        assert_eq!(previous.ended_at_unix_ms, Some(181_001));
        assert_eq!(previous.markers.len(), 2);
        assert_eq!(previous.markers[1].kind, "interrupted");
        assert_eq!(previous.markers[1].at_unix_ms, 181_001);
        let next = store.begin_recording(30, 181_001).unwrap();
        assert_ne!(next.id, old_id);
        assert_eq!(next.status, "recording");
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn io_deltas_use_the_observed_window_not_the_requested_recording_cadence() {
        let mut snapshot = Snapshot::empty("fixture");
        snapshot.origin = Origin::SyntheticFixture;
        let window = Window {
            started_at_unix_ms: 1000,
            finished_at_unix_ms: 6000,
            duration_ms: 5000,
        };
        snapshot.evidence.push(Evidence::measured(
            Metric::ProcessIoReadBytes,
            "proc-fixture",
            10_000,
            "fixture",
            window,
        ));
        let sample = StoredSample::from_snapshot(&snapshot, 6100, 1000);
        let value = &sample.values[0];
        assert_eq!(value.metric, Metric::ProcessIoReadBytesPerSecond);
        assert_eq!(value.value, Some(2000));
        assert_eq!(value.observed_duration_ms, 5000);
        assert_eq!(value.observed_from_unix_ms, Some(1000));
        assert_eq!(value.observed_to_unix_ms, Some(6000));
        assert_eq!(sample.interval_ms, 1000);
        snapshot.evidence[0].window.duration_ms = 0;
        let zero_window = StoredSample::from_snapshot(&snapshot, 6100, 1000);
        assert_eq!(zero_window.values[0].value, None);
        assert_eq!(zero_window.values[0].status, Status::Partial);
    }

    #[test]
    fn minute_means_keep_the_unrounded_numerator_and_actual_window_weights() {
        let mut store = Store::open_in_memory().unwrap();
        let now = 10 * SAMPLE_RETENTION_MS;
        let minute = (now - RAW_RETENTION_MS - 120_000) / 60_000 * 60_000;
        for (offset, value) in [(1000, 0), (6000, 1), (11000, 2)] {
            store
                .append_sample(&sample(minute + offset, Some(value)))
                .unwrap();
        }
        store.maintain_samples(now).unwrap();
        let series = store
            .metric_history(&query(minute, minute + 60_000))
            .unwrap();
        assert_eq!(series.points[0].value, Some(1));
        assert_eq!(series.origin, Some(Origin::SyntheticFixture));
        assert_eq!(series.points[0].observed_duration_ms, 15_000);
        let mut a = sample(10_000, Some(0)).values.remove(0);
        let mut b = sample(11_000, Some(6000)).values.remove(0);
        b.weight_ms = 1000;
        b.weighted_sum = 6_000_000;
        b.observed_duration_ms = 1000;
        merge_values(&mut a, &b);
        assert_eq!(a.value, Some(1000));
    }

    #[test]
    fn complete_minute_is_ok_but_unretained_processes_remain_partial() {
        let mut store = Store::open_in_memory().unwrap();
        let now = 10 * SAMPLE_RETENTION_MS;
        let minute = (now - RAW_RETENTION_MS - 120_000) / 60_000 * 60_000;
        for i in 0..12 {
            let mut s = sample(minute + 1000 + i * 5000, Some(3000));
            if i == 0 {
                let mut p = s.values[0].clone();
                p.entity_ref = "proc-fixture".into();
                p.metric = Metric::ProcessCpuBasisPoints;
                s.values.push(p);
            }
            store.append_sample(&s).unwrap();
        }
        store.maintain_samples(now).unwrap();
        let series = store
            .metric_history(&query(minute, minute + 60_000))
            .unwrap();
        assert_eq!(series.points[0].status, Status::Ok);
        assert_eq!(series.points[0].expected_samples, 12);
        let process = store
            .metric_history(&HistoryQuery {
                metric: Metric::ProcessCpuBasisPoints,
                entity_ref: history_ref(&store, "fixture", "proc-fixture"),
                from_unix_ms: minute,
                to_unix_ms: minute + 60_000,
            })
            .unwrap();
        assert_eq!(process.points[0].measured_samples, 1);
        assert_eq!(process.points[0].expected_samples, 12);
        assert_eq!(process.points[0].status, Status::Partial);
        let overlap = store
            .metric_history(&query(minute + 30_000, minute + 50_000))
            .unwrap();
        assert_eq!(overlap.points.len(), 1);
        assert_eq!(overlap.points[0].unix_ms, minute);
    }

    #[test]
    fn minute_buckets_never_blend_sessions_or_live_and_fixture_origins() {
        let mut store = Store::open_in_memory().unwrap();
        let now = 10 * SAMPLE_RETENTION_MS;
        let minute = (now - RAW_RETENTION_MS - 120_000) / 60_000 * 60_000;
        let a = sample(minute + 1000, Some(1000));
        let mut b = sample(minute + 2000, Some(9000));
        b.session_id = "different-session".into();
        let mut c = sample(minute + 3000, Some(5000));
        c.origin = Origin::Live;
        for s in [a, b, c] {
            store.append_sample(&s).unwrap();
        }
        store.maintain_samples(now).unwrap();
        let series = store
            .metric_history(&query(minute, minute + 60_000))
            .unwrap();
        let mut values: Vec<_> = series.points.iter().filter_map(|p| p.value).collect();
        values.sort_unstable();
        assert_eq!(values, vec![1000, 5000, 9000]);
        assert_eq!(series.origin, None);
        assert!(series
            .gaps
            .iter()
            .any(|gap| gap.reason == "origin_changed" || gap.reason == "collector_restarted"));
    }

    #[test]
    fn event_system_peak_and_its_observed_process_contributor_are_distinct() {
        let mut store = Store::open_in_memory().unwrap();
        for (at, system, process) in [(1000, 9500, 1200), (6000, 9800, 800), (11000, 9600, 9000)] {
            let mut s = sample(at, Some(system));
            let mut p = s.values[0].clone();
            p.metric = Metric::ProcessCpuBasisPoints;
            p.entity_ref = "proc-example".into();
            p.value = Some(process);
            p.min = Some(process);
            p.max = Some(process);
            s.values.push(p);
            store.append_sample(&s).unwrap();
        }
        let events = store.activity_events(0, 20_000, 100).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.entity_ref.as_deref(), Some("system"));
        assert_eq!(event.peak, 9800);
        assert_eq!(
            event.contributor_ref.as_deref(),
            Some(history_ref(&store, "fixture", "proc-example").as_str())
        );
        assert_eq!(event.contributor_value, Some(800));
        assert_eq!(
            event.contributor_metric.as_deref(),
            Some("process_cpu_basis_points")
        );
        assert_eq!(event.origin, Origin::SyntheticFixture);
        store.append_sample(&sample(16_000, None)).unwrap();
        assert!(!store.activity_events(0, 20_000, 100).unwrap()[0].active);
        store.append_sample(&sample(21_000, Some(9500))).unwrap();
        assert_eq!(store.activity_events(0, 25_000, 100).unwrap().len(), 2);
    }

    #[test]
    fn event_c4_metadata_follows_the_peak_not_the_episode_end() {
        let mut store = Store::open_in_memory().unwrap();
        for (at, system, process) in [(1000, 9500, 1200), (6000, 9800, 800), (11000, 9600, 9000)] {
            let mut item = sample(at, Some(system));
            item.values[0].source = format!("fixture::GetSystemTimes-{at}");
            let mut contributor = item.values[0].clone();
            contributor.metric = Metric::ProcessCpuBasisPoints;
            contributor.entity_ref = "proc-aabbccdd".into();
            contributor.value = Some(process);
            contributor.min = Some(process);
            contributor.max = Some(process);
            contributor.source = format!("fixture::NtQuerySystemInformation-{at}");
            contributor.observed_from_unix_ms = Some(at - 100);
            contributor.observed_to_unix_ms = Some(at - 10);
            contributor.observed_duration_ms = 90;
            item.values.push(contributor);
            store.append_sample(&item).unwrap();
        }
        let events = store.activity_events(0, 20_000, 100).unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.peak, 9800);
        assert_eq!(event.ended_at_unix_ms, 11_000);
        let peak = event.peak_observation.as_ref().unwrap();
        assert_eq!(peak.value, 9800);
        assert_eq!(peak.status, Status::Ok);
        assert_eq!(peak.origin, Origin::SyntheticFixture);
        assert_eq!(peak.source, "fixture::GetSystemTimes-6000");
        assert_eq!(peak.measured_at_unix_ms, Some(6000));
        let window = peak.window.as_ref().unwrap();
        assert_eq!(window.started_at_unix_ms, 1000);
        assert_eq!(window.finished_at_unix_ms, 6000);
        assert_eq!(window.duration_ms, 5000);
        let contributor = event.contributor_observation.as_ref().unwrap();
        assert_eq!(contributor.value, 800);
        assert_eq!(contributor.source, "fixture::NtQuerySystemInformation-6000");
        assert_eq!(contributor.origin, Origin::SyntheticFixture);
        assert_eq!(contributor.status, Status::Ok);
        assert_eq!(contributor.measured_at_unix_ms, Some(5990));
        let window = contributor.window.as_ref().unwrap();
        assert_eq!(window.started_at_unix_ms, 5900);
        assert_eq!(window.finished_at_unix_ms, 5990);
        assert_eq!(window.duration_ms, 90);
        assert_eq!(
            event.contributor_ref.as_ref(),
            Some(&contributor.entity_ref)
        );
    }

    #[test]
    fn derived_events_preserve_operands_and_do_not_invent_a_common_window() {
        let mut store = Store::open_in_memory().unwrap();
        let mut item = sample(1000, Some(1000));
        for (metric, value, duration, source) in [
            (Metric::PhysicalTotalBytes, 1000, 0, "GlobalMemoryStatusEx"),
            (
                Metric::PhysicalAvailableBytes,
                50,
                0,
                "GlobalMemoryStatusEx",
            ),
            (Metric::DiskIdleBasisPoints, 100, 800, "PDH disk idle"),
            (
                Metric::DiskReadLatencyMicroseconds,
                70_000,
                750,
                "PDH disk read",
            ),
            (
                Metric::DiskWriteLatencyMicroseconds,
                60_000,
                600,
                "PDH disk write",
            ),
        ] {
            let mut reading = item.values[0].clone();
            reading.metric = metric;
            reading.value = Some(value);
            reading.min = Some(value);
            reading.max = Some(value);
            reading.source = source.into();
            reading.observed_from_unix_ms = Some(1000 - duration);
            reading.observed_to_unix_ms = Some(1000);
            reading.observed_duration_ms = duration;
            item.values.push(reading);
        }
        store.append_sample(&item).unwrap();
        let events = store.activity_events(0, 2000, 100).unwrap();
        let measured = |kind: &str| {
            events
                .iter()
                .find(|event| event.kind == kind)
                .unwrap()
                .peak_observation
                .as_ref()
                .unwrap()
        };
        let memory = measured("memory_pressure");
        assert_eq!(memory.value, 9500);
        assert_eq!(memory.metric, "physical_used_basis_points");
        assert_eq!(memory.inputs.len(), 2);
        assert_eq!(memory.inputs[0].value, 1000);
        assert_eq!(memory.inputs[1].value, 50);
        assert_eq!(memory.source, "GlobalMemoryStatusEx");
        assert_eq!(memory.status, Status::Ok);
        assert_eq!(memory.window.as_ref().unwrap().duration_ms, 0);
        assert!(memory.derivation.is_some());
        let busy = measured("disk_busy");
        assert_eq!(busy.value, 9900);
        assert_eq!(busy.inputs[0].metric, "disk_idle_basis_points");
        assert_eq!(busy.inputs[0].value, 100);
        assert_eq!(busy.source, "PDH disk idle");
        assert_eq!(busy.window.as_ref().unwrap().duration_ms, 800);
        let latency = measured("disk_latency");
        assert_eq!(latency.metric, "disk_read_latency_microseconds");
        assert_eq!(latency.value, 70_000);
        assert_eq!(latency.source, "PDH disk read");
        assert_eq!(latency.window.as_ref().unwrap().duration_ms, 750);
        assert!(latency.derivation.is_some());

        item.id = new_id("snapshot");
        item.measured_at_unix_ms = 6000;
        for reading in &mut item.values {
            if matches!(
                reading.metric,
                Metric::PhysicalTotalBytes | Metric::PhysicalAvailableBytes
            ) {
                let at = if reading.metric == Metric::PhysicalAvailableBytes {
                    6001
                } else {
                    6000
                };
                reading.observed_from_unix_ms = Some(at);
                reading.observed_to_unix_ms = Some(at);
            }
            if reading.metric == Metric::PhysicalAvailableBytes {
                reading.value = Some(40);
            }
        }
        store.append_sample(&item).unwrap();
        let events = store.activity_events(0, 7000, 100).unwrap();
        let memory = events
            .iter()
            .find(|event| event.kind == "memory_pressure")
            .unwrap()
            .peak_observation
            .as_ref()
            .unwrap();
        assert_eq!(memory.value, 9600);
        assert_eq!(memory.status, Status::Partial);
        assert!(memory.window.is_none());
        assert!(memory.measured_at_unix_ms.is_none());
        assert_eq!(memory.inputs[0].measured_at_unix_ms, Some(6000));
        assert_eq!(memory.inputs[1].measured_at_unix_ms, Some(6001));
    }

    #[test]
    fn v3_event_rows_migrate_without_fabricating_peak_metadata() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection.execute_batch(MIGRATION).unwrap();
        connection.pragma_update(None, "user_version", 3).unwrap();
        connection.execute(
            "INSERT INTO activity_events(id,kind,entity_ref,session_id,started_at,ended_at,peak,metric,unit,active,origin)
             VALUES('legacy','cpu_busy','system','private-old-session',1000,2000,9900,'cpu_busy_basis_points','basis_points',0,'synthetic_fixture')",
            [],
        ).unwrap();
        let store = Store::init(connection).unwrap();
        assert_eq!(
            store
                .conn
                .pragma_query_value::<i64, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            crate::SCHEMA_VERSION
        );
        let events = store.activity_events(0, 3000, 100).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].peak, 9900);
        assert_eq!(events[0].started_at_unix_ms, 1000);
        assert_eq!(events[0].ended_at_unix_ms, 2000);
        assert!(events[0].peak_observation.is_none());
        assert!(events[0].contributor_observation.is_none());
        let json = serde_json::to_value(&events[0]).unwrap();
        assert!(json["peak_observation"].is_null());
        assert!(json["contributor_observation"].is_null());
    }

    #[test]
    fn high_physical_disk_activity_is_an_anomaly_without_false_process_attribution() {
        let mut store = Store::open_in_memory().unwrap();
        let mut s = sample(1000, Some(1000));
        let mut disk = s.values[0].clone();
        disk.metric = Metric::DiskIdleBasisPoints;
        disk.value = Some(100);
        disk.min = Some(100);
        disk.max = Some(100);
        s.values.push(disk);
        store.append_sample(&s).unwrap();
        let events = store.activity_events(0, 2000, 100).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "disk_busy");
        assert_eq!(events[0].peak, 9900);
        assert!(events[0].contributor_ref.is_none());
    }

    #[test]
    fn historical_aliases_isolate_colliding_collector_refs_and_keep_names_aligned() {
        let mut store = Store::open_in_memory().unwrap();
        let mut first = sample(1000, Some(1000));
        first.session_id = "private-collector-one".into();
        let mut process = first.values[0].clone();
        process.metric = Metric::ProcessCpuBasisPoints;
        process.entity_ref = "proc-aabbccdd".into();
        first.values.push(process);
        first
            .process_names
            .insert("proc-aabbccdd".into(), "one.exe".into());
        let mut second = first.clone();
        second.id = new_id("snapshot");
        second.session_id = "private-collector-two".into();
        second.measured_at_unix_ms = 6000;
        second.values[1].value = Some(9000);
        second.values[1].min = Some(9000);
        second.values[1].max = Some(9000);
        second.values[1].weighted_sum = 45_000_000;
        second
            .process_names
            .insert("proc-aabbccdd".into(), "two.exe".into());
        assert!(store.append_sample(&first).unwrap());
        assert!(!store.append_sample(&first).unwrap());
        assert!(store.append_sample(&second).unwrap());
        let one = history_ref(&store, &first.session_id, "proc-aabbccdd");
        let two = history_ref(&store, &second.session_id, "proc-aabbccdd");
        assert_ne!(one, two);
        assert!(one.starts_with("proc-") && one.len() == 13);
        assert!(two.starts_with("proc-") && two.len() == 13);
        assert!(!one.contains("private"));
        let query = |reference: &str| HistoryQuery {
            metric: Metric::ProcessCpuBasisPoints,
            entity_ref: reference.into(),
            from_unix_ms: 1,
            to_unix_ms: 10_000,
        };
        let one_series = store.metric_history(&query(&one)).unwrap();
        let two_series = store.metric_history(&query(&two)).unwrap();
        assert_eq!(
            one_series
                .points
                .iter()
                .filter_map(|p| p.value)
                .collect::<Vec<_>>(),
            vec![1000]
        );
        assert_eq!(
            two_series
                .points
                .iter()
                .filter_map(|p| p.value)
                .collect::<Vec<_>>(),
            vec![9000]
        );
        assert_eq!(
            one_series.process_names.get(&one).map(String::as_str),
            Some("one.exe")
        );
        assert_eq!(
            two_series.process_names.get(&two).map(String::as_str),
            Some("two.exe")
        );
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sample_entities", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 2);
        store
            .maintain_samples(SAMPLE_RETENTION_MS + 10_000)
            .unwrap();
        let count: i64 = store
            .conn
            .query_row("SELECT COUNT(*) FROM sample_entities", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
}
