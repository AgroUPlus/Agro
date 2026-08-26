//! Jam sessions: one queue, several people, and a rule for whose turn is next.
//!
//! Deliberately not listen-along. That mirrors one person — there is a host and everyone else
//! follows. A jam has no source: every member adds to the same queue, and in `democracy` mode the
//! order is decided by votes rather than by who typed fastest.
//!
//! The host is whoever created it. That is the only asymmetry: they set the mode, can drop a track
//! somebody else added, and can end the session. Everything else is equal between members.

use rusqlite::{params, Result};

use crate::db::Db;

/// How the next track is chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JamMode {
    /// Anyone adds; it plays in the order it was added. A shared queue, no ceremony.
    Open,
    /// Anyone adds; the most-voted track plays next. Ties fall back to whoever added first, so the
    /// order is always total and never arbitrary.
    Democracy,
}

impl JamMode {
    pub fn as_str(self) -> &'static str {
        match self {
            JamMode::Open => "open",
            JamMode::Democracy => "democracy",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw.trim().to_ascii_lowercase().as_str() {
            "open" => JamMode::Open,
            // Unrecognised means the default, and the default is the one that needs agreement.
            _ => JamMode::Democracy,
        }
    }
}

/// Where a track has got to.
///
/// The middle state is the point. A `played` flag could say "done" but not "the room has not agreed
/// to this yet", so in democracy mode votes had nowhere to act except on the *order* of a queue
/// everything had already entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JamTrackState {
    /// Suggested, waiting on the room.
    Proposed,
    /// Accepted, waiting its turn.
    Queued,
    Played,
}

impl JamTrackState {
    pub fn as_str(self) -> &'static str {
        match self {
            JamTrackState::Proposed => "proposed",
            JamTrackState::Queued => "queued",
            JamTrackState::Played => "played",
        }
    }
}

/// What the room is hearing, and since when.
#[derive(Clone, Debug)]
pub struct JamNowPlaying {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub duration_ms: i64,
    /// A stream with no end. It plays until the room skips it — see `jam_clock`.
    pub is_live: bool,
    pub started_at: String,
    /// How far in the room is, derived from `started_at` rather than reported by anyone.
    pub position_ms: i64,
}

#[derive(Clone, Debug)]
pub struct Jam {
    pub id: String,
    pub code: String,
    pub host: String,
    pub mode: JamMode,
    pub created_at: String,
    /// The track the whole room is on, decided here and pushed out.
    pub now_playing_id: Option<String>,
    /// When that track started, which is what makes this server the clock.
    pub started_at: Option<String>,
    pub visibility: JamVisibility,
}

/// Who can find a jam without being handed its code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JamVisibility {
    /// The code is the only way in. The default, because a room is private until said otherwise.
    Code,
    /// Accepted friends can see it and join it. Never the whole instance — an open jam is open to
    /// people you have already agreed to, not to strangers who happen to be on the server.
    Friends,
}

impl JamVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            JamVisibility::Code => "code",
            JamVisibility::Friends => "friends",
        }
    }

    pub fn parse(raw: &str) -> Self {
        if raw.trim().eq_ignore_ascii_case("friends") {
            JamVisibility::Friends
        } else {
            JamVisibility::Code
        }
    }
}

#[derive(Clone, Debug)]
pub struct JamTrack {
    pub id: String,
    pub added_by: String,
    pub track_uri: String,
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub added_at: String,
    pub duration_ms: i64,
    /// Approvals so far. Only meaningful on a proposal — the queue is not sorted by these.
    pub approvals: i64,
    /// Whether the account asking has already approved it.
    pub approved: bool,
    /// How many more approvals it needs before it joins the queue.
    pub still_needed: i64,
}

const JAM_COLUMNS: &str =
    "id, code, host, mode, created_at, now_playing_id, started_at, visibility";

