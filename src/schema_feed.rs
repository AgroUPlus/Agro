//! The activity feed, and the recap a group of friends gets to share.
//!
//! Both are reads of other people's listening history, so both go through
//! [`require_visible`](crate::schema_social::require_visible) — the feed on `Surface::Activity`,
//! the recap on `Surface::Stats`, because the recap is an aggregate and the feed is a timeline.
//!
//! Neither has storage of its own. The feed is derived by [`crate::db_feed`] and the recap by
//! [`crate::stats`], both from the same `scrobbles` rows the statistics page already reads. That
//! keeps the visibility rules in one place: there is no second copy of anybody's history here that
//! could outlive the switch that permitted it.
//!
//! A recap is one query that reads every participating friend's history, so it is deliberately
//! capped and deliberately not offered for `ALL` time by default — see [`MAX_CIRCLE`].

use crate::db::Db;
use crate::db_feed::events_for;
use crate::schema::caller;
use crate::schema_social::{require_visible, Surface};
use async_graphql::{Context, Object};

/// Most accounts a recap will aggregate: the caller plus their friends.
///
/// A ceiling rather than a page size, because a recap is one request that reads every member's
/// history. Somebody with three hundred friends should get a recap of a circle, not a query that
/// walks a third of the database.
const MAX_CIRCLE: usize = 25;

/// How many feed items one request may return.
const MAX_FEED_ITEMS: i64 = 200;

/// How far back the feed looks when the caller does not say.
const DEFAULT_FEED_DAYS: i64 = 14;

/// How deep the circle's top-track list goes when deciding who got there first.
const TRENDSETTER_DEPTH: usize = 25;

#[derive(Default)]
pub struct FeedQuery;

#[Object]
impl FeedQuery {
    /// What the caller's friends have been into lately.
    ///
    /// Only friends who have opened `showActivity` appear. Anyone else is not merely omitted from
    /// the list — nothing about them is read at all.
    async fn friend_activity(
        &self,
        ctx: &Context<'_>,
        days: Option<i64>,
        limit: Option<i64>,
    ) -> async_graphql::Result<Vec<FeedItemPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();
        let days = days.unwrap_or(DEFAULT_FEED_DAYS).clamp(1, 365);
        let since = now - days * 86_400;
        let limit = limit.unwrap_or(50).clamp(1, MAX_FEED_ITEMS) as usize;

        let mut items = Vec::new();
        for profile in db.friends(authed.username())? {
            // Asked through the one gate rather than by reading the flag here. The flag is the
            // same either way, but a surface that checks it itself is a surface that can be
            // forgotten when the rule changes.
            if require_visible(ctx, &profile.username, Surface::Activity).is_err() {
                continue;
            }
            // The whole history is read, not just the window: a milestone is a statement about a
            // total, and cannot be counted from a slice of it.
            let rows = db.scrobble_rows(&profile.username, None, None)?;
            for mut item in events_for(&rows, since, now) {
                item.username = profile.username.clone();
                items.push(item);
            }
        }

        items.sort_by(|a, b| b.at.cmp(&a.at));
        items.truncate(limit);
        Ok(items.into_iter().map(to_feed_payload).collect())
    }

    /// The circle's shared recap: one anthem, one trendsetter, and how everyone's taste lines up.
    ///
    /// Gated on `Surface::Stats` rather than `Surface::Activity`. A recap reports totals and
    /// overlaps, which is the same kind of disclosure the statistics page already makes, and asking
    /// people to open a second switch for it would mean an empty recap for everyone who had
    /// already said yes to the first.
    ///
    /// `year` overrides `period` with a **calendar** year read in `utc_offset_minutes`, which is
    /// what Agro Replay asks for. `period: "YEAR"` cannot serve that: it means the last 365 days,
    /// so a recap of 2026 opened in February would be half about 2027.
    async fn circle_recap(
        &self,
        ctx: &Context<'_>,
        period: Option<String>,
        year: Option<i32>,
        utc_offset_minutes: Option<i32>,
    ) -> async_graphql::Result<CircleRecapPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();

        // A calendar year needs both edges. A rolling period needs only the near one, because its
        // far edge is now — which is why `until` is an `Option` rather than a very large date.
        let (period, since, until) = match year {
            Some(year) => {
                let offset = utc_offset_minutes.unwrap_or(0);
                let (start, end) = crate::stats_wrapped::year_bounds(year, offset)
                    .ok_or_else(|| async_graphql::Error::new("That is not a year Agro can read"))?;
                (
                    year.to_string(),
                    Some(start),
                    crate::stats::parse_time(&end),
                )
            }
            None => {
                let period = period.unwrap_or_else(|| "MONTH".to_string()).to_uppercase();
                let since = crate::stats::period_start(&period, now);
                (period, since, None)
            }
        };

        // The caller is always in their own recap; their own data needs no permission.
        let mut members = vec![authed.username().to_string()];
        for profile in db.friends(authed.username())? {
            if members.len() >= MAX_CIRCLE {
                break;
            }
            // Asked through the same gate every other read uses, so a friend who closed their
            // statistics is refused here exactly as they would be on the statistics page.
            if require_visible(ctx, &profile.username, Surface::Stats).is_ok() {
                members.push(profile.username);
            }
        }

        // Read once per member and reused for all three sections. Reading again per section would
        // mean three passes over the same rows and, worse, three chances to disagree about which
        // rows were in the window.
        let mut histories: Vec<(String, Vec<crate::db::ScrobbleRow>)> = Vec::new();
        for member in &members {
            let mut rows = db.scrobble_rows(member, None, since.as_deref())?;
            // The far edge is applied here rather than in SQL because `scrobble_rows` takes only a
            // lower bound. A row whose timestamp will not parse is kept, matching what every other
            // reader of these rows does with one.
            if let Some(until) = until {
                rows.retain(|row| {
                    crate::stats::parse_time(&row.played_at).is_none_or(|at| at < until)
                });
            }
            histories.push((member.clone(), rows));
        }

        Ok(CircleRecapPayload {
            period,
            anthem: anthem(&histories),
            top_tracks: circle_top(&histories, track_key, 10),
            top_artists: circle_top(&histories, artist_key, 10),
            trendsetter: trendsetter(&histories),
            matrix: matrix(&histories, now),
            members,
        })
    }
}

mod circle;
mod payloads;
use circle::anthem;
use circle::artist_key;
use circle::circle_top;
use circle::matrix;
use circle::to_feed_payload;
use circle::track_key;
use circle::trendsetter;
pub use payloads::AnthemPayload;
pub use payloads::CircleRecapPayload;
pub use payloads::FeedItemPayload;
pub use payloads::TasteMatrixEntry;
pub use payloads::TrendsetterPayload;
