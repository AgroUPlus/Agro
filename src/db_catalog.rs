//! The shared fingerprint catalogue.
//!
//! Clients identify their own recordings without any of this — the fingerprinter and its index run
//! entirely on the device, and a client with no Agro server behind it loses nothing it had. What
//! the catalogue adds is that the work is done once for everyone, and that a source with poor tags
//! inherits the metadata a source with good ones supplied for the same audio.
//!
//! Matching here is the same two steps as on the client, for the same reason: sixteen-bit halves
//! of each sub-hash propose candidates cheaply, and a comparison of the whole sequence decides. An
//! index on whole sub-hashes would be useless, because one flipped bit changes the entire value
//! and a lossy re-encode flips many.

use crate::db::Db;
use rusqlite::{params, OptionalExtension, Result};

/// Bits in one sub-hash. Matches the client's fingerprinter exactly; they compare across the wire.
const BITS: u32 = 32;

/// How many index hits before a candidate's full sequence is worth reading.
const MIN_CANDIDATE_HITS: i64 = 4;

/// How alike two sequences must be to be one recording.
///
/// Unrelated recordings sit at chance, because unrelated bits agree half the time. A copy that has
/// been through a lossy encoder sits well above that and nowhere near a perfect match, so this
/// sits between them and nearer the noisy end: failing to merge costs a duplicate row, and merging
/// wrongly attaches one recording's metadata to another's audio.
const MATCH_THRESHOLD: f64 = 0.72;

/// One recording as the catalogue knows it.
#[derive(Debug, Clone)]
pub struct CatalogRecording {
    pub recording_id: String,
    pub sub_hashes: Vec<u8>,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub updated_at: i64,
}

/// What a client publishes about one recording it fingerprinted.
#[derive(Debug, Clone)]
pub struct PublishedRecording {
    pub sub_hashes: Vec<u8>,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// The namespaced id this client knows the audio by — `ytm:…`, `navidrome:…`, `local:…`.
    pub source_uri: Option<String>,
}

/// The fraction of bits two fingerprints agree on, over the shorter of the two.
pub fn similarity(a: &[i32], b: &[i32]) -> f64 {
    let length = a.len().min(b.len());
    if length == 0 {
        return 0.0;
    }
    let agreeing: u32 = (0..length)
        .map(|i| BITS - (a[i] ^ b[i]).count_ones())
        .sum();
    f64::from(agreeing) / (length as f64 * f64::from(BITS))
}

