//! Indexed local navigation and a single immutable read of completed recordings.
use super::*;
use crate::{BatteryRecording, ProcessPeaks};

#[derive(Debug, Serialize)]
pub struct HistoryPeriod {
    pub id: &'static str,
    pub from_unix_ms: u64,
    pub to_unix_ms: u64,
    pub event_count: u64,
}

#[derive(Debug, Serialize)]
pub struct HistoryOverview {
    pub periods: Vec<HistoryPeriod>,
    pub latest_event_ended_at_unix_ms: Option<u64>,
    pub latest_sample_unix_ms: Option<u64>,
    pub recordings: Vec<Recording>,
    pub more_recordings: bool,
}

#[derive(Debug, Serialize)]
pub struct HistoryRecordings {
    pub recordings: Vec<Recording>,
    pub more: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordingDetail {
    pub recording: Recording,
    pub chart: HistoryChart,
    pub cpu_peaks: ProcessPeaks,
    pub resource_peaks: ProcessPeaks,
    pub battery: Option<BatteryRecording>,
    pub minute_summaries: bool,
}

impl Store {
    pub fn history_overview(
        &self,
        day_start: u64,
        now: u64,
    ) -> Result<HistoryOverview, StoreError> {
        if day_start > now || now - day_start > 25 * 3_600_000 || now > i64::MAX as u64 {
            return Err(StoreError::InvalidQuery);
        }
        let tx = self.conn.unchecked_transaction()?;
        let mut periods = Vec::new();
        for (id, from) in [
            ("hour", now.saturating_sub(3_600_000)),
            ("day", day_start),
            ("week", now.saturating_sub(SAMPLE_RETENTION_MS)),
        ] {
            let count: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM activity_events WHERE ended_at>=?1 AND started_at<=?2",
                params![stored(from), stored(now)],
                |row| row.get(0),
            )?;
            periods.push(HistoryPeriod {
                id,
                from_unix_ms: from,
                to_unix_ms: now,
                event_count: count as u64,
            });
        }
        let latest_event: Option<i64> =
            self.conn
                .query_row("SELECT MAX(ended_at) FROM activity_events", [], |row| {
                    row.get(0)
                })?;
        let latest_sample: Option<i64> =
            self.conn
                .query_row("SELECT MAX(measured_at) FROM system_samples", [], |row| {
                    row.get(0)
                })?;
        let recordings = self.history_recordings(0)?;
        tx.commit()?;
        Ok(HistoryOverview {
            periods,
            latest_event_ended_at_unix_ms: latest_event.map(|n| n as u64),
            latest_sample_unix_ms: latest_sample.map(|n| n as u64),
            recordings: recordings.recordings,
            more_recordings: recordings.more,
        })
    }

    /// Twenty records per page, with no ceiling on the accessible recording history.
    pub fn history_recordings(&self, offset: u64) -> Result<HistoryRecordings, StoreError> {
        if offset > i64::MAX as u64 {
            return Err(StoreError::InvalidQuery);
        }
        let mut statement = self.conn.prepare(
            "SELECT id FROM recordings ORDER BY started_at DESC,id DESC LIMIT 21 OFFSET ?1",
        )?;
        let ids = statement
            .query_map([offset as i64], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let more = ids.len() > 20;
        let recordings = ids
            .into_iter()
            .take(20)
            .map(|id| self.recording(&id)?.ok_or(StoreError::Corrupt))
            .collect::<Result<_, _>>()?;
        Ok(HistoryRecordings { recordings, more })
    }

    pub fn recording_detail(&self, id: &str) -> Result<Option<RecordingDetail>, StoreError> {
        if !id.starts_with("recording-")
            || id.len() > 128
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        {
            return Err(StoreError::InvalidQuery);
        }
        let tx = self.conn.unchecked_transaction()?;
        let Some(recording) = self.recording(id)? else {
            return Ok(None);
        };
        let Some(to) = recording.ended_at_unix_ms else {
            return Err(StoreError::InvalidQuery);
        };
        let from = recording.started_at_unix_ms;
        if to <= from {
            return Err(StoreError::InvalidQuery);
        }
        let chart = self.history_chart(from, to, 120)?;
        let minute_summaries = chart
            .histories
            .iter()
            .any(|s| s.points.iter().any(|p| p.resolution_ms >= 60_000));
        let detail = RecordingDetail {
            cpu_peaks: self.process_peaks(from, to, 10)?,
            resource_peaks: self.process_resource_peaks(from, to, 10)?,
            battery: self.battery_recording(id)?,
            recording,
            chart,
            minute_summaries,
        };
        tx.commit()?;
        Ok(Some(detail))
    }
}
