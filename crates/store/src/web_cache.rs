//! Disposable public-source cache, separate from private conversation history and its schema.
use crate::StoreError;
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use std::path::Path;

pub struct WebCache {
    conn: Connection,
}

pub struct CachedWebValue {
    pub read_at_unix_ms: u64,
    pub value: Value,
}

impl WebCache {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| StoreError::Database(e.to_string()))?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.busy_timeout(std::time::Duration::from_secs(2))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS web_cache_v1 (
                key TEXT PRIMARY KEY, read_at INTEGER NOT NULL,
                expires_at INTEGER NOT NULL, value TEXT NOT NULL
            );",
        )?;
        Ok(Self { conn })
    }

    /// Expired entries and clock rollback are misses. Neither stale data nor errors are replayed.
    pub fn get(&self, key: &str, now: u64) -> Result<Option<CachedWebValue>, StoreError> {
        let row: Option<(i64, i64, String)> = self
            .conn
            .query_row(
                "SELECT read_at, expires_at, value FROM web_cache_v1 WHERE key = ?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((read_at, expires_at, text)) = row else {
            return Ok(None);
        };
        let valid = read_at >= 0
            && expires_at > read_at
            && now >= read_at as u64
            && now < expires_at as u64;
        let value = valid.then(|| serde_json::from_str(&text).ok()).flatten();
        if let Some(value) = value {
            Ok(Some(CachedWebValue {
                read_at_unix_ms: read_at as u64,
                value,
            }))
        } else {
            self.conn
                .execute("DELETE FROM web_cache_v1 WHERE key = ?1", [key])?;
            Ok(None)
        }
    }

    /// Call only for successful responses. The caller owns the typed response validation.
    pub fn put(&self, key: &str, value: &Value, now: u64, ttl_ms: u64) -> Result<(), StoreError> {
        let expires = now
            .checked_add(ttl_ms)
            .and_then(|v| i64::try_from(v).ok())
            .filter(|_| ttl_ms > 0)
            .ok_or(StoreError::InvalidQuery)?;
        let now = i64::try_from(now).map_err(|_| StoreError::InvalidQuery)?;
        self.conn.execute(
            "DELETE FROM web_cache_v1 WHERE expires_at <= ?1 OR read_at > ?1",
            [now],
        )?;
        self.conn.execute(
            "INSERT INTO web_cache_v1 (key, read_at, expires_at, value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET read_at=excluded.read_at,
             expires_at=excluded.expires_at, value=excluded.value",
            params![key, now, expires, value.to_string()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn expiry_boundary_clock_rollback_and_replacement() {
        let cache = WebCache::in_memory().unwrap();
        cache
            .put("query", &json!({"sources": []}), 100, 50)
            .unwrap();
        assert_eq!(
            cache.get("query", 149).unwrap().unwrap().read_at_unix_ms,
            100
        );
        assert!(cache.get("query", 150).unwrap().is_none());
        cache
            .put("query", &json!({"fresh": true}), 200, 50)
            .unwrap();
        assert!(cache.get("query", 199).unwrap().is_none());
        assert!(cache.get("query", 200).unwrap().is_none());
        assert!(cache.put("query", &json!({}), u64::MAX, 10).is_err());
        assert!(cache.put("query", &json!({}), 100, 0).is_err());
    }

    #[test]
    fn successful_values_persist_across_connections() {
        let path = std::env::temp_dir().join(format!("{}.sqlite", pcagent_shared::new_id("web")));
        {
            let cache = WebCache::open(&path).unwrap();
            cache
                .put("k", &json!({"public": "snippet"}), 100, 20)
                .unwrap();
        }
        {
            let cache = WebCache::open(&path).unwrap();
            assert_eq!(
                cache.get("k", 110).unwrap().unwrap().value["public"],
                "snippet"
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}
