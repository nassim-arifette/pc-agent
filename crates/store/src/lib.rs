//! Local conversation history (contract C2 of docs/plan.md). One SQLite file per Windows user
//! profile keeps each conversation with its turns, as the interface showed them. Nothing here is
//! ever sent anywhere; the person can delete a conversation or everything, and old ones expire.
#![forbid(unsafe_code)]
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use thiserror::Error;

/// Schema version stored in SQLite's `user_version`; older files are migrated, newer refused.
pub const SCHEMA_VERSION: i64 = 1;
/// Longest conversation title, taken from its first question.
pub const MAX_TITLE_CHARS: usize = 80;
/// Default retention of the desktop app.
pub const DEFAULT_RETENTION_DAYS: u64 = 90;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("The history database could not be used: {0}")]
    Database(String),
    #[error("The history database was written by a newer version of PC Agent ({0})")]
    TooNew(i64),
    #[error("A stored turn is not valid JSON")]
    Corrupt,
}
impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Database(e.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConversationSummary {
    pub id: String,
    pub title: String,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
    pub turn_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StoredTurn {
    pub id: String,
    pub question: String,
    pub asked_at_unix_ms: u64,
    /// The report as the interface showed it; its shape belongs to the interface.
    pub report: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Conversation {
    #[serde(flatten)]
    pub summary: ConversationSummary,
    pub turns: Vec<StoredTurn>,
}

pub struct Store {
    conn: Connection,
}

/// A title from the first question: its first line, trimmed and shortened.
pub fn title_from(question: &str) -> String {
    let line = question
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > MAX_TITLE_CHARS {
        let cut: String = line.chars().take(MAX_TITLE_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_owned()
    }
}

fn millis(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}
fn stored(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

impl Store {
    /// Opens or creates the history file, migrating an older schema.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError::Database(e.to_string()))?;
        }
        Self::init(Connection::open(path)?)
    }
    /// A history that lives only in memory, for tests and previews.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(StoreError::TooNew(version));
        }
        if version < 1 {
            conn.execute_batch(
                "BEGIN;
                CREATE TABLE IF NOT EXISTS conversations (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS turns (
                    id TEXT PRIMARY KEY,
                    conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                    position INTEGER NOT NULL,
                    question TEXT NOT NULL,
                    asked_at INTEGER NOT NULL,
                    report TEXT NOT NULL,
                    UNIQUE (conversation_id, position)
                );
                CREATE INDEX IF NOT EXISTS conversations_by_update ON conversations(updated_at);
                PRAGMA user_version = 1;
                COMMIT;",
            )?;
        }
        Ok(Self { conn })
    }

    /// Starts a conversation titled after its first question.
    pub fn create(&self, first_question: &str, now_unix_ms: u64) -> Result<String, StoreError> {
        let id = pcagent_shared::new_id("conv");
        self.conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id, title_from(first_question), stored(now_unix_ms)],
        )?;
        Ok(id)
    }

    /// Appends a finished turn. Returns `None` when the conversation no longer exists (the person
    /// deleted it while the turn ran): the turn is then not kept.
    pub fn add_turn(
        &mut self,
        conversation_id: &str,
        question: &str,
        asked_at_unix_ms: u64,
        report: &Value,
    ) -> Result<Option<String>, StoreError> {
        let tx = self.conn.transaction()?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM conversations WHERE id = ?1",
                params![conversation_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !exists {
            return Ok(None);
        }
        let position: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position), -1) + 1 FROM turns WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;
        let id = pcagent_shared::new_id("turn");
        let text = serde_json::to_string(report).map_err(|_| StoreError::Corrupt)?;
        tx.execute(
            "INSERT INTO turns (id, conversation_id, position, question, asked_at, report)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                conversation_id,
                position,
                question,
                stored(asked_at_unix_ms),
                text
            ],
        )?;
        tx.execute(
            "UPDATE conversations SET updated_at = MAX(updated_at, ?2) WHERE id = ?1",
            params![conversation_id, stored(asked_at_unix_ms)],
        )?;
        tx.commit()?;
        Ok(Some(id))
    }

    /// The most recently updated conversations first.
    pub fn list(&self, limit: usize) -> Result<Vec<ConversationSummary>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT c.id, c.title, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM turns t WHERE t.conversation_id = c.id)
             FROM conversations c ORDER BY c.updated_at DESC, c.created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            Ok(ConversationSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at_unix_ms: millis(row.get(2)?),
                updated_at_unix_ms: millis(row.get(3)?),
                turn_count: usize::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One conversation with its turns in order.
    pub fn get(&self, id: &str) -> Result<Option<Conversation>, StoreError> {
        let Some(summary) = self.list_one(id).optional()? else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT id, question, asked_at, report FROM turns
             WHERE conversation_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut turns = Vec::new();
        for row in rows {
            let (id, question, asked_at, report) = row?;
            turns.push(StoredTurn {
                id,
                question,
                asked_at_unix_ms: millis(asked_at),
                report: serde_json::from_str(&report).map_err(|_| StoreError::Corrupt)?,
            });
        }
        Ok(Some(Conversation { summary, turns }))
    }
    fn list_one(&self, id: &str) -> rusqlite::Result<ConversationSummary> {
        self.conn.query_row(
            "SELECT c.id, c.title, c.created_at, c.updated_at,
                    (SELECT COUNT(*) FROM turns t WHERE t.conversation_id = c.id)
             FROM conversations c WHERE c.id = ?1",
            params![id],
            |row| {
                Ok(ConversationSummary {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    created_at_unix_ms: millis(row.get(2)?),
                    updated_at_unix_ms: millis(row.get(3)?),
                    turn_count: usize::try_from(row.get::<_, i64>(4)?).unwrap_or(0),
                })
            },
        )
    }

    /// Deletes one conversation and its turns.
    pub fn delete(&self, id: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn
            .execute("DELETE FROM conversations WHERE id = ?1", params![id])?
            > 0)
    }

    /// Deletes every conversation; returns how many were removed.
    pub fn clear(&self) -> Result<usize, StoreError> {
        let removed = self.conn.execute("DELETE FROM conversations", [])?;
        // Give the space back instead of keeping deleted pages in the file.
        self.conn.execute_batch("VACUUM;")?;
        Ok(removed)
    }

    /// Deletes conversations not updated since `cutoff_unix_ms`; returns how many were removed.
    pub fn prune(&self, cutoff_unix_ms: u64) -> Result<usize, StoreError> {
        Ok(self.conn.execute(
            "DELETE FROM conversations WHERE updated_at < ?1",
            params![stored(cutoff_unix_ms)],
        )?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_conversation_keeps_its_turns_in_order() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store
            .create("  Pourquoi mon PC rame ?\nDepuis ce matin", 1_000)
            .unwrap();
        for (i, q) in ["Pourquoi mon PC rame ?", "Et Chrome ?"].iter().enumerate() {
            let report = json!({"answer": {"text": format!("Réponse {i} — 2,3 Go [O1]")}});
            store.add_turn(&id, q, 2_000 + i as u64, &report).unwrap();
        }
        let c = store.get(&id).unwrap().unwrap();
        assert_eq!(c.summary.title, "Pourquoi mon PC rame ?");
        assert_eq!(c.summary.turn_count, 2);
        assert_eq!(c.summary.updated_at_unix_ms, 2_001);
        assert_eq!(c.turns[1].question, "Et Chrome ?");
        assert_eq!(
            c.turns[0].report["answer"]["text"],
            "Réponse 0 — 2,3 Go [O1]"
        );
        assert!(store.get("conv-missing").unwrap().is_none());
    }

    #[test]
    fn the_list_puts_recent_conversations_first_and_deletes_cascade() {
        let mut store = Store::open_in_memory().unwrap();
        let old = store.create("old", 1).unwrap();
        let new = store.create("new", 2).unwrap();
        store.add_turn(&old, "q", 10, &json!({})).unwrap();
        let ids: Vec<String> = store.list(10).unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(ids, [old.clone(), new.clone()]);
        assert!(store.delete(&old).unwrap());
        assert!(!store.delete(&old).unwrap());
        // A turn that finishes after its conversation was deleted is not kept.
        assert_eq!(store.add_turn(&old, "late", 11, &json!({})).unwrap(), None);
        assert_eq!(store.list(10).unwrap().len(), 1);
        assert_eq!(store.clear().unwrap(), 1);
        assert!(store.list(10).unwrap().is_empty());
    }

    #[test]
    fn old_conversations_expire() {
        let store = Store::open_in_memory().unwrap();
        store.create("kept", 5_000).unwrap();
        store.create("expired", 1_000).unwrap();
        assert_eq!(store.prune(2_000).unwrap(), 1);
        assert_eq!(store.list(10).unwrap()[0].title, "kept");
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(
            title_from("\n  Mon disque est plein  \nautre"),
            "Mon disque est plein"
        );
        let long = "é".repeat(200);
        let title = title_from(&long);
        assert_eq!(title.chars().count(), MAX_TITLE_CHARS);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn the_file_survives_reopening_and_a_newer_schema_is_refused() {
        let dir = std::env::temp_dir().join(pcagent_shared::new_id("pcagent-store-test"));
        let path = dir.join("history.sqlite");
        let id = {
            let mut store = Store::open(&path).unwrap();
            let id = store.create("persisted", 1).unwrap();
            store
                .add_turn(&id, "persisted", 2, &json!({"ok": true}))
                .unwrap();
            id
        };
        let store = Store::open(&path).unwrap();
        assert_eq!(store.get(&id).unwrap().unwrap().turns[0].report["ok"], true);
        store.conn.pragma_update(None, "user_version", 99).unwrap();
        drop(store);
        assert!(matches!(Store::open(&path), Err(StoreError::TooNew(99))));
        let _ = std::fs::remove_dir_all(dir);
    }
}
