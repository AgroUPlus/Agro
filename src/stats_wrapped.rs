//! The year (or month) in review — "Agro Replay".
//!
//! Split out of [`crate::stats`] rather than left beside it: the two answer different questions.
//! [`crate::stats::compute`] reports a rolling window ending *now*, which is what a dashboard wants;
//! this one reports a **calendar** period, which is what a recap wants. A recap of "the last 365
//! days" is not a recap of 2026, and the difference shows up every January.
//!
//! ## Everything here is local time
//!
//! Plays are stored as RFC3339 in whatever offset the reporting device used, and the hour buckets,
//! the day buckets and — critically — *which year a play belongs to* all have to be answered in the
//! listener's own time. A play at 23:30 on 31 December lands in the wrong recap otherwise. So every
//! row is shifted into a caller-supplied fixed offset before anything is counted.
//!
//! A fixed offset rather than an IANA zone is a deliberate limit: a recap spanning a daylight-saving
//! change attributes up to an hour of plays to the neighbouring hour bucket. That moves the peak
//! only when two hours are already within that margin of each other, and the alternative is a new
//! dependency for one card.

use crate::db::ScrobbleRow;
use crate::stats::{owned, rank};
use chrono::{Datelike, FixedOffset, Timelike};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Default, Clone)]
pub struct AgroWrapped {
    pub year: i32,
    pub month: Option<i32>,
    pub total_minutes: i64,
    pub total_plays: i64,
    pub top_artists: Vec<(String, i64)>,
    pub top_tracks: Vec<(String, i64)>,
    pub top_albums: Vec<(String, i64)>,
    pub top_genres: Vec<(String, i64)>,
    /// Kept for the clients that already read it, and still in UTC so it keeps meaning what it did.
    pub top_hour_utc: Option<i32>,
    /// The same peak, in the caller's offset. This is the one a recap should show.
    pub top_hour_local: Option<i32>,
    /// The longest run of consecutive days with a play. See [`longest_streak`].
    pub longest_streak_days: i64,
    /// Days with any play at all — a different figure, and a card of its own.
    pub active_days_count: i64,
    pub new_artists_count: i64,
    /// Distinct artists in the period. [`AgroWrapped::top_artists`] is a top-N, so its length says
    /// nothing: a listener with eleven artists and one with four hundred both have ten.
    pub total_artists: i64,
    /// Plays per calendar month, January first. Always twelve entries, even for a month recap,
    /// so the client never has to check the length before indexing.
    pub by_month: Vec<i64>,
    /// Plays per hour of the local day, index 0 = midnight.
    pub by_hour: Vec<i64>,
    /// Plays per device, most-played first. Every device, not a top-N — a fleet is a handful of
    /// machines and the interesting one is often the least used.
    pub by_device: Vec<(String, i64)>,
    /// The first and last play of the period, as RFC3339, for the cards that open and close on one.
    pub first_play: Option<String>,
    pub last_play: Option<String>,
}

/// Computes an "Agro Replay" recap for a calendar year, or one month of it.
///
/// [`offset_minutes`] is the listener's offset from UTC; see the module comment for why the whole
/// computation happens in it rather than in UTC.
pub fn compute_wrapped(
    all_rows: &[ScrobbleRow],
    year: i32,
    month: Option<i32>,
    top_n: usize,
    offset_minutes: i32,
) -> AgroWrapped {
    // A malformed offset is treated as UTC rather than refused: the recap is still worth showing,
    // and the alternative is a card that fails because a client sent a nonsense number.
    let zone = FixedOffset::east_opt(offset_minutes * 60)
        .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC is a valid fixed offset"));

    let mut prior_artists = HashSet::new();
    let mut period_rows = Vec::new();

    for row in all_rows {
        if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(&row.played_at) {
            let local = parsed.with_timezone(&zone);
            let row_year = local.year();
            let row_month = local.month() as i32;

            let in_period = match month {
                Some(m) => row_year == year && row_month == m,
                None => row_year == year,
            };
            let before_period = match month {
                Some(m) => row_year < year || (row_year == year && row_month < m),
                None => row_year < year,
            };

            if before_period {
                prior_artists.insert(row.artist_name.clone());
            } else if in_period {
                period_rows.push((row, parsed, local));
            }
        }
    }

    let mut artists: HashMap<&str, i64> = HashMap::new();
    let mut albums: HashMap<String, i64> = HashMap::new();
    let mut tracks: HashMap<String, i64> = HashMap::new();
    let mut genres: HashMap<&str, i64> = HashMap::new();
    let mut devices: HashMap<&str, i64> = HashMap::new();
    let mut hour_counts = [0i64; 24];
    let mut hour_counts_utc = [0i64; 24];
    let mut month_counts = [0i64; 12];
    let mut active_days: HashSet<(i32, u32)> = HashSet::new();
    let mut period_artists: HashSet<String> = HashSet::new();
    let mut total_secs = 0i64;

    for (row, utc, local) in &period_rows {
        total_secs += row.duration_secs.max(0);

        *artists.entry(row.artist_name.as_str()).or_default() += 1;
        period_artists.insert(row.artist_name.clone());
        *devices.entry(row.device_name.as_str()).or_default() += 1;

        *albums
            .entry(format!(
                "{} — {}",
                row.album_name.as_deref().unwrap_or("Unknown Album"),
                row.artist_name
            ))
            .or_default() += 1;

        *tracks
            .entry(format!("{} — {}", row.track_title, row.artist_name))
            .or_default() += 1;

        if let Some(genre) = row.genre.as_deref().filter(|g| !g.trim().is_empty()) {
            *genres.entry(genre).or_default() += 1;
        }

        hour_counts[local.hour() as usize] += 1;
        hour_counts_utc[utc.naive_utc().hour() as usize] += 1;
        month_counts[(local.month() - 1) as usize] += 1;
        active_days.insert((local.year(), local.ordinal()));
    }

    let peak = |counts: &[i64; 24]| {
        counts
            .iter()
            .enumerate()
            .max_by_key(|(_, &count)| count)
            .filter(|(_, &count)| count > 0)
            .map(|(hour, _)| hour as i32)
    };

    // Sorted by the instant, not by the local wall clock, so devices in different offsets still
    // order correctly against each other.
    let mut ordered: Vec<&str> = period_rows
        .iter()
        .map(|(row, _, _)| row.played_at.as_str())
        .collect();
    ordered.sort_by_key(|at| crate::stats::parse_time(at));

    let new_artists_count = period_artists
        .iter()
        .filter(|a| !prior_artists.contains(*a))
        .count() as i64;

    AgroWrapped {
        year,
        month,
        total_minutes: total_secs / 60,
        total_plays: period_rows.len() as i64,
        top_artists: rank(artists.into_iter().map(owned), top_n),
        top_tracks: rank(tracks.into_iter(), top_n),
        top_albums: rank(albums.into_iter(), top_n),
        top_genres: rank(genres.into_iter().map(owned), top_n),
        top_hour_utc: peak(&hour_counts_utc),
        top_hour_local: peak(&hour_counts),
        longest_streak_days: longest_streak(&active_days),
        active_days_count: active_days.len() as i64,
        new_artists_count,
        total_artists: period_artists.len() as i64,
        by_month: month_counts.to_vec(),
        by_hour: hour_counts.to_vec(),
        by_device: rank(devices.into_iter().map(owned), usize::MAX),
        first_play: ordered.first().map(|at| at.to_string()),
        last_play: ordered.last().map(|at| at.to_string()),
    }
}

