//! Jam queue boundary tests.

use super::*;
use crate::ws::WsHub;

/// In democracy mode a suggestion waits for the room, and the queue is what has been accepted.
///
/// This is the whole mechanic. Votes used to *sort* a queue everything had already entered, which
/// meant nothing was ever actually kept out.
#[tokio::test]
pub(super) async fn a_suggestion_waits_for_the_room() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Democracy).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();
    h.db.join_jam(&jam.id, "stranger").unwrap();

    let (track, state) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "Suggested",
            "A",
            None,
            1000,
            false,
            JamMode::Democracy,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        state,
        JamTrackState::Proposed,
        "it went straight into the queue"
    );
    assert!(h
        .db
        .jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
        .unwrap()
        .is_empty());

    // Three members means two others, so it needs two of them.
    assert_eq!(h.db.jam_approvals_needed(&jam.id).unwrap(), 2);

    // The proposer's own approval is recorded and counts for nothing.
    h.db.approve_jam_track(&jam.id, &track, "alpha").unwrap();
    assert!(
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
            .unwrap()
            .is_empty(),
        "the proposer carried their own suggestion"
    );

    h.db.approve_jam_track(&jam.id, &track, "beta").unwrap();
    assert!(h
        .db
        .jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
        .unwrap()
        .is_empty());

    h.db.approve_jam_track(&jam.id, &track, "stranger").unwrap();
    let queue =
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
            .unwrap();
    assert_eq!(
        queue.len(),
        1,
        "the room accepted it and it did not join the queue"
    );
    assert_eq!(queue[0].id, track);
}

/// Alone, there is nobody to ask — so a suggestion cannot be left permanently unpassable.
#[tokio::test]
pub(super) async fn a_solo_jam_queues_without_asking() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Democracy).unwrap();

    let (_, state) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "Only me",
            "A",
            None,
            1000,
            false,
            JamMode::Democracy,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        state,
        JamTrackState::Queued,
        "a solo jam could never pass anything"
    );
}

/// Open mode asks nobody.
#[tokio::test]
pub(super) async fn open_mode_queues_immediately() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let (_, state) =
        h.db.add_jam_track(
            &jam.id,
            "beta",
            "u:1",
            "Straight in",
            "B",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();
    assert_eq!(state, JamTrackState::Queued);
    assert!(h
        .db
        .jam_tracks(&jam.id, JamTrackState::Proposed, "beta")
        .unwrap()
        .is_empty());
}

/// The queue keeps the order things were added, whatever the approvals say.
#[tokio::test]
pub(super) async fn approvals_do_not_reorder_the_queue() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.join_jam(&jam.id, "beta").unwrap();

    let (first, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "First",
            "A",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();
    let (second, _) =
        h.db.add_jam_track(
            &jam.id,
            "beta",
            "u:2",
            "Second",
            "B",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();

    // Piling approvals on the later track must not move it up: votes decide entry, not position.
    h.db.approve_jam_track(&jam.id, &second, "alpha").unwrap();
    let queue =
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
            .unwrap();
    assert_eq!(queue[0].id, first, "approvals reordered the queue");
    assert_eq!(queue[1].id, second);
}

/// The server starts the room itself, and moves it on when the track's duration is up.
#[tokio::test]
pub(super) async fn the_server_advances_the_room_on_its_own() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();

    let (first, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "First",
            "A",
            None,
            40,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();
    let (second, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:2",
            "Second",
            "A",
            None,
            40,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();

    // Nobody has asked for anything: the clock starts the room.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    let now =
        h.db.jam_now_playing(&live)
            .unwrap()
            .expect("something should be playing");
    assert_eq!(now.track_id, first);

    // Still inside its duration, so the room stays put.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    assert_eq!(live.now_playing_id.as_deref(), Some(first.as_str()));

    // Past it, and the room moves on without any client saying so.
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    assert_eq!(live.now_playing_id.as_deref(), Some(second.as_str()));

    // And the finished one is not offered again.
    let queue =
        h.db.jam_tracks(&jam.id, JamTrackState::Queued, "alpha")
            .unwrap();
    assert!(
        !queue.iter().any(|t| t.id == first),
        "a played track stayed in the queue"
    );
}

/// A late joiner is told where the room is, not sent back to the start.
#[tokio::test]
pub(super) async fn now_playing_reports_the_rooms_position() {
    let h = harness();
    let hub = std::sync::Arc::new(WsHub::new());
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();
    h.db.add_jam_track(
        &jam.id,
        "alpha",
        "u:1",
        "Long one",
        "A",
        None,
        60_000,
        false,
        JamMode::Open,
        None,
        None,
    )
    .unwrap();

    crate::jam_clock::tick(&h.db, &hub);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    let now = h.db.jam_now_playing(&live).unwrap().unwrap();
    assert!(
        now.position_ms >= 40,
        "the room's position was reported as {}",
        now.position_ms
    );
    assert!(now.position_ms < 60_000);
}
