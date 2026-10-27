//! One read snapshot serves all system charts. Only bounded, finished series are cached;
//! sample payloads are streamed and never retained in the reader between requests.
use super::*;
use std::collections::VecDeque;
use std::io::{self, Write};

#[cfg(test)]
mod tests;

pub(crate) const SYSTEM_CHARTS: [Metric; 7] = [
    Metric::CpuBusyBasisPoints,
    Metric::PhysicalAvailableBytes,
    Metric::PhysicalTotalBytes,
    Metric::DiskIdleBasisPoints,
    Metric::GpuBusiestEngineBasisPoints,
    Metric::DiskReadLatencyMicroseconds,
    Metric::DiskWriteLatencyMicroseconds,
];
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// Compact local chart transport. Exact C4 observations remain in `histories` on the host.
#[derive(Debug, Clone, Serialize)]
pub struct HistoryChart {
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub buckets: usize,
    pub series: Vec<ChartSeries>,
    pub gaps: Vec<HistoryGap>,
    pub events: Vec<ActivityEvent>,
    pub process_names: BTreeMap<String, String>,
    pub limitations: Vec<String>,
    #[serde(skip)]
    pub histories: Vec<HistorySeries>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChartSeries {
    pub metric: Metric,
    pub unit: String,
    pub status: Status,
    pub points: Vec<ChartPoint>,
}

/// [timestamp, mean, minimum, maximum, measured, expected, resolution, status, origin].
/// Positions keep the transport compact without losing null values or bands.
#[derive(Debug, Clone, Serialize)]
pub struct ChartPoint(
    pub u64,
    pub Option<u64>,
    pub Option<u64>,
    pub Option<u64>,
    pub u64,
    pub u64,
    pub u64,
    pub Status,
    pub Origin,
);

impl HistoryChart {
    fn new(from: u64, to: u64, buckets: usize, mut histories: Vec<HistorySeries>) -> Self {
        let first = histories.first_mut().expect("system metrics");
        let events = std::mem::take(&mut first.events);
        let process_names = std::mem::take(&mut first.process_names);
        let gaps = first.gaps.clone();
        let limitations = first.limitations.clone();
        for history in &mut histories {
            history.events.clear();
            history.process_names.clear();
        }
        let series = histories
            .iter()
            .map(|history| ChartSeries {
                metric: history.metric,
                unit: history.unit.clone(),
                status: history.status,
                points: history
                    .points
                    .iter()
                    .map(|p| {
                        ChartPoint(
                            p.unix_ms,
                            p.value,
                            p.min,
                            p.max,
                            p.measured_samples,
                            p.expected_samples,
                            p.resolution_ms,
                            p.status,
                            p.origin,
                        )
                    })
                    .collect(),
            })
            .collect();
        Self {
            from_unix_ms: from,
            to_unix_ms: to,
            buckets,
            series,
            gaps,
            events,
            process_names,
            limitations,
            histories,
        }
    }
}

pub(crate) struct HistoryCache {
    revision: (i64, u64),
    from: u64,
    to: u64,
    buckets: Option<usize>,
    series: Vec<HistorySeries>,
}

struct SizeLimit(usize);
impl Write for SizeLimit {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > MAX_CACHE_BYTES {
            return Err(io::Error::other("history cache size limit"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct SeriesBuilder {
    metric: Metric,
    points: VecDeque<HistoryPoint>,
    sources: Vec<String>,
    width: usize,
    chunk_len: usize,
    previous_at: u64,
    previous_resolution: u64,
    omitted: bool,
    time_bucket: Option<(u64, u64)>,
}

impl SeriesBuilder {
    fn new(metric: Metric, point_count: usize) -> Self {
        Self {
            metric,
            points: VecDeque::new(),
            sources: Vec::new(),
            width: point_count.div_ceil(MAX_POINTS).max(1),
            chunk_len: 0,
            previous_at: 0,
            previous_resolution: 0,
            omitted: false,
            time_bucket: None,
        }
    }

    fn push(&mut self, sample: &StoredSample, entity_ref: &str) {
        let value = sample
            .values
            .iter()
            .find(|value| value.metric == self.metric && value.entity_ref == entity_ref);
        let (weighted_sum, weight_ms) = value.map(mean_parts).unwrap_or_default();
        let point = HistoryPoint {
            unix_ms: sample.measured_at_unix_ms,
            value: value.and_then(|value| value.value),
            min: value.and_then(|value| value.min),
            max: value.and_then(|value| value.max),
            status: value.map_or(Status::Unsupported, |value| value.status),
            measured_samples: value.map_or(0, |value| value.measured),
            expected_samples: value.map_or(1, |value| value.expected),
            resolution_ms: sample.resolution_ms,
            observed_from_unix_ms: value.and_then(|value| value.observed_from_unix_ms),
            observed_to_unix_ms: value.and_then(|value| value.observed_to_unix_ms),
            observed_duration_ms: value.map_or(0, |value| value.observed_duration_ms),
            origin: sample.origin,
            session_id: sample.session_id.clone(),
            weighted_sum,
            weight_ms,
        };
        let compatible = self.points.back().is_some_and(|last| {
            last.origin == point.origin
                && last.session_id == point.session_id
                && (self.time_bucket.is_none() || last.value.is_some() == point.value.is_some())
                && point.unix_ms.saturating_sub(self.previous_at)
                    <= point.resolution_ms.max(self.previous_resolution) * 2
        });
        self.previous_at = point.unix_ms;
        self.previous_resolution = point.resolution_ms;
        let same_bucket = self
            .time_bucket
            .map_or(self.chunk_len < self.width, |(from, step)| {
                self.points.back().is_some_and(|last| {
                    last.unix_ms.saturating_sub(from) / step
                        == point.unix_ms.saturating_sub(from) / step
                })
            });
        if same_bucket && compatible {
            merge_points(self.points.back_mut().expect("compatible point"), &point);
            self.chunk_len += 1;
        } else {
            if self.time_bucket.is_none() && self.points.len() == MAX_POINTS {
                self.points.pop_front();
                self.omitted = true;
            }
            self.points.push_back(point);
            self.chunk_len = 1;
        }
        if let Some(value) = value {
            if !self.sources.contains(&value.source) {
                self.sources.push(value.source.clone());
            }
        }
    }
}

impl Store {
    pub fn history_chart(
        &self,
        from: u64,
        to: u64,
        buckets: usize,
    ) -> Result<HistoryChart, StoreError> {
        if from >= to || to - from > SAMPLE_RETENTION_MS || to > i64::MAX as u64 || buckets == 0 {
            return Err(StoreError::InvalidQuery);
        }
        let revision = self.history_revision()?;
        if let Some(cache) = self.history_cache.borrow().as_ref().filter(|cache| {
            cache.revision == revision
                && cache.from == from
                && cache.to == to
                && cache.buckets == Some(buckets)
        }) {
            return Ok(HistoryChart::new(from, to, buckets, cache.series.clone()));
        }
        self.history_cache.borrow_mut().take();
        let tx = self
            .conn
            .is_autocommit()
            .then(|| self.conn.unchecked_transaction())
            .transpose()?;
        let query = HistoryQuery {
            metric: Metric::CpuBusyBasisPoints,
            entity_ref: "system".into(),
            from_unix_ms: from,
            to_unix_ms: to,
        };
        let histories = self.read_history(&query, &SYSTEM_CHARTS, Some(buckets))?;
        if let Some(tx) = tx {
            tx.commit()?;
        }
        if revision == self.history_revision()?
            && serde_json::to_writer(SizeLimit(0), &histories).is_ok()
        {
            *self.history_cache.borrow_mut() = Some(HistoryCache {
                revision,
                from,
                to,
                buckets: Some(buckets),
                series: histories.clone(),
            });
        }
        Ok(HistoryChart::new(from, to, buckets, histories))
    }

    fn history_revision(&self) -> Result<(i64, u64), StoreError> {
        Ok((
            self.conn
                .pragma_query_value(None, "data_version", |row| row.get(0))?,
            self.conn.total_changes(),
        ))
    }

    pub fn metric_history(&self, query: &HistoryQuery) -> Result<HistorySeries, StoreError> {
        if query.from_unix_ms >= query.to_unix_ms
            || query.to_unix_ms - query.from_unix_ms > SAMPLE_RETENTION_MS
            || query.to_unix_ms > i64::MAX as u64
            || query.entity_ref.len() > 128
        {
            return Err(StoreError::InvalidQuery);
        }
        let charts = query.entity_ref == "system" && SYSTEM_CHARTS.contains(&query.metric);
        let revision = self.history_revision()?;
        if charts {
            if let Some(cache) = self.history_cache.borrow().as_ref().filter(|cache| {
                cache.revision == revision
                    && cache.buckets.is_none()
                    && cache.from == query.from_unix_ms
                    && cache.to == query.to_unix_ms
            }) {
                if let Some(series) = cache
                    .series
                    .iter()
                    .find(|series| series.metric == query.metric)
                {
                    return Ok(series.clone());
                }
            }
        }
        // Drop stale data before reading. Another connection may be clearing or compacting it.
        self.history_cache.borrow_mut().take();
        let transaction = self.conn.unchecked_transaction()?;
        let metrics = if charts {
            SYSTEM_CHARTS.as_slice()
        } else {
            std::slice::from_ref(&query.metric)
        };
        let series = self.read_history(query, metrics, None)?;
        transaction.commit()?;
        let result = series
            .iter()
            .find(|series| series.metric == query.metric)
            .ok_or(StoreError::Corrupt)?
            .clone();
        // A concurrent commit may have happened before the read snapshot or during it. Its
        // result is valid for that snapshot, but must not be cached under a newer revision.
        if charts
            && revision == self.history_revision()?
            && serde_json::to_writer(SizeLimit(0), &series).is_ok()
        {
            *self.history_cache.borrow_mut() = Some(HistoryCache {
                revision,
                from: query.from_unix_ms,
                to: query.to_unix_ms,
                buckets: None,
                series,
            });
        }
        Ok(result)
    }

    fn read_history(
        &self,
        query: &HistoryQuery,
        metrics: &[Metric],
        buckets: Option<usize>,
    ) -> Result<Vec<HistorySeries>, StoreError> {
        let from = stored(query.from_unix_ms.saturating_sub(60_000));
        let to = stored(query.to_unix_ms);
        let (table, payload, row_limit) = if query.entity_ref == "system" {
            ("system_samples", "measurements", i64::MAX)
        } else {
            ("samples", "payload", MAX_QUERY_ROWS as i64)
        };
        // Count indexed metadata under the same read transaction to choose the original
        // aggregation width before streaming. No process JSON is decoded for this count.
        let point_count: i64 = if buckets.is_some() {
            0
        } else {
            self.conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM (SELECT measured_at,resolution_ms FROM {table}
             WHERE measured_at>=?1 AND measured_at<=?2 ORDER BY measured_at,rowid LIMIT ?3)
             WHERE measured_at>=?4 OR (resolution_ms>=60000 AND measured_at>?4-resolution_ms)"
                ),
                params![from, to, row_limit, stored(query.from_unix_ms)],
                |row| row.get(0),
            )?
        };
        let point_count = usize::try_from(point_count).map_err(|_| StoreError::Corrupt)?;
        let mut builders: Vec<_> = metrics
            .iter()
            .map(|metric| {
                let mut builder = SeriesBuilder::new(*metric, point_count);
                builder.time_bucket = buckets.map(|count| {
                    (
                        query.from_unix_ms,
                        (query.to_unix_ms - query.from_unix_ms)
                            .div_ceil(count as u64)
                            .max(1),
                    )
                });
                builder
            })
            .collect();
        let mut statement = self.conn.prepare(&format!(
            "SELECT measured_at,resolution_ms,{payload} FROM {table} WHERE measured_at>=?1 AND measured_at<=?2 ORDER BY measured_at,rowid LIMIT ?3",
        ))?;
        let rows = statement.query_map(params![from, to, row_limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut gaps = Vec::new();
        let mut names = BTreeMap::new();
        let mut previous: Option<(u64, u64, String, Origin)> = None;
        let mut origins = Vec::new();
        let mut rows_read = 0;
        for row in rows {
            let (at, resolution, text) = row?;
            let at = u64::try_from(at).map_err(|_| StoreError::Corrupt)?;
            let resolution = u64::try_from(resolution).map_err(|_| StoreError::Corrupt)?;
            let sample = if query.entity_ref == "system" {
                codec::decode_system(&text)?
            } else {
                codec::decode(&text)?
            };
            if at != sample.measured_at_unix_ms || resolution != sample.resolution_ms {
                return Err(StoreError::Corrupt);
            }
            rows_read += 1;
            if at < query.from_unix_ms
                && (resolution < 60_000 || at.saturating_add(resolution) <= query.from_unix_ms)
            {
                continue;
            }
            if !origins.contains(&sample.origin) {
                origins.push(sample.origin);
            }
            let tolerance = resolution.max(1000) * 2;
            if let Some((before, interval, session, origin)) = &previous {
                if at.saturating_sub(*before) > tolerance.max(*interval * 2)
                    || session != &sample.session_id
                    || origin != &sample.origin
                {
                    gaps.push(HistoryGap {
                        from_unix_ms: *before,
                        to_unix_ms: at,
                        reason: if origin != &sample.origin {
                            "origin_changed"
                        } else if session != &sample.session_id {
                            "collector_restarted"
                        } else {
                            "not_recorded"
                        }
                        .into(),
                    });
                }
            } else if at.saturating_sub(query.from_unix_ms) > tolerance {
                gaps.push(HistoryGap {
                    from_unix_ms: query.from_unix_ms,
                    to_unix_ms: at,
                    reason: "not_recorded".into(),
                });
            }
            for builder in &mut builders {
                builder.push(&sample, &query.entity_ref);
            }
            previous = Some((at, resolution, sample.session_id, sample.origin));
            for (key, name) in sample.process_names {
                if names.len() < 200 || names.contains_key(&key) {
                    names.insert(key, name);
                }
            }
        }
        let last = previous
            .as_ref()
            .map_or(query.from_unix_ms, |point| point.0.saturating_add(point.1));
        if query.to_unix_ms.saturating_sub(last) > previous.as_ref().map_or(0, |point| point.1 * 2)
        {
            gaps.push(HistoryGap {
                from_unix_ms: last,
                to_unix_ms: query.to_unix_ms,
                reason: "not_recorded".into(),
            });
        }
        let events = self.activity_events(query.from_unix_ms, query.to_unix_ms, usize::MAX)?;
        if query.entity_ref == "system" {
            let mut statement = self.conn.prepare(
                "SELECT DISTINCT n.entity_ref,n.name FROM history_names n
                 JOIN activity_events e ON e.contributor_ref=n.entity_ref
                 WHERE e.ended_at>=?1 AND e.started_at<=?2",
            )?;
            let rows = statement.query_map(
                params![stored(query.from_unix_ms), stored(query.to_unix_ms)],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )?;
            names.extend(rows.collect::<Result<BTreeMap<_, _>, _>>()?);
        }
        let mut results = Vec::with_capacity(builders.len());
        for (index, builder) in builders.into_iter().enumerate() {
            let mut gaps = gaps.clone();
            let points: Vec<_> = builder.points.into_iter().collect();
            let mut limitations = vec!["Persisted local samples; values are not interpolated. After 24 hours, values are duration-weighted minute averages with measured minima and maxima. Instantaneous gauges use the sampling interval as averaging weight, not as a measured window.".into(),
                "Only the top 20 processes at each sample are retained; absence of a process is unknown, not zero.".into(),
                "Observation timestamps and accumulated measured duration are preserved separately from the nominal sampling cadence. A recording's first rate may cover the preceding background interval. Minute buckets overlapping the requested range are returned in full.".into()];
            if rows_read as i64 == row_limit {
                limitations.push("History row limit reached; narrow the requested window.".into());
            }
            if point_count > MAX_POINTS {
                if builder.omitted {
                    gaps.push(HistoryGap {
                        from_unix_ms: query.from_unix_ms,
                        to_unix_ms: points[0].unix_ms,
                        reason: "response_limit".into(),
                    });
                    limitations.push("Too many disconnected segments to aggregate safely; only the latest 2000 segments are returned. Narrow the requested window.".into());
                }
                limitations.push("Response points were aggregated to the 2000-point limit; extrema and sample coverage are retained.".into());
            }
            let status = if points.iter().any(|point| point.value.is_some())
                && gaps.is_empty()
                && points.iter().all(|point| point.status == Status::Ok)
                && (rows_read as i64) < row_limit
            {
                Status::Ok
            } else {
                Status::Partial
            };
            results.push(HistorySeries {
                metric: builder.metric,
                entity_ref: query.entity_ref.clone(),
                unit: builder.metric.unit().into(),
                status,
                origin: if origins.len() == 1 {
                    Some(origins[0])
                } else {
                    None
                },
                from_unix_ms: query.from_unix_ms,
                to_unix_ms: query.to_unix_ms,
                points,
                gaps,
                events: if buckets.is_none() || index == 0 {
                    events.clone()
                } else {
                    vec![]
                },
                process_names: if buckets.is_none() || index == 0 {
                    names.clone()
                } else {
                    BTreeMap::new()
                },
                sources: builder.sources,
                limitations,
            });
        }
        Ok(results)
    }
}
