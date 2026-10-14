use super::*;

const DIM: usize = 128;
use crate::db::Db;

/// A sequence of L2-normalised vectors, deterministic per seed.
///
/// Normalised because that is the contract `AudioEmbedder` writes under and what makes a dot
/// product a cosine — a test built on unnormalised vectors would pass thresholds this code
/// would never see in practice.
pub(super) fn embedding(seed: u32) -> Vec<Vec<f32>> {
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
pub(super) fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// The same audio through a lossy encoder: every value nudged, the shape kept.
pub(super) fn degraded(vectors: &[Vec<f32>]) -> Vec<Vec<f32>> {
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

pub(super) fn normalise(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

pub(super) fn blob(vectors: &[Vec<f32>]) -> Vec<u8> {
    vectors.iter().flat_map(|v| quantise(v)).collect()
}

pub(super) fn published(vectors: &[Vec<f32>], title: &str, source: &str) -> PublishedRecording {
    PublishedRecording {
        embedding: blob(vectors),
        dim: DIM as i64,
        model: "nmfp-triplet".to_string(),
        version: 1,
        duration_ms: 210_000,
        title: Some(title.to_string()),
        artist: Some("An Artist".to_string()),
        album: None,
        lyrics: None,
        lyrics_source: None,
        source_uri: Some(source.to_string()),
    }
}

mod matching_math;
mod publishing;
mod reading;
