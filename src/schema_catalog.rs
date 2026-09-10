//! GraphQL for the shared recording catalogue.
//!
//! Two operations and nothing else: a client publishes what it fingerprinted, and asks for what
//! everyone else has published since it last asked. Everything a client needs to identify its own
//! music works without either of them — this is the part that stops every device redoing the same
//! work, and lets a badly tagged source inherit a well tagged one's metadata.

use async_graphql::{Context, InputObject, Object, Result, SimpleObject};

use crate::auth::AuthedUser;
use crate::db_catalog::PublishedRecording;
use crate::rate_limit::FixedWindow;
use crate::schema::bounded;
use crate::db::Db;
use std::time::Duration;

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

/// Charges one client's publishing budget, or says why it cannot.
///
/// Publishing is the one mutation here that is cheap to send and expensive to serve: every entry
/// runs a sequence comparison against up to twenty candidate recordings, each of which is read
/// from the database in full. Nothing bounded that before — any account could publish without
/// limit, and the catalogue records no submitter, so there was not even anything to point at
/// afterwards.
fn spend_publish_quota(ctx: &Context<'_>, user: &AuthedUser, units: usize) -> Result<()> {
    // A server that predates this data being registered must not start refusing writes over a
    // missing limiter — the quota is a bound on abuse, not a correctness requirement.
    let Ok(limiter) = ctx.data::<PublishQuota>() else {
        return Ok(());
    };
    if limiter.0.charge(user.username(), units, MAX_PUBLISHES, PUBLISH_WINDOW) {
        return Ok(());
    }
    Err(async_graphql::Error::new(
        "too many recordings published just now; try again shortly",
    ))
}

/// Per-account publishing budget, in the GraphQL context so resolvers can reach it.
pub struct PublishQuota(pub FixedWindow);

impl Default for PublishQuota {
    fn default() -> Self {
        Self(FixedWindow::new())
    }
}

/// Recordings one account may publish per [`PUBLISH_WINDOW`].
///
/// Set for the largest honest case rather than the typical one: a client that has just finished
/// measuring a big library has thousands of recordings to file and should be able to, over the
/// course of an evening, without being throttled into a crawl. What it stops is the case that has
/// no honest reading — one account filling the catalogue with generated embeddings faster than
/// anyone can notice.
const MAX_PUBLISHES: usize = 300;

const PUBLISH_WINDOW: Duration = Duration::from_secs(300);

/// Entries one request may carry.
///
/// Far smaller than the 500 the play-count batch allows, because these are not comparable units of
/// work: a play count is an upsert, and a recording is up to twenty full sequence comparisons.
const MAX_PUBLISH_BATCH: usize = 25;

/// Checks one client's submission and turns it into what the database stores.
///
/// Returns the complaint as a plain `String` rather than a GraphQL error so a batch can report it
/// against the one entry it belongs to instead of failing the whole request.
fn validate(input: PublishRecordingInput) -> std::result::Result<PublishedRecording, String> {
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
        title: input.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()),
        artist: input.artist.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
        album: input.album.map(|a| a.trim().to_string()).filter(|a| !a.is_empty()),
        lyrics,
        lyrics_source,
        source_uri,
    })
}

/// Twenty minutes of audio at int8: two vectors a second, 128 values each.
///
/// The client sends int8 precisely so this can stay a sane number — the same audio as float32 is
/// four times larger and a three-minute track alone would pass the old 160 KB cap.
const MAX_EMBEDDING_BYTES: usize = 20 * 60 * 2 * 128;

/// Max length of lyrics payload (synced LRC or plain text) traded per recording.
const MAX_LYRICS_LEN: usize = 65_536;

/// Long enough to name a source, short enough that it cannot become a second lyrics field.
const MAX_LYRICS_SOURCE_LEN: usize = 64;

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
