//! Devices boundary tests.

use super::*;

/// A token has to be named, because the list it joins is read by a human.
///
/// Unnamed tokens were all recorded as "paired device", so an account with several of them had a
/// revocation list where every row looked the same.
#[tokio::test]
pub(super) async fn a_pairing_token_must_be_named() {
    let h = harness();
    let r = h
        .run_as(
            &h.admin,
            r#"mutation { pairDevice(userId: "alpha") { token } }"#,
        )
        .await;
    assert_forbidden(&r, "pairDevice with no label");
}

/// Accounts come from signup, and from nothing else.
///
/// `createAccount` let an administrator conjure an active account straight into being, skipping
/// the username rules, the rate limiter and the approval queue that every stranger goes through.
/// Two ways in means two sets of rules, and only one of them was being maintained.
#[tokio::test]
pub(super) async fn graphql_cannot_create_an_account_at_all() {
    let h = harness();
    for who in [&h.admin, &h.guest] {
        let r = h
            .run_as(
                who,
                r#"mutation { createAccount(username: "puppet") { passphrase } }"#,
            )
            .await;
        assert_forbidden(&r, "createAccount");
        assert!(h.db.account("puppet").unwrap().is_none());
    }
}

/// A device may only be renamed by the account that owns it.
#[tokio::test]
pub(super) async fn nobody_can_rename_another_accounts_device() {
    let h = harness();
    h.db.upsert_node(
        "alpha-phone",
        "alpha",
        crate::db::NodeName::Set("Caffeinated Panda"),
        "wanda",
        None,
        None,
    )
    .unwrap();

    let refused = h
        .run_as(
            &h.guest,
            r#"mutation { renameNode(userId: "alpha", deviceId: "alpha-phone", petname: "Mine now") }"#,
        )
        .await;
    assert_forbidden(&refused, "a guest renaming alpha's device");

    let allowed = h
        .run_as(
            &h.admin,
            r#"mutation { renameNode(userId: "alpha", deviceId: "alpha-phone", petname: "Kitchen tablet") }"#,
        )
        .await;
    assert_allowed(&allowed, "alpha renaming their own device");
    assert_eq!(
        h.db.get_active_nodes("alpha").unwrap()[0].petname,
        "Kitchen tablet"
    );
}

/// An empty name would leave a device with no handle at all.
#[tokio::test]
pub(super) async fn a_device_cannot_be_renamed_to_nothing() {
    let h = harness();
    h.db.upsert_node(
        "alpha-phone",
        "alpha",
        crate::db::NodeName::Set("Caffeinated Panda"),
        "wanda",
        None,
        None,
    )
    .unwrap();
    let refused = h
        .run_as(
            &h.admin,
            r#"mutation { renameNode(userId: "alpha", deviceId: "alpha-phone", petname: "   ") }"#,
        )
        .await;
    assert_forbidden(&refused, "renaming a device to blank");
}

// ── The security log ────────────────────────────────────────────────────────────────────────
//
// An audit log is a record of who signed in from where. Reading it is exactly as sensitive as the
// data it protects, so it needs the same boundary as everything else — and the server-wide view
// needs one more, because "no userId" must not read as "every user".
