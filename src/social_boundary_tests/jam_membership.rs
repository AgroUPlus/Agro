//! Jam membership boundary tests.

use super::*;

/// A wrong code and an ended jam are the same refusal — a code is a credential.
#[tokio::test]
pub(super) async fn a_bad_join_code_reveals_nothing() {
    let h = harness();
    let refused = h
        .run_as(&h.beta, r#"mutation { joinJam(code: "NOPE") { id } }"#)
        .await;
    assert_refused(&refused, "joining with a bad code");
}

/// "Listening now" means now, not "last played".
///
/// The handoff row is durable by design — it is what lets you resume elsewhere hours later — so
/// reading it directly left friends sitting in the presence feed forever, showing a track they
/// finished days ago.
#[tokio::test]
pub(super) async fn a_paused_or_stale_friend_is_not_listening_now() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.db.set_visibility("alpha", true, false).unwrap();

    // Playing, and reported just now.
    h.set_playing("alpha", "Currently On");
    let feed = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    assert_allowed(&feed, "presence feed");
    assert_eq!(
        feed.data.into_json().unwrap()["friendsNowPlaying"]
            .as_array()
            .map(|a| a.len()),
        Some(1)
    );

    // The same row, but paused: they are not listening to anything.
    h.db.update_handoff(
        "alpha",
        "track://1",
        "Currently On",
        "Some Artist",
        None,
        None,
        0,
        0,
        false,
        "device-1",
        None,
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let feed = h
        .run_as(&h.beta, "{ friendsNowPlaying { trackTitle } }")
        .await;
    let listed = feed.data.into_json().unwrap();
    assert_eq!(
        listed["friendsNowPlaying"].as_array().map(|a| a.len()),
        Some(0),
        "a paused friend was still reported as listening"
    );
}
