//! GraphQL for the shared fingerprint catalogue.
//!
//! Two operations and nothing else: a client publishes what it fingerprinted, and asks for what
//! everyone else has published since it last asked. Everything a client needs to identify its own
//! music works without either of them — this is the part that stops every device redoing the same
//! work, and lets a badly tagged source inherit a well tagged one's metadata.

use async_graphql::{Context, Object, Result, SimpleObject};

use crate::auth::AuthedUser;
use crate::db_catalog::PublishedRecording;
use crate::AppState;

/// One recording as the catalogue knows it, on its way to a client.
#[derive(SimpleObject)]
pub struct CatalogEntry {
    pub recording_id: String,
    /// The fingerprint, hex. Clients compare it themselves rather than trusting the match.
    pub sub_hashes: String,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Namespaced ids known to hold this audio — `ytm:…`, `navidrome:…`, `local:…`.
    pub sources: Vec<String>,
    /// This entry's position in the catalogue's order. The highest one seen is the next cursor.
    pub updated_at: i64,
}

#[derive(Default)]
pub struct CatalogQuery;

#[Object]
impl CatalogQuery {
    /// Everything the catalogue learned after [`since`], oldest first.
    ///
    /// The client keeps the highest `updated_at` it has seen and passes it back. Re-reading an
    /// entry it already holds is harmless: these are facts about audio, so a client that sees one
    /// twice simply agrees with itself.
    async fn catalog_since(
        &self,
        ctx: &Context<'_>,
        since: i64,
        #[graphql(default = 200)] limit: i64,
    ) -> Result<Vec<CatalogEntry>> {
        let state = ctx.data::<AppState>()?;
        // Authenticated, but not scoped to the caller: the catalogue is the fleet's shared
        // knowledge about recordings, and holds nothing about who listened to what.
        ctx.data::<AuthedUser>()?;

        let limit = limit.clamp(1, 500);
        let recordings = state.db.catalog_since(since, limit)?;

        recordings
            .into_iter()
            .map(|recording| {
                let sources = state.db.sources_for_recording(&recording.recording_id)?;
                Ok(CatalogEntry {
                    sub_hashes: encode_hashes(&recording.sub_hashes),
                    recording_id: recording.recording_id,
                    duration_ms: recording.duration_ms,
                    title: recording.title,
                    artist: recording.artist,
                    album: recording.album,
                    sources,
                    updated_at: recording.updated_at,
                })
            })
            .collect()
    }

    /// The recording a source id is known to hold, for a client resolving a shared link.
    async fn recording_for_source(&self, ctx: &Context<'_>, source_uri: String) -> Result<Option<String>> {
        let state = ctx.data::<AppState>()?;
        ctx.data::<AuthedUser>()?;
        Ok(state.db.recording_for_source(source_uri.trim())?)
    }
}

#[derive(Default)]
pub struct CatalogMutation;

#[Object]
impl CatalogMutation {
    /// Publishes one fingerprint, merging it into an existing recording if it matches one.
    ///
    /// Returns the recording id the audio now sits under, which may be one another client
    /// created: that is the whole point, and it is how two encodings of one performance stop
    /// being two recordings.
    async fn publish_recording(
        &self,
        ctx: &Context<'_>,
        sub_hashes: String,
        duration_ms: i64,
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        source_uri: Option<String>,
    ) -> Result<String> {
        let state = ctx.data::<AppState>()?;
        ctx.data::<AuthedUser>()?;

        let decoded = decode_hashes_hex(&sub_hashes)
            .ok_or_else(|| async_graphql::Error::new("subHashes is not valid hex"))?;
        if decoded.is_empty() || decoded.len() % 4 != 0 {
            return Err(async_graphql::Error::new(
                "subHashes must be a whole number of four-byte sub-hashes",
            ));
        }
        if decoded.len() > MAX_FINGERPRINT_BYTES {
            return Err(async_graphql::Error::new("fingerprint is too long"));
        }

        Ok(state.db.publish_recording(&PublishedRecording {
            sub_hashes: decoded,
            duration_ms,
            title: title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
            artist: artist.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
            album: album.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
            source_uri,
        })?)
    }
}

/// Roughly twenty minutes of fingerprint at ~31 a second. Past this it is not a recording.
const MAX_FINGERPRINT_BYTES: usize = 160_000;


/// Hex rather than base64: `hex` is already a dependency here and base64 is not, and a
/// fingerprint is small enough that the difference in size does not pay for a new crate.
fn encode_hashes(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn decode_hashes_hex(text: &str) -> Option<Vec<u8>> {
    hex::decode(text.trim()).ok()
}
