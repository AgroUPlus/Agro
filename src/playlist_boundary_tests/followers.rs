//! Followers boundary tests.

use super::*;
use crate::playlist_visibility::PlaylistVisibility;
use serde_json::Value;

#[tokio::test]
pub(super) async fn a_push_reaches_owner_and_followers_and_carries_only_id_and_revision() {
    let h = harness();
    let id = h.playlist(PlaylistVisibility::Friends);
    data(
        h.run_as(
            &h.beta,
            &format!(r#"mutation {{ followPlaylist(id: "{id}") {{ id }} }}"#),
        )
        .await,
    );
    let mut rx = h.hub.channels.tap.subscribe();

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
pub(super) async fn a_follower_who_loses_access_sees_it_as_revoked_not_its_contents() {
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

/// A follow made in the dashboard has to reach the follower's phone, which is not the device that
/// made it. Told to the follower alone: nobody else learns who follows what from this.
#[tokio::test]
pub(super) async fn following_and_unfollowing_reach_the_followers_own_devices() {
    let h = harness();
    let id = h.playlist(PlaylistVisibility::Friends);
    let pushes = |tap: &mut tokio::sync::broadcast::Receiver<crate::ws::WsMessage>| {
        let mut seen = Vec::new();
        while let Ok(msg) = tap.try_recv() {
            if msg.msg_type == "PLAYLIST_FOLLOW" {
                seen.push((msg.user_id.clone(), msg.payload["following"].clone()));
            }
        }
        seen
    };

    let mut tap = h.hub.channels.tap.subscribe();
    data(
        h.run_as(
            &h.beta,
            &format!(r#"mutation {{ followPlaylist(id: "{id}") {{ id }} }}"#),
        )
        .await,
    );
    assert_eq!(
        pushes(&mut tap),
        [(Some("beta".to_string()), Value::Bool(true))]
    );

    data(
        h.run_as(
            &h.beta,
            &format!(r#"mutation {{ unfollowPlaylist(id: "{id}") }}"#),
        )
        .await,
    );
    assert_eq!(
        pushes(&mut tap),
        [(Some("beta".to_string()), Value::Bool(false))]
    );

    // Unfollowing what was not followed changes nothing, and says nothing.
    data(
        h.run_as(
            &h.beta,
            &format!(r#"mutation {{ unfollowPlaylist(id: "{id}") }}"#),
        )
        .await,
    );
    assert!(pushes(&mut tap).is_empty());
}
