//! What a Blend must never let through, tested over GraphQL.
//!
//! A blend is made of people's listening, so its rules are consent rules: nobody is read before
//! they accept, nobody outside it can open it, and nobody — its creator included — can write over
//! what Agro writes.

#![cfg(test)]

use std::sync::Arc;

use async_graphql::{Request, Schema};
use serde_json::Value;

use crate::auth::{AuthedUser, SetupToken};
use crate::db::{Db, ScrobbleEntry};
use crate::db_identity::{Account, AccountState, Role};
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;

pub(crate) struct Harness {
    schema: AgroSchema,
    pub(crate) db: Db,
    pub(crate) alpha: Account,
    pub(crate) beta: Account,
    /// alpha's friend, never asked into anything.
    gamma: Account,
    stranger: Account,
}

pub(crate) fn harness() -> Harness {
    let db = Db::new_in_memory().unwrap();
    let account = |name: &str| {
        db.create_account(
            name,
            &format!("{name}-pass"),
            Role::Member,
            AccountState::Active,
        )
        .unwrap()
    };
    let (alpha, beta, gamma, stranger) = (
        account("alpha"),
        account("beta"),
        account("gamma"),
        account("stranger"),
    );
    for friend in ["beta", "gamma"] {
        assert!(db.send_friend_request(friend, "alpha").unwrap());
        assert!(db.accept_friend_request("alpha", friend).unwrap());
    }
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("UPDATE users SET show_stats = 1", []).unwrap();
    }
    for who in ["alpha", "beta", "gamma"] {
        let plays: Vec<ScrobbleEntry> = (0..5)
            .map(|i| ScrobbleEntry {
                track_title: format!("{who} song {i}"),
                artist_name: format!("{who} artist {i}"),
                album_name: None,
                genre: None,
                duration_secs: 200,
                played_at: chrono::Utc::now().to_rfc3339(),
                play_uid: Some(format!("{who}-{i}")),
            })
            .collect();
        db.record_scrobbles(who, "phone", None, &plays).unwrap();
    }
    let schema = Schema::build(
        Query::default(),
        Mutation::default(),
        async_graphql::EmptySubscription,
    )
    .data(db.clone())
    .data(Arc::new(WsHub::new()))
    .data(Storage::for_tests())
    .data(SetupToken::for_fresh_server(1))
    .finish();
    Harness {
        schema,
        db,
        alpha,
        beta,
        gamma,
        stranger,
    }
}

impl Harness {
    async fn run(&self, account: &Account, query: &str) -> async_graphql::Response {
        let request = Request::new(query).data(AuthedUser {
            account: account.clone(),
            device_label: String::new(),
            token_hash: String::new(),
        });
        self.schema.execute(request).await
    }

    pub(crate) async fn ok(&self, account: &Account, query: &str) -> Value {
        let response = self.run(account, query).await;
        assert!(response.errors.is_empty(), "refused: {:?}", response.errors);
        response.data.into_json().unwrap()
    }

    async fn refused(&self, account: &Account, query: &str, why: &str) {
        let response = self.run(account, query).await;
        assert!(!response.errors.is_empty(), "{why}: it was allowed");
    }

    /// alpha makes a blend asking beta, and returns its id.
    async fn blend(&self) -> String {
        let made = self
            .ok(
                &self.alpha,
                r#"mutation { createBlend(title: "Us", members: ["beta"], size: 25, mix: 50,
                     window: ALL_TIME, refresh: WEEKLY) { playlistId } }"#,
            )
            .await;
        made["createBlend"]["playlistId"]
            .as_str()
            .unwrap()
            .to_string()
    }
}

fn open_query(id: &str) -> String {
    format!(r#"{{ playlist(id: "{id}") {{ isBlend myRole items {{ title addedBy }} }} }}"#)
}

#[tokio::test]
async fn only_accepted_friends_can_be_asked() {
    let h = harness();
    h.refused(
        &h.alpha,
        r#"mutation { createBlend(title: "x", members: ["stranger"], size: 25, mix: 50,
             window: ALL_TIME, refresh: WEEKLY) { playlistId } }"#,
        "asking a stranger",
    )
    .await;
    h.refused(
        &h.alpha,
        r#"mutation { createBlend(title: "x", members: ["beta"], size: 30, mix: 50,
             window: ALL_TIME, refresh: WEEKLY) { playlistId } }"#,
        "a size that is not offered",
    )
    .await;
}

