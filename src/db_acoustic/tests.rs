use super::*;
use crate::db::Db;
use crate::lock::LockExt;

pub(super) fn vector(artist: &str, title: &str, tempo: f64, energy: f64) -> VectorSubmission {
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
pub(super) fn neighbours_come_back_nearest_first() {
    let db = Db::new_in_memory().unwrap();
    db.submit_vectors(&[
        vector("Radiohead", "All I Need", 0.50, 0.5),
        vector("Portishead", "Roads", 0.52, 0.5),
        vector("Aphex Twin", "Windowlicker", 0.95, 0.9),
    ])
    .unwrap();

    let near = db
        .similar_recordings("Radiohead", "All I Need", 10)
        .unwrap();
    assert_eq!(
        near.len(),
        2,
        "the seed must not be returned as its own neighbour"
    );
    assert_eq!(near[0].title, "Roads");
}

/// The refusal that keeps this from becoming a popularity ranking in disguise.
#[test]
pub(super) fn an_unmeasured_seed_gets_an_empty_answer_not_a_substitute() {
    let db = Db::new_in_memory().unwrap();
    db.submit_vectors(&[
        vector("Portishead", "Roads", 0.5, 0.5),
        vector("Aphex Twin", "Windowlicker", 0.9, 0.9),
    ])
    .unwrap();

    let near = db
        .similar_recordings("Nobody", "Never Measured", 10)
        .unwrap();
    assert!(
        near.is_empty(),
        "an unknown seed must not be answered with the well-measured"
    );
}

/// Two clients measuring their own copies must converge, not overwrite one another.
#[test]
pub(super) fn repeated_submissions_average_rather_than_replace() {
    let db = Db::new_in_memory().unwrap();
    db.submit_vectors(&[vector("Radiohead", "All I Need", 0.4, 0.5)])
        .unwrap();
    db.submit_vectors(&[vector("Radiohead", "All I Need", 0.6, 0.5)])
        .unwrap();

    let conn = db.conn.lock_recover();
    let (tempo, observations): (f64, i64) = conn
        .query_row(
            "SELECT tempo, observations FROM acoustic_vectors",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(
        (tempo - 0.5).abs() < 1e-9,
        "expected the mean of the two, got {tempo}"
    );
    assert_eq!(observations, 2);
}

/// One client must not be able to decide what the fleet believes a song sounds like.
#[test]
pub(super) fn a_settled_average_stops_moving_past_the_observation_cap() {
    let db = Db::new_in_memory().unwrap();
    for _ in 0..(MAX_OBSERVATIONS + 20) {
        db.submit_vectors(&[vector("Radiohead", "All I Need", 0.5, 0.5)])
            .unwrap();
    }
    db.submit_vectors(&[vector("Radiohead", "All I Need", 1.0, 1.0)])
        .unwrap();

    let conn = db.conn.lock_recover();
    let observations: i64 = conn
        .query_row("SELECT observations FROM acoustic_vectors", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(observations, MAX_OBSERVATIONS);
}

/// Two spellings of one recording are one row, as everywhere else keyed on `norm.rs`.
#[test]
pub(super) fn normalisation_folds_two_spellings_into_one_average() {
    let db = Db::new_in_memory().unwrap();
    db.submit_vectors(&[vector("Radiohead", "All I Need", 0.4, 0.5)])
        .unwrap();
    db.submit_vectors(&[vector(
        "radiohead",
        "All I Need (Remastered 2011)",
        0.6,
        0.5,
    )])
    .unwrap();

    let conn = db.conn.lock_recover();
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM acoustic_vectors", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(rows, 1);
}

/// A broken client cannot move the mean somewhere the axis does not go.
#[test]
pub(super) fn out_of_range_values_are_clamped_on_the_way_in() {
    let db = Db::new_in_memory().unwrap();
    db.submit_vectors(&[vector("Broken", "Client", 400.0, -5.0)])
        .unwrap();

    let conn = db.conn.lock_recover();
    let (tempo, energy): (f64, f64) = conn
        .query_row("SELECT tempo, energy FROM acoustic_vectors", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(tempo, 1.0);
    assert_eq!(energy, 0.0);
}
