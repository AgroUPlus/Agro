//! What Wanda asks of the Blend API, field for field, so the two cannot drift apart unnoticed.
#![cfg(test)]

use crate::blend_boundary_tests::harness;

/// Wanda's `AgroBlendApi` selections and its shared-playlist fields with `isBlend`, field for field.
#[tokio::test]
async fn the_server_answers_what_wanda_asks() {
    let h = harness();
    let fields = "playlistId title createdBy isCreator size mix window refresh nextRefreshAt
        members { username joined }";
    let made = h
        .ok(
            &h.alpha,
            &format!(
                r#"mutation {{ createBlend(title: "Us", members: ["beta"], size: 50, mix: 60,
                     window: SIX_MONTHS, refresh: WEEKLY) {{ {fields} }} }}"#
            ),
        )
        .await;
    let id = made["createBlend"]["playlistId"]
        .as_str()
        .unwrap()
        .to_string();
    h.ok(&h.beta, &format!("{{ blendInvites {{ {fields} }} }}"))
        .await;
    h.ok(
        &h.alpha,
        &format!(r#"{{ blend(playlistId: "{id}") {{ {fields} }} }}"#),
    )
    .await;
    h.ok(
        &h.alpha,
        &format!(
            r#"{{ playlist(id: "{id}") {{ id userId title description visibility editAccess myRole
                 revision isFollowing items {{ id title artist album durationMs addedBy addedAt }}
                 isBlend }} }}"#
        ),
    )
    .await;
    h.ok(
        &h.alpha,
        &format!(
            r#"mutation {{ updateBlend(playlistId: "{id}", title: "Us two", size: 25, mix: 0,
                 window: FOUR_WEEKS, refresh: FROZEN) {{ {fields} }} }}"#
        ),
    )
    .await;
}
