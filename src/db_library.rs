//! SQL for the music library index.
//!
//! A second `impl Db` block rather than more of `db.rs`, which is already long and about something
//! else. Same connection, same single write lock.
//!
//! Everything here keys on **username** in its `user_id` column, matching `registered_nodes`,
//! `handoff_state` and `synced_settings`. Only `app_passwords.user_id` holds the `users.id` UUID.

use crate::db::Db;
use crate::norm::{recording_key, DURATION_TOLERANCE_MS};
use rusqlite::{params, OptionalExtension, Result};

/// One file the server knows about, whether or not it holds the bytes.
#[derive(Debug, Clone)]
pub struct LibraryTrack {
    pub content_hash: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub format: Option<String>,
    pub bitrate_kbps: Option<i64>,
    pub archived_path: Option<String>,
}

/// An upload in flight.
#[derive(Debug, Clone)]
pub struct UploadSession {
    pub upload_id: String,
    pub user_id: String,
    pub device_id: String,
    pub content_hash: String,
    pub size_bytes: i64,
    pub received_bytes: i64,
    pub target: String,
    /// Declared by the client at `begin_upload`, so a restart mid-transfer does not lose it.
    pub extension: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct LibraryStats {
    pub track_count: i64,
    pub archived_count: i64,
    pub total_bytes: i64,
    pub spool_bytes: i64,
}

impl Db {
    // ── Index ───────────────────────────────────────────────────────────────────────────────

