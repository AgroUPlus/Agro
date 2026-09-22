//! Where one account's year sits among the server's other accounts.
//!
//! The only cross-account read in Agro that is not mediated by a friendship, so it is built to give
//! up exactly one number and nothing else. The SQL groups by `user_id` because it has to — a
//! percentile is a position among accounts — but the usernames never leave [`Db::year_cohort`]:
//! what comes back is a count and a rank, from which nobody's listening can be reconstructed.
//!
//! On a server with a handful of accounts even a rank is a disclosure: "you are top of two" names
//! what the other person did. So the cohort has a floor, below which there is no answer at all.
//! See [`MIN_COHORT`].

use crate::db::Db;
use rusqlite::{params, Result};

/// How many accounts must have listened in the year before a standing is computed.
///
/// The same reasoning as [`crate::db_popularity::MIN_EXPOSURE_COUNT`], and deliberately the same
/// number: below it, a position in the ranking is a statement about identifiable people rather than
/// about a crowd. A household server simply has no charts, which is the honest answer.
pub const MIN_COHORT: usize = 5;

/// One account's position among everyone who listened in the same window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChartStanding {
    /// 0–100. The share of the cohort that listened *less* than the caller.
    pub percentile: i64,
    /// How many accounts were in the ranking.
    pub cohort_size: i64,
    /// The caller's own minutes, so the card can say what the percentile is of.
    pub minutes: i64,
    /// True when the cohort was too small to rank without naming people. Everything else is zero,
    /// and the client draws nothing rather than "top 100%".
    pub suppressed: bool,
}

impl Db {
    /// Seconds listened per account in `[since, until)`, for [`standing_in`].
    ///
    /// Accounts with no plays in the window are absent rather than present with a zero: somebody
    /// who did not listen is not in this year's ranking, and counting them would inflate everybody
    /// else's percentile with people who never took part.
    fn year_cohort(&self, since: &str, until: &str) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT user_id, SUM(MAX(duration_secs, 0)) AS secs
             FROM scrobbles
             WHERE played_at >= ?1 AND played_at < ?2
             GROUP BY user_id",
        )?;
        let rows = stmt.query_map(params![since, until], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        rows.collect()
    }

    /// Where `user_id` stands among everyone who listened in `[since, until)`.
    pub fn standing_in(&self, user_id: &str, since: &str, until: &str) -> Result<ChartStanding> {
        let cohort = self.year_cohort(since, until)?;

        let mine = cohort
            .iter()
            .find(|(name, _)| name == user_id)
            .map(|(_, secs)| *secs);

        // Not having listened at all is not a suppression — it is an empty recap, and the caller
        // already knows it. Reported as an absent standing so the card is dropped either way.
        let Some(mine) = mine else {
            return Ok(ChartStanding {
                suppressed: true,
                ..Default::default()
            });
        };

        if cohort.len() < MIN_COHORT {
            return Ok(ChartStanding {
                suppressed: true,
                ..Default::default()
            });
        }

        // Strictly below, so a tie does not claim to beat the person it tied with.
        let below = cohort.iter().filter(|(_, secs)| *secs < mine).count();
        Ok(ChartStanding {
            percentile: ((below * 100) / cohort.len()) as i64,
            cohort_size: cohort.len() as i64,
            minutes: mine / 60,
            suppressed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One play, which is enough: the cohort ranks on summed seconds, not on how many rows carried
    /// them, so a single row of `secs` and ten rows totalling `secs` are the same account here.
    fn record(db: &Db, user: &str, secs: i64, at: &str) {
        db.record_scrobbles(
            user,
            "phone",
            Some("test"),
            &[crate::db::ScrobbleEntry {
                track_title: format!("A Song For {user}"),
                artist_name: "An Artist".to_string(),
                album_name: None,
                genre: None,
                duration_secs: secs,
                played_at: at.to_string(),
                // Without a uid the server blurs the timestamp to the hour, which these windows do
                // not care about — but a uid keeps the stored time exactly as written, which the
                // out-of-window test does care about.
                play_uid: Some(format!("{user}-{at}-{secs}")),
            }],
        )
        .expect("the play to be recorded");
    }

    fn seeded(plays: &[(&str, i64)]) -> Db {
        let db = Db::new_in_memory().expect("an in-memory database");
        for (user, secs) in plays {
            record(&db, user, *secs, "2026-06-15T12:00:00Z");
        }
        db
    }

    const YEAR: (&str, &str) = ("2026-01-01T00:00:00Z", "2027-01-01T00:00:00Z");

    #[test]
    fn a_small_server_suppresses_the_standing() {
        let db = seeded(&[("ana", 600), ("bo", 300), ("cy", 100)]);
        let standing = db.standing_in("ana", YEAR.0, YEAR.1).expect("a standing");

        assert!(
            standing.suppressed,
            "three accounts cannot be ranked safely"
        );
        assert_eq!(standing.cohort_size, 0, "and the size is not leaked either");
        assert_eq!(standing.percentile, 0);
    }

    #[test]
    fn the_percentile_counts_accounts_below_not_plays() {
        let db = seeded(&[
            ("ana", 1000),
            ("bo", 900),
            ("cy", 800),
            ("di", 700),
            ("ed", 100),
        ]);

        let top = db.standing_in("ana", YEAR.0, YEAR.1).expect("a standing");
        assert!(!top.suppressed);
        assert_eq!(top.cohort_size, 5);
        assert_eq!(top.percentile, 80, "four of five listened less");
        assert_eq!(top.minutes, 1000 / 60);

        let bottom = db.standing_in("ed", YEAR.0, YEAR.1).expect("a standing");
        assert_eq!(bottom.percentile, 0, "nobody listened less than the least");
    }

    #[test]
    fn a_tie_does_not_claim_to_beat_the_account_it_tied_with() {
        let db = seeded(&[
            ("ana", 500),
            ("bo", 500),
            ("cy", 500),
            ("di", 500),
            ("ed", 500),
        ]);
        let standing = db.standing_in("ana", YEAR.0, YEAR.1).expect("a standing");
        assert_eq!(standing.percentile, 0, "everybody listened the same");
    }

    #[test]
    fn an_account_with_no_plays_that_year_is_not_in_the_cohort() {
        let db = seeded(&[
            ("ana", 1000),
            ("bo", 900),
            ("cy", 800),
            ("di", 700),
            ("ed", 100),
        ]);

        let absent = db.standing_in("zoe", YEAR.0, YEAR.1).expect("a standing");
        assert!(
            absent.suppressed,
            "somebody who did not listen has no standing"
        );

        // And the five who did listen still rank against each other, not against six.
        let top = db.standing_in("ana", YEAR.0, YEAR.1).expect("a standing");
        assert_eq!(top.cohort_size, 5);
    }

    #[test]
    fn a_play_outside_the_window_is_not_counted() {
        let db = Db::new_in_memory().expect("an in-memory database");
        for user in ["ana", "bo", "cy", "di", "ed"] {
            record(&db, user, 600, "2026-06-15T12:00:00Z");
        }
        record(&db, "bo", 100_000, "2025-06-15T12:00:00Z");

        let standing = db.standing_in("ana", YEAR.0, YEAR.1).expect("a standing");
        assert_eq!(
            standing.percentile, 0,
            "last year's listening does not move this year's ranking"
        );
    }
}
