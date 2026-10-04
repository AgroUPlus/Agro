//! Per-device public keys on a profile.

use crate::db::*;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

#[test]
fn public_key_can_be_set_and_read_on_profile() {
    let db = db();
    db.create_account(
        "alpha",
        "pass",
        crate::db_identity::Role::Admin,
        crate::db_identity::AccountState::Active,
    )
    .unwrap();
    assert_eq!(db.profile("alpha").unwrap().unwrap().public_key, None);
    assert!(db
        .set_public_key("alpha", Some("base64-pubkey-xyz"))
        .unwrap());
    assert_eq!(
        db.profile("alpha").unwrap().unwrap().public_key.as_deref(),
        Some("base64-pubkey-xyz")
    );
    assert!(db.set_public_key("alpha", None).unwrap());
    assert_eq!(db.profile("alpha").unwrap().unwrap().public_key, None);
}

/// The registry's whole reason for existing: a second device must not displace the first.
#[test]
fn a_second_device_key_does_not_replace_the_first() {
    let db = db();
    db.create_account(
        "alpha",
        "pass",
        crate::db_identity::Role::Admin,
        crate::db_identity::AccountState::Active,
    )
    .unwrap();

    db.register_device_key("alpha", "phone", "key-phone")
        .unwrap();
    db.register_device_key("alpha", "laptop", "key-laptop")
        .unwrap();

    let keys = db.device_keys_for("alpha").unwrap();
    assert_eq!(
        keys.len(),
        2,
        "signing in on a second device must not evict the first"
    );
    assert!(keys
        .iter()
        .any(|k| k.device_id == "phone" && k.public_key == "key-phone"));
    assert!(keys
        .iter()
        .any(|k| k.device_id == "laptop" && k.public_key == "key-laptop"));
}

/// A device replaces its own entry — a reinstall regenerates a keypair — and only its own.
#[test]
fn re_registering_replaces_only_that_devices_key() {
    let db = db();
    db.create_account(
        "alpha",
        "pass",
        crate::db_identity::Role::Admin,
        crate::db_identity::AccountState::Active,
    )
    .unwrap();

    db.register_device_key("alpha", "phone", "key-phone")
        .unwrap();
    db.register_device_key("alpha", "laptop", "key-laptop")
        .unwrap();
    db.register_device_key("alpha", "phone", "key-phone-v2")
        .unwrap();

    let keys = db.device_keys_for("alpha").unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(
        keys.iter()
            .find(|k| k.device_id == "phone")
            .unwrap()
            .public_key,
        "key-phone-v2"
    );
    assert_eq!(
        keys.iter()
            .find(|k| k.device_id == "laptop")
            .unwrap()
            .public_key,
        "key-laptop",
        "one device re-keying must not touch another's entry"
    );
}

/// Signing one device out stops it being sealed to, and leaves the rest of the account alone.
#[test]
fn forgetting_one_device_leaves_the_others() {
    let db = db();
    db.create_account(
        "alpha",
        "pass",
        crate::db_identity::Role::Admin,
        crate::db_identity::AccountState::Active,
    )
    .unwrap();

    db.register_device_key("alpha", "phone", "key-phone")
        .unwrap();
    db.register_device_key("alpha", "laptop", "key-laptop")
        .unwrap();
    assert!(db.forget_device_key("alpha", "phone").unwrap());

    let keys = db.device_keys_for("alpha").unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].device_id, "laptop");
    assert!(
        !db.forget_device_key("alpha", "phone").unwrap(),
        "forgetting twice is a no-op"
    );
}

/// The `legacy` row migration 36 carries over is superseded once a real device claims the key,
/// so a note is not sealed twice for one phone.
#[test]
fn claiming_a_legacy_key_retires_the_legacy_row() {
    let db = db();
    db.create_account(
        "alpha",
        "pass",
        crate::db_identity::Role::Admin,
        crate::db_identity::AccountState::Active,
    )
    .unwrap();

    db.register_device_key("alpha", crate::db_social::LEGACY_DEVICE_ID, "key-one")
        .unwrap();
    db.register_device_key("alpha", "phone", "key-one").unwrap();

    let keys = db.device_keys_for("alpha").unwrap();
    assert_eq!(
        keys.len(),
        1,
        "the same key must not be listed under two device ids"
    );
    assert_eq!(keys[0].device_id, "phone");
}
