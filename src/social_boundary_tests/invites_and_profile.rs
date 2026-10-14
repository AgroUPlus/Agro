//! Invites and profile boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn a_member_cannot_mint_or_read_invites() {
    let h = harness();
    let minted = h
        .run_as(&h.beta, "mutation { createInvite { code } }")
        .await;
    assert_refused(&minted, "createInvite as a member");

    let listed = h.run_as(&h.beta, "{ invites { code } }").await;
    assert_refused(&listed, "invites as a member");

    let queue = h.run_as(&h.beta, "{ pendingAccounts { username } }").await;
    assert_refused(&queue, "pendingAccounts as a member");
}

#[tokio::test]
pub(super) async fn an_invite_cannot_be_spent_more_often_than_it_allows() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 1, None).unwrap();

    assert!(
        h.db.redeem_invite(&invite.code).unwrap(),
        "the first use was refused"
    );
    assert!(
        !h.db.redeem_invite(&invite.code).unwrap(),
        "a single-use code was spent twice"
    );
}

#[tokio::test]
pub(super) async fn a_revoked_invite_stops_working() {
    let h = harness();
    let invite = h.db.create_invite("alpha", 10, None).unwrap();
    assert!(h.db.revoke_invite(&invite.code).unwrap());
    assert!(
        !h.db.redeem_invite(&invite.code).unwrap(),
        "a revoked code was spent"
    );
}

// ── Profile writes ──────────────────────────────────────────────────────────────────────────

/// An avatar URL is rendered in someone else's client, so the scheme is an allowlist.
#[tokio::test]
pub(super) async fn an_avatar_must_be_an_http_url() {
    let h = harness();
    for hostile in ["javascript:alert(1)", "data:text/html,<script>"] {
        let r = h
            .run_as(
                &h.alpha,
                &format!(r#"mutation {{ updateProfile(avatarUrl: "{hostile}") {{ username }} }}"#),
            )
            .await;
        assert_refused(&r, hostile);
    }
}

#[tokio::test]
pub(super) async fn nobody_can_edit_anyone_elses_profile() {
    let h = harness();
    // There is deliberately no `username` argument: the mutation writes the caller and nothing
    // else, so there is no field through which another account could be named.
    let r = h
        .run_as(
            &h.beta,
            r#"mutation { updateProfile(displayName: "Beta") { username } }"#,
        )
        .await;
    assert_allowed(&r, "updateProfile");
    assert_eq!(r.data.to_string(), "{updateProfile: {username: \"beta\"}}");
}

/// A switch left out of the call is a switch left alone.
///
/// The three flags are independent decisions. When a call had to carry all three, a client
/// flipping one had to resend its idea of the other two — and a stale copy sent from a second
/// device would quietly turn back off something the first had just turned on. A privacy switch
/// that reverts itself is worse than one that is hard to reach.
#[tokio::test]
pub(super) async fn setting_one_visibility_switch_leaves_the_others_alone() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();

    let response = h
        .run_as(
            &h.alpha,
            r#"mutation { setVisibility(discoverable: false) { showNowPlaying showStats discoverable } }"#,
        )
        .await;
    assert_allowed(&response, "partial setVisibility");

    let profile = h.db.profile("alpha").unwrap().unwrap();
    assert!(
        profile.show_now_playing,
        "now-playing was reverted by an unrelated call"
    );
    assert!(
        profile.show_stats,
        "stats were reverted by an unrelated call"
    );
    assert!(
        !profile.discoverable,
        "the switch that was actually set did not take"
    );
}

/// Revoking one device must not sign out every device that shares its name.
///
/// Labels are chosen by the client and repeat constantly — a client that logs in again on each
/// launch leaves a row per launch, all named the same. Revocation used to match on that label, so
/// removing "the" desktop token removed every one of them. Per-device revocation that cannot
/// address a single device is not per-device revocation.
#[tokio::test]
pub(super) async fn revoking_one_credential_leaves_its_namesakes_alone() {
    let h = harness();
    for _ in 0..3 {
        h.db.mint_device_token("alpha", "wander-desktop").unwrap();
    }
    let before = h.db.list_app_passwords("alpha").unwrap();
    assert_eq!(before.len(), 3, "expected three same-named credentials");

    let response = h
        .run_as(
            &h.alpha,
            &format!(
                r#"mutation {{ revokeAppPassword(userId: "alpha", id: {}) }}"#,
                before[0].id
            ),
        )
        .await;
    assert_allowed(&response, "revoke one credential");

    let after = h.db.list_app_passwords("alpha").unwrap();
    assert_eq!(
        after.len(),
        2,
        "revoking one token took its namesakes with it"
    );
    assert!(
        !after.iter().any(|record| record.id == before[0].id),
        "the credential that was named is the one that should be gone"
    );
}

/// An id is just a number, so it must not be enough on its own.
#[tokio::test]
pub(super) async fn nobody_can_revoke_a_credential_belonging_to_someone_else() {
    let h = harness();
    h.db.mint_device_token("alpha", "laptop").unwrap();
    let target = h.db.list_app_passwords("alpha").unwrap()[0].id;

    let response = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ revokeAppPassword(userId: "alpha", id: {target}) }}"#),
        )
        .await;
    assert_refused(&response, "beta revoking alpha's credential");
    assert_eq!(
        h.db.list_app_passwords("alpha").unwrap().len(),
        1,
        "the credential was deleted despite the refusal"
    );
}
