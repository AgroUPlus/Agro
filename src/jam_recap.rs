//! What a jam was, worked out from what it played.
//!
//! Pure: [`JamRecap::build`] takes the rows `db_jam_recap` read and decides nothing about storage
//! or delivery, so the rules — who counts as having been there, which track the room loved, which
//! it could not get rid of fast enough — are tested here without a database.
//!
//! Everything in a recap is something its reader already saw in the room: the tracks, who queued
//! them, and the votes that were on screen while it ran. It adds no new disclosure, only memory.

use async_graphql::SimpleObject;
use serde::{Deserialize, Serialize};

/// The most tracks one recap keeps. An all-day jam can play hundreds, and every member gets their
/// own copy; past this the list is cut and [`JamRecap::tracks_omitted`] says by how much.
pub const MAX_RECAP_TRACKS: usize = 300;

/// One track the room heard, with the reaction it got.
#[derive(SimpleObject, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecapTrack {
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub track_uri: String,
    pub added_by: String,
    pub duration_ms: i64,
    /// Approvals from people other than whoever proposed it.
    pub approvals: i64,
    pub skip_votes: i64,
}

/// Who queued the most of what was played.
#[derive(SimpleObject, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RecapContributor {
    pub username: String,
    pub tracks: i64,
}

#[derive(SimpleObject, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct JamRecap {
    pub started_at: String,
    pub ended_at: String,
    /// Wall-clock time the reader spent in the room, not the sum of track lengths: a skipped track
    /// did not last its duration.
    pub duration_ms: i64,
    /// Everyone who was in the room or put something in it, sorted.
    pub people: Vec<String>,
    /// In the order they played.
    pub tracks: Vec<RecapTrack>,
    pub tracks_omitted: i64,
    pub top_contributor: Option<RecapContributor>,
    /// The played track with the most approvals. `None` when nothing was approved by anyone.
    pub most_loved: Option<RecapTrack>,
    /// The played track with the most skip votes. `None` when nobody voted to skip anything.
    pub most_skipped: Option<RecapTrack>,
}

impl JamRecap {
    /// Summarises a jam, or says there is nothing worth summarising.
    ///
    /// A recap needs a track and a second person. A room that played nothing has no story, and
    /// one person alone in a jam was listening to their own queue — their history already has it.
    pub fn build(
        started_at: &str,
        ended_at: &str,
        members: &[String],
        played: Vec<RecapTrack>,
    ) -> Option<JamRecap> {
        let mut people: Vec<String> = members
            .iter()
            .chain(played.iter().map(|t| &t.added_by))
            .map(|name| name.trim().to_lowercase())
            .filter(|name| !name.is_empty())
            .collect();
        people.sort();
        people.dedup();
        if played.is_empty() || people.len() < 2 {
            return None;
        }

        let top_contributor = top_contributor(&played);
        // Ties go to whichever played first: `max_by_key` keeps the last maximum, so the search
        // runs backwards to keep the earliest instead.
        let most_loved = played
            .iter()
            .rev()
            .filter(|t| t.approvals > 0)
            .max_by_key(|t| t.approvals)
            .cloned();
        let most_skipped = played
            .iter()
            .rev()
            .filter(|t| t.skip_votes > 0)
            .max_by_key(|t| t.skip_votes)
            .cloned();

        let total = played.len();
        let mut tracks = played;
        tracks.truncate(MAX_RECAP_TRACKS);

        Some(JamRecap {
            started_at: started_at.to_string(),
            ended_at: ended_at.to_string(),
            duration_ms: elapsed_ms(started_at, ended_at),
            people,
            tracks_omitted: (total - tracks.len()) as i64,
            tracks,
            top_contributor,
            most_loved,
            most_skipped,
        })
    }
}

/// The member with the most played tracks to their name, if anyone named is left; on a tie, the one alphabetically first,
/// so the answer never depends on row order.
pub(crate) fn top_contributor(played: &[RecapTrack]) -> Option<RecapContributor> {
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    // A blank name is a member whose account was deleted (see `forget`); they rank nowhere.
    for track in played.iter().filter(|t| !t.added_by.trim().is_empty()) {
        *counts
            .entry(track.added_by.trim().to_lowercase())
            .or_default() += 1;
    }
    counts
        .into_iter()
        .rev()
        .max_by_key(|(_, n)| *n)
        .map(|(username, tracks)| RecapContributor { username, tracks })
}

