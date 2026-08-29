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

use std::collections::HashMap;

use async_graphql::{Context, Object, SimpleObject};

use crate::db::Db;
use crate::db_feed::{events_for, FeedEvent};
use crate::schema::{caller, StatEntry};
use crate::schema_social::{require_visible, Surface};

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

#[derive(SimpleObject, Clone)]
pub struct FeedItemPayload {
    pub username: String,
    /// RFC3339, from the play that caused it — not the time this was read.
    pub at: String,
    /// `MILESTONE`, `ON_REPEAT` or `NEW_FAVOURITE`. A discriminator rather than a union, so a
    /// client that meets a kind it does not know can still render the summary line.
    pub kind: String,
    /// A ready-made sentence. Sent so that every client says the same thing rather than each
    /// reimplementing the phrasing and disagreeing about it.
    pub summary: String,
    pub artist: String,
    pub title: Option<String>,
    /// Plays for a milestone or a repeat; distinct tracks for a new favourite.
    pub count: i64,
}

#[derive(SimpleObject, Clone)]
pub struct AnthemPayload {
    pub title: String,
    pub artist: String,
    /// Plays across the whole circle.
    pub plays: i64,
    /// Who played it, and how often each. The interesting half of an anthem.
    pub by_member: Vec<StatEntry>,
}

#[derive(SimpleObject, Clone)]
pub struct TrendsetterPayload {
    pub username: String,
    /// How many of the circle's top tracks this account played before anybody else did.
    pub firsts: i64,
    /// A few of them, so the claim can be checked rather than merely asserted.
    pub examples: Vec<String>,
}

/// One cell of the taste matrix. Undirected — `a` and `b` are sorted, and each pair appears once.
#[derive(SimpleObject, Clone)]
pub struct TasteMatrixEntry {
    pub a: String,
    pub b: String,
    pub score: i64,
}

#[derive(SimpleObject, Clone)]
pub struct CircleRecapPayload {
    /// Echoed back, because a client that asked for `MONTH` and a server that defaulted to `WEEK`
    /// would otherwise disagree silently.
    pub period: String,
    /// Who is actually in it. Friends who have their statistics closed are absent, not listed
    /// empty — being in someone's recap is a thing you opt into.
    pub members: Vec<String>,
    pub anthem: Option<AnthemPayload>,
    pub top_tracks: Vec<StatEntry>,
    pub top_artists: Vec<StatEntry>,
    pub trendsetter: Option<TrendsetterPayload>,
    pub matrix: Vec<TasteMatrixEntry>,
}

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
    async fn circle_recap(
        &self,
        ctx: &Context<'_>,
        period: Option<String>,
    ) -> async_graphql::Result<CircleRecapPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();
        let period = period.unwrap_or_else(|| "MONTH".to_string()).to_uppercase();
        let since = crate::stats::period_start(&period, now);

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
            histories.push((
                member.clone(),
                db.scrobble_rows(member, None, since.as_deref())?,
            ));
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

fn to_feed_payload(item: crate::db_feed::FeedItem) -> FeedItemPayload {
    let (kind, summary, artist, title, count) = match &item.event {
        FeedEvent::Milestone { artist, plays } => (
            "MILESTONE",
            format!("{} reached {plays} plays of {artist}", item.username),
            artist.clone(),
            None,
            *plays,
        ),
        FeedEvent::OnRepeat { title, artist, plays } => (
            "ON_REPEAT",
            format!(
                "{} has played {title} by {artist} {plays} times today",
                item.username
            ),
            artist.clone(),
            Some(title.clone()),
            *plays,
        ),
        FeedEvent::NewFavourite { artist, tracks } => (
            "NEW_FAVOURITE",
            format!("{} is getting into {artist} — {tracks} tracks so far", item.username),
            artist.clone(),
            None,
            *tracks,
        ),
    };

    FeedItemPayload {
        username: item.username,
        at: item.at,
        kind: kind.to_string(),
        summary,
        artist,
        title,
        count,
    }
}

fn track_key(row: &crate::db::ScrobbleRow) -> String {
    format!("{} — {}", row.track_title.trim(), row.artist_name.trim())
}

fn artist_key(row: &crate::db::ScrobbleRow) -> String {
    row.artist_name.trim().to_string()
}

