//! Jam skip boundary tests.

use super::*;
use crate::ws::WsHub;

/// Enough of the room asking retires the track at once, rather than at the end of its duration.
#[tokio::test]
pub(super) async fn a_majority_skips_the_playing_track() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.join_jam(&jam.id, "stranger").unwrap();

    let (first, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "Skip me",
            "A",
            None,
            600_000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();
    h.db.add_jam_track(
        &jam.id,
        "alpha",
        "u:2",
        "Next",
        "A",
        None,
        600_000,
        false,
        JamMode::Open,
        None,
        None,
    )
    .unwrap();
    crate::jam_clock::tick(&h.db, &hub);
    assert_eq!(
        h.db.jam_by_id(&jam.id)
            .unwrap()
            .unwrap()
            .now_playing_id
            .as_deref(),
        Some(first.as_str())
    );

    // Three members, so two are needed.
    assert_eq!(h.db.jam_skips_needed(&jam.id).unwrap(), 2);
    assert!(
        !h.db.vote_skip(&jam.id, &first, "alpha").unwrap(),
        "one vote skipped it"
    );
    // The same person again is not a second voice.
    assert!(
        !h.db.vote_skip(&jam.id, &first, "alpha").unwrap(),
        "a repeated vote counted twice"
    );
    assert!(
        h.db.vote_skip(&jam.id, &first, "beta").unwrap(),
        "two of three did not carry it"
    );

    let skipped = h
        .run_as(
            &h.beta,
            "mutation { voteSkipJamTrack { nowPlaying { title } } }",
        )
        .await;
    assert_allowed(&skipped, "voting to skip");

    // The clock takes the next one, and the skipped track does not come back.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    let now = h.db.jam_now_playing(&live).unwrap().unwrap();
    assert_eq!(now.title, "Next", "the room stayed on the skipped track");
}

/// A jam is private until its creator opens it, and open only to friends.
#[tokio::test]
pub(super) async fn only_friends_see_a_jam_that_was_opened_up() {
    let h = harness();
    h.befriend("alpha", "beta");
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();

    // Closed by default: a friend cannot see it.
    assert!(
        h.db.friend_jams("beta").unwrap().is_empty(),
        "a code-only jam was advertised"
    );

    h.db.set_jam_visibility(&jam.id, crate::db_jam::JamVisibility::Friends)
        .unwrap();
    let seen = h.db.friend_jams("beta").unwrap();
    assert_eq!(seen.len(), 1, "an opened jam was not offered to a friend");

    // A stranger is not a friend, however open it is.
    assert!(
        h.db.friend_jams("stranger").unwrap().is_empty(),
        "an opened jam was shown to somebody who is not a friend"
    );

    // And joining without the code works for the friend, not the stranger.
    let refused = h
        .run_as(
            &h.stranger,
            &format!(
                r#"mutation {{ joinFriendJam(jamId: "{}") {{ id }} }}"#,
                jam.id
            ),
        )
        .await;
    assert_refused(&refused, "a stranger joining a friend-only jam");

    let allowed = h
        .run_as(
            &h.beta,
            &format!(
                r#"mutation {{ joinFriendJam(jamId: "{}") {{ id }} }}"#,
                jam.id
            ),
        )
        .await;
    assert_allowed(&allowed, "a friend joining an opened jam");
}

/// Only the creator opens a jam up.
#[tokio::test]
pub(super) async fn a_member_cannot_open_the_jam_to_their_own_friends() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let refused = h
        .run_as(
            &h.beta,
            r#"mutation { setJamVisibility(visibility: "friends") { visibility } }"#,
        )
        .await;
    assert_refused(&refused, "a member opening the jam up");

    let allowed = h
        .run_as(
            &h.alpha,
            r#"mutation { setJamVisibility(visibility: "friends") { visibility } }"#,
        )
        .await;
    assert_allowed(&allowed, "the creator opening the jam up");
}

// ── The activity feed ───────────────────────────────────────────────────────────────────────
