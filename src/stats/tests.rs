use super::*;
use crate::db::ScrobbleRow;

pub(super) fn row(artist: &str, title: &str, secs: i64, at: i64, device: &str) -> ScrobbleRow {
    ScrobbleRow {
        track_title: title.to_string(),
        artist_name: artist.to_string(),
        album_name: Some("An Album".to_string()),
        genre: Some("Rock".to_string()),
        duration_secs: secs,
        device_name: device.to_string(),
        played_at: chrono::DateTime::from_timestamp(at, 0)
            .unwrap()
            .to_rfc3339(),
    }
}

/// The claim that made hour-resolution storage acceptable: every statistic on this page
/// buckets by hour or by day, so throwing the seconds away changes none of them.
///
/// True for any play that is not sitting on a bucket edge, which is what this checks — a few
/// seconds either side of an hour boundary, mid-hour, and at several distances into the past.
/// The edge case is real and is pinned separately by
/// [`rounding_can_move_a_play_one_bucket_at_an_edge`].
#[test]
pub(super) fn rounding_plays_to_the_hour_changes_no_statistic() {
    // Midnight UTC, so "days ago" arithmetic lines up with the day boundary and the test is
    // about the hour rounding rather than about where a day starts.
    let now = 30 * DAY;
    let offsets = [
        61,           // a minute into the current hour
        3599,         // a second before the hour rolls
        3601,         // a second after it
        2 * DAY + 59, // two days back, just past the hour
        2 * DAY + 3599,
        7 * DAY + 1800,  // mid-hour, a week back
        29 * DAY + 1800, // near the far edge of the heatmap, but not on it
    ];

    let exact: Vec<_> = offsets
        .iter()
        .enumerate()
        .map(|(i, off)| row("A", &format!("t{i}"), 240, now - off, "phone"))
        .collect();
    let coarse: Vec<_> = offsets
        .iter()
        .enumerate()
        .map(|(i, off)| {
            let at = now - off;
            row(
                "A",
                &format!("t{i}"),
                240,
                at - at.rem_euclid(3600),
                "phone",
            )
        })
        .collect();

    let a = compute(&exact, 5, now);
    let b = compute(&coarse, 5, now);

    assert_eq!(a.by_hour, b.by_hour, "hour histogram");
    assert_eq!(a.by_day, b.by_day, "sparkline");
    assert_eq!(a.heatmap, b.heatmap, "heatmap");
    assert_eq!(a.streak, b.streak, "streak");
    assert_eq!(a.secs_total, b.secs_total, "total");
    assert_eq!(a.plays_total, b.plays_total, "play count");
    assert_eq!(a.top_artists, b.top_artists, "top artists");
    assert_eq!(a.top_tracks, b.top_tracks, "top tracks");
    assert_eq!(a.top_genres, b.top_genres, "top genres");
    assert_eq!(a.by_device, b.by_device, "per-device totals");
}

/// The honest exception, pinned so it is a known cost rather than a surprise.
///
/// *Every* window here is measured backwards from "now" — `secs_today` and `secs_week`
/// obviously so, but `by_day`, `heatmap` and `streak` too, via `(now - at) / DAY`. None of them
/// is aligned to a calendar day. So a play within an hour of any bucket edge can round across
/// it and land one bucket earlier.
///
/// The cost is bounded and cosmetic: one play moves by one cell, and the totals it contributes
/// to are unchanged. Worth knowing before someone reads a heatmap as gospel.
#[test]
pub(super) fn rounding_can_move_a_play_one_bucket_at_an_edge() {
    // Mid-hour, which is the point: when "now" falls on an hour boundary every window edge
    // does too, and rounding a play down can never carry it past one. Real clocks do not
    // cooperate like that.
    let now = 30 * DAY + 1800;
    // A minute inside the 24-hour window; rounding down to the hour carries it outside.
    let at = now - DAY + 60;
    let exact = [row("A", "one", 300, at, "phone")];
    let coarse = [row("A", "one", 300, at - at.rem_euclid(3600), "phone")];

    let a = compute(&exact, 5, now);
    let b = compute(&coarse, 5, now);

    assert_eq!(a.secs_total, b.secs_total, "the total is never in doubt");
    assert_eq!(a.plays_total, b.plays_total, "nor is the play count");
    assert_eq!(
        a.by_day.iter().sum::<i64>(),
        b.by_day.iter().sum::<i64>(),
        "the play stays on the sparkline, it only moves along it"
    );
    assert_ne!(
        a.secs_today, b.secs_today,
        "this is the documented edge: the play rounds out of the rolling day"
    );
    assert!(
        (a.secs_today - b.secs_today).abs() <= 300,
        "and it is bounded by one track's duration"
    );
}

