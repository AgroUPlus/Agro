//! Visibility boundary tests.

use super::*;
use crate::db_identity::AccountState;

/// The headline case: friendship on its own reveals nothing.
#[tokio::test]
pub(super) async fn a_friend_sees_nothing_until_the_flag_is_set() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.set_playing("alpha", "A Secret Song");

    let closed = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert_allowed(&closed, "friendsNowPlaying");
    assert_eq!(
        closed.data.to_string(),
        r#"{friendsNowPlaying: []}"#,
        "a friend's playback leaked with show_now_playing off"
    );

    h.db.set_visibility("alpha", true, false).unwrap();
    let open = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert!(
        open.data.to_string().contains("A Secret Song"),
        "the flag was set and the friend still saw nothing: {:?}",
        open.data
    );
}

#[tokio::test]
pub(super) async fn a_stranger_never_sees_now_playing_however_open_the_flag() {
    let h = harness();
    h.db.set_visibility("alpha", true, true).unwrap();
    h.set_playing("alpha", "A Secret Song");

    let r = h
        .run_as(&h.stranger, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert_eq!(r.data.to_string(), r#"{friendsNowPlaying: []}"#);

    let along = h
        .run_as(
            &h.stranger,
            r#"mutation { startListenAlong(host: "alpha") { host } }"#,
        )
        .await;
    assert_refused(&along, "startListenAlong on a non-friend");
}

#[tokio::test]
pub(super) async fn stats_are_gated_separately_from_now_playing() {
    let h = harness();
    h.befriend("alpha", "beta");
    // Now playing open, stats closed. One flag must not imply the other.
    h.db.set_visibility("alpha", true, false).unwrap();

    let r = h
        .run_as(&h.beta, r#"{ tasteMatch(username: "alpha") { score } }"#)
        .await;
    assert_refused(&r, "tasteMatch with show_stats off");

    h.db.set_visibility("alpha", true, true).unwrap();
    let allowed = h
        .run_as(&h.beta, r#"{ tasteMatch(username: "alpha") { score } }"#)
        .await;
    assert_allowed(&allowed, "tasteMatch with show_stats on");
}

/// The refusals must be one refusal. Anything else turns an error into a lookup service.
#[tokio::test]
pub(super) async fn a_hidden_account_and_a_missing_one_answer_identically() {
    let h = harness();
    h.db.set_visibility("alpha", false, false).unwrap();

    let hidden = h
        .run_as(
            &h.stranger,
            r#"{ tasteMatch(username: "alpha") { score } }"#,
        )
        .await;
    let missing = h
        .run_as(
            &h.stranger,
            r#"{ tasteMatch(username: "ghost") { score } }"#,
        )
        .await;

    assert_refused(&hidden, "tasteMatch on a hidden account");
    assert_refused(&missing, "tasteMatch on a nonexistent account");
    assert_eq!(
        hidden.errors[0].message, missing.errors[0].message,
        "the error message tells a hidden account apart from a missing one"
    );
}

// ── The directory ───────────────────────────────────────────────────────────────────────────

#[tokio::test]
pub(super) async fn search_lists_only_accounts_that_asked_to_be_listed() {
    let h = harness();
    h.db.set_discoverable("beta", false).unwrap();

    let r = h
        .run_as(&h.alpha, r#"{ searchUsers(query: "b") { username } }"#)
        .await;
    assert_allowed(&r, "searchUsers");
    assert!(
        !r.data.to_string().contains("beta"),
        "a non-discoverable account appeared in the directory: {:?}",
        r.data
    );
}

#[tokio::test]
pub(super) async fn search_never_lists_a_pending_or_suspended_account() {
    let h = harness();
    h.db.set_account_state("beta", AccountState::Pending)
        .unwrap();
    let pending = h
        .run_as(&h.alpha, r#"{ searchUsers(query: "beta") { username } }"#)
        .await;
    assert!(
        !pending.data.to_string().contains("beta"),
        "a pending account was listed"
    );

    h.db.set_account_state("beta", AccountState::Suspended)
        .unwrap();
    let suspended = h
        .run_as(&h.alpha, r#"{ searchUsers(query: "beta") { username } }"#)
        .await;
    assert!(
        !suspended.data.to_string().contains("beta"),
        "a suspended account was listed"
    );
}

/// Prefix-anchored, so the directory cannot be walked one letter at a time.
#[tokio::test]
pub(super) async fn search_does_not_match_the_middle_of_a_username() {
    let h = harness();
    let r = h
        .run_as(&h.alpha, r#"{ searchUsers(query: "trang") { username } }"#)
        .await;
    assert!(
        !r.data.to_string().contains("stranger"),
        "a substring matched, which allows enumeration: {:?}",
        r.data
    );
}

// ── Blocking ────────────────────────────────────────────────────────────────────────────────
