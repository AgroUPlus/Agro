//! Access boundary tests.

use super::*;
use crate::playlist_visibility::PlaylistVisibility;
use serde_json::Value;

#[tokio::test]
pub(super) async fn a_stale_edit_names_its_code_and_the_current_revision() {
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
pub(super) async fn a_private_playlist_and_a_missing_one_answer_identically() {
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
pub(super) async fn a_viewer_cannot_edit_and_a_contributor_cannot_remove_others_tracks() {
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
