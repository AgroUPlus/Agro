//! The jam queue and its approvals.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, Result};

impl Db {
    /// How many approvals a proposal needs: more than half of *everyone else*.
    ///
    /// The proposer is excluded deliberately — you cannot carry your own suggestion. That also
    /// makes a one-person jam unpassable, so [`add_jam_track`] queues directly when there is
    /// nobody else to ask.
    pub fn jam_approvals_needed(&self, jam_id: &str) -> Result<i64> {
        let members = self.jam_members(jam_id)?.len() as i64;
        let others = (members - 1).max(0);
        Ok(others / 2 + 1)
    }

    /// Adds a track, either straight into the queue or as a proposal for the room to accept.
    ///
    /// Returns the id and the state it landed in, so the caller can say which happened without
    /// reading it back.
    #[allow(clippy::too_many_arguments)]
    pub fn add_jam_track(
        &self,
        jam_id: &str,
        added_by: &str,
        track_uri: &str,
        title: &str,
        artist: &str,
        artwork_url: Option<&str>,
        duration_ms: i64,
        is_live: bool,
        mode: JamMode,
        added_by_device: Option<&str>,
        content_hash: Option<&str>,
    ) -> Result<(String, JamTrackState)> {
        // Alone in the room, a proposal could never reach a majority of the people who are not
        // you, because there are none. Queue it rather than leaving it permanently stuck.
        let alone = self.jam_members(jam_id)?.len() <= 1;
        let state = match mode {
            JamMode::Open => JamTrackState::Queued,
            JamMode::Democracy if alone => JamTrackState::Queued,
            JamMode::Democracy => JamTrackState::Proposed,
        };

        let id = uuid::Uuid::new_v4().to_string();
        let adder = added_by.trim().to_lowercase();
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT INTO jam_tracks
               (id, jam_id, added_by, track_uri, title, artist, artwork_url, added_at, played,
                duration_ms, state, is_live, added_by_device, content_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11, ?12, ?13)",
            params![
                id,
                jam_id,
                adder,
                track_uri.trim(),
                title.trim(),
                artist.trim(),
                artwork_url,
                chrono::Utc::now().to_rfc3339(),
                duration_ms.max(0),
                state.as_str(),
                is_live as i64,
                added_by_device.map(str::trim).filter(|d| !d.is_empty()),
                content_hash.map(str::trim).filter(|h| !h.is_empty())
            ],
        )?;
        Ok((id, state))
    }

    /// Who added a track, so "your own, or you run the room" can be applied.
    pub fn jam_track_owner(&self, jam_id: &str, track_id: &str) -> Result<Option<String>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT added_by FROM jam_tracks WHERE id = ?1 AND jam_id = ?2")?;
        let mut rows = stmt.query_map(params![track_id, jam_id], |row| row.get(0))?;
        rows.next().transpose()
    }

    pub fn remove_jam_track(&self, jam_id: &str, track_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock_recover();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM jam_votes WHERE track_id = ?1",
            params![track_id],
        )?;
        let removed = tx.execute(
            "DELETE FROM jam_tracks WHERE id = ?1 AND jam_id = ?2",
            params![track_id, jam_id],
        )?;
        tx.commit()?;
        Ok(removed > 0)
    }

    /// Records one approval and promotes the track once the room has agreed.
    ///
    /// Not a toggle. Withdrawing an approval would let a proposal that had already been accepted
    /// fall back out of the queue — possibly while it was playing — so an approval is one-way and
    /// a proposer who changes their mind removes the track instead.
    ///
    /// An approval from the proposer is accepted and stored but never counts, which keeps the rule
    /// in one place rather than relying on the client to hide the button.
    pub fn approve_jam_track(
        &self,
        jam_id: &str,
        track_id: &str,
        username: &str,
    ) -> Result<JamTrackState> {
        let needed = self.jam_approvals_needed(jam_id)?;
        let username = username.trim().to_lowercase();
        let mut conn = self.conn.lock_recover();
        let tx = conn.transaction()?;

        tx.execute(
            "INSERT OR IGNORE INTO jam_votes (jam_id, track_id, username) VALUES (?1, ?2, ?3)",
            params![jam_id, track_id, username],
        )?;

        // Counted excluding whoever proposed it.
        let approvals: i64 = tx.query_row(
            "SELECT COUNT(*) FROM jam_votes v
               JOIN jam_tracks t ON t.id = v.track_id
              WHERE v.track_id = ?1 AND v.username <> t.added_by COLLATE NOCASE",
            params![track_id],
            |row| row.get(0),
        )?;

        let state = if approvals >= needed {
            tx.execute(
                "UPDATE jam_tracks SET state = 'queued'
                  WHERE id = ?1 AND jam_id = ?2 AND state = 'proposed'",
                params![track_id, jam_id],
            )?;
            JamTrackState::Queued
        } else {
            JamTrackState::Proposed
        };
        tx.commit()?;
        Ok(state)
    }

    /// Tracks in one state, in the order they were added.
    ///
    /// Ordered by arrival, never by approvals: votes decide *whether* a track joins the queue, not
    /// where it lands once it has.
    pub fn jam_tracks(
        &self,
        jam_id: &str,
        state: JamTrackState,
        viewer: &str,
    ) -> Result<Vec<JamTrack>> {
        let needed = self.jam_approvals_needed(jam_id)?;
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT t.id, t.added_by, t.track_uri, t.title, t.artist, t.artwork_url,
                    t.added_at, t.duration_ms,
                    (SELECT COUNT(*) FROM jam_votes v
                      WHERE v.track_id = t.id AND v.username <> t.added_by COLLATE NOCASE),
                    EXISTS (SELECT 1 FROM jam_votes v
                            WHERE v.track_id = t.id AND v.username = ?3 COLLATE NOCASE)
               FROM jam_tracks t
              WHERE t.jam_id = ?1 AND t.state = ?2
              ORDER BY t.added_at ASC",
        )?;
        let rows = stmt.query_map(params![jam_id, state.as_str(), viewer.trim()], |row| {
            let approvals: i64 = row.get(8)?;
            Ok(JamTrack {
                id: row.get(0)?,
                added_by: row.get(1)?,
                track_uri: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                artwork_url: row.get(5)?,
                added_at: row.get(6)?,
                duration_ms: row.get(7)?,
                approvals,
                approved: row.get::<_, i64>(9)? != 0,
                still_needed: (needed - approvals).max(0),
            })
        })?;
        rows.collect()
    }
}
