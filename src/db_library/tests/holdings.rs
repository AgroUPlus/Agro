//! Holdings and what is missing on a device.

use super::*;

/// Reproduces the "download 136 tracks nobody has" report.
///
/// A device that is unregistered — retired, re-paired, or given a new device id — leaves its
/// `device_holdings` rows behind, because `delete_node` only touches `registered_nodes`.
/// `missing_on_device` then joins against those orphans and offers every one of their tracks to
/// every remaining device, sourced from a machine that no longer exists.
#[test]
fn holdings_of_an_unregistered_device_are_still_offered() {
    let db = db_with(&[
        (track("h1", "BoC", "Roygbiv", 200_000), "old-laptop"),
        (track("h2", "BoC", "Olson", 100_000), "old-laptop"),
    ]);
    db.upsert_node(
        "old-laptop",
        "alpha",
        crate::db::NodeName::Set("Old laptop"),
        "wander",
        None,
        None,
    )
    .unwrap();

    assert_eq!(db.missing_on_device("alpha", "phone", 50).unwrap().len(), 2);

    // The device is retired. Its holdings are the only record that these files ever existed
    // anywhere, and nothing removes them.
    assert!(db.delete_node("alpha", "old-laptop").unwrap());

    let still_offered = db.missing_on_device("alpha", "phone", 50).unwrap();
    assert_eq!(
        still_offered.len(),
        0,
        "a device that is gone cannot be a source: {} track(s) still offered",
        still_offered.len()
    );
}

#[test]
fn a_track_only_one_device_has_is_missing_on_the_other() {
    let db = db_with(&[(track("h1", "Nirvana", "Come As You Are", 219_000), "laptop")]);
    db.upsert_holding("alpha", "phone", "h0", None).ok();

    let missing = db.missing_on_device("alpha", "phone", 10).unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].content_hash, "h1");
}

#[test]
fn a_track_both_devices_have_is_not_missing() {
    let db = db_with(&[
        (track("h1", "Nirvana", "Come As You Are", 219_000), "laptop"),
        (track("h1", "Nirvana", "Come As You Are", 219_000), "phone"),
    ]);
    assert!(db
        .missing_on_device("alpha", "phone", 10)
        .unwrap()
        .is_empty());
}

/// The point of the fuzzy layer: different bytes, same recording.
#[test]
fn a_different_rip_of_the_same_recording_is_not_missing() {
    let db = db_with(&[
        (
            track("flac", "Nirvana", "Come As You Are", 219_000),
            "laptop",
        ),
        (track("mp3", "Nirvana", "Come As You Are", 220_500), "phone"),
    ]);
    assert!(
        db.missing_on_device("alpha", "phone", 10)
            .unwrap()
            .is_empty(),
        "a 1.5s-different encode of the same song must not be offered"
    );
}

/// The mistake that must never be made: a live take is not the studio cut.
#[test]
fn a_live_take_is_still_missing_when_you_own_the_studio_cut() {
    let db = db_with(&[
        (
            track("live", "Nirvana", "Come As You Are (Live)", 219_000),
            "laptop",
        ),
        (
            track("studio", "Nirvana", "Come As You Are", 219_000),
            "phone",
        ),
    ]);
    let missing = db.missing_on_device("alpha", "phone", 10).unwrap();
    assert_eq!(missing.len(), 1);
    assert_eq!(missing[0].content_hash, "live");
}

#[test]
fn another_accounts_device_is_not_consulted() {
    let db = Db::new_in_memory().unwrap();
    let t = track("h1", "Nirvana", "Come As You Are", 219_000);
    db.upsert_library_track(&t).unwrap();
    db.upsert_holding("beta", "beta-laptop", "h1", None)
        .unwrap();

    assert!(db
        .missing_on_device("alpha", "phone", 10)
        .unwrap()
        .is_empty());
}

#[test]
fn forgetting_a_holding_makes_it_missing_again() {
    let db = db_with(&[
        (track("h1", "A", "B", 100_000), "laptop"),
        (track("h1", "A", "B", 100_000), "phone"),
    ]);
    assert!(db
        .missing_on_device("alpha", "phone", 10)
        .unwrap()
        .is_empty());

    db.forget_holdings("alpha", "phone", &["h1".to_string()])
        .unwrap();
    assert_eq!(db.missing_on_device("alpha", "phone", 10).unwrap().len(), 1);
}

/// `forget_holdings` filtered on the device alone, and device ids are chosen by the client —
/// so naming someone else's device deleted their rows.
#[test]
fn forgetting_a_holding_cannot_reach_another_account() {
    let db = Db::new_in_memory().unwrap();
    let t = track("h1", "A", "B", 100_000);
    db.upsert_library_track(&t).unwrap();
    db.upsert_holding("alpha", "admin-desktop", "h1", None)
        .unwrap();

    let removed = db
        .forget_holdings("guest", "admin-desktop", &["h1".to_string()])
        .unwrap();
    assert_eq!(removed, 0, "a guest deleted the admin's holding");
    assert_eq!(
        db.device_holding_hashes("alpha", "admin-desktop")
            .unwrap()
            .len(),
        1
    );
}

/// The same smuggled device id, on the read side.
#[test]
fn device_holdings_cannot_be_read_across_accounts() {
    let db = Db::new_in_memory().unwrap();
    let t = track("h1", "A", "B", 100_000);
    db.upsert_library_track(&t).unwrap();
    db.upsert_holding("alpha", "admin-desktop", "h1", None)
        .unwrap();

    assert!(db
        .device_holding_hashes("guest", "admin-desktop")
        .unwrap()
        .is_empty());
}
