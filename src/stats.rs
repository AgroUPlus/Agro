//! Listening statistics, aggregated across every device on an account.
//!
//! Deliberately a port of `history::stats` in the desktop client rather than a fresh design: that
//! computation is what its Home tab has always shown, and a centralised figure that disagreed with
//! the local one it replaces would read as data loss rather than as a wider view. Same buckets,
//! same tiebreaks, same definition of "today".
//!
//! The one thing that is genuinely different is the input. Locally it is one machine's JSONL file;
//! here it is every device's reported plays, which is the whole point.

use crate::db::ScrobbleRow;
use std::collections::{HashMap, HashSet};

const DAY: i64 = 86_400;

/// Days in the daily bars. Matches the client's sparkline so the two can be compared.
const SPARKLINE_DAYS: usize = 14;

/// Days in the heatmap: eight weeks.
const HEATMAP_DAYS: usize = 56;

#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub secs_today: i64,
    pub secs_week: i64,
    pub secs_total: i64,
    pub plays_total: i64,
    /// Consecutive 24-hour buckets ending now that contain at least one play.
    pub streak: i64,
    pub top_artists: Vec<(String, i64)>,
    pub top_albums: Vec<(String, i64)>,
    pub top_tracks: Vec<(String, i64)>,
    pub top_genres: Vec<(String, i64)>,
    /// Seconds per day, oldest first, ending with today.
    pub by_day: Vec<i64>,
    /// Seconds per day over the heatmap window, oldest first.
    pub heatmap: Vec<i64>,
    /// Seconds per hour of the day, index 0 = 00:00 UTC.
    pub by_hour: Vec<i64>,
    /// Seconds per device, most-listened first. Only meaningful across a fleet, so it has no
    /// counterpart in the single-machine version this is ported from.
    pub by_device: Vec<(String, i64)>,
}

pub fn compute(rows: &[ScrobbleRow], top_n: usize, now: i64) -> Stats {
    // Local midnight is not knowable without knowing each device's timezone, so "today" means the
    // last 24 hours — which is the number a listener actually wants anyway.
    let day_ago = now - DAY;
    let week_ago = now - 7 * DAY;

    let mut stats = Stats {
        plays_total: rows.len() as i64,
        by_day: vec![0; SPARKLINE_DAYS],
        heatmap: vec![0; HEATMAP_DAYS],
        by_hour: vec![0; 24],
        ..Default::default()
    };

    let mut artists: HashMap<&str, i64> = HashMap::new();
    let mut albums: HashMap<String, i64> = HashMap::new();
    let mut tracks: HashMap<String, i64> = HashMap::new();
    let mut genres: HashMap<&str, i64> = HashMap::new();
    let mut devices: HashMap<&str, i64> = HashMap::new();
    let mut played_days: HashSet<i64> = HashSet::new();

    for row in rows {
        let secs = row.duration_secs.max(0);
        // A row whose timestamp will not parse is counted in the totals but cannot be placed on any
        // timeline. Dropping it entirely would make the headline number disagree with the bars for
        // no reason the viewer could see.
        let at = parse_time(&row.played_at);

        stats.secs_total += secs;
        if let Some(at) = at {
            if at >= day_ago {
                stats.secs_today += secs;
            }
            if at >= week_ago {
                stats.secs_week += secs;
            }

            let days_ago = (now - at).div_euclid(DAY);
            if days_ago >= 0 {
                played_days.insert(days_ago);
                if (days_ago as usize) < SPARKLINE_DAYS {
                    stats.by_day[SPARKLINE_DAYS - 1 - days_ago as usize] += secs;
                }
                if (days_ago as usize) < HEATMAP_DAYS {
                    stats.heatmap[HEATMAP_DAYS - 1 - days_ago as usize] += secs;
                }
            }
            stats.by_hour[(at.rem_euclid(DAY) / 3600) as usize] += secs;
        }

        *artists.entry(row.artist_name.as_str()).or_default() += 1;
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
        *devices.entry(row.device_name.as_str()).or_default() += secs;
    }

    stats.top_artists = rank(artists.into_iter().map(owned), top_n);
    stats.top_albums = rank(albums.into_iter(), top_n);
    stats.top_tracks = rank(tracks.into_iter(), top_n);
    stats.top_genres = rank(genres.into_iter().map(owned), top_n);
    // Every device, not a top-N: a fleet is a handful of machines and the interesting one is often
    // the least used.
    stats.by_device = rank(devices.into_iter().map(owned), usize::MAX);

    let mut streak = 0;
    while played_days.contains(&streak) {
        streak += 1;
    }
    stats.streak = streak;

    stats
}

