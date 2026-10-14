//! What a jam is playing, and skip votes.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// A skip needs more than half the room, counting everybody — the person who suggested the
    /// track included. Wanting it gone is not the same act as having wanted it in.
    pub fn jam_skips_needed(&self, jam_id: &str) -> Result<i64> {
        let members = self.jam_members(jam_id)?.len() as i64;
        Ok(members / 2 + 1)
    }

    /// Records a skip vote for the playing track, and says whether the room has had enough.
    pub fn vote_skip(&self, jam_id: &str, track_id: &str, username: &str) -> Result<bool> {
        let needed = self.jam_skips_needed(jam_id)?;
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT OR IGNORE INTO jam_skips (jam_id, track_id, username) VALUES (?1, ?2, ?3)",
            params![jam_id, track_id, username.trim().to_lowercase()],
        )?;
        let votes: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jam_skips WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;
        Ok(votes >= needed)
    }

    /// Skip votes on one track, and whether this account is among them.
    pub fn jam_skip_state(&self, track_id: &str, viewer: &str) -> Result<(i64, bool)> {
        let conn = self.read();
        let votes: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jam_skips WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;
        let mine: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jam_skips WHERE track_id = ?1 AND username = ?2 COLLATE NOCASE",
            params![track_id, viewer.trim()],
            |row| row.get(0),
        )?;
        Ok((votes, mine > 0))
    }

    /// What the room is hearing, with the position worked out from when it started.
    pub fn jam_now_playing(&self, jam: &Jam) -> Result<Option<JamNowPlaying>> {
        let (Some(track_id), Some(started_at)) = (&jam.now_playing_id, &jam.started_at) else {
            return Ok(None);
        };
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT title, artist, artwork_url, duration_ms, is_live, added_by, added_by_device,
                    content_hash
               FROM jam_tracks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![track_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)? != 0,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
            ))
        })?;
        let Some((
            title,
            artist,
            artwork_url,
            duration_ms,
            is_live,
            added_by,
            added_by_device,
            content_hash,
        )) = rows.next().transpose()?
        else {
            return Ok(None);
        };
        Ok(Some(JamNowPlaying {
            track_id: track_id.clone(),
            title,
            artist,
            added_by,
            added_by_device,
            content_hash,
            artwork_url,
            duration_ms,
            is_live,
            started_at: started_at.clone(),
            position_ms: elapsed_ms(started_at),
        }))
    }

    pub fn set_jam_now_playing(&self, jam_id: &str, track_id: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE jams SET now_playing_id = ?2, started_at = ?3 WHERE id = ?1",
            params![jam_id, track_id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn clear_jam_now_playing(&self, jam_id: &str) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE jams SET now_playing_id = NULL, started_at = NULL WHERE id = ?1",
            params![jam_id],
        )?;
        Ok(())
    }

    pub fn mark_jam_track_played(&self, jam_id: &str, track_id: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        let changed = conn.execute(
            "UPDATE jam_tracks SET state = 'played', played = 1 WHERE id = ?1 AND jam_id = ?2",
            params![track_id, jam_id],
        )?;
        Ok(changed > 0)
    }
}
