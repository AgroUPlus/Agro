//! Creating, joining and ending jams.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// Opens a jam and puts its creator in it.
    ///
    /// Both in one transaction: a jam whose host is not a member is a session nobody can act in,
    /// including the person who just made it.
    pub fn create_jam(&self, host: &str, mode: JamMode) -> Result<Jam> {
        let id = uuid::Uuid::new_v4().to_string();
        let code = generate_jam_code();
        let now = chrono::Utc::now().to_rfc3339();
        let host = host.trim().to_lowercase();

        let mut conn = self.conn.lock_recover();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO jams (id, code, host, mode, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, code, host, mode.as_str(), now],
        )?;
        tx.execute(
            "INSERT INTO jam_members (jam_id, username, joined_at) VALUES (?1, ?2, ?3)",
            params![id, host, now],
        )?;
        tx.commit()?;

        Ok(Jam {
            id,
            code,
            host,
            mode,
            created_at: now,
            now_playing_id: None,
            started_at: None,
            visibility: JamVisibility::Code,
        })
    }

    /// The live jam this account is in, if any. One at a time.
    pub fn jam_for_member(&self, username: &str) -> Result<Option<Jam>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams j
              WHERE j.ended_at IS NULL
                AND EXISTS (SELECT 1 FROM jam_members m
                            WHERE m.jam_id = j.id AND m.username = ?1 COLLATE NOCASE)
              ORDER BY j.created_at DESC LIMIT 1"
        ))?;
        let mut rows = stmt.query_map(params![username.trim()], jam_from_row)?;
        rows.next().transpose()
    }

    pub fn jam_by_code(&self, code: &str) -> Result<Option<Jam>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams WHERE code = ?1 COLLATE NOCASE AND ended_at IS NULL"
        ))?;
        let mut rows = stmt.query_map(params![code.trim()], jam_from_row)?;
        rows.next().transpose()
    }

    pub fn jam_by_id(&self, id: &str) -> Result<Option<Jam>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams WHERE id = ?1 AND ended_at IS NULL"
        ))?;
        let mut rows = stmt.query_map(params![id.trim()], jam_from_row)?;
        rows.next().transpose()
    }

    /// Adds someone to a jam. Leaving and rejoining is not an error.
    pub fn join_jam(&self, jam_id: &str, username: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT OR IGNORE INTO jam_members (jam_id, username, joined_at) VALUES (?1, ?2, ?3)",
            params![
                jam_id,
                username.trim().to_lowercase(),
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn leave_jam(&self, jam_id: &str, username: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let removed = conn.execute(
            "DELETE FROM jam_members WHERE jam_id = ?1 AND username = ?2 COLLATE NOCASE",
            params![jam_id, username.trim()],
        )?;
        Ok(removed > 0)
    }

    pub fn is_jam_member(&self, jam_id: &str, username: &str) -> Result<bool> {
        let conn = self.read();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jam_members WHERE jam_id = ?1 AND username = ?2 COLLATE NOCASE",
            params![jam_id, username.trim()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn jam_members(&self, jam_id: &str) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT username FROM jam_members WHERE jam_id = ?1 ORDER BY joined_at")?;
        let rows = stmt.query_map(params![jam_id], |row| row.get(0))?;
        rows.collect()
    }

    pub fn set_jam_mode(&self, jam_id: &str, mode: JamMode) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE jams SET mode = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![jam_id, mode.as_str()],
        )?;
        Ok(changed > 0)
    }

    /// Ends a jam without deleting it, so its history stays readable.
    pub fn end_jam(&self, jam_id: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE jams SET ended_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![jam_id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(changed > 0)
    }

    /// Removes a jam and everything belonging to it.
    ///
    /// A jam is a room, not a document: once it is over there is nothing worth keeping, and leaving
    /// the rows behind means every member's client has to keep deciding whether a dead jam counts.
    pub fn delete_jam(&self, jam_id: &str) -> Result<()> {
        let mut conn = self.conn.lock_recover();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM jam_votes WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jam_skips WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jam_tracks WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jam_members WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jams WHERE id = ?1", params![jam_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Opens a jam to friends, or shuts it back to code-only.
    pub fn set_jam_visibility(&self, jam_id: &str, visibility: JamVisibility) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE jams SET visibility = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![jam_id, visibility.as_str()],
        )?;
        Ok(changed > 0)
    }

    /// Live jams run by this account's accepted friends that they have opened up.
    ///
    /// Friendship *and* the switch, both. Being someone's friend is not consent to be pulled into
    /// their listening — it is the same rule the rest of the social surface follows.
    pub fn friend_jams(&self, username: &str) -> Result<Vec<Jam>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams j
              WHERE j.ended_at IS NULL
                AND j.visibility = 'friends'
                AND j.host <> ?1 COLLATE NOCASE
                AND EXISTS (SELECT 1 FROM friendships f
                            WHERE f.user_id = ?1 COLLATE NOCASE
                              AND f.friend_id = j.host COLLATE NOCASE
                              AND f.state = 'accepted')
              ORDER BY j.created_at DESC"
        ))?;
        let rows = stmt.query_map(params![username.trim()], jam_from_row)?;
        rows.collect()
    }

    /// Every jam still running, for the clock to look at.
    pub fn live_jams(&self) -> Result<Vec<Jam>> {
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams WHERE ended_at IS NULL"
        ))?;
        let rows = stmt.query_map([], jam_from_row)?;
        rows.collect()
    }
}