/// The longest run of consecutive days, which is what a streak is.
///
/// This used to be `active_days.len()` — the number of days with *any* play. For someone who
/// listens most days that is near 365 and reads as a year-long streak nobody actually had.
///
/// The `(year, ordinal)` pairs are converted to absolute day numbers first. Comparing the pairs
/// directly would break every run at the turn of the year, since 1 January is ordinal 1 and the
/// 31 December before it is ordinal 365 or 366.
fn longest_streak(active_days: &HashSet<(i32, u32)>) -> i64 {
    let mut days: Vec<i64> = active_days
        .iter()
        .filter_map(|&(year, ordinal)| {
            chrono::NaiveDate::from_yo_opt(year, ordinal).map(|d| d.num_days_from_ce() as i64)
        })
        .collect();
    days.sort_unstable();
    days.dedup();

    let mut best = 0i64;
    let mut run = 0i64;
    let mut previous: Option<i64> = None;
    for day in days {
        run = if previous == Some(day - 1) {
            run + 1
        } else {
            1
        };
        best = best.max(run);
        previous = Some(day);
    }
    best
}

/// Half-open RFC3339 bounds of a calendar year in a fixed offset, `[start, end)`.
///
/// Half-open so this year and the next partition the history rather than sharing midnight.
pub fn year_bounds(year: i32, offset_minutes: i32) -> Option<(String, String)> {
    let zone = FixedOffset::east_opt(offset_minutes * 60)?;
    let start = chrono::NaiveDate::from_ymd_opt(year, 1, 1)?
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(zone)
        .single()?;
    let end = chrono::NaiveDate::from_ymd_opt(year + 1, 1, 1)?
        .and_hms_opt(0, 0, 0)?
        .and_local_timezone(zone)
        .single()?;
    Some((start.to_rfc3339(), end.to_rfc3339()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(artist: &str, title: &str, played_at: &str) -> ScrobbleRow {
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

    fn days(active: &[(i32, u32)]) -> HashSet<(i32, u32)> {
        active.iter().copied().collect()
    }

    /// The defect this replaced: five active days spread over a month reported as a five-day
    /// streak.
    #[test]
    fn a_streak_is_the_longest_run_not_the_count_of_active_days() {
        let active = days(&[(2026, 1), (2026, 2), (2026, 3), (2026, 10), (2026, 20)]);
        assert_eq!(active.len(), 5, "five days were listened on");
        assert_eq!(longest_streak(&active), 3, "but the longest run is three");
    }

    #[test]
    fn a_streak_crosses_the_turn_of_the_year() {
        // 2025 is not a leap year, so 31 December is ordinal 365.
        let active = days(&[(2025, 364), (2025, 365), (2026, 1), (2026, 2)]);
        assert_eq!(longest_streak(&active), 4);
    }

    #[test]
    fn no_plays_is_no_streak() {
        assert_eq!(longest_streak(&days(&[])), 0);
    }

    #[test]
    fn one_day_is_a_streak_of_one() {
        assert_eq!(longest_streak(&days(&[(2026, 200)])), 1);
    }

    /// The reason the whole computation runs in local time: a play just before local midnight on
    /// New Year's Eve belongs to the year that is ending, not the one starting in UTC.
    #[test]
    fn a_play_at_the_turn_of_the_year_lands_in_the_local_year() {
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
    fn a_local_offset_moves_the_peak_hour() {
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
    fn an_empty_year_is_an_empty_recap_not_a_zeroed_one() {
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
    fn counts_the_period_and_names_what_was_new_in_it() {
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
    fn a_month_recap_is_scoped_to_that_month() {
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
    fn year_bounds_are_half_open_in_the_callers_offset() {
        let (start, end) = year_bounds(2026, 120).expect("a valid offset");
        assert!(start.starts_with("2026-01-01T00:00:00+02:00"), "{start}");
        assert!(end.starts_with("2027-01-01T00:00:00+02:00"), "{end}");
    }
}
