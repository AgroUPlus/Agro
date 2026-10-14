use super::*;
use crate::credentials;
use crate::lock::LockExt;
use rusqlite::params;

fn db() -> Db {
    Db::new_in_memory().unwrap()
}

/// A new account starts with no vault key. The client is what notices and enrols one.
#[test]
fn a_fresh_account_has_no_vault_envelope() {
    let db = db();
    db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
        .unwrap();
    assert_eq!(db.vault_envelope("alpha").unwrap(), (None, None));
}

#[test]
fn an_enrolled_vault_key_comes_back_verbatim() {
    let db = db();
    db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
        .unwrap();

    assert!(db.enrol_vault_key("alpha", "s4lt", "sealed-key").unwrap());
    assert_eq!(
        db.vault_envelope("alpha").unwrap(),
        (Some("s4lt".into()), Some("sealed-key".into()))
    );
}

/// Two devices setting up the same account at once must not have the second overwrite the
/// first — the settings already sealed under the first key would become unreadable.
#[test]
fn a_vault_key_cannot_be_replaced_once_set() {
    let db = db();
    db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
        .unwrap();

    assert!(db
        .enrol_vault_key("alpha", "first-salt", "first-key")
        .unwrap());
    assert!(
        !db.enrol_vault_key("alpha", "second-salt", "second-key")
            .unwrap(),
        "the second enrolment should report that it did not land"
    );
    assert_eq!(
        db.vault_envelope("alpha").unwrap(),
        (Some("first-salt".into()), Some("first-key".into())),
        "the key that is already sealing settings must survive"
    );
}

/// The property the whole design exists for: what the server stores about an account's
/// settings must not be enough to read them. It holds a passphrase *hash* and a key sealed
/// under the passphrase — never the passphrase, and so never the key.
#[test]
fn the_server_stores_nothing_that_unwraps_the_vault() {
    let db = db();
    db.create_account(
        "alpha",
        "correct horse battery staple",
        Role::Admin,
        AccountState::Active,
    )
    .unwrap();
    db.enrol_vault_key("alpha", "s4lt", "sealed-key").unwrap();

    let conn = db.conn.lock_recover();
    let (hash, salt, wrapped, api_key): (String, String, String, String) = conn
        .query_row(
            "SELECT passphrase_hash, vault_salt, vault_key_wrapped, api_key FROM users",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .unwrap();

    for stored in [&hash, &salt, &wrapped, &api_key] {
        assert!(
            !stored.contains("correct horse battery staple"),
            "the passphrase must not be recoverable from {stored}"
        );
    }
    assert!(hash.starts_with("$argon2"), "hashed, not stored: {hash}");

    // And the column that used to hold a server-minted key to these settings is gone from the
    // write path entirely.
    assert_eq!(api_key, "", "api_key is no longer a key to anything");
}

#[test]
fn an_account_round_trips_with_a_hashed_passphrase() {
    let db = db();
    db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
        .unwrap();

    let found = db.account("alpha").unwrap().unwrap();
    assert!(found.is_admin());
    assert!(found.state.is_active());
    assert_eq!(found.effective_quota(), None, "admins are uncapped");

    assert!(db.verify_login("alpha", "open sesame").unwrap().is_some());
    assert!(db.verify_login("alpha", "wrong").unwrap().is_none());
}

/// The hole this replaces: logging in as nobody used to *create* nobody.
#[test]
fn logging_in_as_an_unknown_user_does_not_create_it() {
    let db = db();
    assert!(db.verify_login("intruder", "whatever").unwrap().is_none());
    assert!(db.account("intruder").unwrap().is_none());
    assert!(db.list_accounts().unwrap().is_empty());
}

#[test]
fn a_username_cannot_be_taken_over_by_recreating_it() {
    let db = db();
    db.create_account("alpha", "original", Role::Admin, AccountState::Active)
        .unwrap();
    assert!(db
        .create_account("alpha", "attacker", Role::Member, AccountState::Active)
        .is_err());
    assert!(db.verify_login("alpha", "original").unwrap().is_some());
    assert!(db.verify_login("alpha", "attacker").unwrap().is_none());
}

#[test]
fn a_device_token_authenticates_and_is_not_stored_in_the_clear() {
    let db = db();
    db.create_account("guest", "pw", Role::Member, AccountState::Active)
        .unwrap();
    let secret = db.mint_device_token("guest", "phone").unwrap();

    let (account, label) = db.account_for_token(&secret).unwrap().unwrap();
    assert_eq!(account.username, "guest");
    assert!(!account.is_admin());
    assert_eq!(account.effective_quota(), Some(DEFAULT_GUEST_QUOTA));
    // The name its owner gave it travels with the token, so registration need not invent one.
    assert_eq!(label, "phone");

    let conn = db.conn.lock_recover();
    let stored: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM app_passwords WHERE token_hash = ?1 OR token = ?1",
            params![secret],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, 0, "the raw token must not appear in the database");
}

#[test]
fn a_wrong_token_resolves_to_nobody() {
    let db = db();
    db.create_account("guest", "pw", Role::Member, AccountState::Active)
        .unwrap();
    let secret = db.mint_device_token("guest", "phone").unwrap();

    assert!(db.account_for_token("").unwrap().is_none());
    assert!(db.account_for_token("nonsense").unwrap().is_none());
    // Same prefix, wrong secret: the prefix is only an index, never the check.
    let forged = format!("{}{}", &secret[..8], "x".repeat(40));
    assert!(db.account_for_token(&forged).unwrap().is_none());
}

/// The passphrase is no longer a bearer token. This is the escalation path being closed.
#[test]
fn the_passphrase_is_not_accepted_as_a_token() {
    let db = db();
    db.create_account("alpha", "open sesame", Role::Admin, AccountState::Active)
        .unwrap();
    assert!(db.account_for_token("open sesame").unwrap().is_none());
}

#[test]
fn unrecognised_role_and_state_values_fall_to_the_safe_reading() {
    assert_eq!(Role::parse("wizard"), Role::Member);
    assert_eq!(Role::parse(""), Role::Member);
    assert_eq!(Role::parse("ADMIN"), Role::Admin);
    assert_eq!(AccountState::parse("nonsense"), AccountState::Suspended);
    assert_eq!(AccountState::parse(""), AccountState::Suspended);
}

#[test]
fn legacy_plaintext_passphrases_are_hashed_on_startup() {
    let db = db();
    {
        let conn = db.conn.lock_recover();
        conn.execute(
            "INSERT INTO users (id, username, api_key, created_at, passphrase_hash, role, state, quota_bytes)
             VALUES ('legacy-id', 'legacy', 'old-passphrase', '2020-01-01T00:00:00Z', '', 'admin', 'active', 0)",
            [],
        )
        .unwrap();
    }

    assert_eq!(db.migrate_credentials().unwrap(), 1);
    assert!(db
        .verify_login("legacy", "old-passphrase")
        .unwrap()
        .is_some());
    // ...but it is no longer a token.
    assert!(db.account_for_token("old-passphrase").unwrap().is_none());
    // Running twice must not re-hash what is already done.
    assert_eq!(db.migrate_credentials().unwrap(), 0);
}

mod federated_identity;
mod passphrase_change;
mod second_factor;
mod token_lifecycle;
