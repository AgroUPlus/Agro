use super::*;
use crate::db::ScrobbleRow;
use std::collections::HashSet;

pub(super) fn row(artist: &str, title: &str, played_at: &str) -> ScrobbleRow {
    ScrobbleRow {
        track_title: title.to_string(),
        artist_name: artist.to_string(),
        album_name: Some("An Album".to_string()),
        genre: Some("Electronic".to_string()),
        duration_secs: 300,
        device_name: "phone".to_string(),
        played_at: played_at.to_string(),
    }
}

pub(super) fn days(active: &[(i32, u32)]) -> HashSet<(i32, u32)> {
    active.iter().copied().collect()
}

/// The defect this replaced: five active days spread over a month reported as a five-day
/// streak.
#[test]
pub(super) fn a_streak_is_the_longest_run_not_the_count_of_active_days() {
    let active = days(&[(2026, 1), (2026, 2), (2026, 3), (2026, 10), (2026, 20)]);
    assert_eq!(active.len(), 5, "five days were listened on");
    assert_eq!(longest_streak(&active), 3, "but the longest run is three");
}

#[test]
pub(super) fn a_streak_crosses_the_turn_of_the_year() {
    // 2025 is not a leap year, so 31 December is ordinal 365.
    let active = days(&[(2025, 364), (2025, 365), (2026, 1), (2026, 2)]);
    assert_eq!(longest_streak(&active), 4);
}

#[test]
pub(super) fn no_plays_is_no_streak() {
    assert_eq!(longest_streak(&days(&[])), 0);
}

#[test]
pub(super) fn one_day_is_a_streak_of_one() {
    assert_eq!(longest_streak(&days(&[(2026, 200)])), 1);
}

/// The reason the whole computation runs in local time: a play just before local midnight on
/// New Year's Eve belongs to the year that is ending, not the one starting in UTC.
#[test]
pub(super) fn a_play_at_the_turn_of_the_year_lands_in_the_local_year() {
    // 23:30 on 31 December 2026 at UTC+2 is 21:30 UTC on the same day — but at UTC-3 the same
    // instant is already 1 January.
    let rows = vec![row(
        "Boards of Canada",
        "Roygbiv",
        "2026-12-31T23:30:00+02:00",
    )];

    let local = compute_wrapped(&rows, 2026, None, 10, 120);
    assert_eq!(local.total_plays, 1, "it is still 2026 where it was played");

    let west = compute_wrapped(&rows, 2026, None, 10, -300);
    assert_eq!(west.total_plays, 1, "and 2026 in UTC-5 too, at 16:30");

    let far_east = compute_wrapped(&rows, 2026, None, 10, 13 * 60);
    assert_eq!(far_east.total_plays, 0, "but 2027 in UTC+13");
    assert_eq!(
        compute_wrapped(&rows, 2027, None, 10, 13 * 60).total_plays,
        1
    );
}

#[test]
pub(super) fn a_local_offset_moves_the_peak_hour() {
    let rows = vec![
        row("Aphex Twin", "Xtal", "2026-06-15T02:00:00Z"),
        row("Aphex Twin", "Ageispolis", "2026-06-15T02:30:00Z"),
    ];

    let wrapped = compute_wrapped(&rows, 2026, None, 10, 120);
    assert_eq!(wrapped.top_hour_utc, Some(2), "02:00 UTC");
    assert_eq!(wrapped.top_hour_local, Some(4), "is 04:00 at UTC+2");
    assert_eq!(wrapped.by_hour[4], 2);
}

#[test]
pub(super) fn an_empty_year_is_an_empty_recap_not_a_zeroed_one() {
    let rows = vec![row("Daft Punk", "Da Funk", "2025-03-01T12:00:00Z")];
    let wrapped = compute_wrapped(&rows, 2026, None, 10, 0);

    assert_eq!(wrapped.total_plays, 0);
    assert_eq!(wrapped.total_artists, 0);
    assert_eq!(wrapped.longest_streak_days, 0);
    assert!(
        wrapped.top_hour_local.is_none(),
        "no peak hour without plays"
    );
    assert!(wrapped.first_play.is_none());
    assert_eq!(
        wrapped.by_month.len(),
        12,
        "the shape is fixed even when empty"
    );
}

#[test]
pub(super) fn counts_the_period_and_names_what_was_new_in_it() {
    let rows = vec![
        row("Daft Punk", "Around the World", "2025-06-15T14:30:00Z"),
        row("Daft Punk", "One More Time", "2026-02-10T10:00:00Z"),
        row("Aphex Twin", "Windowlicker", "2026-08-20T18:00:00Z"),
        row("Aphex Twin", "Xtal", "2026-08-21T18:00:00Z"),
    ];

    let wrapped = compute_wrapped(&rows, 2026, None, 10, 0);

    assert_eq!(wrapped.total_plays, 3);
    assert_eq!(wrapped.total_minutes, 900 / 60);
    assert_eq!(wrapped.total_artists, 2);
    // Daft Punk was already played in 2025; Aphex Twin was not.
    assert_eq!(wrapped.new_artists_count, 1);
    assert_eq!(wrapped.active_days_count, 3);
    assert_eq!(wrapped.longest_streak_days, 2, "20 and 21 August");
    assert_eq!(wrapped.by_month[1], 1, "February");
    assert_eq!(wrapped.by_month[7], 2, "August");
    assert_eq!(wrapped.by_device, vec![("phone".to_string(), 3)]);
    assert_eq!(wrapped.first_play.as_deref(), Some("2026-02-10T10:00:00Z"));
    assert_eq!(wrapped.last_play.as_deref(), Some("2026-08-21T18:00:00Z"));
}

#[test]
pub(super) fn a_month_recap_is_scoped_to_that_month() {
    let rows = vec![
        row("Daft Punk", "One More Time", "2026-02-10T10:00:00Z"),
        row("Aphex Twin", "Windowlicker", "2026-08-20T18:00:00Z"),
    ];

    let wrapped = compute_wrapped(&rows, 2026, Some(8), 10, 0);
    assert_eq!(wrapped.total_plays, 1);
    // February is "before the period", so Daft Punk counts as already known.
    assert_eq!(wrapped.new_artists_count, 1);
}

#[test]
pub(super) fn year_bounds_are_half_open_in_the_callers_offset() {
    let (start, end) = year_bounds(2026, 120).expect("a valid offset");
    assert!(start.starts_with("2026-01-01T00:00:00+02:00"), "{start}");
    assert!(end.starts_with("2027-01-01T00:00:00+02:00"), "{end}");
}
