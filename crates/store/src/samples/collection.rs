//! Measured SQLite page budget. Recordings are user evidence, never budget eviction candidates.
use super::*;
use crate::millis;

pub const DEFAULT_COLLECTION_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

pub(crate) const SCHEMA: &str = "
CREATE TABLE collection_policy (
 id INTEGER PRIMARY KEY CHECK(id=1), budget_bytes INTEGER NOT NULL,
 budget_removed_samples INTEGER NOT NULL DEFAULT 0, last_budget_eviction INTEGER
);
INSERT INTO collection_policy(id,budget_bytes) VALUES(1,268435456);
";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryCollection {
    pub budget_bytes: u64,
    pub allocated_bytes: u64,
    pub sqlite_bytes: u64,
    pub wal_bytes: u64,
    pub reusable_bytes: u64,
    pub samples: u64,
    pub protected_samples: u64,
    /// UTF-8 payload bytes of retained samples and battery reads, excluding shared page overhead.
    pub protected_payload_bytes: u64,
    pub oldest_unix_ms: Option<u64>,
    pub latest_unix_ms: Option<u64>,
    pub budget_removed_samples: u64,
    pub last_budget_eviction_unix_ms: Option<u64>,
    pub over_budget: bool,
}

const COLLECTION_PAGES: &str = "
SELECT COALESCE(SUM(pgsize),0) FROM dbstat WHERE name IN (
 SELECT name FROM sqlite_schema WHERE tbl_name IN (
 'samples','system_samples','history_names','sample_entities','activity_events',
 'recordings','recording_battery_samples','collection_policy'
 ) AND type IN ('table','index')
)";

pub(super) const UNPROTECTED: &str = "NOT EXISTS (
 SELECT 1 FROM recordings r WHERE s.measured_at+s.resolution_ms>=r.started_at
 AND (r.ended_at IS NULL OR s.measured_at<=r.ended_at)
)";

