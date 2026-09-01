//! The crowd's measurement of what each recording sounds like.
//!
//! # Shape
//!
//! Six numbers per recording — tempo, energy, brightness, danceability and the key as a point on
//! the circle of fifths — measured by clients from audio they hold, and averaged here across
//! everyone who has measured it. Keyed on the same normalised columns as `library_tracks` and
//! `popularity_counters`, from `norm.rs`, so two spellings of one song are one row.
//!
//! **No account column, and there must never be one.** Same structural guarantee as
//! [`crate::db_popularity`]: this table cannot say who owns a recording, because the fact is not
//! in it. What a person's library *contains* is at least as revealing as what they play, and a
//! contribution table with a submitter column would be exactly that list.
//!
//! # What this deliberately does not do
//!
//! It does not rank by popularity, and it must not learn how. A "similar tracks" service that
//! quietly prefers what is already well known is a recommender that narrows every time it runs —
//! the well-measured stay well-measured, and new music can never enter. Neighbours here are
//! ordered by acoustic distance and by nothing else; `popularity_counters` is a separate table and
//! no query joins the two.
//!
//! The exploration share that keeps unmeasured music in a listener's queue lives in the client,
//! where the queue is actually assembled. This end simply answers what is near what.

use crate::db::Db;
use crate::norm::recording_key;
use rusqlite::{params, Result};

/// The extractor contract these numbers were measured under.
///
/// Vectors from two different definitions of "brightness" are not comparable, so a distance
/// between them is a number with no meaning. Submissions carrying another version are stored
/// separately and never mixed into a neighbour search for this one.
pub const VECTOR_VERSION: i32 = 1;

/// The largest batch one request may carry.
pub const MAX_BATCH: usize = 500;

/// How many independent measurements one recording's average is allowed to be built from.
///
/// The mean stops moving once a recording is this well measured. Two purposes: a late submission
/// cannot drag a settled value, and — since a single client could otherwise submit the same
/// recording a thousand times — no one contributor can steer what the fleet believes a song
/// sounds like.
pub const MAX_OBSERVATIONS: i64 = 32;

/// One recording's measured vector, on its way in from a client.
#[derive(Debug, Clone)]
pub struct VectorSubmission {
    pub title: String,
    pub artist: String,
    pub tempo: f64,
    pub energy: f64,
    pub brightness: f64,
    pub danceability: f64,
    pub key_x: f64,
    pub key_y: f64,
}

/// One recording and how far it sits from the seed.
#[derive(Debug, Clone)]
pub struct NeighbourTrack {
    pub title: String,
    pub artist: String,
    pub distance: f64,
    /// How many measurements the average rests on, so a caller can weigh a lone opinion.
    pub observations: i64,
}

/// The stored average for one recording.
#[derive(Debug, Clone)]
struct StoredVector {
    title: String,
    artist: String,
    tempo: f64,
    energy: f64,
    brightness: f64,
    danceability: f64,
    key_x: f64,
    key_y: f64,
    observations: i64,
}

impl StoredVector {
    /// Euclidean distance with the same axis weights the client uses.
    ///
    /// They have to match. A server that ranked by its own weighting would return a different
    /// "nearest" than the client's own index does for the same two songs, and a queue mixing both
    /// would step unevenly for no reason a listener could see.
    fn distance_to(&self, other: &StoredVector) -> f64 {
        let sq = |a: f64, b: f64| (a - b) * (a - b);
        (TEMPO_WEIGHT * sq(self.tempo, other.tempo)
            + ENERGY_WEIGHT * sq(self.energy, other.energy)
            + BRIGHTNESS_WEIGHT * sq(self.brightness, other.brightness)
            + DANCE_WEIGHT * sq(self.danceability, other.danceability)
            + KEY_WEIGHT * (sq(self.key_x, other.key_x) + sq(self.key_y, other.key_y)))
            .sqrt()
    }
}

/// Mirrors `AcousticFeatures` in the client. Changing one without the other is a bug.
const TEMPO_WEIGHT: f64 = 1.6;
const ENERGY_WEIGHT: f64 = 1.3;
const BRIGHTNESS_WEIGHT: f64 = 0.8;
const DANCE_WEIGHT: f64 = 1.0;
const KEY_WEIGHT: f64 = 0.6;

