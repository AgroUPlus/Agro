//! Jam governance boundary tests.

use super::*;

/// Only the creator governs; everyone else is equal.
#[tokio::test]
pub(super) async fn a_jam_is_governed_by_its_creator_alone() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Democracy).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let refused = h
        .run_as(&h.beta, r#"mutation { setJamMode(mode: "open") { mode } }"#)
        .await;
    assert_refused(&refused, "a member changing the mode");

    let allowed = h
        .run_as(
            &h.alpha,
            r#"mutation { setJamMode(mode: "open") { mode } }"#,
        )
        .await;
    assert_allowed(&allowed, "the creator changing the mode");
}

/// A track is yours to remove, or the creator's — not any member's.
#[tokio::test]
pub(super) async fn only_the_owner_or_the_creator_removes_a_track() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.join_jam(&jam.id, "stranger").unwrap();
    let (track, _) =
        h.db.add_jam_track(
            &jam.id,
            "beta",
            "u:1",
            "Beta's pick",
            "B",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();

    let refused = h
        .run_as(
            &h.stranger,
            &format!(r#"mutation {{ removeJamTrack(trackId: "{track}") {{ id }} }}"#),
        )
        .await;
    assert_refused(&refused, "a member removing someone else's track");

    let allowed = h
        .run_as(
            &h.beta,
            &format!(r#"mutation {{ removeJamTrack(trackId: "{track}") {{ id }} }}"#),
        )
        .await;
    assert_allowed(&allowed, "removing your own track");
}

/// Somebody outside the jam can reach none of it.
#[tokio::test]
pub(super) async fn a_non_member_cannot_touch_the_queue() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    let (track, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "One",
            "A",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();

    let refused = h
        .run_as(
            &h.stranger,
            &format!(r#"mutation {{ approveJamTrack(trackId: "{track}") {{ id }} }}"#),
        )
        .await;
    assert_refused(&refused, "a non-member approving");

    let seen = h.run_as(&h.stranger, "{ jam { id } }").await;
    assert_allowed(&seen, "querying jam as a non-member");
    assert_eq!(
        seen.data.into_json().unwrap()["jam"],
        serde_json::Value::Null,
        "a stranger was shown somebody else's jam"
    );
}

/// The creator leaving ends the room, and takes every row with it.
#[tokio::test]
pub(super) async fn ending_a_jam_clears_it_from_the_server() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.add_jam_track(
        &jam.id,
        "alpha",
        "u:1",
        "One",
        "A",
        None,
        1000,
        false,
        JamMode::Open,
        None,
        None,
    )
    .unwrap();

    let left = h.run_as(&h.alpha, "mutation { leaveJam }").await;
    assert_allowed(&left, "the creator leaving");

    assert!(
        h.db.jam_by_id(&jam.id).unwrap().is_none(),
        "the jam outlived its creator"
    );
    assert!(
        h.db.jam_for_member("beta").unwrap().is_none(),
        "beta is still in a deleted jam"
    );
    assert!(
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
            .unwrap()
            .is_empty(),
        "the queue was left behind"
    );
}

/// Ending a jam leaves each member a recap of it — theirs alone — and a stranger gets none.
#[tokio::test]
pub(super) async fn ending_a_jam_leaves_each_member_their_own_recap() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    let (track, _) =
        h.db.add_jam_track(
            &jam.id,
            "beta",
            "u:1",
            "One",
            "A",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();
    h.db.set_jam_now_playing(&jam.id, &track).unwrap();

    let left = h.run_as(&h.alpha, "mutation { leaveJam }").await;
    assert_allowed(&left, "the creator leaving");

    let query = "{ jamRecaps { id recap { people tracks { title addedBy } } } }";
    for who in [&h.alpha, &h.beta] {
        let read = h.run_as(who, query).await;
        assert_allowed(&read, "reading your own recaps");
        let recaps = read.data.into_json().unwrap()["jamRecaps"].clone();
        assert_eq!(recaps.as_array().unwrap().len(), 1, "one recap per member");
        assert_eq!(recaps[0]["recap"]["tracks"][0]["title"], "One");
    }

    let seen = h.run_as(&h.stranger, query).await;
    assert_allowed(&seen, "a stranger reading recaps");
    assert_eq!(
        seen.data.into_json().unwrap()["jamRecaps"],
        serde_json::json!([]),
        "a stranger was shown a recap of a room they were never in"
    );

    let beta_recap = h.db.jam_recaps("beta").unwrap()[0].id.clone();
    let stolen = h
        .run_as(
            &h.stranger,
            &format!(r#"mutation {{ dismissJamRecap(id: "{beta_recap}") }}"#),
        )
        .await;
    assert_eq!(
        stolen.data.into_json().unwrap()["dismissJamRecap"],
        false,
        "a stranger dismissed someone else's recap"
    );
    assert_eq!(h.db.jam_recaps("beta").unwrap().len(), 1);
}
