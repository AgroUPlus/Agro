//! Lifecycle boundary tests.

use super::*;
use serde_json::Value;

#[tokio::test]
pub(super) async fn nobody_edits_a_blend_by_hand_not_even_its_creator() {
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
pub(super) async fn leaving_takes_you_out_and_the_creator_leaving_ends_it() {
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
pub(super) async fn it_is_written_only_once_everyone_asked_has_answered() {
    let h = harness();
    let made = h
        .ok(
            &h.alpha,
            r#"mutation { createBlend(title: "Three", members: ["beta", "gamma"], size: 25,
                 mix: 50, window: ALL_TIME, refresh: WEEKLY) { playlistId } }"#,
        )
        .await;
    let id = made["createBlend"]["playlistId"]
        .as_str()
        .unwrap()
        .to_string();
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

#[test]
pub(super) fn a_blend_written_before_everyone_answered_is_emptied_until_they_have() {
    let h = harness();
    let blend =
        h.db.create_blend(
            "alpha",
            "Early",
            &["beta".to_string()],
            crate::blend_recipe::BlendSettings {
                size: 25,
                mix: 50,
                window: crate::blend_recipe::BlendWindow::AllTime,
                refresh: crate::blend_recipe::BlendRefresh::Weekly,
            },
        )
        .unwrap();
    // What a server before this rule left behind: alpha's half, stamped as written this week.
    {
        let conn = h.db.conn.lock().unwrap();
        crate::db_playlist_items::insert_item(
            &conn,
            &blend.id,
            0,
            "alpha",
            &crate::db_playlist_items::NewPlaylistItem {
                title: "alpha song 0".into(),
                artist: "alpha artist 0".into(),
                album: None,
                duration_ms: None,
                artwork_url: None,
                origin_uri: None,
            },
        )
        .unwrap();
        conn.execute(
            "UPDATE blends SET refreshed_at = ?2 WHERE playlist_id = ?1",
            rusqlite::params![blend.id, chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();
    }

    assert!(
        h.db.refresh_blend_if_due(&blend.id).unwrap().is_some(),
        "members are told"
    );
    assert!(h.db.get_playlist_items(&blend.id).unwrap().is_empty());
    assert!(
        h.db.refresh_blend_if_due(&blend.id).unwrap().is_none(),
        "nothing left to remove"
    );
}
