//! A client keeps the type it declared; a heartbeat only guesses.

use crate::db::*;

fn client_of(db: &Db, user: &str, device: &str) -> String {
    db.get_active_nodes(user)
        .unwrap()
        .into_iter()
        .find(|n| n.device_id == device)
        .expect("node")
        .client_type
}

/// The bug this covers: the socket and handoff paths guessed "wander" from the device id and wrote
/// it, so a third-party client was renamed to `wander` every time it reconnected.
#[test]
fn a_heartbeat_does_not_overwrite_the_declared_client_type() {
    let db = Db::new_in_memory().unwrap();
    db.upsert_node(
        "tui",
        "alpha",
        NodeName::Set("Desk"),
        "myplayer",
        None,
        None,
    )
    .unwrap();

    db.upsert_node(
        "tui",
        "alpha",
        NodeName::KeepOr("Alpaca"),
        "wander",
        None,
        None,
    )
    .unwrap();

    assert_eq!(client_of(&db, "alpha", "tui"), "myplayer");
}

#[test]
fn registering_again_changes_the_client_type() {
    let db = Db::new_in_memory().unwrap();
    db.upsert_node("tui", "alpha", NodeName::Set("Desk"), "wander", None, None)
        .unwrap();

    db.upsert_node(
        "tui",
        "alpha",
        NodeName::Set("Desk"),
        "myplayer",
        None,
        None,
    )
    .unwrap();

    assert_eq!(client_of(&db, "alpha", "tui"), "myplayer");
}

#[test]
fn a_declared_client_type_is_normalised_or_refused() {
    assert_eq!(declared_client_type(" MyPlayer ").unwrap(), "myplayer");
    assert_eq!(declared_client_type("wanda").unwrap(), "wanda");
    assert_eq!(declared_client_type("Wanda Android").unwrap(), "wanda");
    assert_eq!(declared_client_type("wander-tui").unwrap(), "wander");
    assert!(declared_client_type("").is_err());
    assert!(declared_client_type("has space").is_err());
    assert!(declared_client_type(&"a".repeat(33)).is_err());
}

#[test]
fn an_unregistered_device_is_guessed_from_its_id() {
    assert_eq!(inferred_client_type("Pixel-Android"), "wanda");
    assert_eq!(inferred_client_type("alpha-pc"), "wander");
}
