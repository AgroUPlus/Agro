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
    pub lyrics: Option<String>,
    /// Where the lyrics came from — `LRCLIB`, `Native`, whatever the publishing client called it.
    ///
    /// Null on every row written before this column existed, and null whenever [`lyrics`] is: it
    /// describes the text and cannot be reconstructed without it.
    pub lyrics_source: Option<String>,
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
    pub lyrics: Option<String>,
    /// What the publishing client says supplied [`lyrics`]. Ignored when there are none.
    pub lyrics_source: Option<String>,
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

/// Packs L2-normalised floats to int8 at a fixed 127× scale. Inverse of [`unpack`].
pub fn quantise(vectors: &[f32]) -> Vec<u8> {
    vectors
        .iter()
        .map(|v| ((v * 127.0).round().clamp(-127.0, 127.0) as i8) as u8)
        .collect()
}

mod publish;
mod read;

#[cfg(test)]
mod tests;