/// Sixteen-bit halves of every sub-hash, each tagged with the end it came from.
///
/// Tagged because the halves share a value space: without it a low half would collide with some
/// other recording's high half and inflate its score for no reason.
pub fn halves(hashes: &[i32]) -> Vec<i64> {
    let mut out: Vec<i64> = hashes
        .iter()
        .flat_map(|hash| {
            let low = i64::from(*hash & 0xFFFF);
            let high = i64::from((*hash >> 16) & 0xFFFF) | (1 << 16);
            [low, high]
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub fn decode_hashes(bytes: &[u8]) -> Vec<i32> {
    bytes
        .chunks_exact(4)
        .map(|c| i32::from_be_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

impl Db {
    /// Files one client's fingerprint into the catalogue, merging it with a match if there is one.
    ///
    /// Returns the recording it ended up under, so the caller can tell the client which id its
    /// source now maps to.
    pub fn publish_recording(&self, published: &PublishedRecording) -> Result<String> {
        let hashes = decode_hashes(&published.sub_hashes);
        if hashes.is_empty() {
            return Ok(String::new());
        }
        let now = chrono::Utc::now().timestamp();

        let existing = self.match_recording(&hashes)?;
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
                    params![
                        id,
                        published.title,
                        published.artist,
                        published.album,
                        now
                    ],
                )?;
                id
            }
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                {
                    let conn = self.conn.lock().unwrap();
                    conn.execute(
                        "INSERT INTO catalog_recordings
                             (recording_id, sub_hashes, duration_ms, title, artist, album, updated_at)
                         VALUES (?1,?2,?3,?4,?5,?6,?7)",
                        params![
                            id,
                            published.sub_hashes,
                            published.duration_ms,
                            published.title,
                            published.artist,
                            published.album,
                            now
                        ],
                    )?;
                    let mut insert = conn.prepare(
                        "INSERT OR IGNORE INTO catalog_sub_hashes (half, recording_id) VALUES (?1, ?2)",
                    )?;
                    for half in halves(&hashes) {
                        insert.execute(params![half, id])?;
                    }
                }
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

    /// The recording [`hashes`] belongs to, if the catalogue already holds it.
    pub fn match_recording(&self, hashes: &[i32]) -> Result<Option<String>> {
        let candidates = self.candidate_recordings(hashes)?;
        let mut best: Option<(String, f64)> = None;
        for (id, sub_hashes) in candidates {
            let score = similarity(hashes, &decode_hashes(&sub_hashes));
            if score >= MATCH_THRESHOLD && best.as_ref().is_none_or(|(_, b)| score > *b) {
                best = Some((id, score));
            }
        }
        Ok(best.map(|(id, _)| id))
    }

    /// Recordings sharing enough sub-hash halves to be worth comparing in full.
    fn candidate_recordings(&self, hashes: &[i32]) -> Result<Vec<(String, Vec<u8>)>> {
        let halves = halves(hashes);
        if halves.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = vec!["?"; halves.len()].join(",");
        let sql = format!(
            "SELECT r.recording_id, r.sub_hashes
             FROM catalog_sub_hashes h
             JOIN catalog_recordings r ON r.recording_id = h.recording_id
             WHERE h.half IN ({placeholders})
             GROUP BY h.recording_id
             HAVING COUNT(*) >= {MIN_CANDIDATE_HITS}
             ORDER BY COUNT(*) DESC
             LIMIT 20"
        );
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(halves.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.collect()
    }

    /// Everything the catalogue learned since [`since`], oldest first.
    ///
    /// A plain timestamp cursor rather than a change log: the catalogue is a set of facts about
    /// audio, so a client that re-reads one it already has simply agrees with itself.
    pub fn catalog_since(&self, since: i64, limit: i64) -> Result<Vec<CatalogRecording>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT recording_id, sub_hashes, duration_ms, title, artist, album, updated_at
             FROM catalog_recordings
             WHERE updated_at > ?1
             ORDER BY updated_at
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![since, limit], |row| {
            Ok(CatalogRecording {
                recording_id: row.get(0)?,
                sub_hashes: row.get(1)?,
                duration_ms: row.get(2)?,
                title: row.get(3)?,
                artist: row.get(4)?,
                album: row.get(5)?,
                updated_at: row.get(6)?,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fingerprint as the wire carries it: four bytes per sub-hash, big-endian.
    fn bytes(hashes: &[i32]) -> Vec<u8> {
        hashes.iter().flat_map(|h| h.to_be_bytes()).collect()
    }

    /// A long, varied sequence, so halves do not collide by accident.
    fn fingerprint(seed: i32) -> Vec<i32> {
        (0..300i32)
            .map(|i| {
                seed.wrapping_mul(0x9E37_79B9u32 as i32)
                    .wrapping_add(i.wrapping_mul(0x0100_1001))
            })
            .collect()
    }

    /// The same audio, re-encoded: most sub-hashes intact, some flipped.
    fn degraded(hashes: &[i32]) -> Vec<i32> {
        hashes
            .iter()
            .enumerate()
            .map(|(i, h)| if i % 5 == 0 { h ^ 0x0004_0001 } else { *h })
            .collect()
    }

    fn published(hashes: &[i32], title: &str, source: &str) -> PublishedRecording {
        PublishedRecording {
            sub_hashes: bytes(hashes),
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
        let print = fingerprint(1);

        let id = db.publish_recording(&published(&print, "Memories", "local:aaa")).unwrap();
        assert!(!id.is_empty());
        assert_eq!(db.recording_for_source("local:aaa").unwrap().as_deref(), Some(id.as_str()));
    }

    /// The point of the whole catalogue: two encodings of one performance become one recording.
    #[test]
    fn a_re_encode_merges_into_the_recording_it_matches() {
        let db = Db::new_in_memory().unwrap();
        let print = fingerprint(2);

        let first = db.publish_recording(&published(&print, "Memories", "navidrome:1")).unwrap();
        let second = db
            .publish_recording(&published(&degraded(&print), "Memories", "ytm:abc"))
            .unwrap();

        assert_eq!(first, second, "a re-encode should not create a second recording");
        let mut sources = db.sources_for_recording(&first).unwrap();
        sources.sort();
        assert_eq!(sources, vec!["navidrome:1".to_string(), "ytm:abc".to_string()]);
    }

    /// And the half that keeps it honest: different audio stays different.
    #[test]
    fn unrelated_audio_does_not_merge() {
        let db = Db::new_in_memory().unwrap();

        let first = db.publish_recording(&published(&fingerprint(3), "One", "local:1")).unwrap();
        let second = db.publish_recording(&published(&fingerprint(4), "Two", "local:2")).unwrap();

        assert_ne!(first, second);
    }

    /// A source with no tags inherits what a source with tags already supplied.
    #[test]
    fn metadata_is_filled_in_and_never_overwritten() {
        let db = Db::new_in_memory().unwrap();
        let print = fingerprint(5);

        db.publish_recording(&published(&print, "Real Title", "navidrome:9")).unwrap();
        let id = db
            .publish_recording(&PublishedRecording {
                sub_hashes: bytes(&degraded(&print)),
                duration_ms: 210_000,
                title: Some("Real Title (Official Video) [HQ]".to_string()),
                artist: None,
                album: Some("An Album".to_string()),
                source_uri: Some("ytm:zzz".to_string()),
            })
            .unwrap();

        let entry = db.catalog_since(0, 10).unwrap().into_iter().find(|r| r.recording_id == id).unwrap();
        assert_eq!(entry.title.as_deref(), Some("Real Title"), "a worse title must not win");
        assert_eq!(entry.artist.as_deref(), Some("An Artist"));
        assert_eq!(entry.album.as_deref(), Some("An Album"), "a gap should still be filled");
    }

    /// A client asks for what it has not seen, and gets it in an order it can resume from.
    #[test]
    fn the_catalogue_is_readable_from_a_cursor() {
        let db = Db::new_in_memory().unwrap();
        db.publish_recording(&published(&fingerprint(6), "One", "local:6")).unwrap();
        db.publish_recording(&published(&fingerprint(7), "Two", "local:7")).unwrap();

        let all = db.catalog_since(0, 10).unwrap();
        assert_eq!(all.len(), 2);
        assert!(all[0].updated_at <= all[1].updated_at, "oldest first");

        let newest = all.iter().map(|r| r.updated_at).max().unwrap();
        assert!(db.catalog_since(newest, 10).unwrap().is_empty(), "a caught-up client gets nothing");
    }

    #[test]
    fn an_empty_fingerprint_is_ignored_rather_than_stored() {
        let db = Db::new_in_memory().unwrap();
        let id = db
            .publish_recording(&PublishedRecording {
                sub_hashes: Vec::new(),
                duration_ms: 0,
                title: None,
                artist: None,
                album: None,
                source_uri: Some("local:empty".to_string()),
            })
            .unwrap();
        assert!(id.is_empty());
        assert!(db.catalog_since(0, 10).unwrap().is_empty());
    }

    #[test]
    fn similarity_is_one_for_identical_and_near_half_for_unrelated() {
        let a = fingerprint(8);
        assert_eq!(similarity(&a, &a), 1.0);
        let unrelated = similarity(&a, &fingerprint(9));
        assert!(unrelated < 0.72, "unrelated scored {unrelated}");
    }
}
