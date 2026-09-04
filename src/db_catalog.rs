//! The shared fingerprint catalogue.
//!
//! Clients identify their own recordings without any of this — the embedder and its index run
//! entirely on the device, and a client with no Agro server behind it loses nothing it had. What
//! the catalogue adds is that the work is done once for everyone, and that a source with poor tags
//! inherits the metadata a source with good ones supplied for the same audio.
//!
//! Matching here is the same three steps as on the client, in the same order and at the same
//! thresholds, so that both ends agree about what "the same recording" means: duration narrows the
//! field in SQL, a mean vector rejects the rest cheaply, and a full sequence comparison decides.
//! Disagreeing thresholds would mean a client publishing audio the server then failed to merge.
//!
//! ## Why int8
//!
//! The client stores float32 and sends int8. `AudioEmbedder` L2-normalises every vector before it
//! is stored, so each value already lies in [-1, 1] and a fixed 127× scale loses only what the
//! last mantissa bits held. It is a quarter of the bytes for a change in cosine similarity that
//! does not reach the second decimal place — and at 1 KB per second of audio, float32 puts a
//! three-minute track past the request cap on its own.

use crate::db::Db;
use rusqlite::{params, OptionalExtension, Result};

/// How alike two sequences must be to be one recording.
///
/// Mirrors `RecordingIdentityRepository.matchThreshold`. Two encodings of one recording score at
/// or above this and usually well above; unrelated audio, a cover, or a different arrangement sits
/// below 0.70. Failing to merge costs a duplicate row; merging wrongly attaches one recording's
/// metadata to another's audio, which is why this sits nearer the strict end.
const MATCH_THRESHOLD: f64 = 0.88;

/// How alike two mean vectors must be before the full sequence is worth comparing.
///
/// Mirrors `RecordingIdentityRepository.meanSimThreshold`. The sequence comparison is quadratic in
/// segment count — a pair of three-minute tracks is millions of dot products — so nothing reaches
/// it without passing here first.
const MEAN_THRESHOLD: f64 = 0.75;

/// How far two durations may differ and still be the same recording.
///
/// Mirrors `TrackDeduplicator.DURATION_TOLERANCE_MS`. Applied in SQL, so it is the only filter that
/// costs nothing per candidate.
const DURATION_TOLERANCE_MS: i64 = 3_000;

/// Candidates whose full sequence is read. Ordered by mean similarity, so this is a cap on work
/// rather than on correctness.
const MAX_CANDIDATES: usize = 20;

/// One recording as the catalogue knows it.
#[derive(Debug, Clone)]
pub struct CatalogRecording {
    pub recording_id: String,
    pub embedding: Vec<u8>,
    pub dim: i64,
    pub model: String,
    pub version: i64,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub updated_at: i64,
}

/// What a client publishes about one recording it embedded.
#[derive(Debug, Clone)]
pub struct PublishedRecording {
    /// `segments * dim` int8 values, segment-major. See the module note on why int8.
    pub embedding: Vec<u8>,
    pub dim: i64,
    /// Which embedder produced it. A vector from another model is a different alphabet.
    pub model: String,
    pub version: i64,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// The namespaced id this client knows the audio by — `ytm:…`, `navidrome:…`.
    ///
    /// Never a `local:` id: those are filesystem paths from somebody's phone, and this column is
    /// returned verbatim to every account on the server.
    pub source_uri: Option<String>,
}

/// Unpacks an int8 blob into rows of [`dim`] floats, undoing the 127× scale.
pub fn unpack(blob: &[u8], dim: usize) -> Vec<Vec<f32>> {
    if dim == 0 {
        return Vec::new();
    }
    blob.chunks_exact(dim)
        .map(|row| row.iter().map(|b| f32::from(*b as i8) / 127.0).collect())
        .collect()
}