fn jam_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Jam> {
    Ok(Jam {
        id: row.get(0)?,
        code: row.get(1)?,
        host: row.get(2)?,
        mode: JamMode::parse(&row.get::<_, String>(3)?),
        created_at: row.get(4)?,
        now_playing_id: row.get(5)?,
        started_at: row.get(6)?,
        visibility: JamVisibility::parse(&row.get::<_, String>(7)?),
    })
}

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

        let mut conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams WHERE code = ?1 COLLATE NOCASE AND ended_at IS NULL"
        ))?;
        let mut rows = stmt.query_map(params![code.trim()], jam_from_row)?;
        rows.next().transpose()
    }

    pub fn jam_by_id(&self, id: &str) -> Result<Option<Jam>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams WHERE id = ?1 AND ended_at IS NULL"
        ))?;
        let mut rows = stmt.query_map(params![id.trim()], jam_from_row)?;
        rows.next().transpose()
    }

    /// Adds someone to a jam. Leaving and rejoining is not an error.
    pub fn join_jam(&self, jam_id: &str, username: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO jam_members (jam_id, username, joined_at) VALUES (?1, ?2, ?3)",
            params![jam_id, username.trim().to_lowercase(), chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn leave_jam(&self, jam_id: &str, username: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let removed = conn.execute(
            "DELETE FROM jam_members WHERE jam_id = ?1 AND username = ?2 COLLATE NOCASE",
            params![jam_id, username.trim()],
        )?;
        Ok(removed > 0)
    }

    pub fn is_jam_member(&self, jam_id: &str, username: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM jam_members WHERE jam_id = ?1 AND username = ?2 COLLATE NOCASE",
            params![jam_id, username.trim()],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn jam_members(&self, jam_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT username FROM jam_members WHERE jam_id = ?1 ORDER BY joined_at")?;
        let rows = stmt.query_map(params![jam_id], |row| row.get(0))?;
        rows.collect()
    }

    pub fn set_jam_mode(&self, jam_id: &str, mode: JamMode) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE jams SET mode = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![jam_id, mode.as_str()],
        )?;
        Ok(changed > 0)
    }

    /// Ends a jam without deleting it, so its history stays readable.
    pub fn end_jam(&self, jam_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE jams SET ended_at = ?2 WHERE id = ?1 AND ended_at IS NULL",
            params![jam_id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(changed > 0)
    }

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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO jam_tracks
               (id, jam_id, added_by, track_uri, title, artist, artwork_url, added_at, played,
                duration_ms, state, is_live)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11)",
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
                is_live as i64
            ],
        )?;
        Ok((id, state))
    }

    /// Who added a track, so "your own, or you run the room" can be applied.
    pub fn jam_track_owner(&self, jam_id: &str, track_id: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT added_by FROM jam_tracks WHERE id = ?1 AND jam_id = ?2")?;
        let mut rows = stmt.query_map(params![track_id, jam_id], |row| row.get(0))?;
        rows.next().transpose()
    }

    pub fn remove_jam_track(&self, jam_id: &str, track_id: &str) -> Result<bool> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM jam_votes WHERE track_id = ?1", params![track_id])?;
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
        let mut conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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

    /// Opens a jam to friends, or shuts it back to code-only.
    pub fn set_jam_visibility(&self, jam_id: &str, visibility: JamVisibility) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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

    /// A skip needs more than half the room, counting everybody — the person who suggested the
    /// track included. Wanting it gone is not the same act as having wanted it in.
    pub fn jam_skips_needed(&self, jam_id: &str) -> Result<i64> {
        let members = self.jam_members(jam_id)?.len() as i64;
        Ok(members / 2 + 1)
    }

    /// Records a skip vote for the playing track, and says whether the room has had enough.
    pub fn vote_skip(&self, jam_id: &str, track_id: &str, username: &str) -> Result<bool> {
        let needed = self.jam_skips_needed(jam_id)?;
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT title, artist, artwork_url, duration_ms, is_live FROM jam_tracks WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![track_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)? != 0,
            ))
        })?;
        let Some((title, artist, artwork_url, duration_ms, is_live)) = rows.next().transpose()? else {
            return Ok(None);
        };
        Ok(Some(JamNowPlaying {
            track_id: track_id.clone(),
            title,
            artist,
            artwork_url,
            duration_ms,
            is_live,
            started_at: started_at.clone(),
            position_ms: elapsed_ms(started_at),
        }))
    }

    pub fn set_jam_now_playing(&self, jam_id: &str, track_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE jams SET now_playing_id = ?2, started_at = ?3 WHERE id = ?1",
            params![jam_id, track_id, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn clear_jam_now_playing(&self, jam_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE jams SET now_playing_id = NULL, started_at = NULL WHERE id = ?1",
            params![jam_id],
        )?;
        Ok(())
    }

    pub fn mark_jam_track_played(&self, jam_id: &str, track_id: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let changed = conn.execute(
            "UPDATE jam_tracks SET state = 'played', played = 1 WHERE id = ?1 AND jam_id = ?2",
            params![track_id, jam_id],
        )?;
        Ok(changed > 0)
    }

    /// Every jam still running, for the clock to look at.
    pub fn live_jams(&self) -> Result<Vec<Jam>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&format!(
            "SELECT {JAM_COLUMNS} FROM jams WHERE ended_at IS NULL"
        ))?;
        let rows = stmt.query_map([], jam_from_row)?;
        rows.collect()
    }

    /// Removes a jam and everything belonging to it.
    ///
    /// A jam is a room, not a document: once it is over there is nothing worth keeping, and leaving
    /// the rows behind means every member's client has to keep deciding whether a dead jam counts.
    pub fn delete_jam(&self, jam_id: &str) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM jam_votes WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jam_skips WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jam_tracks WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jam_members WHERE jam_id = ?1", params![jam_id])?;
        tx.execute("DELETE FROM jams WHERE id = ?1", params![jam_id])?;
        tx.commit()?;
        Ok(())
    }
}

/// A join code that survives being read out loud.
///
/// Letters and digits only — the token prefix this replaced was base64url, so codes arrived with
/// `-` and `_` in them, which is a poor thing to dictate over a room and worse to type. `0/O` and
/// `1/I` are left out for the same reason: they are alphanumeric but indistinguishable in most
/// fonts, and a code is useless if it cannot be copied by eye.
fn generate_jam_code() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut rng = rand::thread_rng();
    (0..8)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Milliseconds since an RFC3339 instant, never negative.
///
/// An unparseable or future timestamp reads as zero: starting a track from the beginning is a far
/// better failure than seeking to a nonsense offset.
fn elapsed_ms(started_at: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|start| {
            (chrono::Utc::now() - start.with_timezone(&chrono::Utc)).num_milliseconds().max(0)
        })
        .unwrap_or(0)
}
