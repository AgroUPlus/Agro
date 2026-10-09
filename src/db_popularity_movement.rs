//! Where each recording stood in the window before this one, for the chart's rank arrows.
//!
//! The comparison reads the same anonymous day buckets as [`Db::popular_tracks`] and applies the
//! same [`MIN_EXPOSURE_COUNT`] floor to the earlier window, so a recording nobody has played much
//! is never named here either. Nothing about who played anything exists to be revealed.
//!
//! Retention is not stretched to make this work. When the earlier window would reach past
//! [`RETENTION_DAYS`], or holds no recording above the floor (a young server, or a quiet week),
//! there is nothing honest to compare against and the answer is `None` — a chart full of "new"
//! because the history is missing would be a lie about the music.

use crate::db::Db;
use crate::db_popularity::{MIN_EXPOSURE_COUNT, RETENTION_DAYS};
use rusqlite::{params, Result};
use std::collections::HashMap;

/// A recording's identity in `popularity_counters`: normalised artist, title and variants.
pub type RecordingId = (String, String, String);

impl Db {
    /// 1-based rank of every qualifying recording in the `days` buckets that end just before the
    /// current window begins, or `None` when no comparison is possible.
    pub fn previous_popular_ranks(
        &self,
        today: i64,
        days: i64,
    ) -> Result<Option<HashMap<RecordingId, usize>>> {
        let days = days.max(1);
        let newest = today - days;
        let oldest = newest - days + 1;
        if oldest < today - RETENTION_DAYS {
            return Ok(None);
        }
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT norm_artist, norm_title, norm_variants, SUM(count) AS total
             FROM popularity_counters
             WHERE bucket_day BETWEEN ?1 AND ?2
             GROUP BY norm_artist, norm_title, norm_variants
             HAVING total >= ?3
             ORDER BY total DESC, MIN(title) ASC",
        )?;
        let ranks: HashMap<RecordingId, usize> = stmt
            .query_map(params![oldest, newest, MIN_EXPOSURE_COUNT], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<RecordingId>>>()?
            .into_iter()
            .enumerate()
            .map(|(index, id)| (id, index + 1))
            .collect();
        Ok(if ranks.is_empty() { None } else { Some(ranks) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_popularity::CountIncrement;
    use crate::norm::recording_key;

    fn play(db: &Db, day: i64, artist: &str, title: &str, count: i64) {
        db.add_play_counts(
            day,
            &[CountIncrement {
                title: title.to_string(),
                artist: artist.to_string(),
                album: None,
                count,
            }],
        )
        .unwrap();
    }

    fn id(artist: &str, title: &str) -> RecordingId {
        let key = recording_key(artist, title);
        (key.artist, key.title, key.variants)
    }

    #[test]
    fn ranks_come_from_the_window_before_the_current_one() {
        let db = Db::new_in_memory().unwrap();
        // Current 7-day window is days 94..=100; the previous one is 87..=93.
        play(&db, 90, "Radiohead", "All I Need", 9);
        play(&db, 91, "Radiohead", "Weird Fishes", 6);
        play(&db, 100, "Radiohead", "Reckoner", 50);

        let ranks = db.previous_popular_ranks(100, 7).unwrap().unwrap();
        assert_eq!(ranks[&id("Radiohead", "All I Need")], 1);
        assert_eq!(ranks[&id("Radiohead", "Weird Fishes")], 2);
        assert!(!ranks.contains_key(&id("Radiohead", "Reckoner")));
    }

    #[test]
    fn the_exposure_floor_applies_to_the_earlier_window_too() {
        let db = Db::new_in_memory().unwrap();
        play(&db, 90, "Radiohead", "All I Need", MIN_EXPOSURE_COUNT);
        play(&db, 90, "Radiohead", "Weird Fishes", MIN_EXPOSURE_COUNT - 1);

        let ranks = db.previous_popular_ranks(100, 7).unwrap().unwrap();
        assert_eq!(ranks.len(), 1, "a barely-played track must not be named");
    }

    #[test]
    fn a_window_that_outruns_retention_has_no_comparison() {
        let db = Db::new_in_memory().unwrap();
        play(&db, 70, "Radiohead", "All I Need", 20);
        assert!(db.previous_popular_ranks(100, 30).unwrap().is_none());
    }

    #[test]
    fn an_empty_earlier_window_is_no_comparison_not_all_new() {
        let db = Db::new_in_memory().unwrap();
        play(&db, 100, "Radiohead", "All I Need", 20);
        assert!(db.previous_popular_ranks(100, 7).unwrap().is_none());
    }
}
