//! The library index: one row per recording, and the stats over it.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use crate::norm::recording_key;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    pub fn library_track(&self, content_hash: &str) -> Result<Option<LibraryTrack>> {
        let conn = self.read();
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
        let conn = self.conn.lock_recover();
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
        let conn = self.conn.lock_recover();
        conn.execute(
            "UPDATE library_tracks SET archived_path = ?2, updated_at = ?3 WHERE content_hash = ?1",
            params![content_hash, path, chrono::Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    // ── Holdings ────────────────────────────────────────────────────────────────────────────

    /// Totals for one account's library.
    ///
    /// `include_archive` is what separates "my music" from "everything on this server". With it
    /// always on — which is how this behaved — a member opening the dashboard was shown the
    /// operator's whole archive as their own fleet total.
    pub fn library_stats(&self, user_id: &str, include_archive: bool) -> Result<LibraryStats> {
        let conn = self.read();
        let (track_count, archived_count, total_bytes) = conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN t.archived_path IS NOT NULL THEN 1 ELSE 0 END), 0),
                    COALESCE(SUM(t.size_bytes), 0)
             FROM library_tracks t
             WHERE EXISTS (SELECT 1 FROM device_holdings h
                           WHERE h.content_hash = t.content_hash AND h.user_id = ?1)
                OR (?2 AND t.archived_path IS NOT NULL)",
            params![user_id, include_archive],
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

    // ── Browsing ────────────────────────────────────────────────────────────────────────────

    /// Removes a track from the library index entirely.
    pub fn delete_library_item(&self, kind: BrowseKind, id: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        match kind {
            BrowseKind::Track => {
                conn.execute(
                    "DELETE FROM device_holdings WHERE content_hash = ?1",
                    rusqlite::params![id],
                )?;
                let deleted = conn.execute(
                    "DELETE FROM library_tracks WHERE content_hash = ?1",
                    rusqlite::params![id],
                )?;
                Ok(deleted > 0)
            }
            BrowseKind::Album => {
                let mut parts = id.splitn(2, '');
                let album_artist = parts.next().unwrap_or("");
                let album = parts.next().unwrap_or("");
                conn.execute(
                    "DELETE FROM device_holdings WHERE content_hash IN (SELECT content_hash FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1 AND COALESCE(album, '') = ?2)",
                    rusqlite::params![album_artist, album]
                )?;
                let deleted = conn.execute(
                    "DELETE FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1 AND COALESCE(album, '') = ?2",
                    rusqlite::params![album_artist, album]
                )?;
                Ok(deleted > 0)
            }
            BrowseKind::Artist => {
                conn.execute(
                    "DELETE FROM device_holdings WHERE content_hash IN (SELECT content_hash FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1)",
                    rusqlite::params![id]
                )?;
                let deleted = conn.execute(
                    "DELETE FROM library_tracks WHERE COALESCE(album_artist, artist) = ?1",
                    rusqlite::params![id],
                )?;
                Ok(deleted > 0)
            }
        }
    }

    pub fn delete_library_track(&self, content_hash: &str) -> Result<bool> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "DELETE FROM device_holdings WHERE content_hash = ?1",
            params![content_hash],
        )?;
        let deleted = conn.execute(
            "DELETE FROM library_tracks WHERE content_hash = ?1",
            params![content_hash],
        )?;
        Ok(deleted > 0)
    }
}
