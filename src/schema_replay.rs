//! The GraphQL surface for Agro Replay.
//!
//! The recap itself is [`crate::schema::QueryRoot::agro_wrapped`], which predates this file and is
//! friend-readable. What lives here is the one part of Replay that reads *across* accounts, and it
//! is kept separate precisely because that is a different kind of disclosure and deserves to be
//! reviewed as one — see [`crate::db_replay`] for the floor that makes it safe.

use crate::db::Db;
use crate::schema::caller;
use async_graphql::{Context, Object, SimpleObject};

/// Where the caller's year sits among everyone else's on this server.
#[derive(SimpleObject, Clone, Default)]
pub struct ReplayChartStanding {
    /// 0–100: the share of the cohort that listened less than the caller.
    pub percentile: i32,
    /// How many accounts were ranked.
    pub cohort_size: i32,
    /// The caller's own minutes, so the card can say what the percentile is of.
    pub minutes: i32,
    /// True when this server has too few listeners to rank anyone without describing them.
    /// Everything else is zero; a client must draw nothing rather than "top 100%".
    pub suppressed: bool,
}

#[derive(Default)]
pub struct ReplayQuery;

#[Object]
impl ReplayQuery {
    /// How the caller's listening year compares with everybody else's on this server.
    ///
    /// Takes no `userId`. That is the whole design: a `userId` argument would need a visibility
    /// check, and "what percentile is my friend in" is a question nobody asked to have answered
    /// about them. The caller can only ever ask about themselves.
    ///
    /// Answers `suppressed` on a server with fewer than [`crate::db_replay::MIN_COHORT`] listeners
    /// that year, where a ranking would describe identifiable people rather than a crowd.
    async fn replay_chart_standing(
        &self,
        ctx: &Context<'_>,
        year: i32,
        utc_offset_minutes: Option<i32>,
    ) -> async_graphql::Result<ReplayChartStanding> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let (since, until) =
            crate::stats_wrapped::year_bounds(year, utc_offset_minutes.unwrap_or(0))
                .ok_or_else(|| async_graphql::Error::new("That is not a year Agro can read"))?;

        let standing = db.standing_in(authed.username(), &since, &until)?;
        Ok(ReplayChartStanding {
            percentile: standing.percentile as i32,
            cohort_size: standing.cohort_size as i32,
            minutes: standing.minutes as i32,
            suppressed: standing.suppressed,
        })
    }
}