/// The mean of a sequence, L2-normalised — the cheap stand-in a candidate is filtered on.
pub fn mean_vector(vectors: &[Vec<f32>]) -> Vec<f32> {
    let Some(dim) = vectors.first().map(Vec::len) else {
        return Vec::new();
    };
    let mut mean = vec![0.0f32; dim];
    for v in vectors {
        for (i, value) in v.iter().enumerate() {
            mean[i] += value;
        }
    }
    let count = vectors.len() as f32;
    let mut norm_sq = 0.0f32;
    for value in &mut mean {
        *value /= count;
        norm_sq += *value * *value;
    }
    let norm = norm_sq.sqrt();
    if norm > 0.0 {
        for value in &mut mean {
            *value /= norm;
        }
    }
    mean
}

fn dot(a: &[f32], b: &[f32]) -> f64 {
    f64::from(a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>())
}

/// Symmetric sequence similarity: the average of the best cosine match in both directions.
///
/// Symmetric on purpose. Taking one direction alone would score a short excerpt against the track
/// it came from as a perfect match, because every one of its segments finds a partner.
pub fn sequence_similarity(a: &[Vec<f32>], b: &[Vec<f32>]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let best_mean = |from: &[Vec<f32>], to: &[Vec<f32>]| -> f64 {
        from.iter()
            .map(|v| {
                to.iter()
                    .map(|w| dot(v, w))
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .sum::<f64>()
            / from.len() as f64
    };
    (best_mean(a, b) + best_mean(b, a)) / 2.0
}

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
                let conn = self.conn.lock().unwrap();
                conn.execute(
                    "UPDATE catalog_recordings SET
                         title      = COALESCE(title, ?2),
                         artist     = COALESCE(artist, ?3),
                         album      = COALESCE(album, ?4),
                         updated_at = ?5
                     WHERE recording_id = ?1",
                    params![id, published.title, published.artist, published.album, now],
                )?;
                id
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                let mean = quantise(&mean_vector(&vectors));
                let conn = self.conn.lock().unwrap();
                conn.execute(
                    "INSERT INTO catalog_recordings
                         (recording_id, duration_ms, title, artist, album, updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6)",
                    params![
                        id,
                        published.duration_ms,
                        published.title,
                        published.artist,
                        published.album,
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
                let conn = self.conn.lock().unwrap();
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
    fn candidate_recordings(
        &self,
        duration_ms: i64,
        model: &str,
        version: i64,
    ) -> Result<Vec<(String, Vec<f32>, usize)>> {
        let conn = self.conn.lock().unwrap();
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

    fn embedding_blob(&self, recording_id: &str) -> Result<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT embedding FROM catalog_embeddings WHERE recording_id = ?1",
            params![recording_id],
            |row| row.get(0),
        )
        .optional()
    }

    /// Everything the catalogue learned since [`since`], oldest first.
    ///
    /// A plain timestamp cursor rather than a change log: the catalogue is a set of facts about
    /// audio, so a client that re-reads one it already has simply agrees with itself.
    pub fn catalog_since(&self, since: i64, limit: i64) -> Result<Vec<CatalogRecording>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT r.recording_id, e.embedding, e.dim, e.model, e.version,
                    r.duration_ms, r.title, r.artist, r.album, r.updated_at
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
                updated_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// The source ids known to hold one recording.
    pub fn sources_for_recording(&self, recording_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT source_uri FROM catalog_sources WHERE recording_id = ?1 ORDER BY source_uri")?;
        let rows = stmt.query_map(params![recording_id], |row| row.get(0))?;
        rows.collect()
    }

    /// The recording a source id is known to hold, if any.
    pub fn recording_for_source(&self, source_uri: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT recording_id FROM catalog_sources WHERE source_uri = ?1",
            params![source_uri],
            |row| row.get(0),
        )
        .optional()
    }
}

/// Packs L2-normalised floats to int8 at a fixed 127× scale. Inverse of [`unpack`].
pub fn quantise(vectors: &[f32]) -> Vec<u8> {
    vectors
        .iter()
        .map(|v| ((v * 127.0).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIM: usize = 128;

    /// A sequence of L2-normalised vectors, deterministic per seed.
    ///
    /// Normalised because that is the contract `AudioEmbedder` writes under and what makes a dot
    /// product a cosine — a test built on unnormalised vectors would pass thresholds this code
    /// would never see in practice.
    fn embedding(seed: u32) -> Vec<Vec<f32>> {
        (0..60u32)
            .map(|segment| {
                let mut v: Vec<f32> = (0..DIM as u32)
                    .map(|d| {
                        // Avalanched rather than a linear combination of the three indices. A
                        // linear one makes each sequence a lattice, and two seeds then differ by a
                        // shift rather than in direction — every pair scored above 0.98 and the
                        // "unrelated audio" tests passed for the wrong reason.
                        let h = mix(seed ^ mix(segment ^ mix(d)));
                        (h as f32 / u32::MAX as f32) - 0.5
                    })
                    .collect();
                normalise(&mut v);
                v
            })
            .collect()
    }

    /// A 32-bit finaliser, so one changed input bit changes half the output bits.
    fn mix(mut x: u32) -> u32 {
        x ^= x >> 16;
        x = x.wrapping_mul(0x7feb_352d);
        x ^= x >> 15;
        x = x.wrapping_mul(0x846c_a68b);
        x ^= x >> 16;
        x
    }

    /// The same audio through a lossy encoder: every value nudged, the shape kept.
    fn degraded(vectors: &[Vec<f32>]) -> Vec<Vec<f32>> {
        vectors
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let mut out: Vec<f32> = v
                    .iter()
                    .enumerate()
                    .map(|(d, value)| value + if (i + d) % 7 == 0 { 0.01 } else { -0.004 })
                    .collect();
                normalise(&mut out);
                out
            })
            .collect()
    }

    fn normalise(v: &mut [f32]) {
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in v.iter_mut() {
                *x /= norm;
            }
        }
    }

    fn blob(vectors: &[Vec<f32>]) -> Vec<u8> {
        vectors.iter().flat_map(|v| quantise(v)).collect()
    }

    fn published(vectors: &[Vec<f32>], title: &str, source: &str) -> PublishedRecording {
        PublishedRecording {
            embedding: blob(vectors),
            dim: DIM as i64,
            model: "nmfp-triplet".to_string(),
            version: 1,
            duration_ms: 210_000,
            title: Some(title.to_string()),
            artist: Some("An Artist".to_string()),
            album: None,
            source_uri: Some(source.to_string()),
        }
    }

    #[test]
    fn a_first_publish_creates_a_recording() {
        let db = Db::new_in_memory().unwrap();
        let id = db
            .publish_recording(&published(&embedding(1), "Memories", "ytm:aaa"))
            .unwrap();
        assert!(!id.is_empty());
        assert_eq!(
            db.recording_for_source("ytm:aaa").unwrap().as_deref(),
            Some(id.as_str())
        );
    }

    /// The point of the whole catalogue: two encodings of one performance become one recording.
    #[test]
    fn a_re_encode_merges_into_the_recording_it_matches() {
        let db = Db::new_in_memory().unwrap();
        let original = embedding(2);
        let first = db
            .publish_recording(&published(&original, "Memories", "ytm:aaa"))
            .unwrap();
        let second = db
            .publish_recording(&published(&degraded(&original), "Memories", "navidrome:bbb"))
            .unwrap();

        assert_eq!(first, second, "a re-encode became a second recording");
        assert_eq!(db.catalog_since(0, 10).unwrap().len(), 1);
        assert_eq!(
            db.sources_for_recording(&first).unwrap(),
            vec!["navidrome:bbb".to_string(), "ytm:aaa".to_string()]
        );
    }

    #[test]
    fn unrelated_audio_does_not_merge() {
        let db = Db::new_in_memory().unwrap();
        let first = db
            .publish_recording(&published(&embedding(3), "One", "ytm:one"))
            .unwrap();
        let second = db
            .publish_recording(&published(&embedding(4), "Two", "ytm:two"))
            .unwrap();

        assert_ne!(first, second);
        assert_eq!(db.catalog_since(0, 10).unwrap().len(), 2);
    }

    /// A vector from another embedder is a different alphabet: comparing across them would be
    /// confident nonsense, so it never becomes a candidate.
    #[test]
    fn a_different_model_never_matches() {
        let db = Db::new_in_memory().unwrap();
        let audio = embedding(5);
        let first = db.publish_recording(&published(&audio, "One", "ytm:a")).unwrap();

        let mut other = published(&audio, "One", "ytm:b");
        other.model = "some-other-embedder".to_string();
        let second = db.publish_recording(&other).unwrap();

        assert_ne!(first, second, "the same audio merged across two models");
    }

    #[test]
    fn a_different_duration_never_matches() {
        let db = Db::new_in_memory().unwrap();
        let audio = embedding(6);
        let first = db.publish_recording(&published(&audio, "One", "ytm:a")).unwrap();

        let mut longer = published(&audio, "One", "ytm:b");
        longer.duration_ms = 210_000 + 30_000;
        let second = db.publish_recording(&longer).unwrap();

        assert_ne!(first, second, "audio 30 s longer merged anyway");
    }

    #[test]
    fn metadata_is_filled_in_and_never_overwritten() {
        let db = Db::new_in_memory().unwrap();
        let audio = embedding(7);
        db.publish_recording(&published(&audio, "The Real Title", "ytm:a"))
            .unwrap();

        let mut worse = published(&degraded(&audio), "track01", "ytm:b");
        worse.album = Some("An Album".to_string());
        db.publish_recording(&worse).unwrap();

        let entry = &db.catalog_since(0, 10).unwrap()[0];
        assert_eq!(entry.title.as_deref(), Some("The Real Title"), "title overwritten");
        assert_eq!(entry.album.as_deref(), Some("An Album"), "album not filled in");
    }

    /// A client asks for what it has not seen, and gets it in an order it can resume from.
    #[test]
    fn the_catalogue_is_readable_from_a_cursor() {
        let db = Db::new_in_memory().unwrap();
        db.publish_recording(&published(&embedding(8), "One", "ytm:8")).unwrap();
        db.publish_recording(&published(&embedding(9), "Two", "ytm:9")).unwrap();

        let all = db.catalog_since(0, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all[0].updated_at <= all[1].updated_at, "oldest first");

        let newest = all.iter().map(|r| r.updated_at).max().unwrap();
        assert!(
            db.catalog_since(newest, 10).unwrap().is_empty(),
            "a caught-up client gets nothing"
        );
    }

    #[test]
    fn an_empty_embedding_is_ignored_rather_than_stored() {
        let db = Db::new_in_memory().unwrap();
        let id = db
            .publish_recording(&PublishedRecording {
                embedding: Vec::new(),
                dim: DIM as i64,
                model: "nmfp-triplet".to_string(),
                version: 1,
                duration_ms: 0,
                title: None,
                artist: None,
                album: None,
                source_uri: Some("ytm:empty".to_string()),
            })
            .unwrap();
        assert!(id.is_empty());
        assert!(db.catalog_since(0, 10).unwrap().is_empty());
    }

    #[test]
    fn sequence_similarity_is_one_for_identical_and_low_for_unrelated() {
        let a = embedding(10);
        assert!((sequence_similarity(&a, &a) - 1.0).abs() < 1e-6);

        let unrelated = sequence_similarity(&a, &embedding(11));
        assert!(unrelated < MATCH_THRESHOLD, "unrelated scored {unrelated}");
    }

    /// The claim int8 rests on: a round trip through the wire format still matches the original.
    #[test]
    fn quantising_to_int8_does_not_cost_a_match() {
        let original = embedding(12);
        let round_tripped = unpack(&blob(&original), DIM);

        let score = sequence_similarity(&original, &round_tripped);
        assert!(
            score >= MATCH_THRESHOLD,
            "int8 round trip scored {score}, below the match threshold"
        );
    }
}
