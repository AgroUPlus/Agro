//! A jam leaves a recap behind for the people who were in it, and for nobody else.

use crate::db::*;
use crate::db_jam::{Jam, JamMode};
use rusqlite::params;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

fn add(db: &Db, jam: &Jam, by: &str, title: &str) -> String {
    db.add_jam_track(
        &jam.id,
        by,
        &format!("uri:{title}"),
        title,
        "Artist",
        None,
        200_000,
        false,
        JamMode::Open,
        None,
        None,
    )
    .unwrap()
    .0
}

/// Alex hosts, Sam joins; one track has played and the second is on air.
fn a_jam_in_progress(db: &Db) -> Jam {
    let jam = db.create_jam("alex", JamMode::Open).unwrap();
    db.join_jam(&jam.id, "sam").unwrap();
    let first = add(db, &jam, "alex", "first");
    let second = add(db, &jam, "sam", "second");
    db.mark_jam_track_played(&jam.id, &first).unwrap();
    db.set_jam_now_playing(&jam.id, &second).unwrap();
    db.jam_by_id(&jam.id).unwrap().unwrap()
}

#[test]
fn a_member_leaving_keeps_what_they_heard_including_the_track_on_air() {
    let db = db();
    let jam = a_jam_in_progress(&db);

    let id = db.record_jam_recap(&jam, "sam").unwrap().expect("a recap");
    let recaps = db.jam_recaps("sam").unwrap();
    assert_eq!(recaps.len(), 1);
    assert_eq!(recaps[0].id, id);
    let titles: Vec<_> = recaps[0]
        .recap
        .tracks
        .iter()
        .map(|t| t.title.as_str())
        .collect();
    assert_eq!(titles, ["first", "second"]);
    assert_eq!(recaps[0].recap.people, ["alex", "sam"]);
}

#[test]
fn a_recap_is_only_ever_its_owners() {
    let db = db();
    let jam = a_jam_in_progress(&db);
    let id = db.record_jam_recap(&jam, "sam").unwrap().unwrap();

    assert!(db.jam_recaps("alex").unwrap().is_empty());
    assert!(
        !db.dismiss_jam_recap("alex", &id).unwrap(),
        "someone else dismissed it"
    );
    assert_eq!(db.jam_recaps("sam").unwrap().len(), 1);

    assert!(db.dismiss_jam_recap("Sam", &id).unwrap());
    assert!(db.jam_recaps("sam").unwrap().is_empty());
}

#[test]
fn a_jam_alone_or_with_nothing_played_stores_nothing() {
    let db = db();
    let solo = db.create_jam("alex", JamMode::Open).unwrap();
    let track = add(&db, &solo, "alex", "mine");
    db.mark_jam_track_played(&solo.id, &track).unwrap();
    assert!(db.record_jam_recap(&solo, "alex").unwrap().is_none());

    let quiet = db.create_jam("sam", JamMode::Open).unwrap();
    db.join_jam(&quiet.id, "kim").unwrap();
    assert!(db.record_jam_recap(&quiet, "kim").unwrap().is_none());

    let rows: i64 = db
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM jam_recaps", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn the_sweep_removes_recaps_nobody_dismissed() {
    let db = db();
    let jam = a_jam_in_progress(&db);
    let old = db.record_jam_recap(&jam, "sam").unwrap().unwrap();
    let fresh = db.record_jam_recap(&jam, "alex").unwrap().unwrap();
    let long_ago = (chrono::Utc::now() - chrono::Duration::days(31)).to_rfc3339();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE jam_recaps SET created_at = ?1 WHERE id = ?2",
            params![long_ago, old],
        )
        .unwrap();

    db.sweep_retention();

    assert!(db.jam_recaps("sam").unwrap().is_empty());
    assert_eq!(db.jam_recaps("alex").unwrap()[0].id, fresh);
}
