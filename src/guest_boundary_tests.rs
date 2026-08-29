//! What a hostile guest account cannot do.
//!
//! Every case here is a thing that *worked* before the admin/guest boundary existed. They are kept
//! together, and phrased as the attack rather than as the feature, because the failure mode being
//! guarded against is a future change quietly re-opening one of them: a resolver that stops calling
//! `require_admin`, a query that goes back to trusting a caller-supplied device id.
//!
//! These drive the real schema through `Schema::execute`, so they exercise the same resolvers and
//! the same guards a request would. Three things they deliberately do *not* cover, because none of
//! them are reachable from a schema execution:
//!
//! - the middleware in `auth`, which is where a token becomes an identity;
//! - `login` and `bootstrap`, which are unauthenticated REST routes (see `crate::login`) precisely
//!   so that `/graphql` never has to accept an anonymous caller;
//! - the streaming quota enforcement in `library::put_upload`, which needs a live request body.
//!
//! Those are verified against a running server instead.

#![cfg(test)]

use async_graphql::{Request, Schema, Value};
use std::sync::Arc;

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;

struct Harness {
    schema: AgroSchema,
    db: Db,
    admin: Account,
    guest: Account,
}

fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let admin = db
        .create_account("alpha", "admin-pass", Role::Admin, AccountState::Active)
        .unwrap();
    let guest = db
        .create_account("mallory", "guest-pass", Role::Member, AccountState::Active)
        .unwrap();

    let storage = Storage::for_tests();
    let schema = Schema::build(Query::default(), Mutation::default(), async_graphql::EmptySubscription)
        .data(db.clone())
        .data(Arc::new(WsHub::new()))
        .data(storage)
        .data(SetupToken::for_fresh_server(1))
        .finish();

    Harness { schema, db, admin, guest }
}

impl Harness {
    /// Runs a document as an account, exactly as an authenticated request would.
    async fn run_as(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
        });
        self.schema.execute(request).await
    }

    /// Runs a document with no identity at all — a request that got past the middleware without
    /// one, which is what the old fail-open `authorize` allowed.
    async fn run_anonymously(&self, query: &str) -> async_graphql::Response {
        self.schema.execute(Request::new(query)).await
    }
}

fn assert_forbidden(response: &async_graphql::Response, what: &str) {
    assert!(
        !response.errors.is_empty(),
        "{what}: expected a refusal, got data: {:?}",
        response.data
    );
}

fn assert_allowed(response: &async_graphql::Response, what: &str) {
    assert!(
        response.errors.is_empty(),
        "{what}: expected success, got errors: {:?}",
        response.errors
    );
}

// ── The account boundary ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_guest_cannot_enumerate_the_other_accounts() {
    let h = harness();
    let r = h.run_as(&h.guest, "{ users { username } }").await;
    assert_forbidden(&r, "users");

    let allowed = h.run_as(&h.admin, "{ users { username role } }").await;
    assert_allowed(&allowed, "users as admin");
}

