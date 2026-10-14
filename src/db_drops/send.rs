//! Creating, reading, archiving and reacting to drops.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// Records a drop and answers with its id.
    ///
    /// Deliberately does not check that either account exists or that they are friends. Both are
    /// the caller's job, and doing them here as well would mean two places that have to agree about
    /// what a permitted drop is.
    pub fn create_drop(&self, from: &str, to: &str, drop: &NewDrop) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT INTO track_drops
                 (id, from_user, to_user, track_title, artist_name, album_name, artwork_url,
                  content_hash, track_uri, note, note_ciphertext, is_encrypted, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                from.trim().to_lowercase(),
                to.trim().to_lowercase(),
                drop.track_title.trim(),
                drop.artist_name.trim(),
                drop.album_name.as_deref().map(str::trim),
                drop.artwork_url.as_deref().map(str::trim),
                drop.content_hash.as_deref().map(str::trim),
                drop.track_uri.as_deref().map(str::trim),
                drop.note.as_deref().map(str::trim),
                drop.note_ciphertext.as_deref().map(str::trim),
                drop.is_encrypted as i64,
                now,
            ],
        )?;
        // Written in the same lock as the drop itself: a drop whose sealed copies did not land is
        // a message nobody can open, which is worse than one that was never recorded.
        for copy in &drop.note_ciphertexts {
            conn.execute(
                "INSERT OR REPLACE INTO drop_note_ciphertexts (drop_id, device_id, ciphertext)
                      VALUES (?1, ?2, ?3)",
                params![id, copy.device_id.trim(), copy.ciphertext.trim()],
            )?;
        }
        Ok(id)
    }

    /// Stamps a drop read. Idempotent: the first read is the one that counts, so a client that
    /// re-opens an item does not keep moving the timestamp forward.
    pub fn mark_drop_read(&self, user: &str, id: &str) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE track_drops SET read_at = ?1
              WHERE id = ?2 AND to_user = ?3 COLLATE NOCASE AND read_at IS NULL",
            params![now, id, user.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Takes a drop out of the inbox without deleting it, so the sender's record of having sent it
    /// survives. Only the recipient may do this.
    pub fn archive_drop(&self, user: &str, id: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE track_drops SET archived = 1
              WHERE id = ?1 AND to_user = ?2 COLLATE NOCASE",
            params![id, user.trim()],
        )?;
        Ok(changed > 0)
    }

    /// Sets, replaces or clears the recipient's reaction to a drop.
    ///
    /// Only the recipient may react, which is why this is scoped to `to_user`: reacting to your
    /// own message is not a thing, and allowing it would let a sender fabricate a response.
    /// Passing `None` removes the reaction, so tapping the same emoji twice can undo it.
    pub fn react_to_drop(&self, user: &str, id: &str, reaction: Option<&str>) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE track_drops SET reaction = ?1
              WHERE id = ?2 AND to_user = ?3 COLLATE NOCASE",
            params![reaction.map(str::trim), id, user.trim()],
        )?;
        Ok(changed > 0)
    }
}
