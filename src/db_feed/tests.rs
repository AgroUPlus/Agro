use super::*;
use crate::db::ScrobbleRow;

pub(super) fn row(title: &str, artist: &str, at: i64) -> ScrobbleRow {
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
pub(super) fn a_milestone_is_reported_once_at_the_play_that_crossed_it() {
    let rows: Vec<ScrobbleRow> = (0..12)
        .map(|i| row("t", "Boards of Canada", NOW - 1000 + i))
        .collect();
    let items = events_for(&rows, NOW - DAY, NOW);
    let milestones: Vec<_> = items
        .iter()
        .filter(|item| matches!(item.event, FeedEvent::Milestone { .. }))
        .collect();
    assert_eq!(
        milestones.len(),
        1,
        "one crossing, not one per play after it"
    );
    assert!(matches!(
        milestones[0].event,
        FeedEvent::Milestone { plays: 10, .. }
    ));
}

#[test]
pub(super) fn a_milestone_crossed_before_the_window_is_not_news() {
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
pub(super) fn four_plays_in_a_day_is_on_repeat_and_three_is_not() {
    let three: Vec<ScrobbleRow> = (0..3)
        .map(|i| row("Windowlicker", "Aphex Twin", NOW - 100 + i))
        .collect();
    let four: Vec<ScrobbleRow> = (0..4)
        .map(|i| row("Windowlicker", "Aphex Twin", NOW - 100 + i))
        .collect();

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
pub(super) fn on_repeat_only_counts_the_last_day() {
    let rows: Vec<ScrobbleRow> = (0..6)
        .map(|i| row("Xtal", "Aphex Twin", NOW - 3 * DAY + i))
        .collect();
    let items = events_for(&rows, NOW - 7 * DAY, NOW);
    assert!(!items
        .iter()
        .any(|item| matches!(item.event, FeedEvent::OnRepeat { .. })));
}

#[test]
pub(super) fn a_new_favourite_needs_several_distinct_tracks() {
    // Five plays, but all of the same song: enthusiasm for a track, not for an artist.
    let same: Vec<ScrobbleRow> = (0..5)
        .map(|i| row("one", "Burial", NOW - 500 + i))
        .collect();
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
pub(super) fn an_artist_heard_long_ago_is_not_a_discovery() {
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
pub(super) fn events_come_back_newest_first() {
    let mut rows: Vec<ScrobbleRow> = (0..10).map(|i| row("t", "A", NOW - 5000 + i)).collect();
    rows.extend((0..5).map(|i| row(&format!("u{i}"), "B", NOW - 100 + i)));
    let items = events_for(&rows, NOW - DAY, NOW);
    assert!(items.len() >= 2);
    for pair in items.windows(2) {
        assert!(pair[0].at >= pair[1].at);
    }
}

#[test]
pub(super) fn a_row_with_an_unparseable_timestamp_is_skipped_rather_than_panicking() {
    let mut rows: Vec<ScrobbleRow> = (0..10).map(|i| row("t", "A", NOW - 500 + i)).collect();
    rows.push(ScrobbleRow {
        played_at: "not a date".to_string(),
        ..row("t", "A", NOW)
    });
    let items = events_for(&rows, NOW - DAY, NOW);
    assert!(items
        .iter()
        .any(|item| matches!(item.event, FeedEvent::Milestone { .. })));
}
