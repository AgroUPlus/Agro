//! Which device holds which recording, and what is missing where.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use crate::norm::DURATION_TOLERANCE_MS;
use rusqlite::{params, Result};

impl Db {
    pub fn upsert_holding(
        &self,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
        local_ref: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT INTO device_holdings (device_id, user_id, content_hash, local_ref, reported_at)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(device_id, content_hash) DO UPDATE SET
                 local_ref = excluded.local_ref, reported_at = excluded.reported_at",
            params![
                device_id,
                user_id,
                content_hash,
                local_ref,
                chrono::Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// Forgets holdings, scoped to the account that owns them.
    ///
    /// `user_id` is not decoration: this filtered on `device_id` alone, and device ids are chosen
    /// by the client, so any account could delete another account's holding rows by naming its
    /// device. The resolver's own check is not enough on its own — it verifies the *caller*, while
    /// the device id is the part that was smuggled.
    pub fn forget_holdings(
        &self,
        user_id: &str,
        device_id: &str,
        hashes: &[String],
    ) -> Result<usize> {
        let conn = self.conn.lock_recover();
        let mut removed = 0;
        for hash in hashes {
            removed += conn.execute(
                "DELETE FROM device_holdings
                  WHERE user_id = ?1 AND device_id = ?2 AND content_hash = ?3",
                params![user_id, device_id, hash],
            )?;
        }
        Ok(removed)
    }

    /// What a device holds. Scoped by account for the same reason [`Self::forget_holdings`] is.
    pub fn device_holding_hashes(&self, user_id: &str, device_id: &str) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT content_hash FROM device_holdings
              WHERE user_id = ?1 AND device_id = ?2 ORDER BY content_hash",
        )?;
        let rows = stmt.query_map(params![user_id, device_id], |row| row.get(0))?;
        rows.collect()
    }

    /// Whether this account registered this device. Backs `require_own_device`.
    pub fn device_belongs_to(&self, user_id: &str, device_id: &str) -> Result<bool> {
        let conn = self.read();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM registered_nodes WHERE user_id = ?1 AND device_id = ?2",
            params![user_id, device_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Looks up the username that registered a device.
    pub fn owner_of_device(&self, device_id: &str) -> Result<Option<String>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT user_id FROM registered_nodes WHERE device_id = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![device_id])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Tracks another device of this account holds that [`device_id`] does not.
    ///
    /// Two filters, and both matter:
    ///
    /// 1. **Not the same file** — no `device_holdings` row for this device and that hash.
    /// 2. **Not the same recording** — nothing this device *does* hold normalises to the same
    ///    artist and title, with the same performance variants, within
    ///    [`DURATION_TOLERANCE_MS`]. Without this the user is offered a FLAC of a song they
    ///    already own at 128 kbps, over and over, because the bytes differ.
    ///
    /// The duration comparison is why this is a join rather than a `NOT IN`: it needs a tolerance,
    /// not equality.
    pub fn missing_on_device(
        &self,
        user_id: &str,
        device_id: &str,
        limit: i64,
    ) -> Result<Vec<LibraryTrack>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT t.content_hash, t.title, t.artist, t.album, t.album_artist,
                    t.track_no, t.disc_no, t.year, t.genre, t.duration_ms, t.size_bytes,
                    t.format, t.bitrate_kbps, t.archived_path
             FROM library_tracks t
             JOIN device_holdings other
               ON other.content_hash = t.content_hash
              AND other.user_id = ?1
              AND other.device_id <> ?2
             JOIN registered_nodes rn
               ON rn.user_id = other.user_id AND rn.device_id = other.device_id
             WHERE 1=1 
               AND NOT EXISTS (
                 SELECT 1 FROM device_holdings mine
                 WHERE mine.device_id = ?2 AND mine.content_hash = t.content_hash)
               AND NOT EXISTS (
                 SELECT 1 FROM device_holdings mine
                 JOIN library_tracks mt ON mt.content_hash = mine.content_hash
                 WHERE mine.device_id = ?2
                   AND mt.norm_artist   = t.norm_artist
                   AND mt.norm_title    = t.norm_title
                   AND mt.norm_variants = t.norm_variants
                   AND t.duration_ms > 0 AND mt.duration_ms > 0
                   AND ABS(mt.duration_ms - t.duration_ms) <= ?3)
             ORDER BY t.artist, t.album, t.track_no
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![user_id, device_id, DURATION_TOLERANCE_MS, limit],
            row_to_track,
        )?;
        rows.collect()
    }

    /// Finds other registered devices of this account that hold the specified content hash.
    pub fn peer_sources_for_track(
        &self,
        user_id: &str,
        content_hash: &str,
    ) -> Result<Vec<PeerSourceInfo>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            // An inner join, and no `COALESCE` on `last_seen_at`. A holding whose device is gone
            // is not a source that happens to be offline — it is not a source at all. The old
            // left join listed it anyway and, worse, substituted `datetime('now')` for the missing
            // timestamp, so a device that no longer exists was advertised as *just seen*: the
            // caller computes `is_online` from that value. That is what offers a download which can
            // never complete.
            "SELECT h.device_id, COALESCE(NULLIF(rn.petname, ''), h.device_id), rn.last_seen_at
             FROM device_holdings h
             JOIN registered_nodes rn
               ON rn.device_id = h.device_id
              AND rn.user_id = h.user_id
             WHERE h.content_hash = ?1 AND h.user_id = ?2",
        )?;
        let rows = stmt.query_map(params![content_hash, user_id], |row| {
            Ok(PeerSourceInfo {
                device_id: row.get(0)?,
                petname: row.get(1)?,
                lan_address: None,
                last_seen_at: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// Tracks this device holds that the server has already filed into the library.
    ///
    /// The candidate list for "you can free this space" — the device's copy is redundant because
    /// the server has one. Only the index is consulted here; whether the archived file is really
    /// on disk is checked by the caller, which is the half that needs the filesystem.
    ///
    /// Deliberately not filtered by user beyond the holding row: a device belongs to one account,
    /// and `archived_path` is set by this server alone.
    pub fn reclaimable_on_device(
        &self,
        user_id: &str,
        device_id: &str,
        limit: i64,
    ) -> Result<Vec<LibraryTrack>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT t.content_hash, t.title, t.artist, t.album, t.album_artist,
                    t.track_no, t.disc_no, t.year, t.genre, t.duration_ms, t.size_bytes,
                    t.format, t.bitrate_kbps, t.archived_path
             FROM library_tracks t
             JOIN device_holdings h
               ON h.content_hash = t.content_hash
              AND h.user_id = ?1
              AND h.device_id = ?2
             WHERE t.archived_path IS NOT NULL
             ORDER BY t.size_bytes DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![user_id, device_id, limit], row_to_track)?;
        rows.collect()
    }
}
