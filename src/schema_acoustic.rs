//! GraphQL for the crowd-averaged acoustic index.
//!
//! Clients measure their own audio and contribute the six numbers; anyone can ask what sits near
//! what. See [`crate::db_acoustic`] for why there is no submitter column, and why this end is
//! forbidden from knowing anything about popularity.

use async_graphql::{Context, InputObject, Object, Result, SimpleObject};

use crate::auth::AuthedUser;
use crate::db_acoustic::{VectorSubmission, MAX_BATCH};
use crate::AppState;

/// One recording near the seed.
#[derive(SimpleObject)]
pub struct SimilarTrackEntry {
    pub title: String,
    pub artist: String,
    /// Acoustic distance. Smaller is nearer; zero means the two measured identically.
    pub distance: f64,
    /// How many independent measurements the average rests on.
    pub observations: i64,
}

/// One recording's measured vector.
///
/// All six axes are `0..1`, except the key pair which is `-1..1`. Out-of-range values are clamped
/// rather than rejected: a batch is a background contribution, and failing the whole request over
/// one bad row would lose the good ones with it.
#[derive(InputObject)]
pub struct AcousticVectorInput {
    pub title: String,
    pub artist: String,
    pub tempo: f64,
    pub energy: f64,
    pub brightness: f64,
    pub danceability: f64,
    pub key_x: f64,
    pub key_y: f64,
}

#[derive(Default)]
pub struct AcousticQuery;

#[Object]
impl AcousticQuery {
    /// The recordings that sound nearest to this one, nearest first.
    ///
    /// Empty when nobody has measured the seed. That is the honest answer and it is deliberately
    /// not softened by falling back to the best-measured or most-played recordings: a "similar
    /// tracks" endpoint that answers something when it knows nothing is how a recommender collapses
    /// onto whatever is already popular.
    ///
    /// Callers are expected to treat these as *candidates to rank*, not as a queue. The client
    /// holds a share of every radio queue for music nothing has measured, which is what keeps new
    /// recordings reachable at all — this index can only ever speak about what it has been told.
    async fn similar_recordings(
        &self,
        ctx: &Context<'_>,
        artist: String,
        title: String,
        #[graphql(default = 20)] limit: i64,
    ) -> Result<Vec<SimilarTrackEntry>> {
        let state = ctx.data::<AppState>()?;
        ctx.data::<AuthedUser>()?;

        let limit = limit.clamp(1, 100) as usize;
        Ok(state
            .db
            .similar_recordings(&artist, &title, limit)?
            .into_iter()
            .map(|track| SimilarTrackEntry {
                title: track.title,
                artist: track.artist,
                distance: track.distance,
                observations: track.observations,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct AcousticMutation;

#[Object]
impl AcousticMutation {
    /// Contributes measured vectors. Returns how many entries were folded in.
    ///
    /// Takes no `userId`, like `submitPlayCounts` next door and for the same reason: these are
    /// contributions to a shared average that nothing can attribute afterwards. The authenticated
    /// account is checked here and then discarded — it is never passed to the store.
    ///
    /// Re-submitting is harmless rather than idempotent. A repeat is one more observation of the
    /// same recording, which moves a settled average by very little and stops moving it at all
    /// past `MAX_OBSERVATIONS`; a submission id to make it exactly idempotent would be an
    /// identifier, which is the one thing this table must not hold.
    async fn submit_acoustic_vectors(
        &self,
        ctx: &Context<'_>,
        entries: Vec<AcousticVectorInput>,
    ) -> Result<i32> {
        let state = ctx.data::<AppState>()?;
        // The last point at which anyone knows who is speaking. Nothing below this line does.
        ctx.data::<AuthedUser>()?;

        if entries.is_empty() {
            return Ok(0);
        }
        if entries.len() > MAX_BATCH {
            return Err(format!("at most {MAX_BATCH} recordings per request").into());
        }

        let submissions: Vec<VectorSubmission> = entries
            .into_iter()
            .map(|entry| VectorSubmission {
                title: entry.title,
                artist: entry.artist,
                tempo: entry.tempo,
                energy: entry.energy,
                brightness: entry.brightness,
                danceability: entry.danceability,
                key_x: entry.key_x,
                key_y: entry.key_y,
            })
            .collect();

        Ok(state.db.submit_vectors(&submissions)? as i32)
    }
}
