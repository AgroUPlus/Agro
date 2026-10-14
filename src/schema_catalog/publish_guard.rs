//! Rate limits and input validation for publishing recordings.

use super::*;
use crate::auth::AuthedUser;
use crate::db_catalog::PublishedRecording;
use crate::rate_limit::FixedWindow;
use crate::schema::bounded;
use async_graphql::{Context, Result};
use std::time::Duration;

/// Charges one client's publishing budget, or says why it cannot.
///
/// Publishing is the one mutation here that is cheap to send and expensive to serve: every entry
/// runs a sequence comparison against up to twenty candidate recordings, each of which is read
/// from the database in full. Nothing bounded that before — any account could publish without
/// limit, and the catalogue records no submitter, so there was not even anything to point at
/// afterwards.
pub(super) fn spend_publish_quota(
    ctx: &Context<'_>,
    user: &AuthedUser,
    units: usize,
) -> Result<()> {
    // A server that predates this data being registered must not start refusing writes over a
    // missing limiter — the quota is a bound on abuse, not a correctness requirement.
    let Ok(limiter) = ctx.data::<PublishQuota>() else {
        return Ok(());
    };
    if limiter
        .0
        .charge(user.username(), units, MAX_PUBLISHES, PUBLISH_WINDOW)
    {
        return Ok(());
    }
    Err(async_graphql::Error::new(
        "too many recordings published just now; try again shortly",
    ))
}

/// Per-account publishing budget, in the GraphQL context so resolvers can reach it.
pub struct PublishQuota(pub FixedWindow);

/// Recordings one account may publish per [`PUBLISH_WINDOW`].
///
/// Set for the largest honest case rather than the typical one: a client that has just finished
/// measuring a big library has thousands of recordings to file and should be able to, over the
/// course of an evening, without being throttled into a crawl. What it stops is the case that has
/// no honest reading — one account filling the catalogue with generated embeddings faster than
/// anyone can notice.
pub(super) const MAX_PUBLISHES: usize = 300;

pub(super) const PUBLISH_WINDOW: Duration = Duration::from_secs(300);

/// Entries one request may carry.
///
/// Far smaller than the 500 the play-count batch allows, because these are not comparable units of
/// work: a play count is an upsert, and a recording is up to twenty full sequence comparisons.
pub(super) const MAX_PUBLISH_BATCH: usize = 25;

/// Checks one client's submission and turns it into what the database stores.
///
/// Returns the complaint as a plain `String` rather than a GraphQL error so a batch can report it
/// against the one entry it belongs to instead of failing the whole request.
pub(super) fn validate(
    input: PublishRecordingInput,
) -> std::result::Result<PublishedRecording, String> {
    // The dimension is checked before the length, because the length check is stated in terms
    // of it: a blob is only well formed relative to the width of one vector.
    if input.dim <= 0 || input.dim > MAX_DIM {
        return Err("dim is not a plausible vector width".to_string());
    }
    let decoded = decode_bytes_hex(&input.embedding).ok_or("embedding is not valid hex")?;
    if decoded.is_empty() || decoded.len() % (input.dim as usize) != 0 {
        return Err("embedding must be a whole number of vectors".to_string());
    }
    if decoded.len() > MAX_EMBEDDING_BYTES {
        return Err("embedding is too long".to_string());
    }
    let model = bounded(&input.model, MAX_MODEL_LEN, "model").map_err(|e| e.message)?;
    if model.is_empty() {
        return Err("an embedding must name its model".to_string());
    }
    // A `local:` id is a path on somebody's phone, and `sources` is returned to every account
    // on this server. Dropped here rather than trusted to the client: the boundary is the
    // server's to hold, and an older or modified client would otherwise publish them.
    let source_uri = input
        .source_uri
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.starts_with("local:"));

    let lyrics = match input.lyrics {
        Some(l) if !l.trim().is_empty() => {
            Some(bounded(&l, MAX_LYRICS_LEN, "lyrics").map_err(|e| e.message)?)
        }
        _ => None,
    };
    // Attribution without text describes nothing, so it is dropped with it.
    let lyrics_source = match input.lyrics_source {
        Some(s) if lyrics.is_some() && !s.trim().is_empty() => {
            Some(bounded(&s, MAX_LYRICS_SOURCE_LEN, "lyricsSource").map_err(|e| e.message)?)
        }
        _ => None,
    };

    Ok(PublishedRecording {
        embedding: decoded,
        dim: input.dim,
        model,
        version: input.version,
        duration_ms: input.duration_ms,
        title: input
            .title
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty()),
        artist: input
            .artist
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
        album: input
            .album
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
        lyrics,
        lyrics_source,
        source_uri,
    })
}

/// Twenty minutes of audio at int8: two vectors a second, 128 values each.
///
/// The client sends int8 precisely so this can stay a sane number — the same audio as float32 is
/// four times larger and a three-minute track alone would pass the old 160 KB cap.
pub(super) const MAX_EMBEDDING_BYTES: usize = 20 * 60 * 2 * 128;

/// Max length of lyrics payload (synced LRC or plain text) traded per recording.
pub(super) const MAX_LYRICS_LEN: usize = 65_536;

/// Long enough to name a source, short enough that it cannot become a second lyrics field.
pub(super) const MAX_LYRICS_SOURCE_LEN: usize = 64;

/// A plausible width for one vector. The embedder in use produces 128.
pub(super) const MAX_DIM: i64 = 4_096;

/// Long enough for any model name worth having, short enough to be a key.
pub(super) const MAX_MODEL_LEN: usize = 64;

/// Hex rather than base64: `hex` is already a dependency here and base64 is not.
pub(super) fn encode_bytes(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

pub(super) fn decode_bytes_hex(text: &str) -> Option<Vec<u8>> {
    hex::decode(text.trim()).ok()
}