#[tokio::test]
async fn nobody_is_read_before_they_accept_and_only_members_can_open_it() {
    let h = harness();
    let id = h.blend().await;

    // Invited is not joined: beta cannot open it, and nothing is written until beta answers —
    // not even alpha's half, which would only be rewritten the moment beta joined.
    h.refused(&h.beta, &open_query(&id), "an invitee opening it")
        .await;
    let invites = h.ok(&h.beta, "{ blendInvites { playlistId title } }").await;
    assert_eq!(invites["blendInvites"][0]["playlistId"], id.as_str());
    let before = h.ok(&h.alpha, &open_query(&id)).await;
    assert_eq!(
        before["playlist"]["items"].as_array().unwrap().len(),
        0,
        "written before everyone answered"
    );

    h.ok(
        &h.beta,
        &format!(r#"mutation {{ answerBlendInvite(playlistId: "{id}", accept: true) }}"#),
    )
    .await;
    let after = h.ok(&h.beta, &open_query(&id)).await;
    assert_eq!(after["playlist"]["isBlend"], true);
    assert_eq!(after["playlist"]["myRole"], "VIEWER");
    let items = after["playlist"]["items"].as_array().unwrap();
    assert!(
        items.iter().any(|i| i["addedBy"] == "beta"),
        "beta's listening is missing"
    );

    for outsider in [&h.gamma, &h.stranger] {
        h.refused(outsider, &open_query(&id), "a non-member opening it")
            .await;
        h.refused(
            outsider,
            &format!(r#"{{ blend(playlistId: "{id}") {{ title }} }}"#),
            "a non-member reading the recipe",
        )
        .await;
    }
}

#[tokio::test]
async fn accepting_needs_stats_shared() {
    let h = harness();
    let id = h.blend().await;
    h.db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE users SET show_stats = 0 WHERE username = 'beta'",
            [],
        )
        .unwrap();
    h.refused(
        &h.beta,
        &format!(r#"mutation {{ answerBlendInvite(playlistId: "{id}", accept: true) }}"#),
        "joining with stats hidden",
    )
    .await;
}

#[tokio::test]
async fn nobody_edits_a_blend_by_hand_not_even_its_creator() {
    let h = harness();
    let id = h.blend().await;
    let track = r#"track: { title: "Mine", artist: "Me" }"#;
    h.refused(
        &h.alpha,
        &format!(r#"mutation {{ addTrackToPlaylist(playlistId: "{id}", {track}) {{ id }} }}"#),
        "adding to a blend",
    )
    .await;
    h.refused(
        &h.alpha,
        &format!(
            r#"mutation {{ updatePlaylistVisibility(playlistId: "{id}", visibility: PUBLIC) }}"#
        ),
        "opening a blend to everyone",
    )
    .await;
    h.refused(
        &h.alpha,
        &format!(
            r#"mutation {{ updatePlaylistEditAccess(playlistId: "{id}", editAccess: PUBLIC) }}"#
        ),
        "letting others edit a blend",
    )
    .await;
}

#[tokio::test]
async fn leaving_takes_you_out_and_the_creator_leaving_ends_it() {
    let h = harness();
    let id = h.blend().await;
    h.ok(
        &h.beta,
        &format!(r#"mutation {{ answerBlendInvite(playlistId: "{id}", accept: true) }}"#),
    )
    .await;
    h.ok(
        &h.beta,
        &format!(r#"mutation {{ leaveBlend(playlistId: "{id}") }}"#),
    )
    .await;
    h.refused(&h.beta, &open_query(&id), "opening a blend you left")
        .await;

    h.ok(
        &h.alpha,
        &format!(r#"mutation {{ leaveBlend(playlistId: "{id}") }}"#),
    )
    .await;
    assert!(
        h.db.get_playlist(&id).unwrap().is_none(),
        "the blend outlived its creator"
    );
    assert!(
        h.db.blend(&id).unwrap().is_none(),
        "its recipe was left behind"
    );
}

#[tokio::test]
async fn it_is_written_only_once_everyone_asked_has_answered() {
    let h = harness();
    let made = h
        .ok(
            &h.alpha,
            r#"mutation { createBlend(title: "Three", members: ["beta", "gamma"], size: 25,
                 mix: 50, window: ALL_TIME, refresh: WEEKLY) { playlistId } }"#,
        )
        .await;
    let id = made["createBlend"]["playlistId"].as_str().unwrap().to_string();
    let count = |data: &Value| data["playlist"]["items"].as_array().unwrap().len();

    h.ok(
        &h.beta,
        &format!(r#"mutation {{ answerBlendInvite(playlistId: "{id}", accept: true) }}"#),
    )
    .await;
    assert_eq!(
        count(&h.ok(&h.alpha, &open_query(&id)).await),
        0,
        "gamma has not answered, so it would change again the moment they do"
    );

    // A decline is an answer too: it starts from whoever is in it.
    h.ok(
        &h.gamma,
        &format!(r#"mutation {{ answerBlendInvite(playlistId: "{id}", accept: false) }}"#),
    )
    .await;
    assert!(count(&h.ok(&h.alpha, &open_query(&id)).await) > 0);
}