    pub fn library_track(&self, content_hash: &str) -> Result<Option<LibraryTrack>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT content_hash, title, artist, album, album_artist, track_no, disc_no, year,
                    genre, duration_ms, size_bytes, format, bitrate_kbps, archived_path
             FROM library_tracks WHERE content_hash = ?1",
            params![content_hash],
            row_to_track,
        )
        .optional()
    }

    /// Inserts or refreshes an index entry.
    ///
    /// `norm_artist`/`norm_title` are computed here, from the metadata as given, so the whole
    /// index shares one convention no matter which client or which version reported it.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_library_track(&self, track: &LibraryTrack) -> Result<()> {
        let key = recording_key(&track.artist, &track.title);
        let now = chrono::Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO library_tracks (
                 content_hash, title, artist, album, album_artist, track_no, disc_no, year, genre,
                 duration_ms, size_bytes, format, bitrate_kbps, norm_artist, norm_title,
                 norm_variants, archived_path, first_seen_at, updated_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?18)
             ON CONFLICT(content_hash) DO UPDATE SET
                 title=excluded.title, artist=excluded.artist, album=excluded.album,
                 album_artist=excluded.album_artist, track_no=excluded.track_no,
                 disc_no=excluded.disc_no, year=excluded.year, genre=excluded.genre,
                 duration_ms=excluded.duration_ms, size_bytes=excluded.size_bytes,
                 format=excluded.format, bitrate_kbps=excluded.bitrate_kbps,
                 norm_artist=excluded.norm_artist, norm_title=excluded.norm_title,
                 norm_variants=excluded.norm_variants,
                 -- An existing archive location is never cleared by a later report from a device
                 -- that only holds its own copy.
                 archived_path=COALESCE(excluded.archived_path, library_tracks.archived_path),
                 updated_at=excluded.updated_at",
            params![
                track.content_hash,
                track.title,
                track.artist,
                track.album,
                track.album_artist,
                track.track_no,
                track.disc_no,
                track.year,
                track.genre,
                track.duration_ms,
                track.size_bytes,
                track.format,
                track.bitrate_kbps,
                key.artist,
                key.title,
                key.variants,
                track.archived_path,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn set_archived_path(&self, content_hash: &str, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE library_tracks SET archived_path = ?2, updated_at = ?3 WHERE content_hash = ?1",
            params![content_hash, path, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // ── Holdings ────────────────────────────────────────────────────────────────────────────

    pub fn upsert_holding(
        &self,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
        local_ref: Option<&str>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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

    pub fn forget_holdings(&self, device_id: &str, hashes: &[String]) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let mut removed = 0;
        for hash in hashes {
            removed += conn.execute(
                "DELETE FROM device_holdings WHERE device_id = ?1 AND content_hash = ?2",
                params![device_id, hash],
            )?;
        }
        Ok(removed)
    }

    pub fn device_holding_hashes(&self, device_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content_hash FROM device_holdings WHERE device_id = ?1 ORDER BY content_hash",
        )?;
        let rows = stmt.query_map(params![device_id], |row| row.get(0))?;
        rows.collect()
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
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT t.content_hash, t.title, t.artist, t.album, t.album_artist,
                    t.track_no, t.disc_no, t.year, t.genre, t.duration_ms, t.size_bytes,
                    t.format, t.bitrate_kbps, t.archived_path
             FROM library_tracks t
             JOIN device_holdings other
               ON other.content_hash = t.content_hash
              AND other.user_id = ?1
              AND other.device_id <> ?2
             WHERE NOT EXISTS (
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

    pub fn library_stats(&self, user_id: &str) -> Result<LibraryStats> {
        let conn = self.conn.lock().unwrap();
        let (track_count, archived_count, total_bytes) = conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN t.archived_path IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(t.size_bytes), 0)
             FROM library_tracks t
             WHERE EXISTS (SELECT 1 FROM device_holdings h
                           WHERE h.content_hash = t.content_hash AND h.user_id = ?1)
                OR t.archived_path IS NOT NULL",
            params![user_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        let spool_bytes: i64 = conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM spool_items WHERE user_id = ?1",
            params![user_id],
            |row| row.get(0),
        )?;
        Ok(LibraryStats {
            track_count,
            archived_count,
            total_bytes,
            spool_bytes,
        })
    }

    // ── Upload sessions ─────────────────────────────────────────────────────────────────────

    pub fn create_upload(
        &self,
        upload_id: &str,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
        size_bytes: i64,
        target: &str,
        extension: Option<&str>,
        ttl_hours: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO upload_sessions
                 (upload_id, user_id, device_id, content_hash, size_bytes, received_bytes,
                  target, created_at, expires_at, extension)
             VALUES (?1,?2,?3,?4,?5,0,?6,?7,?8,?9)",
            params![
                upload_id,
                user_id,
                device_id,
                content_hash,
                size_bytes,
                target,
                now.to_rfc3339(),
                (now + chrono::Duration::hours(ttl_hours)).to_rfc3339(),
                extension,
            ],
        )?;
        Ok(())
    }

    /// An unfinished upload of the same file by the same device, so a dropped transfer resumes.
    pub fn resumable_upload(
        &self,
        user_id: &str,
        device_id: &str,
        content_hash: &str,
    ) -> Result<Option<UploadSession>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT upload_id, user_id, device_id, content_hash, size_bytes, received_bytes, target,
                    extension
             FROM upload_sessions
             WHERE user_id = ?1 AND device_id = ?2 AND content_hash = ?3
               AND expires_at > ?4
             ORDER BY created_at DESC LIMIT 1",
            params![
                user_id,
                device_id,
                content_hash,
                chrono::Utc::now().to_rfc3339()
            ],
            row_to_upload,
        )
        .optional()
    }

    pub fn upload_session(&self, upload_id: &str) -> Result<Option<UploadSession>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT upload_id, user_id, device_id, content_hash, size_bytes, received_bytes, target,
                    extension
             FROM upload_sessions WHERE upload_id = ?1",
            params![upload_id],
            row_to_upload,
        )
        .optional()
    }

    pub fn set_upload_received(&self, upload_id: &str, received: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE upload_sessions SET received_bytes = ?2 WHERE upload_id = ?1",
            params![upload_id, received],
        )?;
        Ok(())
    }

    pub fn delete_upload(&self, upload_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM upload_sessions WHERE upload_id = ?1",
            params![upload_id],
        )?;
        Ok(())
    }

    /// Upload sessions whose TTL has passed, so their `.part` files can be removed too.
    pub fn expired_uploads(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT upload_id FROM upload_sessions WHERE expires_at <= ?1")?;
        let rows = stmt.query_map(params![chrono::Utc::now().to_rfc3339()], |row| row.get(0))?;
        rows.collect()
    }

    // ── Spool ───────────────────────────────────────────────────────────────────────────────

    pub fn spool_insert(
        &self,
        content_hash: &str,
        size_bytes: i64,
        from_device: &str,
        user_id: &str,
        ttl_hours: i64,
    ) -> Result<()> {
        let now = chrono::Utc::now();
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO spool_items (content_hash, size_bytes, from_device, user_id,
                                      created_at, expires_at)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(content_hash) DO UPDATE SET
                 expires_at = excluded.expires_at, from_device = excluded.from_device",
            params![
                content_hash,
                size_bytes,
                from_device,
                user_id,
                now.to_rfc3339(),
                (now + chrono::Duration::hours(ttl_hours)).to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn spool_total_bytes(&self) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COALESCE(SUM(size_bytes),0) FROM spool_items", [], |r| {
            r.get(0)
        })
    }

    pub fn spool_contains(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let found: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM spool_items WHERE content_hash = ?1",
                params![content_hash],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    /// Spool entries to remove, oldest first — expired ones always, then whatever else it takes to
    /// get back under [`budget`].
    pub fn spool_evictable(&self, budget: i64) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT content_hash, size_bytes, expires_at FROM spool_items ORDER BY created_at ASC",
        )?;
        let now = chrono::Utc::now().to_rfc3339();
        let rows: Vec<(String, i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<_>>()?;

        let mut total: i64 = rows.iter().map(|(_, size, _)| size).sum();
        let mut doomed = Vec::new();
        for (hash, size, expires_at) in rows {
            if expires_at <= now {
                doomed.push((hash, size));
                total -= size;
            } else if total > budget {
                doomed.push((hash, size));
                total -= size;
            }
        }
        Ok(doomed)
    }

    /// Which account spooled a file, so a fetch can be scoped to it.
    pub fn spool_owner(&self, content_hash: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT user_id FROM spool_items WHERE content_hash = ?1",
            params![content_hash],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn spool_delete(&self, content_hash: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM spool_items WHERE content_hash = ?1",
            params![content_hash],
        )?;
        Ok(())
    }
}

