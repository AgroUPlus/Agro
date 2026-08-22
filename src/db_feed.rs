//! The activity feed: what someone has been into lately, read out of the plays themselves.
//!
//! Nothing here is stored. The feed is *derived* from `scrobbles` on demand, and that is the whole
//! design decision worth recording:
//!
//!   * A stored feed would be a second copy of listening history, which means the visibility rules
//!     would have to be applied to it separately from the rows it was copied from — two places that
//!     have to agree, which is how a surface ends up leaking after the switch behind it was closed.
//!   * A stored feed would also start empty. Everyone who listened to anything before this module
//!     existed would have no history until they played something new, and a "milestone" feature
//!     whose first month is silent is not the feature.
//!   * Deriving costs one indexed query per friend, over rows that are already being read for the
//!     statistics page. That is cheap enough that storing was never worth what it costs.
//!
//! The consequence, stated plainly: a play that is deleted stops having happened, and the feed
//! rewrites itself. That is the correct behaviour for a history nobody asked to have pinned down.

use std::collections::{HashMap, HashSet};

use crate::db::ScrobbleRow;
use crate::stats::parse_time;

const DAY: i64 = 86_400;

/// Play counts worth saying out loud. Round numbers, because a milestone is a human observation
/// rather than a measurement — nobody cares about their 37th play of an album.
const MILESTONES: &[i64] = &[10, 25, 50, 100, 250, 500, 1000];

/// Plays of one track inside a day before it counts as being on repeat.
///
/// Four rather than three: three plays of a song over an evening is just liking it, and a feed that
/// announces that says nothing. Four in twenty-four hours is a person with a song stuck in them.
const ON_REPEAT_PLAYS: i64 = 4;

/// Distinct tracks by one artist before a new arrival counts as a favourite rather than a try.
const NEW_FAVOURITE_TRACKS: i64 = 5;

