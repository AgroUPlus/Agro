//! Activity feed boundary tests.

use super::*;

/// The headline case for the feed: it has its own switch, and neither of the other two opens it.
#[tokio::test]
pub(super) async fn activity_is_gated_separately_from_now_playing_and_stats() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 12, 3600);

    // Both of the *other* switches wide open, and the feed still says nothing.
    h.db.set_visibility("alpha", true, true).unwrap();
    let response = h.run_as(&h.beta, r#"{ friendActivity { summary } }"#).await;
    assert_allowed(&response, "reading the feed at all");
    assert!(
        response.data.to_string().contains("[]"),
        "showNowPlaying and showStats must not open the activity feed: {:?}",
        response.data
    );

    h.db.set_show_activity("alpha", true).unwrap();
    let opened = h
        .run_as(&h.beta, r#"{ friendActivity { summary kind } }"#)
        .await;
    assert_allowed(&opened, "reading the feed once it is opened");
    assert!(
        opened.data.to_string().contains("MILESTONE"),
        "an opened feed should carry alpha's milestone: {:?}",
        opened.data
    );
}

/// A stranger is not merely shown an empty feed — they are never one of the accounts it reads.
#[tokio::test]
pub(super) async fn a_strangers_feed_never_contains_someone_they_are_not_friends_with() {
    let h = harness();
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 12, 3600);
    h.db.set_show_activity("alpha", true).unwrap();

    let response = h
        .run_as(&h.stranger, r#"{ friendActivity { username } }"#)
        .await;
    assert_allowed(&response, "a stranger reading their own feed");
    assert!(
        !response.data.to_string().contains("alpha"),
        "alpha is not this account's friend: {:?}",
        response.data
    );
}

/// Closing the switch closes the surface, with nothing left behind from when it was open.
#[tokio::test]
pub(super) async fn closing_activity_empties_the_feed_again() {
    let h = harness();
    h.befriend("alpha", "beta");
    h.seed_plays("alpha", "Xtal", "Aphex Twin", 12, 3600);
    h.db.set_show_activity("alpha", true).unwrap();

    let open = h.run_as(&h.beta, r#"{ friendActivity { summary } }"#).await;
    assert!(open.data.to_string().contains("MILESTONE") || open.data.to_string().contains("Aphex"));

    h.db.set_show_activity("alpha", false).unwrap();
    let closed = h.run_as(&h.beta, r#"{ friendActivity { summary } }"#).await;
    assert!(
        !closed.data.to_string().contains("Aphex"),
        "the feed is derived, so closing the switch must empty it: {:?}",
        closed.data
    );
}

/// `setVisibility` must not turn the feed on as a side effect of setting something else.
#[tokio::test]
pub(super) async fn setting_other_switches_leaves_activity_alone() {
    let h = harness();
    let response = h
        .run_as(
            &h.alpha,
            r#"mutation { setVisibility(showNowPlaying: true, showStats: true) { showActivity } }"#,
        )
        .await;
    assert_allowed(&response, "opening the other two switches");
    assert!(
        response.data.to_string().contains("false"),
        "showActivity must default closed and stay closed: {:?}",
        response.data
    );
}

// ── The circle recap ────────────────────────────────────────────────────────────────────────
