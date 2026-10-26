//! Prepared image bytes are loaded on demand, never with the conversation's text context.
use crate::{Store, StoreError};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{fmt, sync::Arc};

pub const MAX_IMAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_IMAGE_SIDE: u32 = 1600;

pub(super) const MIGRATION: &str = "
BEGIN;
CREATE TABLE turn_images (
    turn_id TEXT PRIMARY KEY REFERENCES turns(id) ON DELETE CASCADE,
    width INTEGER NOT NULL CHECK(width BETWEEN 1 AND 1600),
    height INTEGER NOT NULL CHECK(height BETWEEN 1 AND 1600),
    sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
    jpeg BLOB NOT NULL CHECK(length(jpeg) BETWEEN 4 AND 2097152)
);
PRAGMA user_version = 6;
COMMIT;";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImageInfo {
    pub mime_type: String,
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Clone, PartialEq, Eq)]
pub struct ImageRecord {
    info: ImageInfo,
    jpeg: Arc<[u8]>,
}

impl fmt::Debug for ImageRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ImageRecord")
            .field("info", &self.info)
            .finish()
    }
}

impl ImageRecord {
    /// Store only an already prepared JPEG. Decoding and re-encoding belong to the input boundary.
    pub fn new(jpeg: Vec<u8>, width: u32, height: u32) -> Result<Self, StoreError> {
        if jpeg.len() > MAX_IMAGE_BYTES
            || !jpeg.starts_with(&[0xff, 0xd8])
            || !jpeg.ends_with(&[0xff, 0xd9])
            || !(1..=MAX_IMAGE_SIDE).contains(&width)
            || !(1..=MAX_IMAGE_SIDE).contains(&height)
        {
            return Err(StoreError::InvalidQuery);
        }
        Ok(Self {
            info: ImageInfo {
                mime_type: "image/jpeg".into(),
                width,
                height,
                bytes: jpeg.len(),
                sha256: format!("{:x}", Sha256::digest(&jpeg)),
            },
            jpeg: jpeg.into(),
        })
    }

    pub fn info(&self) -> &ImageInfo {
        &self.info
    }

    pub fn bytes(&self) -> &[u8] {
        &self.jpeg
    }
}

pub(super) fn insert(
    tx: &Transaction<'_>,
    turn_id: &str,
    image: &ImageRecord,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO turn_images (turn_id, width, height, sha256, jpeg) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![turn_id, image.info.width, image.info.height, image.info.sha256, image.bytes()],
    )?;
    Ok(())
}

pub(super) fn info(conn: &Connection, turn_id: &str) -> Result<Option<ImageInfo>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT width, height, length(jpeg), sha256 FROM turn_images WHERE turn_id = ?1",
            params![turn_id],
            |row| {
                Ok(ImageInfo {
                    mime_type: "image/jpeg".into(),
                    width: row.get(0)?,
                    height: row.get(1)?,
                    bytes: row.get::<_, u32>(2)? as usize,
                    sha256: row.get(3)?,
                })
            },
        )
        .optional()?)
}

impl Store {
    pub fn image(
        &self,
        conversation_id: &str,
        turn_id: &str,
    ) -> Result<Option<ImageRecord>, StoreError> {
        let stored = self
            .conn
            .query_row(
                "SELECT i.jpeg, i.width, i.height, i.sha256
                 FROM turn_images i JOIN turns t ON t.id = i.turn_id
                 WHERE t.conversation_id = ?1 AND i.turn_id = ?2",
                params![conversation_id, turn_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, u32>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        stored
            .map(|(jpeg, width, height, sha256)| {
                let image =
                    ImageRecord::new(jpeg, width, height).map_err(|_| StoreError::Corrupt)?;
                if image.info.sha256 != sha256 {
                    return Err(StoreError::Corrupt);
                }
                Ok(image)
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> ImageRecord {
        ImageRecord::new(vec![0xff, 0xd8, 1, 2, 3, 0xff, 0xd9], 1, 1).unwrap()
    }

    fn insert_image(store: &mut Store, now: u64) -> String {
        let conversation = store.create("A screenshot", now).unwrap();
        store
            .add_turn_with_image(
                &conversation,
                "turn-image",
                "Question",
                now,
                &json!({}),
                Some(&fixture()),
            )
            .unwrap();
        conversation
    }

    #[test]
    fn image_bytes_are_separate_from_the_text_history_and_checked_on_read() {
        let mut store = Store::open_in_memory().unwrap();
        let id = insert_image(&mut store, 5);
        let conversation = store.get(&id).unwrap().unwrap();
        assert_eq!(conversation.turns[0].image.as_ref(), Some(fixture().info()));
        let json = serde_json::to_string(&conversation).unwrap();
        assert!(!json.contains("base64"));
        assert!(!json.contains("\"jpeg\""));
        assert_eq!(store.image(&id, "turn-image").unwrap(), Some(fixture()));
        assert_eq!(
            store.image("another-conversation", "turn-image").unwrap(),
            None
        );
        store
            .conn
            .execute(
                "UPDATE turn_images SET sha256 = ?1",
                params!["0".repeat(64)],
            )
            .unwrap();
        assert!(matches!(
            store.image(&id, "turn-image"),
            Err(StoreError::Corrupt)
        ));
    }

    #[test]
    fn deletion_clear_and_retention_remove_the_image_rows() {
        for operation in ["delete", "clear", "prune"] {
            let mut store = Store::open_in_memory().unwrap();
            let id = insert_image(&mut store, 5);
            match operation {
                "delete" => assert!(store.delete(&id).unwrap()),
                "clear" => assert_eq!(store.clear().unwrap(), 1),
                _ => assert_eq!(store.prune(6).unwrap(), 1),
            }
            assert_eq!(store.image(&id, "turn-image").unwrap(), None);
            let count: i64 = store
                .conn
                .query_row("SELECT COUNT(*) FROM turn_images", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0);
            assert_eq!(
                store
                    .add_turn_with_image(&id, "turn-late", "Late", 9, &json!({}), Some(&fixture()))
                    .unwrap(),
                None
            );
        }
    }

    #[test]
    fn attachment_failure_rolls_back_the_whole_turn() {
        let mut store = Store::open_in_memory().unwrap();
        let id = store.create("Question", 5).unwrap();
        store.conn.execute_batch(
            "CREATE TRIGGER reject_image BEFORE INSERT ON turn_images BEGIN SELECT RAISE(ABORT, 'test'); END;"
        ).unwrap();
        assert!(store
            .add_turn_with_image(
                &id,
                "turn-failed",
                "Question",
                9,
                &json!({}),
                Some(&fixture())
            )
            .is_err());
        let conversation = store.get(&id).unwrap().unwrap();
        assert!(conversation.turns.is_empty());
        assert_eq!(conversation.summary.updated_at_unix_ms, 5);
    }
}