impl Store {
    pub fn history_collection(&self) -> Result<HistoryCollection, StoreError> {
        let (budget, removed, last): (i64, i64, Option<i64>) = self.conn.query_row(
            "SELECT budget_bytes,budget_removed_samples,last_budget_eviction FROM collection_policy WHERE id=1",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let allocated = self.collection_bytes()?;
        let (samples, oldest, latest): (i64, Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT COUNT(*),MIN(measured_at),MAX(measured_at) FROM samples",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let protected: i64 = self.conn.query_row(
            &format!("SELECT COUNT(*) FROM samples s WHERE NOT ({UNPROTECTED})"),
            [],
            |row| row.get(0),
        )?;
        let protected_bytes: i64 = self.conn.query_row(
            &format!(
                "SELECT COALESCE(SUM(length(CAST(s.payload AS BLOB))+
             length(CAST(y.measurements AS BLOB))),0) +
             (SELECT COALESCE(SUM(length(CAST(payload AS BLOB))),0) FROM recording_battery_samples)
             FROM samples s JOIN system_samples y ON y.sample_key=s.sample_key
             WHERE NOT ({UNPROTECTED})"
            ),
            [],
            |row| row.get(0),
        )?;
        let page_count: i64 = self
            .conn
            .pragma_query_value(None, "page_count", |r| r.get(0))?;
        let page_size: i64 = self
            .conn
            .pragma_query_value(None, "page_size", |r| r.get(0))?;
        let free: i64 = self
            .conn
            .pragma_query_value(None, "freelist_count", |r| r.get(0))?;
        let wal_bytes = self
            .conn
            .path()
            .filter(|path| !path.is_empty())
            .and_then(|path| {
                std::fs::metadata(format!("{path}-wal"))
                    .ok()
                    .map(|file| file.len())
            })
            .unwrap_or(0);
        Ok(HistoryCollection {
            budget_bytes: millis(budget),
            allocated_bytes: allocated,
            sqlite_bytes: millis(page_count).saturating_mul(millis(page_size)),
            reusable_bytes: millis(free).saturating_mul(millis(page_size)),
            wal_bytes,
            samples: millis(samples),
            protected_samples: millis(protected),
            protected_payload_bytes: millis(protected_bytes),
            oldest_unix_ms: oldest.map(millis),
            latest_unix_ms: latest.map(millis),
            budget_removed_samples: millis(removed),
            last_budget_eviction_unix_ms: last.map(millis),
            over_budget: allocated > millis(budget),
        })
    }

    pub fn set_collection_budget(&mut self, bytes: u64, now: u64) -> Result<(), StoreError> {
        // SQLite needs some pages even for an empty schema. This is storage overhead, not a
        // limit on evidence or a sampling quota; there is no product upper bound.
        if bytes < 1024 * 1024 || bytes > i64::MAX as u64 {
            return Err(StoreError::InvalidQuery);
        }
        self.conn.execute(
            "UPDATE collection_policy SET budget_bytes=?1 WHERE id=1",
            [stored(bytes)],
        )?;
        self.maintain_samples(now)?;
        Ok(())
    }

    pub(super) fn collection_bytes(&self) -> Result<u64, StoreError> {
        let bytes: i64 = self
            .conn
            .query_row(COLLECTION_PAGES, [], |row| row.get(0))?;
        Ok(millis(bytes))
    }

    pub(super) fn collection_budget(&self) -> Result<u64, StoreError> {
        let bytes: i64 = self.conn.query_row(
            "SELECT budget_bytes FROM collection_policy WHERE id=1",
            [],
            |row| row.get(0),
        )?;
        Ok(millis(bytes))
    }

    pub(super) fn enforce_collection_budget(&mut self, now: u64) -> Result<usize, StoreError> {
        let budget = self.collection_budget()?;
        let mut removed = 0;
        while self.collection_bytes()? > budget {
            let tx = self.conn.transaction()?;
            let batch = tx.execute(
                &format!(
                    "DELETE FROM samples WHERE id IN (
                 SELECT s.id FROM samples s WHERE {UNPROTECTED}
                 ORDER BY s.measured_at,s.id LIMIT 512)"
                ),
                [],
            )?;
            if batch == 0 {
                // All remaining pages belong to protected recordings or their evidence.
                // Keep them and report over_budget; never pretend the budget was enforced.
                break;
            }
            tx.execute(
                "DELETE FROM activity_events WHERE ended_at <
                 COALESCE((SELECT MIN(measured_at) FROM samples),9223372036854775807)
                 AND NOT EXISTS (SELECT 1 FROM recordings r
                 WHERE activity_events.ended_at>=r.started_at
                 AND (r.ended_at IS NULL OR activity_events.started_at<=r.ended_at))",
                [],
            )?;
            tx.execute(
                "DELETE FROM history_names WHERE last_seen <
                 COALESCE((SELECT MIN(measured_at) FROM samples),9223372036854775807)
                 AND entity_ref NOT IN (SELECT contributor_ref FROM activity_events WHERE contributor_ref IS NOT NULL)", [],
            )?;
            tx.execute(
                "DELETE FROM sample_entities WHERE last_seen <
                 COALESCE((SELECT MIN(measured_at) FROM samples),9223372036854775807)
                 AND entity_ref NOT IN (SELECT entity_ref FROM history_names)",
                [],
            )?;
            tx.execute(
                "UPDATE collection_policy SET budget_removed_samples=budget_removed_samples+?1,
                 last_budget_eviction=?2 WHERE id=1",
                params![batch as i64, stored(now)],
            )?;
            tx.commit()?;
            removed += batch;
        }
        // Unused pages remain reusable by SQLite; a PASSIVE checkpoint never blocks a reader.
        self.conn.execute_batch("PRAGMA wal_checkpoint(PASSIVE);")?;
        Ok(removed)
    }
}