#[tokio::test]
async fn a_guest_cannot_read_another_account() {
    let h = harness();
    let r = h.run_as(&h.guest, r#"{ me(username: "alpha") { id } }"#).await;
    assert_forbidden(&r, "me for another account");
}

/// The escalation this whole change exists to close: a device token used to buy the passphrase.
#[tokio::test]
async fn no_query_returns_a_credential() {
    let h = harness();
    let r = h.run_as(&h.admin, r#"{ me(username: "alpha") { id username role } }"#).await;
    assert_allowed(&r, "me");

    let rendered = format!("{:?}", r.data);
    assert!(!rendered.contains("admin-pass"), "a passphrase leaked: {rendered}");

    // The fields themselves are gone from the schema, so asking for them is an error.
    let asked = h.run_as(&h.admin, r#"{ me(username: "alpha") { passphrase } }"#).await;
    assert_forbidden(&asked, "me.passphrase should not exist");
}
#[tokio::test]
async fn a_guest_cannot_suspend_or_requota_anyone() {
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
async fn an_admin_cannot_deactivate_themselves() {
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
async fn an_admin_can_remove_a_guest_and_a_guest_cannot_remove_anyone_else() {
    let h = harness();
    h.db.create_account("bystander", "pw", Role::Member, AccountState::Active)
        .unwrap();

    let stolen = h
        .run_as(&h.guest, r#"mutation { deleteAccount(username: "bystander") }"#)
        .await;
    assert_forbidden(&stolen, "a guest removed another account");
    assert!(h.db.account("bystander").unwrap().is_some());

    let own = h
        .run_as(&h.guest, r#"mutation { deleteAccount(username: "mallory") }"#)
        .await;
    assert_allowed(&own, "a guest removing itself");
    assert!(h.db.account("mallory").unwrap().is_none());

    let by_admin = h
        .run_as(&h.admin, r#"mutation { deleteAccount(username: "bystander") }"#)
        .await;
    assert_allowed(&by_admin, "an admin removing a guest");
    assert!(h.db.account("bystander").unwrap().is_none());
}

/// Removing the only admin leaves a server nobody can administer — and no setup token can rescue
/// it, because one is only minted for a database with no accounts at all.
#[tokio::test]
async fn the_last_administrator_cannot_be_removed() {
    let h = harness();
    let r = h
        .run_as(&h.admin, r#"mutation { deleteAccount(username: "alpha") }"#)
        .await;
    assert_forbidden(&r, "the last admin was removed");
    assert!(h.db.account("alpha").unwrap().is_some());
}

// ── The server's own configuration ──────────────────────────────────────────────────────────

#[tokio::test]
async fn a_guest_cannot_flip_server_wide_plugin_state() {
    let h = harness();
    let r = h
        .run_as(
            &h.guest,
            r#"mutation { togglePlugin(pluginId: "subsonic-navidrome", isEnabled: false) }"#,
        )
        .await;
    assert_forbidden(&r, "togglePlugin");
}

/// `plugins` reports "connected" by reading whichever account owns the first node, so for a guest
/// it answered with the admin's Navidrome address and username.
#[tokio::test]
async fn a_guest_cannot_read_the_plugin_registry() {
    let h = harness();
    assert_forbidden(&h.run_as(&h.guest, "{ plugins { id isConnected } }").await, "plugins");
}

/// The `/listen` allowlist is the only thing between the operator's share domain and an open
/// redirect.
#[tokio::test]
async fn a_guest_cannot_widen_the_share_allowlist() {
    let h = harness();
    let r = h
        .run_as(
            &h.guest,
            r#"mutation { updateSyncedSettings(input: {
                 userId: "mallory", shareHosts: "evil.example", shareEnabled: true
               }) { shareHosts } }"#,
        )
        .await;
    assert_forbidden(&r, "share settings");
}

#[tokio::test]
async fn a_malformed_share_host_is_refused_even_for_the_admin() {
    let h = harness();
    for bad in [
        "https://evil.example",
        "evil.example/path",
        "*.example.com",
        "localhost",
    ] {
        let r = h
            .run_as(
                &h.admin,
                &format!(
                    r#"mutation {{ updateSyncedSettings(input: {{
                         userId: "alpha", shareHosts: "{bad}"
                       }}) {{ shareHosts }} }}"#
                ),
            )
            .await;
        assert_forbidden(&r, &format!("share host {bad:?}"));
    }
}

// ── Other accounts' devices ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_guest_cannot_read_or_delete_another_accounts_holdings() {
    let h = harness();
    h.db.upsert_holding("alpha", "admin-desktop", &"a".repeat(64), None)
        .unwrap();

    // Naming their own account but the admin's device: `authorize` passes, and the device check is
    // what has to catch it.
    let read = h
        .run_as(
            &h.guest,
            r#"{ deviceHoldings(userId: "mallory", deviceId: "admin-desktop") }"#,
        )
        .await;
    assert_forbidden(&read, "deviceHoldings with a smuggled device id");

    let forget = h
        .run_as(
            &h.guest,
            &format!(
                r#"mutation {{ forgetHoldings(userId: "mallory", deviceId: "admin-desktop", hashes: ["{}"]) }}"#,
                "a".repeat(64)
            ),
        )
        .await;
    assert_forbidden(&forget, "forgetHoldings with a smuggled device id");
    assert_eq!(
        h.db.device_holding_hashes("alpha", "admin-desktop").unwrap().len(),
        1,
        "the admin's holding was deleted"
    );
}

// ── The library ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_guest_cannot_browse_the_library() {
    let h = harness();
    let r = h
        .run_as(
            &h.guest,
            r#"{ libraryBrowse(userId: "mallory", kind: ARTIST) { name } }"#,
        )
        .await;
    assert_forbidden(&r, "libraryBrowse");
}

// ── Fail-closed ─────────────────────────────────────────────────────────────────────────────

/// `authorize` used to return `Ok` when nobody was authenticated, to leave room for a first-run
/// window in the middleware. Both halves are gone.
#[tokio::test]
async fn an_unauthenticated_request_is_refused_everywhere() {
    let h = harness();
    for query in [
        r#"{ me(username: "alpha") { id } }"#,
        r#"{ users { username } }"#,
        r#"{ playbackHandoff(userId: "alpha") { trackTitle } }"#,
        r#"{ listeningStats(userId: "alpha") { playsTotal } }"#,
        r#"{ syncedSettings(userId: "alpha") { serverUrl } }"#,
        r#"mutation { createAccount(username: "x") { passphrase } }"#,
        r#"mutation { togglePlugin(pluginId: "p", isEnabled: true) }"#,
    ] {
        assert_forbidden(&h.run_anonymously(query).await, query);
    }
}

// ── What a guest *can* do ───────────────────────────────────────────────────────────────────

