//! GraphQL for the blinded popularity counters.
//!
//! Two operations: a client adds what it played since it last synced, and asks what the fleet has
//! been playing. Neither carries an account id past the authentication check — see
//! [`crate::db_popularity`] for what that does and does not buy, including why this is deliberately
//! not called zero-knowledge.

use async_graphql::{Context, InputObject, Object, Result, SimpleObject};

use crate::auth::AuthedUser;
use crate::db_popularity::CountIncrement;
use crate::AppState;

/// The largest batch one request may carry.
///
/// A fortnight offline on a heavy listener is a few hundred distinct recordings, so this is
/// generous for the honest case and still bounds one request's write.
const MAX_BATCH: usize = 500;

/// One recording the fleet has been playing.
#[derive(SimpleObject)]
pub struct PopularTrackEntry {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Plays across the whole window, from everyone, attributed to nobody.
    pub count: i64,
}

/// One recording's plays since the client last reported.
#[derive(InputObject)]
pub struct PlayCountInput {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub count: i64,
}

#[derive(Default)]
pub struct PopularityQuery;

#[Object]
impl PopularityQuery {
    /// What the fleet has been playing over a rolling window, most played first.
    ///
    /// Authenticated but not scoped to the caller — like the catalogue, this is the fleet's shared
    /// view and holds nothing about who listened to what. Recordings below the exposure floor are
    /// absent rather than zeroed, so a caller cannot learn that something was played a little by
    /// noticing a row with a small number on it.
    async fn popular_tracks(
        &self,
        ctx: &Context<'_>,
        #[graphql(default = 7)] days: i64,
        #[graphql(default = 20)] limit: i64,
    ) -> Result<Vec<PopularTrackEntry>> {
        let state = ctx.data::<AppState>()?;
        ctx.data::<AuthedUser>()?;

        let days = days.clamp(1, crate::db_popularity::RETENTION_DAYS);
        let limit = limit.clamp(1, 100) as usize;
        Ok(state
            .db
            .popular_tracks(today(), days, limit)?
            .into_iter()
            .map(|track| PopularTrackEntry {
                title: track.title,
                artist: track.artist,
                album: track.album,
                count: track.count,
            })
            .collect())
    }
}

#[derive(Default)]
pub struct PopularityMutation;

#[Object]
impl PopularityMutation {
    /// Adds a batch of plays to today's counters. Returns how many entries were counted.
    ///
    /// Takes no `userId`, unlike `recordScrobbles` next door, and that difference is the feature:
    /// scrobbles are the caller's own history and are theirs to read and purge, while these are
    /// contributions to a shared total that nothing can attribute afterwards. The authenticated
    /// account is checked here and then deliberately discarded — it is never passed to the store.
    ///
    /// Not idempotent, and cannot be: a repeated submission is indistinguishable from listening to
    /// something twice, precisely because nothing identifies the submitter. Clients must therefore
    /// clear their pending counts on success and accept losing a batch on a failed request. Losing
    /// a few counts costs a shelf a little accuracy; the alternative is a per-client submission id,
    /// which is an identifier, which is the one thing this table must not hold.
    async fn submit_play_counts(
        &self,
        ctx: &Context<'_>,
        entries: Vec<PlayCountInput>,
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

        let increments: Vec<CountIncrement> = entries
            .into_iter()
            .map(|entry| CountIncrement {
                title: entry.title,
                artist: entry.artist,
                album: entry.album,
                count: entry.count,
            })
            .collect();

        Ok(state.db.add_play_counts(today(), &increments)? as i32)
    }
}

/// Whole days since the epoch, UTC. The only unit of time this subsystem knows.
fn today() -> i64 {
    chrono::Utc::now().timestamp() / 86_400
}
