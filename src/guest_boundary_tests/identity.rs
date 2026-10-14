//! Identity boundary tests.

use super::*;
use crate::db_identity::{AccountState, Role};

#[tokio::test]
pub(super) async fn a_guest_cannot_enumerate_the_other_accounts() {
    let h = harness();
    let r = h.run_as(&h.guest, "{ users { username } }").await;
    assert_forbidden(&r, "users");

    let allowed = h.run_as(&h.admin, "{ users { username role } }").await;
    assert_allowed(&allowed, "users as admin");
}

#[tokio::test]
pub(super) async fn a_guest_cannot_read_another_account() {
    let h = harness();
    let r = h
        .run_as(&h.guest, r#"{ me(username: "alpha") { id } }"#)
        .await;
    assert_forbidden(&r, "me for another account");
}

/// The escalation this whole change exists to close: a device token used to buy the passphrase.
#[tokio::test]
pub(super) async fn no_query_returns_a_credential() {
    let h = harness();
    let r = h
        .run_as(
            &h.admin,
            r#"{ me(username: "alpha") { id username role } }"#,
        )
        .await;
    assert_allowed(&r, "me");

    let rendered = format!("{:?}", r.data);
    assert!(
        !rendered.contains("admin-pass"),
        "a passphrase leaked: {rendered}"
    );

    // The fields themselves are gone from the schema, so asking for them is an error.
    let asked = h
        .run_as(&h.admin, r#"{ me(username: "alpha") { passphrase } }"#)
        .await;
    assert_forbidden(&asked, "me.passphrase should not exist");
}

#[tokio::test]
pub(super) async fn a_guest_cannot_suspend_or_requota_anyone() {
    let h = harness();
    let suspend = h
        .run_as(
            &h.guest,
            r#"mutation { setAccountState(username: "alpha", state: "suspended") { state } }"#,
        )
        .await;
    assert_forbidden(&suspend, "setAccountState");

    let quota = h
        .run_as(
            &h.guest,
            r#"mutation { setAccountQuota(username: "mallory", quotaBytes: 0) { quotaBytes } }"#,
        )
        .await;
    assert_forbidden(&quota, "setAccountQuota");
    assert_eq!(
        h.db.account("mallory").unwrap().unwrap().effective_quota(),
        Some(crate::db_identity::DEFAULT_GUEST_QUOTA),
        "a guest raised its own quota"
    );
}

/// An admin who suspends themselves cannot restore themselves, and nobody else can either.
#[tokio::test]
pub(super) async fn an_admin_cannot_deactivate_themselves() {
    let h = harness();
    let r = h
        .run_as(
            &h.admin,
            r#"mutation { setAccountState(username: "alpha", state: "suspended") { state } }"#,
        )
        .await;
    assert_forbidden(&r, "self-suspension");
    assert!(h.db.account("alpha").unwrap().unwrap().state.is_active());
}

/// The gap this suite did not catch until it was run against a live server: `deleteAccount` used
/// `authorize`, which compares the caller to the *named* account — so an admin could remove nobody
/// but themselves, and a guest account could never be removed at all.
#[tokio::test]
pub(super) async fn an_admin_can_remove_a_guest_and_a_guest_cannot_remove_anyone_else() {
    let h = harness();
    h.db.create_account("bystander", "pw", Role::Member, AccountState::Active)
        .unwrap();

    let stolen = h
        .run_as(
            &h.guest,
            r#"mutation { deleteAccount(username: "bystander") }"#,
        )
        .await;
    assert_forbidden(&stolen, "a guest removed another account");
    assert!(h.db.account("bystander").unwrap().is_some());

    let own = h
        .run_as(
            &h.guest,
            r#"mutation { deleteAccount(username: "mallory") }"#,
        )
        .await;
    assert_allowed(&own, "a guest removing itself");
    assert!(h.db.account("mallory").unwrap().is_none());

    let by_admin = h
        .run_as(
            &h.admin,
            r#"mutation { deleteAccount(username: "bystander") }"#,
        )
        .await;
    assert_allowed(&by_admin, "an admin removing a guest");
    assert!(h.db.account("bystander").unwrap().is_none());
}

/// Removing the only admin leaves a server nobody can administer — and no setup token can rescue
/// it, because one is only minted for a database with no accounts at all.
#[tokio::test]
pub(super) async fn the_last_administrator_cannot_be_removed() {
    let h = harness();
    let r = h
        .run_as(&h.admin, r#"mutation { deleteAccount(username: "alpha") }"#)
        .await;
    assert_forbidden(&r, "the last admin was removed");
    assert!(h.db.account("alpha").unwrap().is_some());
}

// ── The server's own configuration ──────────────────────────────────────────────────────────
