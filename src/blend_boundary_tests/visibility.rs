//! Visibility boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn only_accepted_friends_can_be_asked() {
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
pub(super) async fn nobody_is_read_before_they_accept_and_only_members_can_open_it() {
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
pub(super) async fn accepting_needs_stats_shared() {
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