/// How far back an artist must be unheard-of for a burst of plays to read as *new*.
///
/// Without this every artist anyone had ever paused on for a fortnight would resurface as a
/// discovery the moment they came back to it.
const NEW_FAVOURITE_LOOKBACK: i64 = 90 * DAY;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FeedEvent {
    /// Crossed a round number of plays of one artist.
    Milestone { artist: String, plays: i64 },
    /// Played the same track several times inside a day.
    OnRepeat { title: String, artist: String, plays: i64 },
    /// Took to an artist they had not been listening to before.
    NewFavourite { artist: String, tracks: i64 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedItem {
    /// Filled in by the caller, which is the only thing that knows whose rows these were.
    pub username: String,
    /// RFC3339, taken from the play that caused the event — not the time the feed was read. A feed
    /// that restamped itself on every request would reorder as you watched it.
    pub at: String,
    pub event: FeedEvent,
}

/// Every event visible in `rows`, newest first.
///
/// `since` is a unix timestamp: events caused by a play older than it are not reported. The full
/// history is still *read*, because a milestone is a statement about a total and cannot be counted
/// from a window — only the moment of crossing has to fall inside one.
pub fn events_for(rows: &[ScrobbleRow], since: i64, now: i64) -> Vec<FeedItem> {
    // Oldest first: every rule below is about the order things happened in, and the caller's rows
    // arrive in whatever order the query gave them.
    let mut ordered: Vec<(&ScrobbleRow, i64)> = rows
        .iter()
        .filter_map(|row| parse_time(&row.played_at).map(|at| (row, at)))
        .collect();
    ordered.sort_by_key(|(_, at)| *at);

    let mut items = Vec::new();
    items.extend(milestones(&ordered, since));
    items.extend(on_repeat(&ordered, now));
    items.extend(new_favourites(&ordered, since, now));

    // Newest first, and stable beyond that so two events stamped the same second do not swap
    // places between two reads of the same data.
    items.sort_by(|a, b| b.at.cmp(&a.at));
    items
}

/// Round-number play counts per artist, reported at the play that crossed them.
fn milestones(ordered: &[(&ScrobbleRow, i64)], since: i64) -> Vec<FeedItem> {
    let mut counts: HashMap<&str, i64> = HashMap::new();
    let mut items = Vec::new();

    for (row, at) in ordered {
        let artist = row.artist_name.trim();
        if artist.is_empty() {
            continue;
        }
        let count = counts.entry(artist).or_insert(0);
        *count += 1;
        // Only the crossing play reports, and only if it is recent enough to still be news.
        if MILESTONES.contains(count) && *at >= since {
            items.push(FeedItem {
                username: String::new(),
                at: row.played_at.clone(),
                event: FeedEvent::Milestone {
                    artist: artist.to_string(),
                    plays: *count,
                },
            });
        }
    }
    items
}

/// Tracks played [`ON_REPEAT_PLAYS`] times or more in the last day.
///
/// One item per track however many times it was played, stamped with the most recent play: this is
/// a statement about today, not a list of the individual plays that made it true.
fn on_repeat(ordered: &[(&ScrobbleRow, i64)], now: i64) -> Vec<FeedItem> {
    let day_ago = now - DAY;
    let mut counts: HashMap<(&str, &str), (i64, &str)> = HashMap::new();

    for (row, at) in ordered {
        if *at < day_ago {
            continue;
        }
        let key = (row.track_title.trim(), row.artist_name.trim());
        if key.0.is_empty() {
            continue;
        }
        let entry = counts.entry(key).or_insert((0, &row.played_at));
        entry.0 += 1;
        // Rows are in time order, so the last one seen is the latest.
        entry.1 = &row.played_at;
    }

    counts
        .into_iter()
        .filter(|(_, (plays, _))| *plays >= ON_REPEAT_PLAYS)
        .map(|((title, artist), (plays, at))| FeedItem {
            username: String::new(),
            at: at.to_string(),
            event: FeedEvent::OnRepeat {
                title: title.to_string(),
                artist: artist.to_string(),
                plays,
            },
        })
        .collect()
}

/// Artists first heard inside the window who have since accumulated several distinct tracks.
fn new_favourites(ordered: &[(&ScrobbleRow, i64)], since: i64, now: i64) -> Vec<FeedItem> {
    let lookback = now - NEW_FAVOURITE_LOOKBACK;
    let mut first_heard: HashMap<&str, i64> = HashMap::new();
    let mut tracks: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut latest: HashMap<&str, &str> = HashMap::new();

    for (row, at) in ordered {
        let artist = row.artist_name.trim();
        if artist.is_empty() {
            continue;
        }
        first_heard.entry(artist).or_insert(*at);
        tracks.entry(artist).or_default().insert(row.track_title.trim());
        latest.insert(artist, &row.played_at);
    }

    first_heard
        .into_iter()
        // Heard for the first time recently — both inside the reporting window, and with nothing
        // before the lookback that would make this a return rather than a discovery.
        .filter(|(_, at)| *at >= since && *at >= lookback)
        .filter_map(|(artist, _)| {
            let count = tracks.get(artist).map(|set| set.len() as i64).unwrap_or(0);
            if count < NEW_FAVOURITE_TRACKS {
                return None;
            }
            Some(FeedItem {
                username: String::new(),
                at: latest.get(artist)?.to_string(),
                event: FeedEvent::NewFavourite {
                    artist: artist.to_string(),
                    tracks: count,
                },
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(title: &str, artist: &str, at: i64) -> ScrobbleRow {
        ScrobbleRow {
            track_title: title.to_string(),
            artist_name: artist.to_string(),
            album_name: None,
            genre: None,
            duration_secs: 180,
            device_name: "test".to_string(),
            played_at: chrono::DateTime::from_timestamp(at, 0)
                .unwrap()
                .to_rfc3339(),
        }
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_milestone_is_reported_once_at_the_play_that_crossed_it() {
        let rows: Vec<ScrobbleRow> = (0..12)
            .map(|i| row("t", "Boards of Canada", NOW - 1000 + i))
            .collect();
        let items = events_for(&rows, NOW - DAY, NOW);
        let milestones: Vec<_> = items
            .iter()
            .filter(|item| matches!(item.event, FeedEvent::Milestone { .. }))
            .collect();
        assert_eq!(milestones.len(), 1, "one crossing, not one per play after it");
        assert!(matches!(
            milestones[0].event,
            FeedEvent::Milestone { plays: 10, .. }
        ));
    }

    #[test]
    fn a_milestone_crossed_before_the_window_is_not_news() {
        let rows: Vec<ScrobbleRow> = (0..10)
            .map(|i| row("t", "Autechre", NOW - 10 * DAY + i))
            .collect();
        let items = events_for(&rows, NOW - DAY, NOW);
        assert!(
            !items
                .iter()
                .any(|item| matches!(item.event, FeedEvent::Milestone { .. })),
            "the tenth play happened ten days ago"
        );
    }

    #[test]
    fn four_plays_in_a_day_is_on_repeat_and_three_is_not() {
        let three: Vec<ScrobbleRow> = (0..3).map(|i| row("Windowlicker", "Aphex Twin", NOW - 100 + i)).collect();
        let four: Vec<ScrobbleRow> = (0..4).map(|i| row("Windowlicker", "Aphex Twin", NOW - 100 + i)).collect();

        let repeats = |rows: &[ScrobbleRow]| {
            events_for(rows, NOW - DAY, NOW)
                .into_iter()
                .filter(|item| matches!(item.event, FeedEvent::OnRepeat { .. }))
                .count()
        };
        assert_eq!(repeats(&three), 0);
        assert_eq!(repeats(&four), 1);
    }

    #[test]
    fn on_repeat_only_counts_the_last_day() {
        let rows: Vec<ScrobbleRow> = (0..6)
            .map(|i| row("Xtal", "Aphex Twin", NOW - 3 * DAY + i))
            .collect();
        let items = events_for(&rows, NOW - 7 * DAY, NOW);
        assert!(!items
            .iter()
            .any(|item| matches!(item.event, FeedEvent::OnRepeat { .. })));
    }

    #[test]
    fn a_new_favourite_needs_several_distinct_tracks() {
        // Five plays, but all of the same song: enthusiasm for a track, not for an artist.
        let same: Vec<ScrobbleRow> = (0..5).map(|i| row("one", "Burial", NOW - 500 + i)).collect();
        // Five different songs.
        let spread: Vec<ScrobbleRow> = (0..5)
            .map(|i| row(&format!("t{i}"), "Burial", NOW - 500 + i))
            .collect();

        let favourites = |rows: &[ScrobbleRow]| {
            events_for(rows, NOW - DAY, NOW)
                .into_iter()
                .filter(|item| matches!(item.event, FeedEvent::NewFavourite { .. }))
                .count()
        };
        assert_eq!(favourites(&same), 0);
        assert_eq!(favourites(&spread), 1);
    }

    #[test]
    fn an_artist_heard_long_ago_is_not_a_discovery() {
        let mut rows = vec![row("old", "Portishead", NOW - 200 * DAY)];
        rows.extend((0..5).map(|i| row(&format!("t{i}"), "Portishead", NOW - 500 + i)));
        let items = events_for(&rows, NOW - DAY, NOW);
        assert!(
            !items
                .iter()
                .any(|item| matches!(item.event, FeedEvent::NewFavourite { .. })),
            "they were already listening to this artist months ago"
        );
    }

    #[test]
    fn events_come_back_newest_first() {
        let mut rows: Vec<ScrobbleRow> = (0..10).map(|i| row("t", "A", NOW - 5000 + i)).collect();
        rows.extend((0..5).map(|i| row(&format!("u{i}"), "B", NOW - 100 + i)));
        let items = events_for(&rows, NOW - DAY, NOW);
        assert!(items.len() >= 2);
        for pair in items.windows(2) {
            assert!(pair[0].at >= pair[1].at);
        }
    }

    #[test]
    fn a_row_with_an_unparseable_timestamp_is_skipped_rather_than_panicking() {
        let mut rows: Vec<ScrobbleRow> = (0..10).map(|i| row("t", "A", NOW - 500 + i)).collect();
        rows.push(ScrobbleRow {
            played_at: "not a date".to_string(),
            ..row("t", "A", NOW)
        });
        let items = events_for(&rows, NOW - DAY, NOW);
        assert!(items.iter().any(|item| matches!(item.event, FeedEvent::Milestone { .. })));
    }
}
