//! A switched-off feature is refused, and only an admin can switch one.

#![cfg(test)]

use std::sync::Arc;

use async_graphql::{Request, Schema};
use serde_json::Value;

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;

struct Harness {
    schema: AgroSchema,
    db: Db,
    hub: Arc<WsHub>,
    admin: Account,
    member: Account,
}

fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let admin = db
        .create_account("root", "root-pass", Role::Admin, AccountState::Active)
        .unwrap();
    let member = db
        .create_account("alpha", "alpha-pass", Role::Member, AccountState::Active)
        .unwrap();
    let hub = Arc::new(WsHub::new());
    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(hub.clone())
    .data(Storage::for_tests())
    .data(SetupToken::for_fresh_server(1))
    .finish();
    Harness {
        schema,
        db,
        hub,
        admin,
        member,
    }
}

impl Harness {
    async fn run_as(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
            token_hash: String::new(),
        });
        self.schema.execute(request).await
    }

    async fn toggle(&self, as_: &Account, id: &str, on: bool) -> async_graphql::Response {
        self.run_as(
            as_,
            &format!(r#"mutation {{ togglePlugin(pluginId: "{id}", isEnabled: {on}) }}"#),
        )
        .await
    }
}

impl Harness {
    /// Sends one heartbeat as `alpha` and reports whether `friend` was pushed presence for it.
    async fn heartbeat_reaches(&self, friend: &str) -> bool {
        let mut tap = self.hub.channels.tap.subscribe();
        let heartbeat = r#"mutation { updateHandoff(input: { userId: "alpha", trackUri: "t",
            trackTitle: "T", artistName: "A", positionMs: 0, durationMs: 1000, isPlaying: true,
            deviceId: "phone" }) }"#;
        let response = self.run_as(&self.member, heartbeat).await;
        assert!(response.errors.is_empty(), "{:?}", response.errors);
        let mut reached = false;
        while let Ok(msg) = tap.try_recv() {
            reached |= msg.msg_type == "FRIEND_PRESENCE" && msg.user_id.as_deref() == Some(friend);
        }
        reached
    }
}

fn code(response: &async_graphql::Response) -> Option<String> {
    let ext = response.errors.first()?.extensions.as_ref()?;
    match ext.get("code")? {
        async_graphql::Value::String(code) => Some(code.clone()),
        _ => None,
    }
}

#[tokio::test]
async fn a_switched_off_feature_is_refused_with_a_code_and_comes_back_on() {
    let h = harness();
    let query = "{ friendJams { __typename } }";
    assert!(h.run_as(&h.member, query).await.errors.is_empty());

    assert!(h.toggle(&h.admin, "jams", false).await.errors.is_empty());
    let refused = h.run_as(&h.member, query).await;
    assert_eq!(code(&refused).as_deref(), Some("FEATURE_DISABLED"));

    assert!(h.toggle(&h.admin, "jams", true).await.errors.is_empty());
    assert!(h.run_as(&h.member, query).await.errors.is_empty());
}

#[tokio::test]
async fn only_an_admin_can_switch_a_feature() {
    let h = harness();
    assert!(!h.toggle(&h.member, "jams", false).await.errors.is_empty());
    assert!(crate::features::Feature::Jams.is_on(&h.db));
}

/// The old mutation stored any id it was handed, so the dashboard's switches wrote rows nothing
/// read. Only what the server can refuse is accepted now.
#[tokio::test]
async fn a_switch_for_nothing_is_refused() {
    let h = harness();
    assert!(!h
        .toggle(&h.admin, "wifi-precache", false)
        .await
        .errors
        .is_empty());
    assert!(!h
        .toggle(&h.admin, "wanda-android", false)
        .await
        .errors
        .is_empty());
}

#[tokio::test]
async fn clients_can_read_which_features_are_on() {
    let h = harness();
    h.toggle(&h.admin, "audio-relay", false).await;
    let response = h
        .run_as(&h.member, "{ serverFeatures { id enabled } }")
        .await;
    let data: Value = response.data.into_json().unwrap();
    let relay = data["serverFeatures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "audio-relay")
        .unwrap();
    assert_eq!(relay["enabled"], false);
}

#[tokio::test]
async fn the_admin_list_marks_what_can_be_switched() {
    let h = harness();
    let response = h
        .run_as(&h.admin, "{ plugins { id toggleable isEnabled } }")
        .await;
    let data: Value = response.data.into_json().unwrap();
    let plugins = data["plugins"].as_array().unwrap();
    let find = |id: &str| plugins.iter().find(|p| p["id"] == id).unwrap().clone();
    assert_eq!(find("friend-presence")["toggleable"], true);
    assert_eq!(find("wanda-android")["toggleable"], false);
}

/// Presence is the costliest push on a busy server; off means nothing is sent at all.
#[tokio::test]
async fn presence_off_sends_friends_nothing() {
    let h = harness();
    h.db.create_account("beta", "beta-pass", Role::Member, AccountState::Active)
        .unwrap();
    assert!(h.db.send_friend_request("beta", "alpha").unwrap());
    assert!(h.db.accept_friend_request("alpha", "beta").unwrap());
    h.db.set_visibility("alpha", true, false).unwrap();

    // On, the friend hears about it — so silence below means the switch, not a broken setup.
    assert!(h.heartbeat_reaches("beta").await);
    h.toggle(&h.admin, "friend-presence", false).await;
    assert!(!h.heartbeat_reaches("beta").await);
}