impl Db {
    /// Folds a batch of client measurements into the running averages.
    ///
    /// A running mean rather than the newest value: clients measure from their own copy of a
    /// recording, and copies differ — a quiet transfer and a loud remaster of one song disagree
    /// about energy. Averaging converges on what the recording is; last-write-wins would make the
    /// fleet's answer depend on whose sync ran most recently.
    ///
    /// Values are clamped to the axis ranges on the way in. A client sending a tempo of 400 is
    /// broken or hostile, and either way the fix is the same: it cannot move the mean anywhere the
    /// axis does not go.
    pub fn submit_vectors(&self, entries: &[VectorSubmission]) -> Result<usize> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut stored = 0usize;

        for entry in entries {
            if entry.title.trim().is_empty() || entry.artist.trim().is_empty() {
                continue;
            }
            let key = recording_key(&entry.artist, &entry.title);
            let clamp = |value: f64, low: f64, high: f64| value.clamp(low, high);

            // The mean is advanced in SQL so a concurrent submission cannot read a stale count and
            // write back an average computed from it.
            tx.execute(
                "INSERT INTO acoustic_vectors (
                     norm_artist, norm_title, norm_variants, version,
                     title, artist, tempo, energy, brightness, danceability, key_x, key_y,
                     observations
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1)
                 ON CONFLICT (norm_artist, norm_title, norm_variants, version) DO UPDATE SET
                     tempo        = (tempo        * observations + excluded.tempo)        / (observations + 1),
                     energy       = (energy       * observations + excluded.energy)       / (observations + 1),
                     brightness   = (brightness   * observations + excluded.brightness)   / (observations + 1),
                     danceability = (danceability * observations + excluded.danceability) / (observations + 1),
                     key_x        = (key_x        * observations + excluded.key_x)        / (observations + 1),
                     key_y        = (key_y        * observations + excluded.key_y)        / (observations + 1),
                     observations = observations + 1
                 WHERE observations < ?13",
                params![
                    key.artist,
                    key.title,
                    key.variants,
                    VECTOR_VERSION,
                    entry.title.trim(),
                    entry.artist.trim(),
                    clamp(entry.tempo, 0.0, 1.0),
                    clamp(entry.energy, 0.0, 1.0),
                    clamp(entry.brightness, 0.0, 1.0),
                    clamp(entry.danceability, 0.0, 1.0),
                    clamp(entry.key_x, -1.0, 1.0),
                    clamp(entry.key_y, -1.0, 1.0),
                    MAX_OBSERVATIONS,
                ],
            )?;
            stored += 1;
        }

        tx.commit()?;
        Ok(stored)
    }

    /// The recordings nearest [`artist`]/[`title`], acoustically.
    ///
    /// A linear scan, held in memory for the duration. At the size a self-hosted server's index
    /// reaches — tens of thousands of recordings, six floats each — this is a few milliseconds and
    /// under a megabyte, and it is exact. An approximate index is what you reach for when the scan
    /// stops being affordable, and it has not; building one first would be a structure to maintain
    /// in exchange for nothing measurable.
    pub fn similar_recordings(
        &self,
        artist: &str,
        title: &str,
        limit: usize,
    ) -> Result<Vec<NeighbourTrack>> {
        let key = recording_key(artist, title);
        let conn = self.conn.lock().unwrap();

        let mut select = conn.prepare(
            "SELECT title, artist, tempo, energy, brightness, danceability, key_x, key_y,
                    observations, norm_artist, norm_title, norm_variants
             FROM acoustic_vectors WHERE version = ?1",
        )?;
        let mut seed: Option<StoredVector> = None;
        let mut others: Vec<StoredVector> = Vec::new();

        let rows = select.query_map(params![VECTOR_VERSION], |row| {
            Ok((
                StoredVector {
                    title: row.get(0)?,
                    artist: row.get(1)?,
                    tempo: row.get(2)?,
                    energy: row.get(3)?,
                    brightness: row.get(4)?,
                    danceability: row.get(5)?,
                    key_x: row.get(6)?,
                    key_y: row.get(7)?,
                    observations: row.get(8)?,
                },
                row.get::<_, String>(9)?,
                row.get::<_, String>(10)?,
                row.get::<_, String>(11)?,
            ))
        })?;

        for row in rows {
            let (vector, norm_artist, norm_title, norm_variants) = row?;
            if norm_artist == key.artist && norm_title == key.title && norm_variants == key.variants
            {
                seed = Some(vector);
            } else {
                others.push(vector);
            }
        }

        // Nothing measured for the seed means no opinion, and an empty answer says exactly that.
        // Returning the most-measured recordings instead would be the popularity ranking this
        // module refuses to become.
        let Some(seed) = seed else {
            return Ok(Vec::new());
        };

        let mut scored: Vec<NeighbourTrack> = others
            .into_iter()
            .map(|candidate| NeighbourTrack {
                distance: seed.distance_to(&candidate),
                title: candidate.title,
                artist: candidate.artist,
                observations: candidate.observations,
            })
            .collect();
        scored.sort_by(|a, b| a.distance.total_cmp(&b.distance));
        scored.truncate(limit);
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector(artist: &str, title: &str, tempo: f64, energy: f64) -> VectorSubmission {
        VectorSubmission {
            title: title.to_string(),
            artist: artist.to_string(),
            tempo,
            energy,
            brightness: 0.5,
            danceability: 0.5,
            key_x: 0.0,
            key_y: 0.0,
        }
    }

    #[test]
    fn neighbours_come_back_nearest_first() {
        let db = Db::new_in_memory().unwrap();
        db.submit_vectors(&[
            vector("Radiohead", "All I Need", 0.50, 0.5),
            vector("Portishead", "Roads", 0.52, 0.5),
            vector("Aphex Twin", "Windowlicker", 0.95, 0.9),
        ])
        .unwrap();

        let near = db.similar_recordings("Radiohead", "All I Need", 10).unwrap();
        assert_eq!(near.len(), 2, "the seed must not be returned as its own neighbour");
        assert_eq!(near[0].title, "Roads");
    }

    /// The refusal that keeps this from becoming a popularity ranking in disguise.
    #[test]
    fn an_unmeasured_seed_gets_an_empty_answer_not_a_substitute() {
        let db = Db::new_in_memory().unwrap();
        db.submit_vectors(&[
            vector("Portishead", "Roads", 0.5, 0.5),
            vector("Aphex Twin", "Windowlicker", 0.9, 0.9),
        ])
        .unwrap();

        let near = db.similar_recordings("Nobody", "Never Measured", 10).unwrap();
        assert!(near.is_empty(), "an unknown seed must not be answered with the well-measured");
    }

    /// Two clients measuring their own copies must converge, not overwrite one another.
    #[test]
    fn repeated_submissions_average_rather_than_replace() {
        let db = Db::new_in_memory().unwrap();
        db.submit_vectors(&[vector("Radiohead", "All I Need", 0.4, 0.5)]).unwrap();
        db.submit_vectors(&[vector("Radiohead", "All I Need", 0.6, 0.5)]).unwrap();

        let conn = db.conn.lock().unwrap();
        let (tempo, observations): (f64, i64) = conn
            .query_row(
                "SELECT tempo, observations FROM acoustic_vectors",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert!((tempo - 0.5).abs() < 1e-9, "expected the mean of the two, got {tempo}");
        assert_eq!(observations, 2);
    }

    /// One client must not be able to decide what the fleet believes a song sounds like.
    #[test]
    fn a_settled_average_stops_moving_past_the_observation_cap() {
        let db = Db::new_in_memory().unwrap();
        for _ in 0..(MAX_OBSERVATIONS + 20) {
            db.submit_vectors(&[vector("Radiohead", "All I Need", 0.5, 0.5)]).unwrap();
        }
        db.submit_vectors(&[vector("Radiohead", "All I Need", 1.0, 1.0)]).unwrap();

        let conn = db.conn.lock().unwrap();
        let observations: i64 = conn
            .query_row("SELECT observations FROM acoustic_vectors", [], |row| row.get(0))
            .unwrap();
        assert_eq!(observations, MAX_OBSERVATIONS);
    }

    /// Two spellings of one recording are one row, as everywhere else keyed on `norm.rs`.
    #[test]
    fn normalisation_folds_two_spellings_into_one_average() {
        let db = Db::new_in_memory().unwrap();
        db.submit_vectors(&[vector("Radiohead", "All I Need", 0.4, 0.5)]).unwrap();
        db.submit_vectors(&[vector("radiohead", "All I Need (Remastered 2011)", 0.6, 0.5)])
            .unwrap();

        let conn = db.conn.lock().unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM acoustic_vectors", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
    }

    /// A broken client cannot move the mean somewhere the axis does not go.
    #[test]
    fn out_of_range_values_are_clamped_on_the_way_in() {
        let db = Db::new_in_memory().unwrap();
        db.submit_vectors(&[vector("Broken", "Client", 400.0, -5.0)]).unwrap();

        let conn = db.conn.lock().unwrap();
        let (tempo, energy): (f64, f64) = conn
            .query_row("SELECT tempo, energy FROM acoustic_vectors", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();
        assert_eq!(tempo, 1.0);
        assert_eq!(energy, 0.0);
    }
}
