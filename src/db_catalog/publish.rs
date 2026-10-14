//! Publishing a recording and matching it against the catalogue.

use super::*;
use crate::db::Db;
use crate::lock::LockExt;
use rusqlite::{params, OptionalExtension, Result};

impl Db {
    /// Files one client's embedding into the catalogue, merging it with a match if there is one.
    ///
    /// Returns the recording it ended up under, so the caller can tell the client which id its
    /// source now maps to.
    pub fn publish_recording(&self, published: &PublishedRecording) -> Result<String> {
        let dim = published.dim.max(0) as usize;
        let vectors = unpack(&published.embedding, dim);
        if vectors.is_empty() {
            return Ok(String::new());
        }
        let now = chrono::Utc::now().timestamp();

        let existing = self.match_recording(
            &vectors,
            published.duration_ms,
            &published.model,
            published.version,
        )?;
        let recording_id = match existing {
            Some(id) => {
                // Metadata is filled in, never overwritten. The first client to supply a title
                // usually had tags worth having; a later one may be the source with none.
                let conn = self.conn.lock_recover();
                conn.execute(
                    // `lyrics_source` is filled in only alongside the lyrics it describes: a row
                    // that already holds text keeps the attribution that came with it, and a row
                    // that gains text now gains this client's attribution with it.
                    "UPDATE catalog_recordings SET
                         title         = COALESCE(title, ?2),
                         artist        = COALESCE(artist, ?3),
                         album         = COALESCE(album, ?4),
                         lyrics_source = CASE WHEN lyrics IS NULL AND ?5 IS NOT NULL
                                              THEN ?6 ELSE lyrics_source END,
                         lyrics        = COALESCE(lyrics, ?5),
                         updated_at    = ?7
                     WHERE recording_id = ?1",
                    params![
                        id,
                        published.title,
                        published.artist,
                        published.album,
                        published.lyrics,
                        published.lyrics_source,
                        now
                    ],
                )?;
                id
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                let mean = quantise(&mean_vector(&vectors));
                let conn = self.conn.lock_recover();
                conn.execute(
                    "INSERT INTO catalog_recordings
                         (recording_id, duration_ms, title, artist, album, lyrics, lyrics_source,
                          updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        id,
                        published.duration_ms,
                        published.title,
                        published.artist,
                        published.album,
                        published.lyrics,
                        published
                            .lyrics
                            .as_ref()
                            .and(published.lyrics_source.as_ref()),
                        now
                    ],
                )?;
                conn.execute(
                    "INSERT INTO catalog_embeddings
                         (recording_id, embedding, mean, dim, segments, model, version, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        id,
                        published.embedding,
                        mean,
                        published.dim,
                        vectors.len() as i64,
                        published.model,
                        published.version,
                        now
                    ],
                )?;
                id
            }
        };

        if let Some(source_uri) = published.source_uri.as_deref().map(str::trim) {
            if !source_uri.is_empty() {
                let conn = self.conn.lock_recover();
                conn.execute(
                    "INSERT INTO catalog_sources (source_uri, recording_id, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT(source_uri) DO UPDATE SET
                         recording_id = excluded.recording_id,
                         updated_at   = excluded.updated_at",
                    params![source_uri, recording_id, now],
                )?;
            }
        }

        // Joined to an artist row here rather than when somebody asks. A recording published while
        // nobody was subscribed would otherwise never belong to an artist at all, and so would be
        // invisible to a subscriber who arrived a minute later — which is most of them.
        //
        // The name is read back from the row rather than taken from `published`: on a match the
        // metadata was filled in with `COALESCE`, so the artist that stuck may be the one an
        // earlier client supplied and this one left null.
        let artist: Option<String> = self
            .conn
            .lock_recover()
            .query_row(
                "SELECT artist FROM catalog_recordings WHERE recording_id = ?1",
                params![recording_id],
                |row| row.get(0),
            )
            .optional()?
            .flatten();
        self.link_recording_artist(&recording_id, artist.as_deref())?;

        Ok(recording_id)
    }

    /// The recording these vectors belong to, if the catalogue already holds it.
    pub fn match_recording(
        &self,
        vectors: &[Vec<f32>],
        duration_ms: i64,
        model: &str,
        version: i64,
    ) -> Result<Option<String>> {
        let query_mean = mean_vector(vectors);
        let mut shortlist = self.candidate_recordings(duration_ms, model, version)?;
        shortlist.retain(|(_, mean, _)| dot(&query_mean, mean) >= MEAN_THRESHOLD);
        shortlist.sort_by(|a, b| {
            dot(&query_mean, &b.1)
                .partial_cmp(&dot(&query_mean, &a.1))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        shortlist.truncate(MAX_CANDIDATES);

        let mut best: Option<(String, f64)> = None;
        for (id, _, dim) in shortlist {
            let Some(blob) = self.embedding_blob(&id)? else {
                continue;
            };
            let score = sequence_similarity(vectors, &unpack(&blob, dim));
            if score >= MATCH_THRESHOLD && best.as_ref().is_none_or(|(_, b)| score > *b) {
                best = Some((id, score));
            }
        }
        Ok(best.map(|(id, _)| id))
    }

    /// Recordings of about the right length, with their mean vectors, for the cheap filter.
    ///
    /// Only the means are read here. The full sequences are megabytes and most candidates are
    /// rejected on the mean alone.
    pub(super) fn candidate_recordings(
        &self,
        duration_ms: i64,
        model: &str,
        version: i64,
    ) -> Result<Vec<(String, Vec<f32>, usize)>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT e.recording_id, e.mean, e.dim
             FROM catalog_embeddings e
             JOIN catalog_recordings r ON r.recording_id = e.recording_id
             WHERE e.model = ?1 AND e.version = ?2
               AND ABS(r.duration_ms - ?3) <= ?4",
        )?;
        let rows = stmt.query_map(
            params![model, version, duration_ms, DURATION_TOLERANCE_MS],
            |row| {
                let id: String = row.get(0)?;
                let mean: Vec<u8> = row.get(1)?;
                let dim: i64 = row.get(2)?;
                Ok((id, mean, dim.max(0) as usize))
            },
        )?;
        let mut out = Vec::new();
        for row in rows {
            let (id, mean, dim) = row?;
            let unpacked = unpack(&mean, dim);
            out.push((id, unpacked.into_iter().next().unwrap_or_default(), dim));
        }
        Ok(out)
    }

    pub(super) fn embedding_blob(&self, recording_id: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.read();
        conn.query_row(
            "SELECT embedding FROM catalog_embeddings WHERE recording_id = ?1",
            params![recording_id],
            |row| row.get(0),
        )
        .optional()
    }
}
