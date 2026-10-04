//! Scrobble deduplication, timestamp coarsening, and purging.

use crate::db::retention::SCROBBLE_EXACT_TIME_DAYS;
use crate::db::*;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

fn entry(title: &str, at: &str, uid: Option<&str>) -> ScrobbleEntry {
    ScrobbleEntry {
        track_title: title.into(),
        artist_name: "Boards of Canada".into(),
        album_name: None,
        genre: None,
        duration_secs: 200,
        played_at: at.into(),
        play_uid: uid.map(str::to_string),
    }
}

fn times(db: &Db) -> Vec<String> {
    let conn = db.conn.lock().unwrap();
    let mut stmt = conn
        .prepare("SELECT played_at FROM scrobbles ORDER BY id")
        .unwrap();
    let rows = stmt.query_map([], |r| r.get(0)).unwrap();
    rows.map(|r| r.unwrap()).collect()
}

/// The whole reason `play_uid` exists. Four plays of one track inside one hour must stay four
/// rows after their timestamps are rounded into the same bucket — the on-repeat feed needs
/// four, and the old timestamp-keyed index would have left one.
#[test]
fn repeat_plays_in_one_hour_survive_coarsening() {
    let db = db();
    let batch: Vec<_> = ["03:05:11", "03:19:40", "03:31:02", "03:58:59"]
        .iter()
        .enumerate()
        .map(|(i, t)| {
            entry(
                "Roygbiv",
                &format!("2026-08-29T{t}+00:00"),
                Some(&format!("uid-{i}")),
            )
        })
        .collect();

    assert_eq!(
        db.record_scrobbles("alpha", "phone", None, &batch).unwrap(),
        4
    );

    let stored = times(&db);
    assert_eq!(stored.len(), 4, "one row per play");
    assert!(
        stored.iter().all(|t| t == &stored[0]),
        "all four land in the same hour bucket: {stored:?}"
    );
    assert!(stored[0].contains("T03:00:00"), "rounded down: {stored:?}");
}

/// The retry case the unique index was built for, now keyed on the id instead of the clock.
#[test]
fn a_resent_batch_inserts_nothing() {
    let db = db();
    let batch = vec![entry(
        "Dayvan Cowboy",
        "2026-08-29T03:05:11+00:00",
        Some("uid-a"),
    )];

    assert_eq!(
        db.record_scrobbles("alpha", "phone", None, &batch).unwrap(),
        1
    );
    assert_eq!(
        db.record_scrobbles("alpha", "phone", None, &batch).unwrap(),
        0,
        "the same play offered twice is still one play"
    );
    assert_eq!(times(&db).len(), 1);
}

/// Two accounts may both play a track at the same moment; the id is only unique within one.
#[test]
fn the_same_uid_under_two_accounts_is_two_plays() {
    let db = db();
    let batch = vec![entry("Olson", "2026-08-29T03:05:11+00:00", Some("uid-a"))];
    db.record_scrobbles("alpha", "phone", None, &batch).unwrap();
    assert_eq!(
        db.record_scrobbles("delta", "phone", None, &batch).unwrap(),
        1
    );
}

/// A client that has not been updated keeps the exact timestamp, because the timestamp is
/// still the only thing making its retries idempotent.
#[test]
fn a_client_without_a_uid_keeps_its_exact_timestamp() {
    let db = db();
    let batch = vec![entry("Amo Bishop Roden", "2026-08-29T03:05:11+00:00", None)];

    db.record_scrobbles("alpha", "phone", None, &batch).unwrap();
    assert_eq!(times(&db), vec!["2026-08-29T03:05:11+00:00"]);

    // And it is still deduplicated the old way.
    assert_eq!(
        db.record_scrobbles("alpha", "phone", None, &batch).unwrap(),
        0
    );
}

/// Settled history is rounded by the sweep even when it arrived without an id, which is how
/// rows written before any of this get cleaned up.
#[test]
fn the_sweep_coarsens_old_exact_timestamps() {
    let db = db();
    let old = (chrono::Utc::now() - chrono::Duration::days(SCROBBLE_EXACT_TIME_DAYS + 1))
        .with_timezone(&chrono::Utc)
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();
    let recent = (chrono::Utc::now() - chrono::Duration::days(1))
        .format("%Y-%m-%dT%H:%M:%S+00:00")
        .to_string();

    db.record_scrobbles(
        "alpha",
        "phone",
        None,
        &[entry("Old", &old, None), entry("Recent", &recent, None)],
    )
    .unwrap();

    db.sweep_retention();

    let stored = times(&db);
    assert!(
        stored[0].ends_with(":00:00+00:00"),
        "settled history should be rounded: {stored:?}"
    );
    assert_eq!(
        stored[1], recent,
        "recent history may still be retried, so it keeps its seconds"
    );
}

/// Rounding must not invent a time for something it cannot read.
#[test]
fn an_unparseable_timestamp_is_left_alone() {
    let db = db();
    db.record_scrobbles(
        "alpha",
        "phone",
        None,
        &[entry("Broken", "not a date", Some("uid-x"))],
    )
    .unwrap();
    assert_eq!(times(&db), vec!["not a date"]);
}

#[test]
fn purge_scrobbles_by_year_and_all() {
    let db = db();
    let batch = vec![
        ScrobbleEntry {
            track_title: "Track 2024".to_string(),
            artist_name: "Artist".to_string(),
            album_name: None,
            genre: None,
            duration_secs: 180,
            played_at: "2024-06-15T12:00:00+00:00".to_string(),
            play_uid: Some("u1".to_string()),
        },
        ScrobbleEntry {
            track_title: "Track 2025".to_string(),
            artist_name: "Artist".to_string(),
            album_name: None,
            genre: None,
            duration_secs: 200,
            played_at: "2025-03-10T14:00:00+00:00".to_string(),
            play_uid: Some("u2".to_string()),
        },
        ScrobbleEntry {
            track_title: "Track 2025 B".to_string(),
            artist_name: "Artist".to_string(),
            album_name: None,
            genre: None,
            duration_secs: 210,
            played_at: "2025-08-20T16:00:00+00:00".to_string(),
            play_uid: Some("u3".to_string()),
        },
    ];
    db.record_scrobbles("alpha", "phone", None, &batch).unwrap();
    assert_eq!(db.scrobble_rows("alpha", None, None).unwrap().len(), 3);

    // Purge 2024
    let purged_2024 = db.purge_scrobbles("alpha", Some(2024), None).unwrap();
    assert_eq!(purged_2024, 1);
    assert_eq!(db.scrobble_rows("alpha", None, None).unwrap().len(), 2);

    // Purge all remaining
    let purged_all = db.purge_scrobbles("alpha", None, None).unwrap();
    assert_eq!(purged_all, 2);
    assert_eq!(db.scrobble_rows("alpha", None, None).unwrap().len(), 0);
}
