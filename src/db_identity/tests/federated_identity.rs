//! The identity boundary that the whole OIDC design rests on.
use super::*;

fn db_with_alpha() -> Db {
    let db = db();
    db.create_account("alpha", "open sesame", Role::Member, AccountState::Active)
        .unwrap();
    db
}

/// **The central invariant.** An identity provider's `preferred_username` is a value its
/// own administrator can edit. If a callback could match it against an existing account,
/// setting it to `alpha` would hand over `alpha`. It must produce a *new* account instead.
#[test]
fn a_username_that_collides_never_resolves_to_the_existing_account() {
    let db = db_with_alpha();
    let derived = db.available_username_like("alpha").unwrap();
    assert_ne!(
        derived, "alpha",
        "an SSO login must never land on an existing account"
    );
    assert_eq!(derived, "alpha2");
    assert!(
        db.account(&derived).unwrap().is_none(),
        "and it must be free"
    );
}

#[test]
fn a_free_username_is_used_as_is() {
    let db = db_with_alpha();
    assert_eq!(
        db.available_username_like("brand-new").unwrap(),
        "brand-new"
    );
}

#[test]
fn suffixes_keep_climbing_past_repeated_collisions() {
    let db = db_with_alpha();
    db.create_account("alpha2", "x", Role::Member, AccountState::Active)
        .unwrap();
    assert_eq!(db.available_username_like("alpha").unwrap(), "alpha3");
}

/// The IdP chooses this string; it must go through the same rule every other username does.
#[test]
fn a_hostile_preferred_username_is_normalised_or_refused() {
    let db = db_with_alpha();
    assert_eq!(db.available_username_like("Alpha").unwrap(), "alpha2");
    assert!(db.available_username_like("../../etc/passwd").is_none());
    assert!(db.available_username_like("").is_none());
    assert!(db.available_username_like("   ").is_none());
}

#[test]
fn an_unlinked_identity_resolves_to_nobody() {
    let db = db_with_alpha();
    assert!(db
        .account_for_federated_identity("https://id.example.com", "sub-1")
        .unwrap()
        .is_none());
}

#[test]
fn a_linked_identity_resolves_to_its_account() {
    let db = db_with_alpha();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();
    let found = db
        .account_for_federated_identity("https://id.example.com", "sub-1")
        .unwrap()
        .unwrap();
    assert_eq!(found.username, "alpha");
}

/// A subject maps to at most one account. Re-pointing it would strand the first.
#[test]
fn an_identity_cannot_be_relinked_to_a_second_account() {
    let db = db_with_alpha();
    db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
        .unwrap();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();

    assert!(db
        .link_federated_identity("mallory", "https://id.example.com", "sub-1", None)
        .is_err());
    assert_eq!(
        db.account_for_federated_identity("https://id.example.com", "sub-1")
            .unwrap()
            .unwrap()
            .username,
        "alpha"
    );
}

/// Two providers can both be the same person.
#[test]
fn one_account_may_link_more_than_one_provider() {
    let db = db_with_alpha();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();
    db.link_federated_identity("alpha", "https://other.example.com", "sub-9", None)
        .unwrap();
    assert_eq!(db.federated_identities("alpha").unwrap().len(), 2);
}

/// The same subject string from a *different* issuer is a different person.
#[test]
fn the_issuer_is_part_of_the_identity() {
    let db = db_with_alpha();
    db.link_federated_identity("alpha", "https://id.example.com", "shared-sub", None)
        .unwrap();
    assert!(db
        .account_for_federated_identity("https://evil.example.com", "shared-sub")
        .unwrap()
        .is_none());
}

#[test]
fn linking_the_same_identity_twice_is_harmless() {
    let db = db_with_alpha();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();
    assert_eq!(db.federated_identities("alpha").unwrap().len(), 1);
}

/// An SSO account's passphrase is generated and never shown, so unlinking its only identity
/// would leave nobody able to get in.
#[test]
fn the_last_way_into_an_account_cannot_be_unlinked() {
    let db = db_with_alpha();
    db.mark_passphrase_unusable("alpha").unwrap();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();

    assert!(db
        .unlink_federated_identity("alpha", "https://id.example.com", "sub-1")
        .is_err());
    assert_eq!(db.federated_identities("alpha").unwrap().len(), 1);
}

/// Setting a passphrase is what makes unlinking safe.
#[test]
fn unlinking_is_allowed_once_a_passphrase_exists() {
    let db = db_with_alpha();
    db.mark_passphrase_unusable("alpha").unwrap();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();
    db.change_passphrase("alpha", "open sesame", "a much longer new one", None)
        .unwrap();

    assert!(db
        .unlink_federated_identity("alpha", "https://id.example.com", "sub-1")
        .unwrap());
    assert!(db.federated_identities("alpha").unwrap().is_empty());
}

/// An account that has a real passphrase can always unlink.
#[test]
fn an_account_with_a_passphrase_can_unlink_freely() {
    let db = db_with_alpha();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();
    assert!(db
        .unlink_federated_identity("alpha", "https://id.example.com", "sub-1")
        .unwrap());
}

/// One account must not be able to unlink another's identity by naming it.
#[test]
fn unlinking_is_scoped_to_the_owning_account() {
    let db = db_with_alpha();
    db.create_account("mallory", "hunter2", Role::Member, AccountState::Active)
        .unwrap();
    db.link_federated_identity("alpha", "https://id.example.com", "sub-1", None)
        .unwrap();

    assert!(!db
        .unlink_federated_identity("mallory", "https://id.example.com", "sub-1")
        .unwrap());
    assert_eq!(db.federated_identities("alpha").unwrap().len(), 1);
}
