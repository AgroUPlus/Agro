//! GraphQL for the shared recording catalogue.
//!
//! Two operations and nothing else: a client publishes what it fingerprinted, and asks for what
//! everyone else has published since it last asked. Everything a client needs to identify its own
//! music works without either of them — this is the part that stops every device redoing the same
//! work, and lets a badly tagged source inherit a well tagged one's metadata.

use async_graphql::{Context, Object, Result, SimpleObject};

use crate::auth::AuthedUser;
use crate::db_catalog::PublishedRecording;
use crate::schema::bounded;
use crate::db::Db;

/// One recording as the catalogue knows it, on its way to a client.
#[derive(SimpleObject)]
pub struct CatalogEntry {
    pub recording_id: String,
    /// The embedding, hex-encoded int8. Clients compare it themselves rather than trusting the
    /// server's match — the same audio has to look the same to both ends or a pull is worthless.
    pub embedding: String,
    /// Values per vector, so a client can split the blob into segments.
    pub dim: i64,
    /// Which embedder produced it. A client ignores an entry from a model it does not run.
    pub model: String,
    pub version: i64,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Namespaced ids known to hold this audio — `ytm:…`, `navidrome:…`. Never a `local:` id:
    /// those are filesystem paths from somebody's phone, and this list goes to every account.
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
        let db = ctx.data::<Db>()?;
        // Authenticated, but not scoped to the caller: the catalogue is the fleet's shared
        // knowledge about recordings, and holds nothing about who listened to what.
        ctx.data::<AuthedUser>()?;

        let limit = limit.clamp(1, 500);
        let recordings = db.catalog_since(since, limit)?;

        recordings
            .into_iter()
            .map(|recording| {
                let sources = db.sources_for_recording(&recording.recording_id)?;
                Ok(CatalogEntry {
                    embedding: encode_bytes(&recording.embedding),
                    dim: recording.dim,
                    model: recording.model,
                    version: recording.version,
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
        let db = ctx.data::<Db>()?;
        ctx.data::<AuthedUser>()?;
        Ok(db.recording_for_source(source_uri.trim())?)
    }
}

#[derive(Default)]
pub struct CatalogMutation;

#[Object]
impl CatalogMutation {
    /// Publishes one embedding, merging it into an existing recording if it matches one.
    ///
    /// Returns the recording id the audio now sits under, which may be one another client
    /// created: that is the whole point, and it is how two encodings of one performance stop
    /// being two recordings.
    #[allow(clippy::too_many_arguments)]
    async fn publish_recording(
        &self,
        ctx: &Context<'_>,
        embedding: String,
        dim: i64,
        model: String,
        version: i64,
        duration_ms: i64,
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        source_uri: Option<String>,
    ) -> Result<String> {
        let db = ctx.data::<Db>()?;
        ctx.data::<AuthedUser>()?;

        // The dimension is checked before the length, because the length check is stated in terms
        // of it: a blob is only well formed relative to the width of one vector.
        if dim <= 0 || dim > MAX_DIM {
            return Err(async_graphql::Error::new("dim is not a plausible vector width"));
        }
        let decoded = decode_bytes_hex(&embedding)
            .ok_or_else(|| async_graphql::Error::new("embedding is not valid hex"))?;
        if decoded.is_empty() || decoded.len() % (dim as usize) != 0 {
            return Err(async_graphql::Error::new(
                "embedding must be a whole number of vectors",
            ));
        }
        if decoded.len() > MAX_EMBEDDING_BYTES {
            return Err(async_graphql::Error::new("embedding is too long"));
        }
        let model = bounded(&model, MAX_MODEL_LEN, "model")?;
        if model.is_empty() {
            return Err(async_graphql::Error::new("an embedding must name its model"));
        }
        // A `local:` id is a path on somebody's phone, and `sources` is returned to every account
        // on this server. Dropped here rather than trusted to the client: the boundary is the
        // server's to hold, and an older or modified client would otherwise publish them.
        let source_uri = source_uri
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty() && !s.starts_with("local:"));

        Ok(db.publish_recording(&PublishedRecording {
            embedding: decoded,
            dim,
            model,
            version,
            duration_ms,
            title: title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
            artist: artist.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
            album: album.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
            source_uri,
        })?)
    }
}

/// Twenty minutes of audio at int8: two vectors a second, 128 values each.
///
/// The client sends int8 precisely so this can stay a sane number — the same audio as float32 is
/// four times larger and a three-minute track alone would pass the old 160 KB cap.
const MAX_EMBEDDING_BYTES: usize = 20 * 60 * 2 * 128;

/// A plausible width for one vector. The embedder in use produces 128.
const MAX_DIM: i64 = 4_096;

/// Long enough for any model name worth having, short enough to be a key.
const MAX_MODEL_LEN: usize = 64;

/// Hex rather than base64: `hex` is already a dependency here and base64 is not.
fn encode_bytes(bytes: &[u8]) -> String {
    hex::encode(bytes)
}

fn decode_bytes_hex(text: &str) -> Option<Vec<u8>> {
    hex::decode(text.trim()).ok()
}