fn owned((name, count): (&str, i64)) -> (String, i64) {
    (name.to_string(), count)
}

fn rank(entries: impl Iterator<Item = (String, i64)>, top_n: usize) -> Vec<(String, i64)> {
    let mut entries: Vec<(String, i64)> = entries.collect();
    // Name as a tiebreak so the list does not jitter between equal counts.
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    entries.truncate(top_n);
    entries
}

pub(crate) fn parse_time(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp())
}

/// The earliest timestamp a period includes, as RFC3339, for the SQL to filter on.
///
/// `None` means everything, which is why the query takes an `Option` rather than a very old date:
/// a sentinel would quietly exclude anything stamped before it.
pub fn period_start(period: &str, now: i64) -> Option<String> {
    let days = match period.to_ascii_uppercase().as_str() {
        "DAY" => 1,
        "WEEK" => 7,
        "MONTH" => 30,
        "YEAR" => 365,
        _ => return None,
    };
    chrono::DateTime::from_timestamp(now - days * DAY, 0).map(|dt| dt.to_rfc3339())
}

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
    pub top_hour_utc: Option<i32>,
    pub longest_streak_days: i64,
    pub new_artists_count: i64,
}

/// Computes an "Agro Wrapped" recap for a given year and optional month.
pub fn compute_wrapped(
    all_rows: &[ScrobbleRow],
    year: i32,
    month: Option<i32>,
    top_n: usize,
) -> AgroWrapped {
    use chrono::{Datelike, Timelike};

    let mut prior_artists = HashSet::new();
    let mut period_rows = Vec::new();

    for row in all_rows {
        if let Some(dt) = chrono::DateTime::parse_from_rfc3339(&row.played_at).ok() {
            let row_year = dt.year();
            let row_month = dt.month() as i32;

            let in_period = if let Some(m) = month {
                row_year == year && row_month == m
            } else {
                row_year == year
            };

            let before_period = if let Some(m) = month {
                row_year < year || (row_year == year && row_month < m)
            } else {
                row_year < year
            };

            if before_period {
                prior_artists.insert(row.artist_name.clone());
            } else if in_period {
                period_rows.push((row, dt));
            }
        }
    }

    let mut artists: HashMap<&str, i64> = HashMap::new();
    let mut albums: HashMap<String, i64> = HashMap::new();
    let mut tracks: HashMap<String, i64> = HashMap::new();
    let mut genres: HashMap<&str, i64> = HashMap::new();
    let mut hour_counts = [0i64; 24];
    let mut active_days: HashSet<(i32, u32)> = HashSet::new(); // (year, day_of_year)
    let mut period_artists: HashSet<String> = HashSet::new();
    let mut total_secs = 0i64;

    for (row, dt) in &period_rows {
        let secs = row.duration_secs.max(0);
        total_secs += secs;

        *artists.entry(row.artist_name.as_str()).or_default() += 1;
        period_artists.insert(row.artist_name.clone());

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

        hour_counts[dt.hour() as usize] += 1;
        active_days.insert((dt.year(), dt.ordinal()));
    }

    let top_hour_utc = hour_counts
        .iter()
        .enumerate()
        .max_by_key(|(_, &count)| count)
        .filter(|(_, &count)| count > 0)
        .map(|(hour, _)| hour as i32);

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
        top_hour_utc,
        longest_streak_days: active_days.len() as i64,
        new_artists_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(artist: &str, title: &str, secs: i64, at: i64, device: &str) -> ScrobbleRow {
        ScrobbleRow {
            track_title: title.to_string(),
            artist_name: artist.to_string(),
            album_name: Some("An Album".to_string()),
            genre: Some("Rock".to_string()),
            duration_secs: secs,
            device_name: device.to_string(),
            played_at: chrono::DateTime::from_timestamp(at, 0).unwrap().to_rfc3339(),
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
    fn rounding_plays_to_the_hour_changes_no_statistic() {
        // Midnight UTC, so "days ago" arithmetic lines up with the day boundary and the test is
        // about the hour rounding rather than about where a day starts.
        let now = 30 * DAY;
        let offsets = [
            61,            // a minute into the current hour
            3599,          // a second before the hour rolls
            3601,          // a second after it
            2 * DAY + 59,  // two days back, just past the hour
            2 * DAY + 3599,
            7 * DAY + 1800, // mid-hour, a week back
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
                row("A", &format!("t{i}"), 240, at - at.rem_euclid(3600), "phone")
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
    fn rounding_can_move_a_play_one_bucket_at_an_edge() {
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
    fn totals_split_by_window() {
        let now = 10 * DAY;
        let rows = [
            row("A", "one", 300, now - 60, "phone"),
            row("A", "two", 300, now - 3 * DAY, "laptop"),
            row("B", "three", 300, now - 9 * DAY, "laptop"),
        ];
        let stats = compute(&rows, 5, now);

        assert_eq!(stats.secs_total, 900);
        assert_eq!(stats.secs_today, 300, "only the play from a minute ago");
        assert_eq!(stats.secs_week, 600, "the nine-day-old play is outside the week");
        assert_eq!(stats.plays_total, 3);
    }

    #[test]
    fn ranks_by_count_then_name() {
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
    fn streak_counts_consecutive_days_and_stops_at_a_gap() {
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
    fn seconds_are_attributed_per_device() {
        let now = 10 * DAY;
        let rows = [
            row("A", "one", 300, now - 60, "phone"),
            row("A", "two", 100, now - 60, "laptop"),
        ];
        let stats = compute(&rows, 5, now);
        assert_eq!(stats.by_device, vec![
            ("phone".to_string(), 300),
            ("laptop".to_string(), 100)
        ]);
    }

    #[test]
    fn an_unparseable_timestamp_still_counts_toward_the_total() {
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
    fn period_start_is_none_for_all_time() {
        assert!(period_start("ALL", 0).is_none());
        assert!(period_start("WEEK", 10 * DAY).is_some());
    }

    #[test]
    fn test_agro_wrapped_computation() {
        let rows = vec![
            ScrobbleRow {
                track_title: "Around the World".to_string(),
                artist_name: "Daft Punk".to_string(),
                album_name: Some("Homework".to_string()),
                genre: Some("Electronic".to_string()),
                duration_secs: 420,
                device_name: "phone".to_string(),
                played_at: "2025-06-15T14:30:00Z".to_string(),
            },
            ScrobbleRow {
                track_title: "One More Time".to_string(),
                artist_name: "Daft Punk".to_string(),
                album_name: Some("Discovery".to_string()),
                genre: Some("Electronic".to_string()),
                duration_secs: 320,
                device_name: "phone".to_string(),
                played_at: "2026-02-10T10:00:00Z".to_string(),
            },
            ScrobbleRow {
                track_title: "Windowlicker".to_string(),
                artist_name: "Aphex Twin".to_string(),
                album_name: Some("Windowlicker".to_string()),
                genre: Some("IDM".to_string()),
                duration_secs: 360,
                device_name: "desktop".to_string(),
                played_at: "2026-08-20T18:00:00Z".to_string(),
            },
        ];

        let wrapped = compute_wrapped(&rows, 2026, None, 10);
        assert_eq!(wrapped.year, 2026);
        assert_eq!(wrapped.total_plays, 2);
        assert_eq!(wrapped.total_minutes, (320 + 360) / 60);
        assert_eq!(wrapped.top_artists.len(), 2);
        // Daft Punk was played in 2025 (prior artist), Aphex Twin was new in 2026
        assert_eq!(wrapped.new_artists_count, 1);
    }
}