/// The circle's most-played, by whatever key is asked for.
fn circle_top(
    histories: &[(String, Vec<crate::db::ScrobbleRow>)],
    key: fn(&crate::db::ScrobbleRow) -> String,
    top_n: usize,
) -> Vec<StatEntry> {
    let mut counts: HashMap<String, i64> = HashMap::new();
    for (_, rows) in histories {
        for row in rows {
            let name = key(row);
            if name.trim().is_empty() {
                continue;
            }
            *counts.entry(name).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, i64)> = counts.into_iter().collect();
    // Plays first, then name — so a tie has one answer rather than whichever the hash map felt
    // like, and two reads of unchanged data agree.
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(top_n);
    ranked
        .into_iter()
        .map(|(name, value)| StatEntry { name, value })
        .collect()
}

/// The one track the circle played most, and who was responsible.
fn anthem(histories: &[(String, Vec<crate::db::ScrobbleRow>)]) -> Option<AnthemPayload> {
    let top = circle_top(histories, track_key, 1).into_iter().next()?;

    let mut by_member: Vec<StatEntry> = histories
        .iter()
        .map(|(member, rows)| StatEntry {
            name: member.clone(),
            value: rows.iter().filter(|row| track_key(row) == top.name).count() as i64,
        })
        .filter(|entry| entry.value > 0)
        .collect();
    by_member.sort_by(|a, b| b.value.cmp(&a.value).then_with(|| a.name.cmp(&b.name)));

    // The key is "Title — Artist"; split it back apart so a client does not have to.
    let (title, artist) = match top.name.split_once(" — ") {
        Some((title, artist)) => (title.to_string(), artist.to_string()),
        None => (top.name.clone(), String::new()),
    };

    Some(AnthemPayload {
        title,
        artist,
        plays: top.value,
        by_member,
    })
}

/// Who in the circle tends to get to things first.
///
/// For each of the circle's top tracks, the member whose earliest play precedes everyone else's
/// wins it; whoever wins most is the trendsetter. A track only one person played counts — being
/// alone in liking something early is still being early.
///
/// A track where the earliest plays are simultaneous is awarded to nobody. This used to break the
/// tie by username, which was harmless when play times were exact and a tie meant the same second.
/// Play times are now stored to the hour (see `Db::record_scrobbles`), so a tie means "both of them
/// some time that hour" and is the common case rather than a freak one — and breaking it
/// alphabetically would hand a real, displayed credit to whoever is nearer the start of the
/// alphabet, every time. There is no answer in the data, so the honest output is no winner for
/// that track rather than a confident wrong one.
fn trendsetter(histories: &[(String, Vec<crate::db::ScrobbleRow>)]) -> Option<TrendsetterPayload> {
    let top = circle_top(histories, track_key, TRENDSETTER_DEPTH);
    let mut wins: HashMap<&str, Vec<String>> = HashMap::new();

    for entry in &top {
        let mut earliest: Option<(&str, i64)> = None;
        let mut tied = false;
        for (member, rows) in histories {
            let first = rows
                .iter()
                .filter(|row| track_key(row) == entry.name)
                .filter_map(|row| crate::stats::parse_time(&row.played_at))
                .min();
            let Some(at) = first else { continue };
            match earliest {
                None => earliest = Some((member.as_str(), at)),
                Some((_, held_at)) if at < held_at => {
                    earliest = Some((member.as_str(), at));
                    tied = false;
                }
                Some((_, held_at)) if at == held_at => tied = true,
                Some(_) => {}
            }
        }
        if let Some((member, _)) = earliest {
            if !tied {
                wins.entry(member).or_default().push(entry.name.clone());
            }
        }
    }

    let (username, examples) = wins
        .into_iter()
        .max_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| b.0.cmp(a.0)))?;

    Some(TrendsetterPayload {
        username: username.to_string(),
        firsts: examples.len() as i64,
        examples: examples.into_iter().take(3).collect(),
    })
}

/// Every pair's taste-match score, computed once per pair.
///
/// Reuses `schema_social::taste_match` rather than scoring again here. Two overlap rules that were
/// meant to be the same rule is exactly the kind of thing that drifts.
fn matrix(histories: &[(String, Vec<crate::db::ScrobbleRow>)], now: i64) -> Vec<TasteMatrixEntry> {
    let computed: Vec<(&str, crate::stats::Stats)> = histories
        .iter()
        .map(|(member, rows)| (member.as_str(), crate::stats::compute(rows, 50, now)))
        .collect();

    let mut entries = Vec::new();
    for (i, (a, stats_a)) in computed.iter().enumerate() {
        for (b, stats_b) in computed.iter().skip(i + 1) {
            entries.push(TasteMatrixEntry {
                a: a.to_string(),
                b: b.to_string(),
                score: crate::schema_social::taste_match(stats_a, stats_b).score,
            });
        }
    }
    entries.sort_by(|x, y| y.score.cmp(&x.score).then_with(|| x.a.cmp(&y.a)));
    entries
}
