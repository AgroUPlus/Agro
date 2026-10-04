//! Jam recaps at rest: read out of a jam before it goes, kept per member until dismissed.
//!
//! Written by [`Db::record_jam_recap`] as each member leaves — including every member left in the
//! room when its host ends it — because that is the last moment the jam's rows are guaranteed to
//! exist. The retention sweep removes any recap nobody dismissed after `JAM_RECAP_TTL_DAYS`.

use rusqlite::{params, Result};

use crate::db::Db;
use crate::db_jam::Jam;
use crate::jam_recap::{JamRecap, RecapTrack};

/// One stored recap, as its owner reads it back.
#[derive(Clone, Debug)]
pub struct StoredJamRecap {
    pub id: String,
    pub created_at: String,
    pub recap: JamRecap,
}

impl Db {
    /// What the room has played so far, in order, with the votes each track drew.
    ///
    /// The track on air counts: someone leaving halfway through a song heard it. Order is by
    /// `added_at`, which is play order — the clock always takes the earliest queued track.
    pub fn jam_played_tracks(&self, jam: &Jam) -> Result<Vec<RecapTrack>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT t.title, t.artist, t.artwork_url, t.track_uri, t.added_by, t.duration_ms,
                    (SELECT COUNT(*) FROM jam_votes v
                      WHERE v.track_id = t.id AND v.username <> t.added_by COLLATE NOCASE),
                    (SELECT COUNT(*) FROM jam_skips s WHERE s.track_id = t.id)
               FROM jam_tracks t
              WHERE t.jam_id = ?1 AND (t.state = 'played' OR t.id = ?2)
              ORDER BY t.added_at ASC",
        )?;
        let rows = stmt.query_map(params![jam.id, jam.now_playing_id], |row| {
            Ok(RecapTrack {
                title: row.get(0)?,
                artist: row.get(1)?,
                artwork_url: row.get(2)?,
                track_uri: row.get(3)?,
                added_by: row.get(4)?,
                duration_ms: row.get(5)?,
                approvals: row.get(6)?,
                skip_votes: row.get(7)?,
            })
        })?;
        rows.collect()
    }

    /// Summarises `jam` for `member` and keeps it, returning the new recap's id.
    ///
    /// `None` when the jam had nothing worth a recap (see [`JamRecap::build`]); nothing is stored.
    pub fn record_jam_recap(&self, jam: &Jam, member: &str) -> Result<Option<String>> {
        let members = self.jam_members(&jam.id)?;
        let played = self.jam_played_tracks(jam)?;
        let now = chrono::Utc::now().to_rfc3339();
        let Some(recap) = JamRecap::build(&jam.created_at, &now, &members, played) else {
            return Ok(None);
        };
        let payload = serde_json::to_string(&recap)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;

        let id = uuid::Uuid::new_v4().to_string();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO jam_recaps (id, username, payload_json, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![id, member.trim().to_lowercase(), payload, now],
        )?;
        Ok(Some(id))
    }

    /// This account's recaps, newest first.
    ///
    /// A row whose payload no longer decodes is reported as an error rather than skipped: it was
    /// written by this server, so failing to read it is a bug to see, not a recap to hide.
    pub fn jam_recaps(&self, username: &str) -> Result<Vec<StoredJamRecap>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT id, created_at, payload_json FROM jam_recaps
              WHERE username = ?1 COLLATE NOCASE
              ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map(params![username.trim()], |row| {
            let payload: String = row.get(2)?;
            let recap = serde_json::from_str(&payload).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(StoredJamRecap {
                id: row.get(0)?,
                created_at: row.get(1)?,
                recap,
            })
        })?;
        rows.collect()
    }

    /// Removes one of this account's recaps. `false` when it was not theirs or already gone,
    /// which reads the same on purpose: an id is not a way to learn whose recaps exist.
    pub fn dismiss_jam_recap(&self, username: &str, id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM jam_recaps WHERE id = ?1 AND username = ?2 COLLATE NOCASE",
            params![id.trim(), username.trim()],
        )?;
        Ok(removed > 0)
    }
}

/// Strikes a deleted account from every recap that names it, inside the deletion's transaction.
///
/// Only rows that can mention the name are read: `instr` looks for it as a JSON string — quoted,
/// so `sam` does not match `samantha` — in SQL, and only those few rows are decoded and rewritten.
/// A row that matched but does not decode fails the whole deletion rather than keep the mention.
pub(crate) fn forget_in_jam_recaps(conn: &rusqlite::Connection, username: &str) -> Result<()> {
    let name = username.trim().to_lowercase();
    let needle = serde_json::Value::String(name.clone()).to_string();
    let candidates: Vec<(String, String)> = conn
        .prepare("SELECT id, payload_json FROM jam_recaps WHERE instr(payload_json, ?1) > 0")?
        .query_map(params![needle], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_>>()?;

    for (id, payload) in candidates {
        let mut recap: JamRecap = serde_json::from_str(&payload).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(1, rusqlite::types::Type::Text, Box::new(e))
        })?;
        if recap.forget(&name) {
            let rewritten = serde_json::to_string(&recap)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            conn.execute(
                "UPDATE jam_recaps SET payload_json = ?1 WHERE id = ?2",
                params![rewritten, id],
            )?;
        }
    }
    Ok(())
}