/// The boundary is only correct if it still lets a guest be a guest.
#[tokio::test]
async fn a_guest_can_still_use_their_own_account() {
    let h = harness();

    assert_allowed(
        &h.run_as(&h.guest, r#"{ me(username: "mallory") { id username role } }"#).await,
        "own account",
    );
    assert_allowed(
        &h.run_as(&h.guest, r#"{ listeningStats(userId: "mallory") { playsTotal } }"#).await,
        "own stats",
    );
    assert_allowed(
        &h.run_as(&h.guest, r#"{ activeNodes(userId: "mallory") { deviceId } }"#).await,
        "own devices",
    );
    assert_allowed(
        &h.run_as(
            &h.guest,
            r#"mutation { updateSyncedSettings(input: { userId: "mallory", streamFormat: "OPUS" }) { streamFormat } }"#,
        )
        .await,
        "own non-sharing settings",
    );
}

#[tokio::test]
async fn a_guest_quota_defaults_to_ten_megabytes_and_the_admin_has_none() {
    let h = harness();
    assert_eq!(
        h.guest.effective_quota(),
        Some(10 * 1024 * 1024),
        "guest quota"
    );
    assert_eq!(h.admin.effective_quota(), None, "admin is uncapped");
}

#[tokio::test]
async fn usernames_are_validated() {
    let h = harness();
    for bad in ["", "  ", "has space", "sql'injection", &"x".repeat(64)] {
        let r = h
            .run_as(
                &h.admin,
                &format!(r#"mutation {{ createAccount(username: "{bad}") {{ passphrase }} }}"#),
            )
            .await;
        assert_forbidden(&r, &format!("username {bad:?}"));
    }
}

/// Pairing hands over a device token, and the QR carries that rather than the passphrase —
/// photographing the old one handed over the account permanently.
#[tokio::test]
async fn pairing_carries_a_device_token_not_a_passphrase() {
    let h = harness();
    let r = h
        .run_as(
            &h.admin,
            r#"mutation { pairDevice(userId: "alpha", label: "Living room laptop") { token qrData label } }"#,
        )
        .await;
    assert_allowed(&r, "pairDevice");

    let Value::Object(ref data) = r.data else {
        panic!("unexpected shape: {:?}", r.data)
    };
    let Some(Value::Object(payload)) = data.get("pairDevice") else {
        panic!("no payload: {:?}", r.data)
    };
    let text = |key: &str| match payload.get(key) {
        Some(Value::String(s)) => s.clone(),
        other => panic!("{key} was {other:?}"),
    };

    let device_token = text("token");
    let qr = text("qrData");

    assert!(!device_token.is_empty());
    assert_eq!(text("label"), "Living room laptop");
    assert!(qr.contains(&device_token), "the QR does not carry the device token");
    assert!(
        !qr.contains("alpha-pass"),
        "the QR still carries the passphrase: {qr}"
    );
    assert!(h.db.account_for_token(&device_token).unwrap().is_some());
}

/// A token has to be named, because the list it joins is read by a human.
///
/// Unnamed tokens were all recorded as "paired device", so an account with several of them had a
/// revocation list where every row looked the same.
#[tokio::test]
async fn a_pairing_token_must_be_named() {
    let h = harness();
    let r = h
        .run_as(&h.admin, r#"mutation { pairDevice(userId: "alpha") { token } }"#)
        .await;
    assert_forbidden(&r, "pairDevice with no label");
}

/// Accounts come from signup, and from nothing else.
///
/// `createAccount` let an administrator conjure an active account straight into being, skipping
/// the username rules, the rate limiter and the approval queue that every stranger goes through.
/// Two ways in means two sets of rules, and only one of them was being maintained.
#[tokio::test]
async fn graphql_cannot_create_an_account_at_all() {
    let h = harness();
    for who in [&h.admin, &h.guest] {
        let r = h
            .run_as(who, r#"mutation { createAccount(username: "puppet") { passphrase } }"#)
            .await;
        assert_forbidden(&r, "createAccount");
        assert!(h.db.account("puppet").unwrap().is_none());
    }
}

/// A device may only be renamed by the account that owns it.
#[tokio::test]
async fn nobody_can_rename_another_accounts_device() {
    let h = harness();
    h.db.upsert_node("alpha-phone", "alpha", crate::db::NodeName::Set("Caffeinated Panda"), "wanda", None, None)
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
async fn a_device_cannot_be_renamed_to_nothing() {
    let h = harness();
    h.db.upsert_node("alpha-phone", "alpha", crate::db::NodeName::Set("Caffeinated Panda"), "wanda", None, None)
        .unwrap();
    let refused = h
        .run_as(
            &h.admin,
            r#"mutation { renameNode(userId: "alpha", deviceId: "alpha-phone", petname: "   ") }"#,
        )
        .await;
    assert_forbidden(&refused, "renaming a device to blank");
}
