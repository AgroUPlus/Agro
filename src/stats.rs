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
    /// When the earliest play counted here happened, Unix seconds. None with no dated play. What
    /// lets a client turn an all-time total into a per-day average: ALL has no length of its own.
    pub first_played_at: Option<i64>,
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
            stats.first_played_at = Some(stats.first_played_at.map_or(at, |first| first.min(at)));
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

pub(crate) fn owned((name, count): (&str, i64)) -> (String, i64) {
    (name.to_string(), count)
}

pub(crate) fn rank(
    entries: impl Iterator<Item = (String, i64)>,
    top_n: usize,
) -> Vec<(String, i64)> {
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

#[cfg(test)]
mod tests;
