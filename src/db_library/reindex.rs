//! Re-deriving normalised keys after the normalisation convention changes.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use crate::norm::recording_key;
use rusqlite::{params, Result};

impl Db {
    /// Recomputes the `norm_*` columns from the metadata already stored.
    ///
    /// These columns are the matcher's whole index, and they are computed in Rust at insert
    /// (`upsert_library_track`, `add_playlist_item`) from `norm.rs`. That means a change to
    /// `norm.rs` — a new variant marker, a fix to the diacritic folding — leaves every existing row
    /// carrying the *old* convention, and no SQL `UPDATE` can repair them, because the regexes
    /// exist only here. Without this entry point the only remedy is re-reporting the entire library
    /// from every client, which is precisely the work the index exists to avoid.
    ///
    /// Both tables in one pass, deliberately. `norm.rs` opens by saying the values are computed in
    /// one place so the index holds one convention; reindexing one table and not the other would
    /// break exactly that, and a playlist matched against a freshly normalised library would
    /// quietly stop resolving.
    ///
    /// **Bounded and resumable.** Every row is read to find the stale ones — cheap, and it needs no
    /// lock beyond the connection's — but at most `batch` of each table is rewritten per call, so a
    /// large index is never one long write transaction blocking every other request. Re-running
    /// after [`ReindexOutcome::done`] is true is a scan and nothing else, which also makes this
    /// safe to call speculatively after a deploy.
    pub fn reindex_normalisation(&self, batch: usize) -> Result<ReindexOutcome> {
        let mut conn = self.conn.lock_recover();

        let stale_tracks: Vec<(String, String, String)> = {
            let mut stmt = conn.prepare(
                "SELECT content_hash, artist, title, norm_artist, norm_title, norm_variants
                 FROM library_tracks",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })?;
            let mut stale = Vec::new();
            for row in rows {
                let (hash, artist, title, was_artist, was_title, was_variants) = row?;
                let key = recording_key(&artist, &title);
                if key.artist != was_artist
                    || key.title != was_title
                    || key.variants != was_variants
                {
                    stale.push((hash, artist, title));
                }
            }
            stale
        };

        let stale_items: Vec<(String, String, String)> = {
            let mut stmt = conn
                .prepare("SELECT id, artist, title, norm_artist, norm_title FROM playlist_items")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })?;
            let mut stale = Vec::new();
            for row in rows {
                let (id, artist, title, was_artist, was_title) = row?;
                if crate::norm::normalize_artist(&artist) != was_artist
                    || crate::norm::normalize_title(&title) != was_title
                {
                    stale.push((id, artist, title));
                }
            }
            stale
        };

        let track_batch = stale_tracks.len().min(batch);
        let item_batch = stale_items.len().min(batch);

        let tx = conn.transaction()?;
        // `updated_at` is deliberately left alone. It records when a *client* last told the server
        // something about this file, and nothing about the file changed here — only the server's
        // own derived index did. Touching it would report a library-wide edit that never happened.
        for (hash, artist, title) in &stale_tracks[..track_batch] {
            let key = recording_key(artist, title);
            tx.execute(
                "UPDATE library_tracks
                 SET norm_artist = ?1, norm_title = ?2, norm_variants = ?3
                 WHERE content_hash = ?4",
                params![key.artist, key.title, key.variants, hash],
            )?;
        }
        for (id, artist, title) in &stale_items[..item_batch] {
            tx.execute(
                "UPDATE playlist_items SET norm_artist = ?1, norm_title = ?2 WHERE id = ?3",
                params![
                    crate::norm::normalize_artist(artist),
                    crate::norm::normalize_title(title),
                    id
                ],
            )?;
        }
        tx.commit()?;

        Ok(ReindexOutcome {
            tracks: track_batch,
            playlist_items: item_batch,
            done: track_batch == stale_tracks.len() && item_batch == stale_items.len(),
        })
    }
}
