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

use rusqlite::{params, params_from_iter, OptionalExtension, Result};

use crate::db::Db;

/// One sealed copy of a note, and the device key it was sealed to.
///
/// A note is sealed once per device in the recipient's registry *and* once to the sender's own
/// device, which is what makes a sent note readable by the person who sent it. Before this existed
/// a drop carried exactly one ciphertext, sealed to the recipient alone, and the sender's own copy
/// of their message was mathematically unopenable.
#[derive(Clone, Debug)]
pub struct DeviceCiphertext {
    pub device_id: String,
    pub ciphertext: String,
}

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
    /// The recipient's copy, duplicated out of [`note_ciphertexts`](Self::note_ciphertexts).
    ///
    /// Kept only for clients that predate the per-device list; nothing new should read it.
    pub note_ciphertext: Option<String>,
    /// One sealed copy per device key, including the sender's own.
    pub note_ciphertexts: Vec<DeviceCiphertext>,
    pub is_encrypted: bool,
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
    /// The recipient's copy. Older clients read this and nothing else.
    pub note_ciphertext: Option<String>,
    /// Every sealed copy, by device. Empty for a drop sent before the registry existed.
    pub note_ciphertexts: Vec<DeviceCiphertext>,
    pub is_encrypted: bool,
    pub created_at: String,
    /// When the recipient first read it, or `None` while it is still unread.
    pub read_at: Option<String>,
    pub archived: bool,
    /// The recipient's one-emoji reply, or `None` if they have not reacted.
    ///
    /// Unlike [`read_at`], this is shown to the sender. A read receipt is something the server
    /// observed; a reaction is something the recipient chose to send, and withholding it would
    /// make the feature pointless.
    pub reaction: Option<String>,
}

const DROP_COLUMNS: &str = "id, from_user, to_user, track_title, artist_name, album_name, \
                            artwork_url, content_hash, track_uri, note, note_ciphertext, \
                            is_encrypted, created_at, read_at, archived, reaction";

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
        note_ciphertext: row.get(10)?,
        // Filled by `load_note_ciphertexts`; the column list above does not reach the other table.
        note_ciphertexts: Vec::new(),
        is_encrypted: row.get::<_, i64>(11)? != 0,
        created_at: row.get(12)?,
        read_at: row.get(13)?,
        archived: row.get::<_, i64>(14)? != 0,
        reaction: row.get(15)?,
    })
}

/// Attaches each drop's per-device sealed copies.
///
/// A second statement rather than a join: joining would multiply every drop row by the number of
/// devices it was sealed to, and [`drop_from_row`] would then have to collapse what SQL had just
/// duplicated. One extra query per page is the cheaper shape.
fn load_note_ciphertexts(conn: &rusqlite::Connection, drops: &mut [Drop]) -> Result<()> {
    if drops.is_empty() {
        return Ok(());
    }
    let placeholders = vec!["?"; drops.len()].join(",");
    let mut stmt = conn.prepare(&format!(
        "SELECT drop_id, device_id, ciphertext FROM drop_note_ciphertexts
          WHERE drop_id IN ({placeholders})
          ORDER BY device_id ASC"
    ))?;
    let sealed = stmt
        .query_map(params_from_iter(drops.iter().map(|d| d.id.clone())), |row| {
            Ok((
                row.get::<_, String>(0)?,
                DeviceCiphertext {
                    device_id: row.get(1)?,
                    ciphertext: row.get(2)?,
                },
            ))
        })?
        .collect::<Result<Vec<_>>>()?;

    for (drop_id, copy) in sealed {
        if let Some(drop) = drops.iter_mut().find(|d| d.id == drop_id) {
            drop.note_ciphertexts.push(copy);
        }
    }
    Ok(())
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
        let mut drops = stmt.query_map(params![user.trim(), limit, offset], drop_from_row)?
            .collect::<Result<Vec<_>>>()?;
        load_note_ciphertexts(&conn, &mut drops)?;
        Ok(drops)
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
        let mut drops = stmt.query_map(params![user.trim(), limit, offset], drop_from_row)?
            .collect::<Result<Vec<_>>>()?;
        load_note_ciphertexts(&conn, &mut drops)?;
        Ok(drops)
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
        .optional()?
        .map(|drop| {
            let mut one = [drop];
            load_note_ciphertexts(&conn, &mut one)?;
            let [drop] = one;
            Ok(drop)
        })
        .transpose()
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

    /// One drop from either side of it — the sender's copy or the recipient's.
    ///
    /// [`drop_for`] is deliberately recipient-only, because everything it guards (reading,
    /// archiving) belongs to the recipient. A conversation view is the other case: both people
    /// are looking at the same exchange, and the sender has to be able to load a message they
    /// sent.
    pub fn drop_for_party(&self, user: &str, id: &str) -> Result<Option<Drop>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            &format!(
                "SELECT {DROP_COLUMNS} FROM track_drops
                  WHERE id = ?1
                    AND (to_user = ?2 COLLATE NOCASE OR from_user = ?2 COLLATE NOCASE)"
            ),
            params![id, user.trim()],
            drop_from_row,
        )
        .optional()?
        .map(|drop| {
            let mut one = [drop];
            load_note_ciphertexts(&conn, &mut one)?;
            let [drop] = one;
            Ok(drop)
        })
        .transpose()
    }

    /// The whole exchange between two accounts, oldest first, both directions in one list.
    ///
    /// Ordered ascending because this is read as a conversation rather than as an inbox: a thread
    /// is followed downwards, and the newest message belongs at the bottom.
    ///
    /// Archived drops are included. Archiving takes something out of the *inbox*, which is a queue
    /// of things to deal with; a conversation is a record, and silently dropping half of it would
    /// make the history lie.
    pub fn conversation(&self, user: &str, other: &str, limit: i64) -> Result<Vec<Drop>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DROP_COLUMNS} FROM track_drops
              WHERE (from_user = ?1 COLLATE NOCASE AND to_user = ?2 COLLATE NOCASE)
                 OR (from_user = ?2 COLLATE NOCASE AND to_user = ?1 COLLATE NOCASE)
              ORDER BY created_at ASC
              LIMIT ?3"
        ))?;
        let mut drops = stmt
            .query_map(params![user.trim(), other.trim(), limit], drop_from_row)?
            .collect::<Result<Vec<_>>>()?;
        load_note_ciphertexts(&conn, &mut drops)?;
        Ok(drops)
    }

    /// Sets, replaces or clears the recipient's reaction to a drop.
    ///
    /// Only the recipient may react, which is why this is scoped to `to_user`: reacting to your
    /// own message is not a thing, and allowing it would let a sender fabricate a response.
    /// Passing `None` removes the reaction, so tapping the same emoji twice can undo it.
    pub fn react_to_drop(&self, user: &str, id: &str, reaction: Option<&str>) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE track_drops SET reaction = ?1
              WHERE id = ?2 AND to_user = ?3 COLLATE NOCASE",
            params![reaction.map(str::trim), id, user.trim()],
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
