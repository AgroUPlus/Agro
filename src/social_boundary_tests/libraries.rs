//! Libraries boundary tests.

use super::*;

/// `me` with no argument answers about the caller, never about a guess.
///
/// A client's first question is "who am I?", and it cannot name itself to ask. The dashboard used
/// to open with a hard-coded `alpha`; signed in as anyone else it asked `me(username: "alpha")`,
/// got a refusal, never corrected itself, and went on showing another account's name over the
/// session it actually had.
#[tokio::test]
pub(super) async fn me_without_an_argument_is_the_caller() {
    let h = harness();

    let response = h.run_as(&h.beta, r#"{ me { username role } }"#).await;
    assert_allowed(&response, "me with no argument");
    let answered = response.data.into_json().unwrap();
    assert_eq!(
        answered["me"]["username"], "beta",
        "me answered about somebody other than the caller"
    );
}

/// Naming someone else is still refused — the default must not become a way in.
#[tokio::test]
pub(super) async fn me_cannot_be_pointed_at_another_account() {
    let h = harness();
    h.befriend("alpha", "beta");

    let response = h
        .run_as(&h.beta, r#"{ me(username: "alpha") { username role } }"#)
        .await;
    assert_refused(&response, "beta asking about alpha through me");
}

// ── The library ─────────────────────────────────────────────────────────────────────────────

/// The operator's archive is not everybody's library.
///
/// `library_stats` and `library_browse` both scoped themselves as "tracks this account holds OR
/// anything archived on the server". The second half made every member's dashboard report the
/// whole of the admin's collection as their own — the track count, the byte total and a "100%
/// archived" bar describing music they had never had.
#[tokio::test]
pub(super) async fn a_member_does_not_inherit_the_servers_archive() {
    let h = harness();
    h.archive_a_track("Something The Admin Owns");

    let response = h
        .run_as(
            &h.beta,
            r#"{ libraryStats(userId: "beta") { trackCount archivedCount } }"#,
        )
        .await;
    assert_allowed(&response, "beta reading their own library stats");
    let answered = response.data.into_json().unwrap();
    assert_eq!(
        answered["libraryStats"]["trackCount"], 0,
        "the server's archive was counted as this member's own library"
    );
}

/// A library is private until it is shared, and shared only with friends.
#[tokio::test]
pub(super) async fn a_library_is_not_readable_until_it_is_shared() {
    let h = harness();

    // A stranger, with the switch off.
    let refused = h
        .run_as(
            &h.stranger,
            r#"{ libraryStats(userId: "alpha") { trackCount } }"#,
        )
        .await;
    assert_refused(&refused, "a stranger reading alpha's library");

    // A friend, with the switch still off.
    h.befriend("alpha", "beta");
    let still_refused = h
        .run_as(
            &h.beta,
            r#"{ libraryStats(userId: "alpha") { trackCount } }"#,
        )
        .await;
    assert_refused(
        &still_refused,
        "a friend reading a library that is not shared",
    );

    // The switch on, for that friend.
    h.db.set_share_library("alpha", true).unwrap();
    let allowed = h
        .run_as(
            &h.beta,
            r#"{ libraryStats(userId: "alpha") { trackCount } }"#,
        )
        .await;
    assert_allowed(&allowed, "a friend reading a shared library");

    // Still not for the stranger.
    let stranger_again = h
        .run_as(
            &h.stranger,
            r#"{ libraryStats(userId: "alpha") { trackCount } }"#,
        )
        .await;
    assert_refused(&stranger_again, "a stranger reading a shared library");
}

/// Sharing a library shows what the devices hold, never the server's archive.
#[tokio::test]
pub(super) async fn a_shared_library_does_not_expose_the_archive() {
    let h = harness();
    h.archive_a_track("Something The Admin Owns");
    h.befriend("alpha", "beta");
    h.db.set_share_library("alpha", true).unwrap();

    let response = h
        .run_as(
            &h.beta,
            r#"{ libraryStats(userId: "alpha") { trackCount } }"#,
        )
        .await;
    assert_allowed(&response, "friend reading a shared library");
    let answered = response.data.into_json().unwrap();
    assert_eq!(
        answered["libraryStats"]["trackCount"], 0,
        "a shared library handed over the server archive as well"
    );
}

/// Being an administrator is not a reason to read someone else's collection.
#[tokio::test]
pub(super) async fn an_admin_cannot_read_another_accounts_library() {
    let h = harness();
    let response = h
        .run_as(
            &h.alpha,
            r#"{ libraryStats(userId: "beta") { trackCount } }"#,
        )
        .await;
    assert_refused(&response, "an admin reading a member's library");
}

/// A friend's statistics are readable once they open them, and not before.
///
/// `listeningStats` was gated with `authorize`, which permits only the account itself — so
/// `showStats` was a switch with nothing behind it. Turning it on changed nothing, and a friend's
/// listening could not be read however open they set it.
#[tokio::test]
pub(super) async fn a_friends_statistics_follow_their_switch() {
    let h = harness();
    h.befriend("alpha", "beta");

    let closed = h
        .run_as(
            &h.beta,
            r#"{ listeningStats(userId: "alpha") { playsTotal } }"#,
        )
        .await;
    assert_refused(&closed, "reading a friend's stats with the switch off");

    h.db.set_visibility("alpha", false, true).unwrap();
    let opened = h
        .run_as(
            &h.beta,
            r#"{ listeningStats(userId: "alpha") { playsTotal } }"#,
        )
        .await;
    assert_allowed(&opened, "reading a friend's stats with the switch on");

    // A stranger is still refused, switch or no switch.
    let stranger = h
        .run_as(
            &h.stranger,
            r#"{ listeningStats(userId: "alpha") { playsTotal } }"#,
        )
        .await;
    assert_refused(&stranger, "a stranger reading open stats");
}

// ── Jam sessions ────────────────────────────────────────────────────────────────────────────
