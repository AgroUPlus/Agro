//! Reading the catalogue back: cursors and sources.

use super::*;
use crate::db::Db;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// Everything the catalogue learned since [`since`], oldest first.
    ///
    /// A plain timestamp cursor rather than a change log: the catalogue is a set of facts about
    /// audio, so a client that re-reads one it already has simply agrees with itself.
    pub fn catalog_since(&self, since: i64, limit: i64) -> Result<Vec<CatalogRecording>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT r.recording_id, e.embedding, e.dim, e.model, e.version,
                    r.duration_ms, r.title, r.artist, r.album, r.lyrics, r.lyrics_source,
                    r.updated_at
             FROM catalog_recordings r
             JOIN catalog_embeddings e ON e.recording_id = r.recording_id
             WHERE r.updated_at > ?1
             ORDER BY r.updated_at
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since, limit], |row| {
            Ok(CatalogRecording {
                recording_id: row.get(0)?,
                embedding: row.get(1)?,
                dim: row.get(2)?,
                model: row.get(3)?,
                version: row.get(4)?,
                duration_ms: row.get(5)?,
                title: row.get(6)?,
                artist: row.get(7)?,
                album: row.get(8)?,
                lyrics: row.get(9)?,
                lyrics_source: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })?;
        rows.collect()
    }

    /// The source ids known to hold one recording.
    pub fn sources_for_recording(&self, recording_id: &str) -> Result<Vec<String>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT source_uri FROM catalog_sources WHERE recording_id = ?1 ORDER BY source_uri",
        )?;
        let rows = stmt.query_map(params![recording_id], |row| row.get(0))?;
        rows.collect()
    }

    /// The recording a source id is known to hold, if any.
    pub fn recording_for_source(&self, source_uri: &str) -> Result<Option<String>> {
        let conn = self.read();
        conn.query_row(
            "SELECT recording_id FROM catalog_sources WHERE source_uri = ?1",
            params![source_uri],
            |row| row.get(0),
        )
        .optional()
    }
}