#[test]
pub(super) fn totals_split_by_window() {
    let now = 10 * DAY;
    let rows = [
        row("A", "one", 300, now - 60, "phone"),
        row("A", "two", 300, now - 3 * DAY, "laptop"),
        row("B", "three", 300, now - 9 * DAY, "laptop"),
    ];
    let stats = compute(&rows, 5, now);

    assert_eq!(stats.secs_total, 900);
    assert_eq!(stats.secs_today, 300, "only the play from a minute ago");
    assert_eq!(
        stats.secs_week, 600,
        "the nine-day-old play is outside the week"
    );
    assert_eq!(stats.plays_total, 3);
}

#[test]
pub(super) fn ranks_by_count_then_name() {
    let now = 10 * DAY;
    let rows = [
        row("Zed", "x", 10, now - 60, "phone"),
        row("Abe", "y", 10, now - 60, "phone"),
        row("Abe", "z", 10, now - 60, "phone"),
    ];
    let stats = compute(&rows, 5, now);
    assert_eq!(stats.top_artists[0], ("Abe".to_string(), 2));
    assert_eq!(stats.top_artists[1], ("Zed".to_string(), 1));
}

#[test]
pub(super) fn streak_counts_consecutive_days_and_stops_at_a_gap() {
    let now = 10 * DAY;
    let rows = [
        row("A", "today", 10, now - 60, "phone"),
        row("A", "yesterday", 10, now - DAY - 60, "phone"),
        // Nothing two days ago, so the streak is two.
        row("A", "older", 10, now - 3 * DAY, "phone"),
    ];
    assert_eq!(compute(&rows, 5, now).streak, 2);
}

#[test]
pub(super) fn seconds_are_attributed_per_device() {
    let now = 10 * DAY;
    let rows = [
        row("A", "one", 300, now - 60, "phone"),
        row("A", "two", 100, now - 60, "laptop"),
    ];
    let stats = compute(&rows, 5, now);
    assert_eq!(
        stats.by_device,
        vec![("phone".to_string(), 300), ("laptop".to_string(), 100)]
    );
}

#[test]
pub(super) fn an_unparseable_timestamp_still_counts_toward_the_total() {
    let now = 10 * DAY;
    let mut rows = vec![row("A", "one", 300, now - 60, "phone")];
    rows.push(ScrobbleRow {
        played_at: "not a date".to_string(),
        ..row("A", "two", 120, now, "phone")
    });
    let stats = compute(&rows, 5, now);
    assert_eq!(stats.secs_total, 420);
    assert_eq!(stats.secs_today, 300, "but it cannot be placed in a window");
}

#[test]
pub(super) fn period_start_is_none_for_all_time() {
    assert!(period_start("ALL", 0).is_none());
    assert!(period_start("WEEK", 10 * DAY).is_some());
}

#[test]
pub(super) fn reports_when_the_earliest_dated_play_happened() {
    let now = 1_800_000_000;
    let rows = vec![
        row("A", "Late", 60, now - DAY, "phone"),
        row("A", "Early", 60, now - 30 * DAY, "phone"),
        ScrobbleRow {
            played_at: "not a date".to_string(),
            ..row("A", "Undated", 60, now, "phone")
        },
    ];
    assert_eq!(
        compute(&rows, 10, now).first_played_at,
        Some(now - 30 * DAY)
    );
    assert_eq!(compute(&[], 10, now).first_played_at, None);
}
