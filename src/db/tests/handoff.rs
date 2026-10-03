//! Handoff rows are per device and per account.

use crate::db::*;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

fn report_with_duration(
    db: &Db,
    user: &str,
    device: &str,
    title: &str,
    playing: bool,
    duration_ms: i64,
) {
    db.update_handoff(
        user,
        &format!("uri:{title}"),
        title,
        "Artist",
        None,
        None,
        0,
        duration_ms,
        playing,
        device,
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
}

fn report(db: &Db, user: &str, device: &str, title: &str, playing: bool) {
    report_with_duration(db, user, device, title, playing, 0);
}

/// The bug this covers: one row per account meant the desktop pausing overwrote the phone's
/// playing session, and the desktop filters out its own device — so the fleet's track
/// disappeared at the moment it became the one worth showing.
#[test]
fn one_device_pausing_does_not_erase_another_ones_session() {
    let db = db();
    report(&db, "alpha", "phone", "Phone Song", true);
    report(&db, "alpha", "desktop", "Desktop Song", false);

    let elsewhere = db
        .get_handoff_excluding("alpha", "desktop")
        .unwrap()
        .unwrap();
    assert_eq!(elsewhere.track_title, "Phone Song");
    assert!(elsewhere.is_playing);
    assert_eq!(elsewhere.device_id, "phone");
}

/// The original question, unchanged: whatever happened last, whoever it was.
#[test]
fn the_accounts_handoff_is_still_the_most_recent_report() {
    let db = db();
    report(&db, "alpha", "phone", "Phone Song", true);
    report(&db, "alpha", "desktop", "Desktop Song", true);

    assert_eq!(
        db.get_handoff("alpha").unwrap().unwrap().track_title,
        "Desktop Song"
    );
}

#[test]
fn a_device_updates_its_own_row_rather_than_adding_one() {
    let db = db();
    report(&db, "alpha", "phone", "First", true);
    report(&db, "alpha", "phone", "Second", true);

    assert_eq!(
        db.get_handoff("alpha").unwrap().unwrap().track_title,
        "Second"
    );
    assert!(db
        .get_handoff_excluding("alpha", "phone")
        .unwrap()
        .is_none());
}

/// A position with nothing to measure it against can only ever be an elapsed count. The
/// length travels so whatever renders the session can draw a bar with two ends.
#[test]
fn the_track_length_travels_with_the_handoff() {
    let db = db();
    report_with_duration(&db, "alpha", "phone", "Song", true, 214_000);

    assert_eq!(
        db.get_handoff("alpha").unwrap().unwrap().duration_ms,
        214_000
    );
}

/// Zero is "did not say", which is also what a livestream reports. Neither may overwrite a
/// length another heartbeat already established.
#[test]
fn a_heartbeat_without_a_length_keeps_the_one_already_stored() {
    let db = db();
    report_with_duration(&db, "alpha", "phone", "Song", true, 214_000);
    report_with_duration(&db, "alpha", "phone", "Song", true, 0);

    assert_eq!(
        db.get_handoff("alpha").unwrap().unwrap().duration_ms,
        214_000
    );
}

#[test]
fn one_account_never_sees_anothers_handoff() {
    let db = db();
    report(&db, "alpha", "phone", "Phone Song", true);
    report(&db, "delta", "phone", "Someone Elses Song", true);

    assert_eq!(
        db.get_handoff("alpha").unwrap().unwrap().track_title,
        "Phone Song"
    );
    assert!(db
        .get_handoff_excluding("alpha", "phone")
        .unwrap()
        .is_none());
}
