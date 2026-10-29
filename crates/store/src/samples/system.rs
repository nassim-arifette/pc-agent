//! Lossless, process-independent system history. Only migration reads old process payloads.
use super::{codec, origin_key, stored, StoredSample};
use crate::StoreError;
use pcagent_shared::Metric;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

const SCHEMA: &str = "
CREATE TABLE system_samples (
 sample_key TEXT PRIMARY KEY REFERENCES samples(sample_key) ON DELETE CASCADE,
 session_id TEXT NOT NULL, measured_at INTEGER NOT NULL, resolution_ms INTEGER NOT NULL,
 interval_ms INTEGER NOT NULL, origin TEXT NOT NULL,
 cpu TEXT, memory_available TEXT, memory_total TEXT, disk_idle TEXT, gpu TEXT,
 measurements TEXT NOT NULL
);
CREATE INDEX system_samples_by_time ON system_samples(measured_at);
CREATE TABLE history_names (
 entity_ref TEXT PRIMARY KEY, name TEXT NOT NULL, last_seen INTEGER NOT NULL
);
CREATE INDEX history_names_by_time ON history_names(last_seen);
";

pub(crate) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // A second connection may have waited for another opener to finish the backfill.
    let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version >= 8 {
        return Ok(());
    }
    if version != 7 {
        return Err(StoreError::Corrupt);
    }
    tx.execute_batch(SCHEMA)?;
    tx.execute_batch(super::collection::SCHEMA)?;
    {
        let mut statement = tx.prepare(
            "SELECT sample_key,measured_at,resolution_ms,payload,session_id FROM samples ORDER BY id",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let sample = codec::decode(&row.get::<_, String>(3)?)?;
            if sample.id != row.get::<_, String>(0)?
                || stored(sample.measured_at_unix_ms) != row.get::<_, i64>(1)?
                || stored(sample.resolution_ms) != row.get::<_, i64>(2)?
                || sample.session_id != row.get::<_, String>(4)?
            {
                return Err(StoreError::Corrupt);
            }
            insert(&tx, &sample)?;
        }
    }
    tx.pragma_update(None, "user_version", 8)?;
    tx.commit()?;
    Ok(())
}

pub(super) fn insert(conn: &Connection, sample: &StoredSample) -> Result<(), StoreError> {
    let system = StoredSample {
        values: sample
            .values
            .iter()
            .filter(|value| value.entity_ref == "system")
            .cloned()
            .collect(),
        process_names: Default::default(),
        ..sample.clone()
    };
    // Decimal columns do not truncate u64 counters to SQLite's signed integer range. The
    // lossless metadata includes exact u128 weights, statuses, source windows and extrema.
    let amount = |metric| {
        system
            .values
            .iter()
            .find(|value| value.metric == metric)
            .and_then(|value| value.value)
            .map(|value| value.to_string())
    };
    conn.execute(
        "INSERT INTO system_samples(sample_key,session_id,measured_at,resolution_ms,interval_ms,
         origin,cpu,memory_available,memory_total,disk_idle,gpu,measurements)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![
            system.id,
            system.session_id,
            stored(system.measured_at_unix_ms),
            stored(system.resolution_ms),
            stored(system.interval_ms),
            origin_key(system.origin),
            amount(Metric::CpuBusyBasisPoints),
            amount(Metric::PhysicalAvailableBytes),
            amount(Metric::PhysicalTotalBytes),
            amount(Metric::DiskIdleBasisPoints),
            amount(Metric::GpuBusiestEngineBasisPoints),
            codec::encode(&system)?,
        ],
    )?;
    for (reference, name) in &sample.process_names {
        conn.execute(
            "INSERT INTO history_names(entity_ref,name,last_seen) VALUES(?1,?2,?3)
             ON CONFLICT(entity_ref) DO UPDATE SET name=CASE WHEN excluded.last_seen>=last_seen
             THEN excluded.name ELSE name END,last_seen=MAX(last_seen,excluded.last_seen)",
            params![reference, name, stored(sample.measured_at_unix_ms)],
        )?;
    }
    Ok(())
}
