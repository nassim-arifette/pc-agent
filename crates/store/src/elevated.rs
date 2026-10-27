//! Host-approved, already measured inspections. Persistence never grants an execution permit.
use crate::{stored, Store, StoreError};
use pcagent_inspect::{elevation::ElevatedTask, Inspection};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const ELEVATED_RETENTION_MS: u64 = 24 * 60 * 60 * 1000;
pub const MAX_ELEVATED_RECORD_BYTES: usize = pcagent_inspect::elevation::MAX_RESPONSE_BYTES * 4;

#[derive(Clone, Serialize, Deserialize)]
pub struct ElevatedInspection {
    pub id: String,
    pub task: ElevatedTask,
    pub inspection: Inspection,
    /// Local presentation data only: section statuses, coverage notes and the metadata tree.
    pub local_details: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    task: ElevatedTask,
    inspection: Inspection,
    local_details: Value,
    /// Inspection deliberately omits names from its wire format. Keep the C5 dictionary local
    /// so reopening the app does not bypass (or lose) the person's explicit sharing choice.
    local_names: BTreeMap<String, String>,
}

pub(crate) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 9 {
        return Ok(());
    }
    if version != 8 {
        return Err(StoreError::Corrupt);
    }
    tx.execute_batch(
        "CREATE TABLE elevated_inspections (
            id TEXT PRIMARY KEY,
            measured_at INTEGER NOT NULL CHECK(measured_at >= 0),
            payload TEXT NOT NULL
        );
        CREATE INDEX elevated_inspections_by_time ON elevated_inspections(measured_at);
        PRAGMA user_version = 9;",
    )?;
    tx.commit()?;
    Ok(())
}

impl Store {
    pub fn prune_elevated_inspections(&self, now_unix_ms: u64) -> Result<usize, StoreError> {
        let cutoff = now_unix_ms.checked_sub(ELEVATED_RETENTION_MS);
        Ok(self.conn.execute(
            "DELETE FROM elevated_inspections
             WHERE measured_at > ?1 OR (?2 IS NOT NULL AND measured_at <= ?2)",
            params![stored(now_unix_ms), cutoff.map(stored)],
        )?)
    }

    /// Only the host completion path writes here; no model-facing tool can create records.
    /// Retention starts at the measurement, not this write, and reloads never extend it.
    pub fn save_elevated_inspection(
        &self,
        task: &ElevatedTask,
        inspection: &Inspection,
        local_details: &Value,
        now_unix_ms: u64,
    ) -> Result<Option<ElevatedInspection>, StoreError> {
        self.prune_elevated_inspections(now_unix_ms)?;
        if now_unix_ms
            .checked_sub(inspection.measured_at_unix_ms)
            .is_none_or(|age| age >= ELEVATED_RETENTION_MS)
        {
            return Ok(None);
        }
        let payload = serde_json::to_string(&Payload {
            task: task.clone(),
            inspection: inspection.clone(),
            local_details: local_details.clone(),
            local_names: inspection.names.clone(),
        })
        .map_err(|_| StoreError::Corrupt)?;
        if payload.len() > MAX_ELEVATED_RECORD_BYTES {
            return Err(StoreError::InvalidQuery);
        }
        let id = pcagent_shared::new_id("elevated");
        self.conn.execute(
            "INSERT INTO elevated_inspections(id, measured_at, payload) VALUES (?1, ?2, ?3)",
            params![id, stored(inspection.measured_at_unix_ms), payload],
        )?;
        Ok(Some(ElevatedInspection {
            id,
            task: task.clone(),
            inspection: inspection.clone(),
            local_details: local_details.clone(),
        }))
    }

    pub fn elevated_inspections(
        &self,
        now_unix_ms: u64,
    ) -> Result<Vec<ElevatedInspection>, StoreError> {
        self.prune_elevated_inspections(now_unix_ms)?;
        let mut statement = self.conn.prepare(
            "SELECT id, measured_at, payload FROM elevated_inspections ORDER BY measured_at, rowid",
        )?;
        let mut rows = statement.query([])?;
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push(decode(row.get(0)?, row.get(1)?, row.get::<_, String>(2)?)?);
        }
        Ok(results)
    }

    pub fn elevated_inspection(
        &self,
        id: &str,
        now_unix_ms: u64,
    ) -> Result<Option<ElevatedInspection>, StoreError> {
        self.prune_elevated_inspections(now_unix_ms)?;
        let row: Option<(String, i64, String)> = self
            .conn
            .query_row(
                "SELECT id, measured_at, payload FROM elevated_inspections WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        row.map(|(id, measured_at, payload)| decode(id, measured_at, payload))
            .transpose()
    }

    pub fn clear_elevated_inspections(&self) -> Result<usize, StoreError> {
        Ok(self.conn.execute("DELETE FROM elevated_inspections", [])?)
    }
}

