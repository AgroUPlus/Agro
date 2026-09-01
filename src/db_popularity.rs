//! What the fleet is playing, counted without recording who played it.
//!
//! # The privacy shape, and its limits
//!
//! `popularity_counters` has **no account column**, and adding one would defeat the entire point.
//! The guarantee is structural rather than procedural: the table cannot answer "what does this
//! person listen to" because the fact is not in it, not because a query politely declines to ask.
//! A stolen database, a subpoena and a bug in a resolver all get the same nothing.
//!
//! Two supporting properties:
//!
//! - **Day buckets and no clock.** `bucket_day` is a whole day since the epoch, UTC, and is the
//!   only time recorded. A play at 03:12 and one at 22:47 are the same row, so the table cannot
//!   reconstruct a routine, a sleep schedule or a working pattern.
//! - **Old buckets are deleted, not archived.** [`RETENTION_DAYS`] of history exists because the
//!   shelf asks for a rolling window; nothing keeps a year of it, so there is no long-term profile
//!   of a household to leak.
//!
//! **What this is not.** It is not zero-knowledge in the cryptographic sense, and calling it that
//! would be a lie a user might rely on. The server sees an authenticated request arrive, so it
//! knows *that* an account submitted counts, and on a single-user server "the fleet played this"
//! and "that person played this" are the same sentence. [`MIN_EXPOSURE_COUNT`] is the mitigation
//! that actually helps on a small server: a recording nobody has played much is never named back
//! to anyone, so the shelf cannot become a way of reading a housemate's week. Real blinding would
//! need an oblivious aggregator, and that is a different system, not a flag on this one.
//!
//! Counted per *recording* via `norm.rs`, so two encodings of one song are one row and
//! `reindex_normalisation` keeps this table's convention identical to the library index's.

use crate::db::Db;
use crate::norm::recording_key;
use rusqlite::{params, Result};

/// How many days of buckets are kept. Past this they are deleted on the next write.
///
/// Comfortably wider than the seven-day window the shelf asks for, so a client that has been
/// offline for a fortnight still lands its counts inside a bucket that will be read, and narrow
/// enough that the table never becomes a season of a household's listening.
pub const RETENTION_DAYS: i64 = 30;

/// How many plays of one recording must exist in a window before it can be named in a result.
///
/// The one protection that means anything on a household server. Without it, a shelf built from
/// three people's listening tells each of them what the other two played this week, one play at a
/// time. With it, the shelf can only ever say what several plays agree on — which is what a
/// "popular" shelf was asking for anyway, so the privacy floor and the feature want the same
/// number.
pub const MIN_EXPOSURE_COUNT: i64 = 5;

/// The largest increment one entry may carry in a single request.
///
/// A client reports what it played since it last synced, so a genuine number is small. The clamp
/// is against a client — buggy or hostile — deciding what the whole server's shelf says by
/// submitting a million of something.
pub const MAX_INCREMENT: i64 = 50;

/// One recording's plays, on their way in from a client.
#[derive(Debug, Clone)]
pub struct CountIncrement {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Plays since the client last reported. Clamped to [`MAX_INCREMENT`].
    pub count: i64,
}

/// One recording the fleet has been playing, on its way out to a shelf.
#[derive(Debug, Clone)]
pub struct PopularTrack {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    /// Plays across the whole window, from everyone, unattributed.
    pub count: i64,
}

impl Db {
    /// Adds a batch of plays to today's bucket.
    ///
    /// Takes no account id, by design — the caller is authenticated before it gets here, and that
    /// is the last point at which anyone knows who is speaking. Nothing below this line does.
    ///
    /// Display metadata is written on insert and left alone afterwards. The first client to report
    /// a recording names it; a later one with different capitalisation does not get to rewrite what
    /// everyone else sees, which would otherwise make the shelf flicker between spellings.
    pub fn add_play_counts(&self, today: i64, entries: &[CountIncrement]) -> Result<usize> {
        if entries.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;

        let mut counted = 0;
        for entry in entries {
            let increment = entry.count.clamp(0, MAX_INCREMENT);
            if increment == 0 || entry.title.trim().is_empty() {
                continue;
            }
            let key = recording_key(&entry.artist, &entry.title);
            tx.execute(
                "INSERT INTO popularity_counters
                     (bucket_day, norm_artist, norm_title, norm_variants, title, artist, album, count)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(bucket_day, norm_artist, norm_title, norm_variants) DO UPDATE SET
                     count = count + excluded.count",
                params![
                    today,
                    key.artist,
                    key.title,
                    key.variants,
                    entry.title,
                    entry.artist,
                    entry.album,
                    increment
                ],
            )?;
            counted += 1;
        }

        // Pruned on write rather than on a timer: there is no scheduler in this process, and a
        // server nobody is listening on does not need its old buckets deleted on schedule — it has
        // stopped gaining new ones.
        tx.execute(
            "DELETE FROM popularity_counters WHERE bucket_day < ?1",
            params![today - RETENTION_DAYS],
        )?;
        tx.commit()?;
        Ok(counted)
    }

