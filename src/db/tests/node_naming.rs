//! A device keeps the name it was given.

use crate::db::*;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

fn petname_of(db: &Db, user: &str, device: &str) -> String {
    db.get_active_nodes(user)
        .unwrap()
        .into_iter()
        .find(|n| n.device_id == device)
        .expect("node")
        .petname
}

/// The bug this covers: every WebSocket connect invented a name and wrote it, so a device
/// was renamed to a fresh random animal each time the server restarted.
#[test]
fn a_reconnect_does_not_rename_a_named_device() {
    let db = db();
    db.upsert_node(
        "pixel",
        "alpha",
        NodeName::Set("Pixel 10"),
        "wanda",
        None,
        None,
    )
    .unwrap();

    db.upsert_node(
        "pixel",
        "alpha",
        NodeName::KeepOr("Glitchy Alpaca"),
        "wanda",
        None,
        None,
    )
    .unwrap();

    assert_eq!(petname_of(&db, "alpha", "pixel"), "Pixel 10");
}

#[test]
fn a_device_seen_for_the_first_time_takes_the_fallback_name() {
    let db = db();
    db.upsert_node(
        "pixel",
        "alpha",
        NodeName::KeepOr("Glitchy Alpaca"),
        "wanda",
        None,
        None,
    )
    .unwrap();

    assert_eq!(petname_of(&db, "alpha", "pixel"), "Glitchy Alpaca");
}

#[test]
fn naming_a_device_still_renames_it() {
    let db = db();
    db.upsert_node(
        "pixel",
        "alpha",
        NodeName::Set("Pixel 10"),
        "wanda",
        None,
        None,
    )
    .unwrap();
    db.upsert_node(
        "pixel",
        "alpha",
        NodeName::Set("Work phone"),
        "wanda",
        None,
        None,
    )
    .unwrap();

    assert_eq!(petname_of(&db, "alpha", "pixel"), "Work phone");
}

/// Two accounts can choose the same device id, and one must not rename the other's device.
#[test]
fn one_account_cannot_rename_another_accounts_device() {
    let db = db();
    db.upsert_node(
        "laptop",
        "alpha",
        NodeName::Set("Cachy"),
        "wander",
        None,
        None,
    )
    .unwrap();
    db.upsert_node(
        "laptop",
        "delta",
        NodeName::Set("Lenovo"),
        "wander",
        None,
        None,
    )
    .unwrap();

    assert_eq!(petname_of(&db, "alpha", "laptop"), "Cachy");
    assert_eq!(petname_of(&db, "delta", "laptop"), "Lenovo");
}
