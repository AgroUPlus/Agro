//! Friendship boundary tests.

use super::*;

#[tokio::test]
pub(super) async fn a_block_hides_both_directions() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let blocked = h
        .run_as(&h.alpha, r#"mutation { blockUser(username: "beta") }"#)
        .await;
    assert_allowed(&blocked, "blockUser");

    // The blocked account loses the friendship, and with it everything the friendship permitted.
    let theirs = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert_eq!(theirs.data.to_string(), r#"{friendsNowPlaying: []}"#);

    // And neither can find the other again.
    let search = h
        .run_as(&h.beta, r#"{ searchUsers(query: "alpha") { username } }"#)
        .await;
    assert!(
        !search.data.to_string().contains("alpha"),
        "a blocker stayed findable by the account they blocked"
    );
}

/// A block is never disclosed as a block — from the other side it is indistinguishable from
/// never having been connected at all.
#[tokio::test]
pub(super) async fn a_block_is_not_reported_to_the_blocked_account() {
    let h = harness();
    h.db.block_user("alpha", "beta").unwrap();

    let r = h
        .run_as(&h.beta, r#"{ profile(username: "alpha") { friendState } }"#)
        .await;
    assert_allowed(&r, "profile");
    assert!(
        !r.data.to_string().contains("blocked"),
        "the block was disclosed to the account it was applied to: {:?}",
        r.data
    );
}

// ── Requests ────────────────────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn a_request_must_be_accepted_before_anything_is_shared() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let sent = h
        .run_as(
            &h.beta,
            r#"mutation { sendFriendRequest(username: "alpha") }"#,
        )
        .await;
    assert_allowed(&sent, "sendFriendRequest");

    // Pending is not accepted.
    let r = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert_eq!(
        r.data.to_string(),
        r#"{friendsNowPlaying: []}"#,
        "an unanswered request behaved like a friendship"
    );
}

#[tokio::test]
pub(super) async fn only_the_addressee_can_accept_a_request() {
    let h = harness();
    assert!(h.db.send_friend_request("beta", "alpha").unwrap());

    // The sender cannot accept their own request, and neither can a bystander.
    let by_sender = h
        .run_as(
            &h.beta,
            r#"mutation { acceptFriendRequest(username: "alpha") }"#,
        )
        .await;
    assert_eq!(by_sender.data.to_string(), "{acceptFriendRequest: false}");

    let by_bystander = h
        .run_as(
            &h.stranger,
            r#"mutation { acceptFriendRequest(username: "beta") }"#,
        )
        .await;
    assert_eq!(
        by_bystander.data.to_string(),
        "{acceptFriendRequest: false}"
    );

    assert!(!h.db.are_friends("alpha", "beta").unwrap());
}

// ── Listen along ────────────────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn a_session_ends_when_the_friendship_does() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let started = h
        .run_as(
            &h.beta,
            r#"mutation { startListenAlong(host: "alpha") { host } }"#,
        )
        .await;
    assert_allowed(&started, "startListenAlong");

    h.db.remove_friend("alpha", "beta").unwrap();

    // The row may still be there, but it must stop resolving and stop being fanned out to.
    let after = h.run_as(&h.beta, "{ listenAlong { host } }").await;
    assert_eq!(after.data.to_string(), "{listenAlong: null}");
    assert!(h.db.listeners_of("alpha").unwrap().is_empty());
}

/// Turning now-playing off has to end sessions that are already running, not merely stop new ones.
#[tokio::test]
pub(super) async fn closing_now_playing_ends_the_sessions_it_permitted() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");
    h.run_as(
        &h.beta,
        r#"mutation { startListenAlong(host: "alpha") { host } }"#,
    )
    .await;

    let closed = h
        .run_as(
            &h.alpha,
            "mutation { setVisibility(showNowPlaying: false, showStats: false, discoverable: true) { username } }",
        )
        .await;
    assert_allowed(&closed, "setVisibility");

    assert!(
        h.db.listen_along_of("beta").unwrap().is_none(),
        "a session survived the flag that permitted it being turned off"
    );
}

// ── Invites and the approval queue ──────────────────────────────────────────────────────────