    /// The most-played recordings over the `days` buckets ending today, most played first.
    ///
    /// Anything below [`MIN_EXPOSURE_COUNT`] is not returned at all — see the module docs. The
    /// threshold is applied to the window's total rather than to any one day, so a recording played
    /// steadily all week qualifies while one played twice yesterday does not.
    pub fn popular_tracks(&self, today: i64, days: i64, limit: usize) -> Result<Vec<PopularTrack>> {
        let since = today - days.max(1) + 1;
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT MIN(title), MIN(artist), MIN(album), SUM(count) AS total
             FROM popularity_counters
             WHERE bucket_day >= ?1
             GROUP BY norm_artist, norm_title, norm_variants
             HAVING total >= ?2
             ORDER BY total DESC, MIN(title) ASC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![since, MIN_EXPOSURE_COUNT, limit as i64], |row| {
            Ok(PopularTrack {
                title: row.get(0)?,
                artist: row.get(1)?,
                album: row.get(2)?,
                count: row.get(3)?,
            })
        })?;
        rows.collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn increment(artist: &str, title: &str, count: i64) -> CountIncrement {
        CountIncrement {
            title: title.to_string(),
            artist: artist.to_string(),
            album: Some("Album".to_string()),
            count,
        }
    }

    /// The shelf's whole job, and the floor that guards it.
    #[test]
    fn only_recordings_above_the_exposure_floor_are_named() {
        let db = Db::new_in_memory().unwrap();
        db.add_play_counts(100, &[increment("Radiohead", "All I Need", MIN_EXPOSURE_COUNT)])
            .unwrap();
        db.add_play_counts(100, &[increment("Radiohead", "Weird Fishes", 1)])
            .unwrap();

        let popular = db.popular_tracks(100, 7, 10).unwrap();
        assert_eq!(popular.len(), 1, "the once-played track must not be named");
        assert_eq!(popular[0].title, "All I Need");
    }

    /// Two spellings of one recording are one row, or the shelf splits a song against itself.
    #[test]
    fn differently_tagged_reports_of_one_recording_are_counted_together() {
        let db = Db::new_in_memory().unwrap();
        db.add_play_counts(100, &[increment("Radiohead", "All I Need", 3)])
            .unwrap();
        db.add_play_counts(
            100,
            &[increment("radiohead", "All I Need (Official Video) [HQ]", 3)],
        )
        .unwrap();

        let popular = db.popular_tracks(100, 7, 10).unwrap();
        assert_eq!(popular.len(), 1);
        assert_eq!(popular[0].count, 6);
    }

    /// A live take is a different performance and must not be folded into the studio cut.
    #[test]
    fn a_variant_is_counted_separately() {
        let db = Db::new_in_memory().unwrap();
        db.add_play_counts(100, &[increment("Radiohead", "All I Need", 6)])
            .unwrap();
        db.add_play_counts(100, &[increment("Radiohead", "All I Need (Live)", 6)])
            .unwrap();

        let popular = db.popular_tracks(100, 7, 10).unwrap();
        assert_eq!(popular.len(), 2);
    }

    /// The window rolls: what mattered a fortnight ago is not what is popular now.
    #[test]
    fn buckets_outside_the_window_are_not_counted() {
        let db = Db::new_in_memory().unwrap();
        db.add_play_counts(90, &[increment("Radiohead", "All I Need", 20)])
            .unwrap();

        assert!(db.popular_tracks(100, 7, 10).unwrap().is_empty());
        assert_eq!(db.popular_tracks(100, 30, 10).unwrap().len(), 1);
    }

    /// One client cannot decide what the whole server's shelf says.
    #[test]
    fn an_absurd_increment_is_clamped() {
        let db = Db::new_in_memory().unwrap();
        db.add_play_counts(100, &[increment("Radiohead", "All I Need", 1_000_000)])
            .unwrap();

        assert_eq!(db.popular_tracks(100, 7, 10).unwrap()[0].count, MAX_INCREMENT);
    }

    /// Retention is enforced by the write path, so the table cannot quietly grow a history.
    #[test]
    fn writing_prunes_buckets_past_retention() {
        let db = Db::new_in_memory().unwrap();
        db.add_play_counts(10, &[increment("Radiohead", "All I Need", 10)])
            .unwrap();
        db.add_play_counts(10 + RETENTION_DAYS + 1, &[increment("Radiohead", "Weird Fishes", 10)])
            .unwrap();

        let conn = db.conn.lock().unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM popularity_counters", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "the old bucket must be gone, not merely unread");
    }
}
