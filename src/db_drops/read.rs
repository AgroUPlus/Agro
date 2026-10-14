//! Reading drops: inbox, sent, conversations.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// What was sent to `user`, newest first.
    ///
    /// Archived drops are excluded rather than flagged: archiving is how a recipient says they are
    /// done with one, and an inbox that keeps showing them is not an inbox.
    pub fn inbox(&self, user: &str, limit: i64, offset: i64) -> Result<Vec<Drop>> {
        let conn = self.conn.lock_recover();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DROP_COLUMNS} FROM track_drops
              WHERE to_user = ?1 COLLATE NOCASE AND archived = 0
              ORDER BY created_at DESC
              LIMIT ?2 OFFSET ?3"
        ))?;
        let mut drops = stmt
            .query_map(params![user.trim(), limit, offset], drop_from_row)?
            .collect::<Result<Vec<_>>>()?;
        load_note_ciphertexts(&conn, &mut drops)?;
        Ok(drops)
    }

    /// What `user` has sent, newest first. Their own record of it, not the recipient's.
    pub fn sent_drops(&self, user: &str, limit: i64, offset: i64) -> Result<Vec<Drop>> {
        let conn = self.conn.lock_recover();
        let mut stmt = conn.prepare(&format!(
            "SELECT {DROP_COLUMNS} FROM track_drops
              WHERE from_user = ?1 COLLATE NOCASE
              ORDER BY created_at DESC
              LIMIT ?2 OFFSET ?3"
        ))?;
        let mut drops = stmt
            .query_map(params![user.trim(), limit, offset], drop_from_row)?
            .collect::<Result<Vec<_>>>()?;
        load_note_ciphertexts(&conn, &mut drops)?;
        Ok(drops)
    }

    /// How many unread, unarchived drops are waiting. The number a badge shows.
    pub fn unread_drop_count(&self, user: &str) -> Result<i64> {
        let conn = self.conn.lock_recover();
        conn.query_row(
            "SELECT COUNT(*) FROM track_drops
              WHERE to_user = ?1 COLLATE NOCASE AND archived = 0 AND read_at IS NULL",
            params![user.trim()],
            |row| row.get(0),
        )
    }

    /// One drop, but only if it was addressed to `user`.
    pub fn drop_for(&self, user: &str, id: &str) -> Result<Option<Drop>> {
        let conn = self.conn.lock_recover();
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

    /// One drop from either side of it — the sender's copy or the recipient's.
    ///
    /// [`drop_for`] is deliberately recipient-only, because everything it guards (reading,
    /// archiving) belongs to the recipient. A conversation view is the other case: both people
    /// are looking at the same exchange, and the sender has to be able to load a message they
    /// sent.
    pub fn drop_for_party(&self, user: &str, id: &str) -> Result<Option<Drop>> {
        let conn = self.conn.lock_recover();
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
        let conn = self.conn.lock_recover();
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

    /// How many drops `from` has sent `to` since `since`. Feeds the send rate limit, which exists
    /// so that "a friend may hand you a song" does not also mean "a friend may fill your inbox".
    pub fn drops_sent_since(&self, from: &str, to: &str, since: &str) -> Result<i64> {
        let conn = self.read();
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
