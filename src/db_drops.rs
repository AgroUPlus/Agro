//! Songs handed from one account to another.
//!
//! Its own `impl Db` block for the same reason `db_social` and `db_library` have theirs: `db.rs` is
//! already long enough without unrelated SQL in it.
//!
//! A drop is a *message that happens to be about a track*. It therefore stores the track as a
//! description rather than as a reference into the library index — the sender may not have the file
//! on this server at all, and a foreign key that only sometimes resolves would mean an inbox whose
//! rows quietly stop rendering. `content_hash` and `track_uri` ride along when the sender has them,
//! so the recipient can be offered the file rather than only the name, but neither is required.
//!
//! Nothing here is scoped by friendship. That check belongs at the API boundary, where sending
//! happens; once a drop exists it belongs to the person who received it, and withdrawing a
//! friendship does not reach back into their inbox to remove it. Every read and write below is
//! scoped to one account *inside the statement*, so a row belonging to somebody else is a
//! not-found rather than a refusal — the same shape `delete_link` uses.

use rusqlite::{params, OptionalExtension, Result};

use crate::db::Db;

/// A drop as it is about to be created. The recipient is passed separately, because it is the one
/// field the caller has to have authorised.
#[derive(Clone, Debug, Default)]
pub struct NewDrop {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    /// Present when the sender's copy is in this server's index. Lets the recipient be offered the
    /// bytes rather than only the name.
    pub content_hash: Option<String>,
    /// The sender's namespaced identifier for the track, e.g. `navidrome:<id>`. Meaningful to a
    /// client that shares the same backend, and inert to one that does not.
    pub track_uri: Option<String>,
    /// What they said about it. Optional — handing someone a song without comment is a complete
    /// thought.
    pub note: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Drop {
    pub id: String,
    pub from_user: String,
    pub to_user: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub content_hash: Option<String>,
    pub track_uri: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    /// When the recipient first read it, or `None` while it is still unread.
    pub read_at: Option<String>,
    pub archived: bool,
}

const DROP_COLUMNS: &str = "id, from_user, to_user, track_title, artist_name, album_name, \
                            artwork_url, content_hash, track_uri, note, created_at, read_at, archived";

fn drop_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Drop> {
    Ok(Drop {
        id: row.get(0)?,
        from_user: row.get(1)?,
        to_user: row.get(2)?,
        track_title: row.get(3)?,
        artist_name: row.get(4)?,
        album_name: row.get(5)?,
        artwork_url: row.get(6)?,
        content_hash: row.get(7)?,
        track_uri: row.get(8)?,
        note: row.get(9)?,
        created_at: row.get(10)?,
        read_at: row.get(11)?,
        archived: row.get::<_, i64>(12)? != 0,
    })
}

impl Db {
    /// Records a drop and answers with its id.
    ///
    /// Deliberately does not check that either account exists or that they are friends. Both are
    /// the caller's job, and doing them here as well would mean two places that have to agree about
    /// what a permitted drop is.
    pub fn create_drop(&self, from: &str, to: &str, drop: &NewDrop) -> Result<String> {
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO track_drops
                 (id, from_user, to_user, track_title, artist_name, album_name, artwork_url,
                  content_hash, track_uri, note, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
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
                now,
            ],
        )?;
        Ok(id)
    }

    /// What was sent to `user`, newest first.
    ///
    /// Archived drops are excluded rather than flagged: archiving is how a recipient says they are
    /// done with one, and an inbox that keeps showing them is not an inbox.
    pub fn inbox(&self, user: &str, limit: i64, offset: i64) -> Result<Vec<Drop>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DROP_COLUMNS} FROM track_drops
              WHERE to_user = ?1 COLLATE NOCASE AND archived = 0
              ORDER BY created_at DESC
              LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(params![user.trim(), limit, offset], drop_from_row)?;
        rows.collect()
    }

    /// What `user` has sent, newest first. Their own record of it, not the recipient's.
    pub fn sent_drops(&self, user: &str, limit: i64, offset: i64) -> Result<Vec<Drop>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DROP_COLUMNS} FROM track_drops
              WHERE from_user = ?1 COLLATE NOCASE
              ORDER BY created_at DESC
              LIMIT ?2 OFFSET ?3"
        ))?;
        let rows = stmt.query_map(params![user.trim(), limit, offset], drop_from_row)?;
        rows.collect()
    }

    /// How many unread, unarchived drops are waiting. The number a badge shows.
    pub fn unread_drop_count(&self, user: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM track_drops
              WHERE to_user = ?1 COLLATE NOCASE AND archived = 0 AND read_at IS NULL",
            params![user.trim()],
            |row| row.get(0),
        )
    }

    /// One drop, but only if it was addressed to `user`.
    pub fn drop_for(&self, user: &str, id: &str) -> Result<Option<Drop>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {DROP_COLUMNS} FROM track_drops
                  WHERE id = ?1 AND to_user = ?2 COLLATE NOCASE"
            ),
            params![id, user.trim()],
            drop_from_row,
        )
        .optional()
    }

    /// Stamps a drop read. Idempotent: the first read is the one that counts, so a client that
    /// re-opens an item does not keep moving the timestamp forward.
    pub fn mark_drop_read(&self, user: &str, id: &str) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE track_drops SET archived = 1
              WHERE id = ?1 AND to_user = ?2 COLLATE NOCASE",
            params![id, user.trim()],
        )?;
        Ok(changed > 0)
    }

    /// How many drops `from` has sent `to` since `since`. Feeds the send rate limit, which exists
    /// so that "a friend may hand you a song" does not also mean "a friend may fill your inbox".
    pub fn drops_sent_since(&self, from: &str, to: &str, since: &str) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM track_drops
              WHERE from_user = ?1 COLLATE NOCASE
                AND to_user = ?2 COLLATE NOCASE
                AND created_at >= ?3",
            params![from.trim(), to.trim(), since],
            |row| row.get(0),
        )
    }
}
