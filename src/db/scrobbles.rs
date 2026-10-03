//! Plays, as reported by the clients and stored for stats.

use chrono::DurationRound;
use rusqlite::{params, Result};

use super::Db;

impl Db {
    /// Rounds a play time down to the hour it fell in.
    ///
    /// An exact play time is a lifestyle record: a run of them says when someone woke, commuted,
    /// worked and went to bed, and that is the single most identifying thing in this database.
    /// Every statistic Agro computes — the 24-bar hour histogram, the day sparkline, the 8-week
    /// heatmap, streaks, top artists, taste match — buckets by hour or by day already, so the
    /// seconds are precision nobody reads.
    ///
    /// An unparseable timestamp is returned untouched. It is already excluded from every timeline
    /// (`stats::compute` counts it in totals but cannot place it), and inventing an hour for it
    /// would be worse than leaving it alone.
    fn to_hour(played_at: &str) -> String {
        match chrono::DateTime::parse_from_rfc3339(played_at) {
            Ok(dt) => dt
                .with_timezone(&chrono::Utc)
                .duration_trunc(chrono::Duration::hours(1))
                .map(|t| t.to_rfc3339())
                .unwrap_or_else(|_| played_at.to_string()),
            Err(_) => played_at.to_string(),
        }
    }

    /// Ingests a batch of plays from one device.
    ///
    /// `INSERT OR IGNORE` against the unique index from migration 8, so a client re-sending an
    /// outbox it was not sure landed does not double every play in it. Returns how many rows were
    /// genuinely new, which is what lets a client tell "already had it" from "did not work".
    pub fn record_scrobbles(
        &self,
        user_id: &str,
        device_name: &str,
        client_type: Option<&str>,
        entries: &[ScrobbleEntry],
    ) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut inserted = 0;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO scrobbles
                     (user_id, track_title, artist_name, album_name, genre, duration_secs,
                      device_name, played_at, client_type, play_uid)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for entry in entries {
                // Only a play that names itself may be blurred. Without a `play_uid` the unique
                // index on `(user_id, artist_name, track_title, played_at)` is still the only thing
                // standing between a retried outbox and double-counted plays, and rounding the
                // timestamp would collapse four plays of one track in one hour into one row —
                // silently breaking the on-repeat feed and deflating every count that feeds it.
                // A client that sends an id has moved its idempotency off the clock, so the clock
                // is free to lose its seconds.
                let played_at = match entry.play_uid {
                    Some(_) => Self::to_hour(&entry.played_at),
                    None => entry.played_at.clone(),
                };
                inserted += stmt.execute(params![
                    user_id,
                    entry.track_title,
                    entry.artist_name,
                    entry.album_name,
                    entry.genre,
                    entry.duration_secs,
                    device_name,
                    played_at,
                    client_type,
                    entry.play_uid,
                ])?;
            }
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// Raw plays for an account, newest last.
    ///
    /// Deliberately returns rows rather than aggregates. The desktop client already computes a
    /// specific set of statistics from a local history file, and the numbers here have to agree
    /// with those exactly or switching a device between local and centralised stats looks like data
    /// loss. Sharing the *shape* of the computation is how that is guaranteed, so the aggregation
    /// lives in one place (`stats.rs`) rather than being re-expressed in SQL.
    pub fn scrobble_rows(
        &self,
        user_id: &str,
        device_name: Option<&str>,
        since: Option<&str>,
    ) -> Result<Vec<ScrobbleRow>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT s.track_title, s.artist_name, s.album_name, s.genre, s.duration_secs,
                    COALESCE(NULLIF(rn.petname, ''), s.device_name) AS device_name,
                    s.played_at
             FROM scrobbles s
             LEFT JOIN registered_nodes rn
                    ON rn.user_id = s.user_id COLLATE NOCASE
                   AND (rn.device_id = s.device_name OR rn.petname = s.device_name)
             WHERE s.user_id = ?1
               AND (?2 IS NULL OR s.device_name = ?2 OR rn.petname = ?2 OR rn.device_id = ?2)
               AND (?3 IS NULL OR s.played_at >= ?3)
             ORDER BY s.played_at",
        )?;
        let mut rows = stmt.query(params![user_id, device_name, since])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(ScrobbleRow {
                track_title: row.get(0)?,
                artist_name: row.get(1)?,
                album_name: row.get(2)?,
                genre: row.get(3)?,
                duration_secs: row.get(4)?,
                device_name: row.get(5)?,
                played_at: row.get(6)?,
            });
        }
        Ok(out)
    }

    /// Purges scrobbles for an account, optionally restricted by year or before a given timestamp.
    ///
    /// Allows users to actively wipe listening history (e.g. at the conclusion of viewing a Rewind).
    pub fn purge_scrobbles(
        &self,
        user_id: &str,
        year: Option<i32>,
        before: Option<&str>,
    ) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let count = match (year, before) {
            (Some(y), _) => {
                let start = format!("{y:04}-01-01T00:00:00+00:00");
                let end = format!("{:04}-01-01T00:00:00+00:00", y + 1);
                conn.execute(
                    "DELETE FROM scrobbles WHERE user_id = ?1 AND played_at >= ?2 AND played_at < ?3",
                    params![user_id, start, end],
                )?
            }
            (None, Some(b)) => conn.execute(
                "DELETE FROM scrobbles WHERE user_id = ?1 AND played_at < ?2",
                params![user_id, b],
            )?,
            (None, None) => {
                conn.execute("DELETE FROM scrobbles WHERE user_id = ?1", params![user_id])?
            }
        };
        Ok(count)
    }
}

/// One play, as a client reports it.
///
/// `played_at` is RFC3339 and comes from the client, not from this server: a phone that was offline
/// for a day is reporting yesterday's listening, and stamping it on arrival would pile a week of
/// history onto one afternoon.
pub struct ScrobbleEntry {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: i64,
    pub played_at: String,
    /// What makes a retry a retry. Minted by the client when the play happens and kept in its
    /// outbox, so the same play re-sent carries the same id however many times it is offered.
    /// `None` from a client that predates this, which falls back to the timestamp rule.
    pub play_uid: Option<String>,
}

/// One play, as stored.
pub struct ScrobbleRow {
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub genre: Option<String>,
    pub duration_secs: i64,
    pub device_name: String,
    pub played_at: String,
}