fn decode(id: String, measured_at: i64, payload: String) -> Result<ElevatedInspection, StoreError> {
    if payload.len() > MAX_ELEVATED_RECORD_BYTES {
        return Err(StoreError::Corrupt);
    }
    let mut payload: Payload = serde_json::from_str(&payload).map_err(|_| StoreError::Corrupt)?;
    if u64::try_from(measured_at).ok() != Some(payload.inspection.measured_at_unix_ms) {
        return Err(StoreError::Corrupt);
    }
    payload.inspection.names = payload.local_names;
    Ok(ElevatedInspection {
        id,
        task: payload.task,
        inspection: payload.inspection,
        local_details: payload.local_details,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inspection(at: u64) -> Inspection {
        let mut result: Inspection = serde_json::from_value(json!({
            "status": "partial", "measured_at_unix_ms": at,
            "window": {"started_at_unix_ms": at, "finished_at_unix_ms": at, "duration_ms": 0},
            "origin": "synthetic_fixture", "observations": [],
            "coverage": [{"scope":"disk_reliability", "status":"needs_admin",
                "source":"fixture", "detail":"No usable counter"}]
        }))
        .unwrap();
        result
            .names
            .insert("name-fixture".into(), "Local fixture name".into());
        result
    }

    #[test]
    fn retention_uses_measurement_time_and_purges_on_reads_and_writes() {
        let store = Store::open_in_memory().unwrap();
        let task = ElevatedTask::SystemCaches;
        let record = store
            .save_elevated_inspection(&task, &inspection(100), &json!({}), 200)
            .unwrap()
            .unwrap();
        assert!(store
            .elevated_inspection(&record.id, 100 + ELEVATED_RETENTION_MS - 1)
            .unwrap()
            .is_some());
        assert!(store
            .elevated_inspection(&record.id, 100 + ELEVATED_RETENTION_MS)
            .unwrap()
            .is_none());
        assert!(store
            .save_elevated_inspection(
                &task,
                &inspection(100),
                &json!({}),
                100 + ELEVATED_RETENTION_MS
            )
            .unwrap()
            .is_none());
        assert!(store
            .save_elevated_inspection(&task, &inspection(300), &json!({}), 200)
            .unwrap()
            .is_none());
        store
            .save_elevated_inspection(&task, &inspection(300), &json!({}), 300)
            .unwrap();
        store
            .save_elevated_inspection(
                &task,
                &inspection(ELEVATED_RETENTION_MS + 300),
                &json!({}),
                ELEVATED_RETENTION_MS + 300,
            )
            .unwrap();
        assert_eq!(
            store
                .elevated_inspections(ELEVATED_RETENTION_MS + 300)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn reopen_preserves_partial_status_dates_and_local_only_names_without_reapproval() {
        let path = std::env::temp_dir().join(format!(
            "{}.sqlite",
            pcagent_shared::new_id("elevated-test")
        ));
        let id = {
            let store = Store::open(&path).unwrap();
            store
                .save_elevated_inspection(
                    &ElevatedTask::DeepAnalysis {
                        tasks: vec![ElevatedTask::SystemCaches, ElevatedTask::DiskReliability],
                    },
                    &inspection(100),
                    &json!({"sections":[{"status":"partial"}]}),
                    200,
                )
                .unwrap()
                .unwrap()
                .id
        };
        {
            let store = Store::open(&path).unwrap();
            let record = store.elevated_inspection(&id, 300).unwrap().unwrap();
            assert_eq!(record.inspection.measured_at_unix_ms, 100);
            assert_eq!(record.inspection.status, pcagent_shared::Status::Partial);
            assert_eq!(
                record.inspection.names["name-fixture"],
                "Local fixture name"
            );
            assert!(!serde_json::to_string(&record)
                .unwrap()
                .contains("Local fixture name"));
            assert_eq!(record.local_details["sections"][0]["status"], "partial");
            assert_eq!(store.clear_elevated_inspections().unwrap(), 1);
            assert!(store.elevated_inspection(&id, 300).unwrap().is_none());
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn migration_nine_requires_real_eight_and_keeps_predecessor_data() {
        let store = Store::open_in_memory().unwrap();
        let conversation = store.create("Before migration", 1).unwrap();
        store
            .conn
            .execute_batch("DROP TABLE elevated_inspections; PRAGMA user_version = 8;")
            .unwrap();
        migrate(&store.conn).unwrap();
        assert!(store.get(&conversation).unwrap().is_some());
        for table in ["watches", "system_samples"] {
            let exists: i64 = store
                .conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                    [table],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "{table}");
        }
        store
            .conn
            .execute_batch("PRAGMA user_version = 7;")
            .unwrap();
        assert!(matches!(migrate(&store.conn), Err(StoreError::Corrupt)));
    }

    #[test]
    fn corrupt_timestamp_never_restores_evidence() {
        let store = Store::open_in_memory().unwrap();
        let record = store
            .save_elevated_inspection(
                &ElevatedTask::SystemCaches,
                &inspection(100),
                &json!({}),
                100,
            )
            .unwrap()
            .unwrap();
        store
            .conn
            .execute("UPDATE elevated_inspections SET measured_at = 101", [])
            .unwrap();
        assert!(matches!(
            store.elevated_inspection(&record.id, 200),
            Err(StoreError::Corrupt)
        ));
    }
}
