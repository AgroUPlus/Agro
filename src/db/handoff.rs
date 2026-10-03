//! The handoff: what each device is playing, so another one can pick it up.

use rusqlite::{params, Result};

use super::Db;

impl Db {
    // One row's worth of handoff state, all decided together by the caller — splitting this into
    // setters per field would let two concurrent handoffs interleave into a state neither sent.
    #[allow(clippy::too_many_arguments)]
    pub fn update_handoff(
        &self,
        user_id: &str,
        track_uri: &str,
        track_title: &str,
        artist_name: &str,
        album_name: Option<&str>,
        artwork_url: Option<&str>,
        position_ms: i64,
        duration_ms: i64,
        is_playing: bool,
        device_id: &str,
        queue_json: Option<&str>,
        queue_index: Option<i64>,
        content_hash: Option<&str>,
        encrypted_payload: Option<&str>,
        presence_ciphertexts: Option<&[crate::db_presence::PresenceCiphertext]>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO handoff_state (user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, updated_at, queue_json, queue_index, duration_ms, content_hash, encrypted_payload)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(user_id, device_id) DO UPDATE SET
             track_uri = excluded.track_uri,
             track_title = excluded.track_title,
             artist_name = excluded.artist_name,
             album_name = excluded.album_name,
             artwork_url = excluded.artwork_url,
             position_ms = excluded.position_ms,
             -- A sender that does not know the length must not erase one that did: a livestream
             -- and a length not measured yet both arrive as 0, and only one of them is an answer.
             duration_ms = CASE WHEN excluded.duration_ms > 0
                                THEN excluded.duration_ms
                                ELSE handoff_state.duration_ms END,
             is_playing = excluded.is_playing,
             updated_at = excluded.updated_at,
             -- A heartbeat that carries no queue must not erase the one already stored: only a
             -- client that actually sent a queue replaces it.
             queue_json = COALESCE(excluded.queue_json, handoff_state.queue_json),
             queue_index = COALESCE(excluded.queue_index, handoff_state.queue_index),
             -- Same rule as the queue: a heartbeat that does not name a hash must not erase the
             -- one the track change already established.
             content_hash = COALESCE(excluded.content_hash, handoff_state.content_hash),
             -- Not COALESCE: this one *must* clear. A sealed track followed by an ordinary one
             -- arrives with no payload, and leaving the old envelope in place would mark a public
             -- session private. Every handoff names its own privacy.
             encrypted_payload = excluded.encrypted_payload",
            params![user_id, track_uri, track_title, artist_name, album_name, artwork_url, position_ms, is_playing, device_id, now, queue_json, queue_index, duration_ms, content_hash, encrypted_payload],
        )?;
        // Written under the same lock as the row, for the reason `create_drop` gives: a session
        // whose sealed copies did not land is one no friend can open.
        //
        // `None` and `Some(&[])` mean different things here, and the difference is the whole
        // reason this is not an `&[..]`. A heartbeat repeats every ten seconds and the metadata it
        // repeats has not changed, so re-sealing it once per friend device each time is work
        // nobody asked for: a heartbeat passes `None` and the stored copies stand. `Some(&[])` is
        // the opposite instruction — this session is over, or is no longer sealed — and clears
        // them. Only a track change pays for a new set.
        //
        // `encrypted_payload` above is deliberately not treated this way. It is a single envelope,
        // cheap enough to re-send on every heartbeat, so it can afford to say what it means every
        // time and clear when absent.
        if let Some(copies) = presence_ciphertexts {
            Self::replace_presence_ciphertexts_in(&conn, user_id, device_id, copies)?;
        }
        Ok(())
    }

    /// Where the account left off — the most recent report from any of its devices.
    ///
    /// One row per device since migration 23, so "the account's handoff" is now a choice rather
    /// than the only row there is. This keeps the original answer: whatever happened last.
    pub fn get_handoff(&self, user_id: &str) -> Result<Option<HandoffRecord>> {
        self.latest_handoff(user_id, None)
    }

    /// The same, from any device *except* one.
    ///
    /// What a client asks when it is looking for the rest of the fleet rather than for itself: a
    /// desktop player proxying the phone's track to Discord has to be able to tell the two apart,
    /// and its own paused state is the one answer that is never useful to it.
    pub fn get_handoff_excluding(
        &self,
        user_id: &str,
        device_id: &str,
    ) -> Result<Option<HandoffRecord>> {
        self.latest_handoff(user_id, Some(device_id))
    }

    fn latest_handoff(
        &self,
        user_id: &str,
        exclude_device: Option<&str>,
    ) -> Result<Option<HandoffRecord>> {
        let conn = self.conn.lock().unwrap();
        // `updated_at` is an RFC 3339 stamp written by this process, always at the same offset, so
        // it orders lexicographically — no date parsing, which would fail silently to NULL and
        // scramble the order rather than erroring.
        let mut stmt = conn.prepare(
            "SELECT track_uri, track_title, artist_name, album_name, artwork_url, position_ms,
                    is_playing, device_id, updated_at, queue_json, queue_index, duration_ms,
                    content_hash, encrypted_payload
               FROM handoff_state
              WHERE user_id = ?1 AND (?2 IS NULL OR device_id != ?2)
              ORDER BY updated_at DESC
              LIMIT 1",
        )?;
        let mut rows = stmt.query(params![user_id, exclude_device])?;
        if let Some(row) = rows.next()? {
            Ok(Some(HandoffRecord {
                track_uri: row.get(0)?,
                track_title: row.get(1)?,
                artist_name: row.get(2)?,
                album_name: row.get(3)?,
                artwork_url: row.get(4)?,
                position_ms: row.get(5)?,
                is_playing: row.get(6)?,
                device_id: row.get(7)?,
                updated_at: row.get(8)?,
                queue_json: row.get(9)?,
                queue_index: row.get(10)?,
                duration_ms: row.get(11)?,
                content_hash: row.get(12)?,
                encrypted_payload: row.get(13)?,
            }))
        } else {
            Ok(None)
        }
    }
}

pub struct HandoffRecord {
    pub track_uri: String,
    /// How long the track is. 0 when the sender did not say, or when it is a livestream.
    pub duration_ms: i64,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    pub updated_at: String,
    /// SHA-256 of the bytes being played, when the sender knows it. `None` for anything that is
    /// not a hashed local file, which is what makes a direct transfer impossible and a name match
    /// the only option left.
    pub content_hash: Option<String>,
    /// The whole queue as a JSON array, so a resumed session continues rather than stopping after
    /// one track. Kept opaque here: the clients agree on the shape, the server only stores it.
    pub queue_json: Option<String>,
    pub queue_index: Option<i64>,
    /// An authenticated envelope holding the real metadata when the sender sealed it. `None` for
    /// an ordinary handoff. The server stores and forwards it without being able to open it.
    pub encrypted_payload: Option<String>,
}
