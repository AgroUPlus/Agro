//! What shared and collaborative playlists must never let through, tested over GraphQL.
//!
//! The database tests prove the rules; these prove the API does not route around them: that an
//! id cannot be probed, that a push carries nothing but an id and a revision, and that a stale
//! edit comes back with a code a client can act on.

#![cfg(test)]

use std::sync::Arc;

use async_graphql::{Request, Schema};
use serde_json::Value;

use crate::auth::{AuthedUser, SetupToken};
use crate::db::Db;
use crate::db_identity::{Account, AccountState, Role};
use crate::playlist_visibility::PlaylistVisibility;
use crate::schema::{AgroSchema, Mutation, Query};
use crate::storage::Storage;
use crate::ws::WsHub;

struct Harness {
    schema: AgroSchema,
    db: Db,
    hub: Arc<WsHub>,
    alpha: Account,
    beta: Account,
    stranger: Account,
}

fn harness() -> Harness {
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
    let (alpha, beta, stranger) = (account("alpha"), account("beta"), account("stranger"));
    assert!(db.send_friend_request("beta", "alpha").unwrap());
    assert!(db.accept_friend_request("alpha", "beta").unwrap());

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
        alpha,
        beta,
        stranger,
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

    fn playlist(&self, visibility: PlaylistVisibility) -> String {
        self.db
            .create_playlist("alpha", "Mix", None, visibility)
            .unwrap()
            .id
    }
}

fn data(response: async_graphql::Response) -> Value {
    assert!(
        response.errors.is_empty(),
        "unexpected errors: {:?}",
        response.errors
    );
    response.data.into_json().unwrap()
}

const ADD: &str = r#"add: { track: { title: "Song", artist: "Artist" } }"#;

#[tokio::test]
async fn a_stale_edit_names_its_code_and_the_current_revision() {
    let h = harness();
    let id = h.playlist(PlaylistVisibility::Friends);
    h.run_as(
        &h.alpha,
        &format!(
            r#"mutation {{ updatePlaylistEditAccess(playlistId: "{id}", editAccess: FRIENDS) }}"#
        ),
    )
    .await;

    let first = format!(
        r#"mutation {{ applyPlaylistEdits(playlistId: "{id}", baseRevision: 1, edits: [{{ {ADD} }}]) {{ revision }} }}"#
    );
    assert_eq!(
        data(h.run_as(&h.beta, &first).await)["applyPlaylistEdits"]["revision"],
        2
    );

    let stale = h.run_as(&h.alpha, &first).await;
    let error = stale.errors.first().expect("a stale edit must be refused");
    let ext = error.extensions.as_ref().expect("with extensions");
    assert_eq!(
        ext.get("code").unwrap().clone().into_json().unwrap(),
        "STALE_REVISION"
    );
    assert_eq!(
        ext.get("currentRevision")
            .unwrap()
            .clone()
            .into_json()
            .unwrap(),
        2
    );
}

#[tokio::test]
async fn a_private_playlist_and_a_missing_one_answer_identically() {
    let h = harness();
    let private = h.playlist(PlaylistVisibility::Private);
    let missing = "00000000-0000-0000-0000-000000000000";

    for id in [private.as_str(), missing] {
        let follow = h
            .run_as(
                &h.stranger,
                &format!(r#"mutation {{ followPlaylist(id: "{id}") {{ id }} }}"#),
            )
            .await;
        assert_eq!(
            follow.errors[0].message, "playlist not found",
            "follow {id}"
        );

        let edit = h
            .run_as(&h.stranger, &format!(r#"mutation {{ applyPlaylistEdits(playlistId: "{id}", baseRevision: 0, edits: [{{ {ADD} }}]) {{ id }} }}"#))
            .await;
        assert_eq!(edit.errors[0].message, "playlist not found", "edit {id}");
    }

    let revisions = data(
        h.run_as(&h.stranger, &format!(r#"{{ playlistRevisions(ids: ["{private}", "{missing}"]) {{ revision accessible }} }}"#))
            .await,
    );
    for entry in revisions["playlistRevisions"].as_array().unwrap() {
        assert_eq!(entry["accessible"], false);
        assert_eq!(entry["revision"], Value::Null);
    }
}

#[tokio::test]
async fn a_viewer_cannot_edit_and_a_contributor_cannot_remove_others_tracks() {
    let h = harness();
    let id = h.playlist(PlaylistVisibility::Public);
    let edit = format!(
        r#"mutation {{ applyPlaylistEdits(playlistId: "{id}", baseRevision: 0, edits: [{{ {ADD} }}]) {{ revision }} }}"#
    );
    assert!(h.run_as(&h.stranger, &edit).await.errors[0]
        .message
        .starts_with("Forbidden"));

    h.run_as(
        &h.alpha,
        &format!(
            r#"mutation {{ updatePlaylistEditAccess(playlistId: "{id}", editAccess: PUBLIC) }}"#
        ),
    )
    .await;
    let added = data(
        h.run_as(
            &h.alpha,
            &edit.replace("baseRevision: 0", "baseRevision: 1"),
        )
        .await,
    );
    assert_eq!(added["applyPlaylistEdits"]["revision"], 2);
    let item = h.db.get_playlist_items(&id).unwrap()[0].id.clone();

    let remove = format!(
        r#"mutation {{ applyPlaylistEdits(playlistId: "{id}", baseRevision: 2, edits: [{{ remove: "{item}" }}]) {{ revision }} }}"#
    );
    assert!(h.run_as(&h.stranger, &remove).await.errors[0]
        .message
        .starts_with("Forbidden"));
    assert_eq!(h.db.get_playlist_items(&id).unwrap().len(), 1);
}

#[tokio::test]
async fn a_push_reaches_owner_and_followers_and_carries_only_id_and_revision() {
    let h = harness();
    let id = h.playlist(PlaylistVisibility::Friends);
    data(
        h.run_as(
            &h.beta,
            &format!(r#"mutation {{ followPlaylist(id: "{id}") {{ id }} }}"#),
        )
        .await,
    );
    let mut rx = h.hub.tx.subscribe();

    h.run_as(
        &h.alpha,
        &format!(
            r#"mutation {{ updatePlaylistVisibility(playlistId: "{id}", visibility: PUBLIC) }}"#
        ),
    )
    .await;

    let mut recipients = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        assert_eq!(msg.msg_type, "PLAYLIST_UPDATED");
        let keys: Vec<&String> = msg.payload.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            vec!["id", "revision"],
            "nothing but an id and a revision"
        );
        assert_eq!(msg.payload["id"], id.as_str());
        recipients.push(msg.user_id.unwrap());
    }
    recipients.sort();
    assert_eq!(recipients, vec!["alpha", "beta"]);
}

#[tokio::test]
async fn a_follower_who_loses_access_sees_it_as_revoked_not_its_contents() {
    let h = harness();
    let id = h.playlist(PlaylistVisibility::Friends);
    data(
        h.run_as(
            &h.beta,
            &format!(r#"mutation {{ followPlaylist(id: "{id}") {{ id }} }}"#),
        )
        .await,
    );
    h.db.update_playlist_visibility(&id, "alpha", PlaylistVisibility::Private)
        .unwrap();

    let followed = data(
        h.run_as(
            &h.beta,
            "{ followedPlaylists { playlists { id } revokedIds } }",
        )
        .await,
    );
    assert_eq!(
        followed["followedPlaylists"]["playlists"],
        serde_json::json!([])
    );
    assert_eq!(
        followed["followedPlaylists"]["revokedIds"],
        serde_json::json!([id])
    );
}
