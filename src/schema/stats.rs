//! Listening stats and the yearly recap.

use crate::db::Db;
use async_graphql::{Context, Object, SimpleObject};

/// A named total: an artist, an album, a genre, a device.
#[derive(SimpleObject, Clone)]
pub struct StatEntry {
    pub name: String,
    /// Plays for the top-N lists; seconds for the per-device breakdown.
    pub value: i64,
}

/// Listening statistics for one account, across every device that reports to it.
#[derive(SimpleObject, Clone)]
pub struct ListeningStats {
    /// The last 24 hours, not since midnight — there is no one timezone across a fleet.
    pub secs_today: i64,
    pub secs_week: i64,
    pub secs_total: i64,
    pub plays_total: i64,
    pub streak: i64,
    pub top_artists: Vec<StatEntry>,
    pub top_albums: Vec<StatEntry>,
    pub top_tracks: Vec<StatEntry>,
    pub top_genres: Vec<StatEntry>,
    /// Seconds per day for the last fourteen days, oldest first.
    pub by_day: Vec<i64>,
    /// Seconds per day for the last eight weeks, oldest first.
    pub heatmap: Vec<i64>,
    /// Seconds per hour of the day, UTC, index 0 = midnight.
    pub by_hour: Vec<i64>,
    /// Seconds per device, most-listened first.
    pub by_device: Vec<StatEntry>,
}

fn to_listening_stats(stats: crate::stats::Stats) -> ListeningStats {
    ListeningStats {
        secs_today: stats.secs_today,
        secs_week: stats.secs_week,
        secs_total: stats.secs_total,
        plays_total: stats.plays_total,
        streak: stats.streak,
        top_artists: to_entries(stats.top_artists),
        top_albums: to_entries(stats.top_albums),
        top_tracks: to_entries(stats.top_tracks),
        top_genres: to_entries(stats.top_genres),
        by_day: stats.by_day,
        heatmap: stats.heatmap,
        by_hour: stats.by_hour,
        by_device: to_entries(stats.by_device),
    }
}

fn to_entries(pairs: Vec<(String, i64)>) -> Vec<StatEntry> {
    pairs
        .into_iter()
        .map(|(name, value)| StatEntry { name, value })
        .collect()
}

#[derive(SimpleObject, Clone)]
pub struct AgroWrappedPayload {
    pub year: i32,
    pub month: Option<i32>,
    pub total_minutes: i64,
    pub total_plays: i64,
    pub top_artists: Vec<StatEntry>,
    pub top_tracks: Vec<StatEntry>,
    pub top_albums: Vec<StatEntry>,
    pub top_genres: Vec<StatEntry>,
    pub top_hour_utc: Option<i32>,
    /// The peak hour in the caller's own offset. `null` when there were no plays at all.
    pub top_hour_local: Option<i32>,
    /// The longest run of *consecutive* days with a play.
    pub longest_streak_days: i64,
    /// Days with any play at all, which is a different and usually much larger number.
    pub active_days_count: i64,
    pub new_artists_count: i64,
    /// Distinct artists in the period. `topArtists` is a top ten, so its length says nothing.
    pub total_artists: i64,
    /// Plays per calendar month, January first. Always twelve entries.
    pub by_month: Vec<i64>,
    /// Plays per hour of the caller's local day, index 0 = midnight. Always twenty-four entries.
    pub by_hour: Vec<i64>,
    /// Plays per device, most-played first. The one figure only a fleet-wide server can answer.
    pub by_device: Vec<StatEntry>,
    pub first_play: Option<String>,
    pub last_play: Option<String>,
}

#[derive(Default)]
pub struct StatsQuery;

#[Object]
impl StatsQuery {
    /// The account's listening, aggregated across every device that reports to it.
    ///
    /// `period` is DAY, WEEK, MONTH, YEAR or ALL. `deviceName` narrows it to one device's plays,
    /// which is how a client answers "what did *I* listen to" while still holding the fleet total.
    async fn listening_stats(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        period: Option<String>,
        device_name: Option<String>,
    ) -> async_graphql::Result<ListeningStats> {
        // Your own always; a friend's only when they have opened their statistics. This used to be
        // `authorize`, which is self-only — so `showStats` was a switch with nothing on the other
        // side of it and a friend's listening could never be read however open they set it.
        crate::schema_social::require_visible(ctx, &user_id, crate::schema_social::Surface::Stats)?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();
        let since = crate::stats::period_start(period.as_deref().unwrap_or("ALL"), now);

        let rows = db.scrobble_rows(
            &user_id,
            device_name.as_deref().filter(|d| !d.is_empty()),
            since.as_deref(),
        )?;
        Ok(to_listening_stats(crate::stats::compute(&rows, 10, now)))
    }

    /// Computes a private Year / Month in Review recap for the account.
    ///
    /// `utc_offset_minutes` is the listener's offset from UTC. It defaults to zero, which is what
    /// this query did before the argument existed, so an older client keeps the answer it used to
    /// get. A recap that omits it buckets by UTC hours and days — see [`crate::stats_wrapped`] for
    /// why that is wrong for everyone who does not live on the meridian.
    async fn agro_wrapped(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        year: i32,
        month: Option<i32>,
        utc_offset_minutes: Option<i32>,
    ) -> async_graphql::Result<AgroWrappedPayload> {
        crate::schema_social::require_visible(ctx, &user_id, crate::schema_social::Surface::Stats)?;
        let db = ctx.data::<Db>()?;
        let rows = db.scrobble_rows(&user_id, None, None)?;
        let wrapped = crate::stats_wrapped::compute_wrapped(
            &rows,
            year,
            month,
            10,
            utc_offset_minutes.unwrap_or(0),
        );
        Ok(AgroWrappedPayload {
            year: wrapped.year,
            month: wrapped.month,
            total_minutes: wrapped.total_minutes,
            total_plays: wrapped.total_plays,
            top_artists: to_entries(wrapped.top_artists),
            top_tracks: to_entries(wrapped.top_tracks),
            top_albums: to_entries(wrapped.top_albums),
            top_genres: to_entries(wrapped.top_genres),
            top_hour_utc: wrapped.top_hour_utc,
            top_hour_local: wrapped.top_hour_local,
            longest_streak_days: wrapped.longest_streak_days,
            active_days_count: wrapped.active_days_count,
            new_artists_count: wrapped.new_artists_count,
            total_artists: wrapped.total_artists,
            by_month: wrapped.by_month,
            by_hour: wrapped.by_hour,
            by_device: to_entries(wrapped.by_device),
            first_play: wrapped.first_play,
            last_play: wrapped.last_play,
        })
    }
}
