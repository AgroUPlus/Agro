//! Changing a passphrase has to move the vault with it, or the settings become unreadable.
use super::*;

fn account(db: &Db) -> String {
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    db.enrol_vault_key("alpha", "salt-one", "wrapped-under-one")
        .unwrap();
    db.mint_device_token("alpha", "laptop").unwrap()
}

#[test]
fn the_wrong_current_passphrase_changes_nothing() {
    let db = db();
    let token = account(&db);
    assert!(!db
        .change_passphrase("alpha", "not the passphrase", "a much longer new one", None)
        .unwrap());
    assert!(db.verify_login("alpha", "open sesame").unwrap().is_some());
    assert!(db.account_for_token(&token).unwrap().is_some());
}

#[test]
fn a_successful_change_swaps_the_passphrase_and_the_envelope() {
    let db = db();
    account(&db);
    assert!(db
        .change_passphrase(
            "alpha",
            "open sesame",
            "a much longer new one",
            Some(("salt-two", "wrapped-under-two")),
        )
        .unwrap());

    assert!(db.verify_login("alpha", "open sesame").unwrap().is_none());
    assert!(db
        .verify_login("alpha", "a much longer new one")
        .unwrap()
        .is_some());
    assert_eq!(
        db.vault_envelope("alpha").unwrap(),
        (Some("salt-two".into()), Some("wrapped-under-two".into()))
    );
}

/// The tokens bought with the old passphrase are exactly what changing it invalidates.
#[test]
fn changing_the_passphrase_signs_every_device_out() {
    let db = db();
    let token = account(&db);
    db.change_passphrase("alpha", "open sesame", "a much longer new one", None)
        .unwrap();
    assert!(db.account_for_token(&token).unwrap().is_none());
}

/// The write-once rule on first enrolment still holds — re-sealing is a different door.
#[test]
fn the_first_vault_enrolment_is_still_write_once() {
    let db = db();
    account(&db);
    assert!(
        !db.enrol_vault_key("alpha", "salt-two", "wrapped-under-two")
            .unwrap(),
        "enrol_vault_key must still refuse to replace a live key"
    );
}

#[test]
fn a_short_passphrase_is_refused() {
    let db = db();
    account(&db);
    assert!(db
        .change_passphrase("alpha", "open sesame", "short", None)
        .is_err());
    assert!(db.verify_login("alpha", "open sesame").unwrap().is_some());
}