fn row_to_track(row: &rusqlite::Row<'_>) -> Result<LibraryTrack> {
    Ok(LibraryTrack {
        content_hash: row.get(0)?,
        title: row.get(1)?,
        artist: row.get(2)?,
        album: row.get(3)?,
        album_artist: row.get(4)?,
        track_no: row.get(5)?,
        disc_no: row.get(6)?,
        year: row.get(7)?,
        genre: row.get(8)?,
        duration_ms: row.get(9)?,
        size_bytes: row.get(10)?,
        format: row.get(11)?,
        bitrate_kbps: row.get(12)?,
        archived_path: row.get(13)?,
    })
}

fn row_to_upload(row: &rusqlite::Row<'_>) -> Result<UploadSession> {
    Ok(UploadSession {
        upload_id: row.get(0)?,
        user_id: row.get(1)?,
        device_id: row.get(2)?,
        content_hash: row.get(3)?,
        size_bytes: row.get(4)?,
        received_bytes: row.get(5)?,
        target: row.get(6)?,
        extension: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(hash: &str, artist: &str, title: &str, duration_ms: i64) -> LibraryTrack {
        LibraryTrack {
            content_hash: hash.to_string(),
            title: title.to_string(),
            artist: artist.to_string(),
            album: Some("Album".to_string()),
            album_artist: None,
            track_no: Some(1),
            disc_no: None,
            year: None,
            genre: None,
            duration_ms,
            size_bytes: 1_000,
            format: Some("flac".to_string()),
            bitrate_kbps: None,
            archived_path: None,
        }
    }

    fn db_with(tracks: &[(LibraryTrack, &str)]) -> Db {
        let db = Db::new_in_memory().unwrap();
        for (t, device) in tracks {
            db.upsert_library_track(t).unwrap();
            db.upsert_holding("alpha", device, &t.content_hash, None)
                .unwrap();
        }
        db
    }

    #[test]
    fn a_track_only_one_device_has_is_missing_on_the_other() {
        let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
        db.upsert_holding("alpha", "phone", "h0", None).ok();

        let missing = db.missing_on_device("alpha", "phone", 10).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].content_hash, "h1");
    }

    #[test]
    fn a_track_both_devices_have_is_not_missing() {
        let db = db_with(&[
            (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
            (track("h1", "Nirvana", "Come As You Are", 219_000), "phone"),
        ]);
        assert!(db.missing_on_device("alpha", "phone", 10).unwrap().is_empty());
    }

    /// The point of the fuzzy layer: different bytes, same recording.
    #[test]
    fn a_different_rip_of_the_same_recording_is_not_missing() {
        let db = db_with(&[
            (track("flac", "Nirvana", "Come As You Are", 219_000), "laptop"),
            (track("mp3", "Nirvana", "Come As You Are", 220_500), "phone"),
        ]);
        assert!(
            db.missing_on_device("alpha", "phone", 10).unwrap().is_empty(),
            "a 1.5s-different encode of the same song must not be offered"
        );
    }

    /// The mistake that must never be made: a live take is not the studio cut.
    #[test]
    fn a_live_take_is_still_missing_when_you_own_the_studio_cut() {
        let db = db_with(&[
            (
                track("live", "Nirvana", "Come As You Are (Live)", 219_000),
                "laptop",
            ),
            (track("studio", "Nirvana", "Come As You Are", 219_000), "phone"),
        ]);
        let missing = db.missing_on_device("alpha", "phone", 10).unwrap();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].content_hash, "live");
    }

    #[test]
    fn another_accounts_device_is_not_consulted() {
        let db = Db::new_in_memory().unwrap();
        let t = track("h1", "Nirvana", "Come As You Are", 219_000);
        db.upsert_library_track(&t).unwrap();
        db.upsert_holding("beta", "beta-laptop", "h1", None).unwrap();

        assert!(db.missing_on_device("alpha", "phone", 10).unwrap().is_empty());
    }

    #[test]
    fn forgetting_a_holding_makes_it_missing_again() {
        let db = db_with(&[
            (track("h1", "A", "B", 100_000), "laptop"),
            (track("h1", "A", "B", 100_000), "phone"),
        ]);
        assert!(db.missing_on_device("alpha", "phone", 10).unwrap().is_empty());

        db.forget_holdings("phone", &["h1".to_string()]).unwrap();
        assert_eq!(db.missing_on_device("alpha", "phone", 10).unwrap().len(), 1);
    }

    #[test]
    fn archived_path_survives_a_later_report_without_one() {
        let db = Db::new_in_memory().unwrap();
        let mut t = track("h1", "A", "B", 100_000);
        t.archived_path = Some("A/Album/01 - B.flac".to_string());
        db.upsert_library_track(&t).unwrap();

        t.archived_path = None;
        db.upsert_library_track(&t).unwrap();

        assert_eq!(
            db.library_track("h1").unwrap().unwrap().archived_path.as_deref(),
            Some("A/Album/01 - B.flac")
        );
    }

    #[test]
    fn spool_evicts_oldest_first_when_over_budget() {
        let db = Db::new_in_memory().unwrap();
        for (hash, size) in [("a", 100), ("b", 100), ("c", 100)] {
            db.spool_insert(hash, size, "laptop", "alpha", 72).unwrap();
            // created_at has second resolution in RFC3339, so order the inserts explicitly.
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(db.spool_total_bytes().unwrap(), 300);

        let doomed = db.spool_evictable(150).unwrap();
        assert_eq!(doomed.len(), 2, "two must go to get under 150");
        assert_eq!(doomed[0].0, "a", "oldest first");
    }

    #[test]
    fn spool_keeps_everything_when_under_budget() {
        let db = Db::new_in_memory().unwrap();
        db.spool_insert("a", 100, "laptop", "alpha", 72).unwrap();
        assert!(db.spool_evictable(1_000).unwrap().is_empty());
    }
}
