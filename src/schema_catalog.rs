//! GraphQL for the shared recording catalogue.
//!
//! Two operations and nothing else: a client publishes what it fingerprinted, and asks for what
//! everyone else has published since it last asked. Everything a client needs to identify its own
//! music works without either of them — this is the part that stops every device redoing the same
//! work, and lets a badly tagged source inherit a well tagged one's metadata.

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::rate_limit::FixedWindow;
use async_graphql::{Context, InputObject, Object, Result, SimpleObject};

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
    pub lyrics: Option<String>,
    /// What supplied [`lyrics`] — `LRCLIB`, `Native`, and so on.
    ///
    /// Null for anything published before the catalogue recorded this, which cannot be filled in
    /// after the fact: the row remembers the text, not who sent it. A client that cares where its
    /// lyrics came from can decide what to do with an unattributed one.
    pub lyrics_source: Option<String>,
    /// Namespaced ids known to hold this audio — `ytm:…`, `navidrome:…`. Never a `local:` id:
    /// those are filesystem paths from somebody's phone, and this list goes to every account.
    pub sources: Vec<String>,
    /// This entry's position in the catalogue's order. The highest one seen is the next cursor.
    pub updated_at: i64,
}

/// One recording on its way *into* the catalogue.
///
/// Exists for [`CatalogMutation::publish_recordings`], which cannot take eleven positional
/// arguments per entry the way the single-shot mutation does.
#[derive(InputObject)]
pub struct PublishRecordingInput {
    pub embedding: String,
    pub dim: i64,
    pub model: String,
    pub version: i64,
    pub duration_ms: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub source_uri: Option<String>,
    pub lyrics: Option<String>,
    pub lyrics_source: Option<String>,
}

/// What became of one entry in a batch.
///
/// Positional: the nth result describes the nth entry sent. A rejected entry carries [`error`] and
/// an empty [`recording_id`], because one malformed embedding among twenty good ones should cost
/// the client that one entry and not the other nineteen — the same position the acoustic and
/// popularity batches take.
#[derive(SimpleObject)]
pub struct PublishResult {
    pub recording_id: String,
    pub error: Option<String>,
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
                    lyrics: recording.lyrics,
                    lyrics_source: recording.lyrics_source,
                    sources,
                    updated_at: recording.updated_at,
                })
            })
            .collect()
    }

    /// The recording a source id is known to hold, for a client resolving a shared link.
    async fn recording_for_source(
        &self,
        ctx: &Context<'_>,
        source_uri: String,
    ) -> Result<Option<String>> {
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
        #[graphql(default)] lyrics: Option<String>,
        #[graphql(default)] lyrics_source: Option<String>,
    ) -> Result<String> {
        let db = ctx.data::<Db>()?;
        let user = ctx.data::<AuthedUser>()?;
        spend_publish_quota(ctx, user, 1)?;

        let published = validate(PublishRecordingInput {
            embedding,
            dim,
            model,
            version,
            duration_ms,
            title,
            artist,
            album,
            source_uri,
            lyrics,
            lyrics_source,
        })
        .map_err(async_graphql::Error::new)?;

        Ok(db.publish_recording(&published)?)
    }

    /// Publishes a batch of embeddings, answering for each in turn.
    ///
    /// One request instead of one per recording. A client that has just measured its library has
    /// hundreds to file, and each was a separate round trip — on a phone, a separate wake of the
    /// radio.
    ///
    /// Each entry is matched and merged exactly as [`Self::publish_recording`] does, one at a time
    /// and each under its own lock. The batch is not a transaction: holding the database for the
    /// length of twenty sequence comparisons would stall every other request on the server, and a
    /// half-applied batch is not a problem here — an entry that did not land is republished on the
    /// next sync, and re-publishing one that did is what the catalogue already handles by merging.
    async fn publish_recordings(
        &self,
        ctx: &Context<'_>,
        entries: Vec<PublishRecordingInput>,
    ) -> Result<Vec<PublishResult>> {
        let db = ctx.data::<Db>()?;
        let user = ctx.data::<AuthedUser>()?;

        if entries.is_empty() {
            return Ok(Vec::new());
        }
        if entries.len() > MAX_PUBLISH_BATCH {
            return Err(async_graphql::Error::new(format!(
                "at most {MAX_PUBLISH_BATCH} recordings per request"
            )));
        }
        // Charged for what it carries, so a batch cannot buy throughput one request at a time
        // could not. Checked once, before any work: a client over budget is told so without the
        // server first paying for the entries it is about to refuse.
        spend_publish_quota(ctx, user, entries.len())?;

        Ok(entries
            .into_iter()
            .map(|entry| match validate(entry) {
                Err(message) => PublishResult {
                    recording_id: String::new(),
                    error: Some(message),
                },
                Ok(published) => match db.publish_recording(&published) {
                    Ok(recording_id) => PublishResult {
                        recording_id,
                        error: None,
                    },
                    Err(error) => PublishResult {
                        recording_id: String::new(),
                        error: Some(error.to_string()),
                    },
                },
            })
            .collect())
    }
}

impl Default for PublishQuota {
    fn default() -> Self {
        Self(FixedWindow::new())
    }
}

mod publish_guard;
use publish_guard::encode_bytes;
use publish_guard::spend_publish_quota;
use publish_guard::validate;
pub use publish_guard::PublishQuota;
use publish_guard::MAX_PUBLISH_BATCH;