fn elapsed_ms(from: &str, to: &str) -> i64 {
    match (
        chrono::DateTime::parse_from_rfc3339(from),
        chrono::DateTime::parse_from_rfc3339(to),
    ) {
        (Ok(a), Ok(b)) => (b - a).num_milliseconds().max(0),
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, by: &str, approvals: i64, skips: i64) -> RecapTrack {
        RecapTrack {
            title: title.into(),
            artist: "Artist".into(),
            artwork_url: None,
            track_uri: format!("uri:{title}"),
            added_by: by.into(),
            duration_ms: 180_000,
            approvals,
            skip_votes: skips,
        }
    }

    const START: &str = "2026-10-04T20:00:00+00:00";
    const END: &str = "2026-10-04T21:30:00+00:00";

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn nothing_played_means_no_recap() {
        assert!(JamRecap::build(START, END, &names(&["alex", "sam"]), vec![]).is_none());
    }

    #[test]
    fn a_room_of_one_is_not_a_jam_worth_recapping() {
        let played = vec![track("a", "alex", 0, 0)];
        assert!(JamRecap::build(START, END, &names(&["alex"]), played).is_none());
    }

    #[test]
    fn someone_who_left_still_counts_through_what_they_queued() {
        let played = vec![track("a", "sam", 0, 0)];
        let recap = JamRecap::build(START, END, &names(&["Alex"]), played).unwrap();
        assert_eq!(recap.people, names(&["alex", "sam"]));
    }

    #[test]
    fn duration_is_wall_clock_not_track_lengths() {
        let played = vec![track("a", "alex", 0, 0), track("b", "sam", 0, 0)];
        let recap = JamRecap::build(START, END, &names(&["alex", "sam"]), played).unwrap();
        assert_eq!(recap.duration_ms, 90 * 60 * 1000);
    }

    #[test]
    fn highlights_pick_the_extremes_and_ties_go_to_the_earliest() {
        let played = vec![
            track("first", "alex", 2, 0),
            track("second", "sam", 2, 1),
            track("third", "sam", 0, 3),
        ];
        let recap = JamRecap::build(START, END, &names(&["alex", "sam"]), played).unwrap();
        assert_eq!(recap.most_loved.unwrap().title, "first");
        assert_eq!(recap.most_skipped.unwrap().title, "third");
        let top = recap.top_contributor.unwrap();
        assert_eq!((top.username.as_str(), top.tracks), ("sam", 2));
    }

    #[test]
    fn no_votes_means_no_highlight_rather_than_an_arbitrary_one() {
        let played = vec![track("a", "alex", 0, 0), track("b", "sam", 0, 0)];
        let recap = JamRecap::build(START, END, &names(&["alex", "sam"]), played).unwrap();
        assert!(recap.most_loved.is_none());
        assert!(recap.most_skipped.is_none());
    }

    #[test]
    fn a_contributor_tie_goes_alphabetically() {
        let played = vec![track("a", "sam", 0, 0), track("b", "alex", 0, 0)];
        let recap = JamRecap::build(START, END, &names(&["alex", "sam"]), played).unwrap();
        assert_eq!(recap.top_contributor.unwrap().username, "alex");
    }

    #[test]
    fn a_long_jam_is_cut_and_says_by_how_much() {
        let played: Vec<_> = (0..MAX_RECAP_TRACKS + 5)
            .map(|i| {
                track(
                    &i.to_string(),
                    if i % 2 == 0 { "alex" } else { "sam" },
                    0,
                    0,
                )
            })
            .collect();
        let recap = JamRecap::build(START, END, &names(&["alex", "sam"]), played).unwrap();
        assert_eq!(recap.tracks.len(), MAX_RECAP_TRACKS);
        assert_eq!(recap.tracks_omitted, 5);
    }

    #[test]
    fn a_recap_survives_its_own_storage_format() {
        let played = vec![track("a", "alex", 1, 0), track("b", "sam", 0, 2)];
        let recap = JamRecap::build(START, END, &names(&["alex", "sam"]), played).unwrap();
        let json = serde_json::to_string(&recap).unwrap();
        assert_eq!(serde_json::from_str::<JamRecap>(&json).unwrap(), recap);
    }
}
