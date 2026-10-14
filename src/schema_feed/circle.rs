//! Computing the circle recap: anthem, trendsetter and taste matrix.

use super::*;
use crate::db_feed::FeedEvent;
use crate::schema::StatEntry;
use std::collections::HashMap;

pub(super) fn to_feed_payload(item: crate::db_feed::FeedItem) -> FeedItemPayload {
    let (kind, summary, artist, title, count) = match &item.event {
        FeedEvent::Milestone { artist, plays } => (
            "MILESTONE",
            format!("{} reached {plays} plays of {artist}", item.username),
            artist.clone(),
            None,
            *plays,
        ),
        FeedEvent::OnRepeat {
            title,
            artist,
            plays,
        } => (
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
            format!(
                "{} is getting into {artist} — {tracks} tracks so far",
                item.username
            ),
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

pub(super) fn track_key(row: &crate::db::ScrobbleRow) -> String {
    format!("{} — {}", row.track_title.trim(), row.artist_name.trim())
}

pub(super) fn artist_key(row: &crate::db::ScrobbleRow) -> String {
    row.artist_name.trim().to_string()
}

/// The circle's most-played, by whatever key is asked for.
pub(super) fn circle_top(
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
pub(super) fn anthem(histories: &[(String, Vec<crate::db::ScrobbleRow>)]) -> Option<AnthemPayload> {
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
pub(super) fn trendsetter(
    histories: &[(String, Vec<crate::db::ScrobbleRow>)],
) -> Option<TrendsetterPayload> {
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
pub(super) fn matrix(
    histories: &[(String, Vec<crate::db::ScrobbleRow>)],
    now: i64,
) -> Vec<TasteMatrixEntry> {
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
