//! Album cover bookkeeping.

use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    pub fn set_cover(
        &self,
        album_key: &str,
        album_artist: &str,
        album: &str,
        extension: &str,
    ) -> Result<()> {
        let conn = self.conn.lock_recover();
        conn.execute(
            "INSERT OR REPLACE INTO library_covers
                 (album_key, album_artist, album, extension, updated_at)
             VALUES (?1, ?2, ?3, ?4, strftime('%s', 'now'))",
            params![album_key, album_artist, album, extension],
        )?;
        Ok(())
    }

    /// The stored cover's file extension, or `None` when the album has no artwork.
    pub fn cover_extension(&self, album_key: &str) -> Result<Option<String>> {
        let conn = self.read();
        conn.query_row(
            "SELECT extension FROM library_covers WHERE album_key = ?1",
            params![album_key],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn has_cover(&self, album_key: &str) -> bool {
        matches!(self.cover_extension(album_key), Ok(Some(_)))
    }

    /// One archived file per album, for the backfill pass to read artwork out of.
    pub fn albums_for_cover_backfill(&self) -> Result<Vec<(String, String, String)>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT COALESCE(t.album_artist, t.artist), t.album, MIN(t.archived_path)
             FROM library_tracks t
             WHERE t.archived_path IS NOT NULL AND COALESCE(t.album, '') != ''
             GROUP BY COALESCE(t.album_artist, t.artist), t.album",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push((row.get(0)?, row.get(1)?, row.get(2)?));
        }
        Ok(out)
    }

    // ── Upload sessions ─────────────────────────────────────────────────────────────────────
}
