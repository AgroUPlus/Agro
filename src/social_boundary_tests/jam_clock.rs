//! Jam clock boundary tests.

use super::*;
use crate::ws::WsHub;

/// A track with no length must not be able to park the room forever.
///
/// The clock advances on the track's own duration, so a zero meant there was no moment to advance
/// at and the jam simply stopped — permanently, because this sweep is the only thing that can move
/// a room on. Zero is not rare: YouTube Music's radio entries often carry no length and a
/// livestream has none at all.
#[tokio::test]
pub(super) async fn a_jam_moves_on_from_a_track_with_no_duration() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();

    let (first, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "No Duration",
            "A",
            None,
            0,
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
            "Next Up",
            "A",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();

    let hub = std::sync::Arc::new(WsHub::new());
    h.db.set_jam_now_playing(&jam.id, &first).unwrap();

    // Straight away, the zero-duration track is still the room's — a lease is a backstop, not an
    // excuse to cut every unmeasured track off the moment it starts.
    crate::jam_clock::tick(&h.db, &hub);
    let live = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    assert_eq!(
        live.now_playing_id.as_deref(),
        Some(first.as_str()),
        "the lease expired immediately"
    );

    // Backdated past the lease, standing in for the time passing.
    let long_ago = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    h.db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE jams SET started_at = ?1 WHERE id = ?2",
            rusqlite::params![long_ago, jam.id],
        )
        .unwrap();

    crate::jam_clock::tick(&h.db, &hub);
    let moved = h.db.jam_by_id(&jam.id).unwrap().unwrap();
    assert_eq!(
        moved.now_playing_id.as_deref(),
        Some(second.as_str()),
        "the room stayed stuck on a track it could not time"
    );
}

/// A radio has no end of its own, so the clock must not invent one for it.
///
/// The lease exists for a recording whose duration failed to parse. A stream is endless on
/// purpose and stops when the room votes it off — anything else is the clock deciding a broadcast
/// is over, which it has no way to know.
#[tokio::test]
pub(super) async fn a_live_track_holds_a_jam_until_it_is_skipped() {
    let h = harness();
    let jam = h.db.create_jam("alpha", JamMode::Open).unwrap();

    let (radio, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:1",
            "Some Radio",
            "A",
            None,
            0,
            true,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();
    let (next, _) =
        h.db.add_jam_track(
            &jam.id,
            "alpha",
            "u:2",
            "Next Up",
            "A",
            None,
            1000,
            false,
            JamMode::Open,
            None,
            None,
        )
        .unwrap();

    let hub = std::sync::Arc::new(WsHub::new());
    h.db.set_jam_now_playing(&jam.id, &radio).unwrap();

    // Far past the lease an unmeasured recording would have been given.
    let long_ago = (chrono::Utc::now() - chrono::Duration::hours(3)).to_rfc3339();
    h.db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE jams SET started_at = ?1 WHERE id = ?2",
            rusqlite::params![long_ago, jam.id],
        )
        .unwrap();

    crate::jam_clock::tick(&h.db, &hub);
    assert_eq!(
        h.db.jam_by_id(&jam.id)
            .unwrap()
            .unwrap()
            .now_playing_id
            .as_deref(),
        Some(radio.as_str()),
        "the clock retired a stream that had not been skipped"
    );

    // Alone in the room, one vote is the whole room.
    assert!(h.db.vote_skip(&jam.id, &radio, "alpha").unwrap());
    h.db.mark_jam_track_played(&jam.id, &radio).unwrap();
    h.db.clear_jam_now_playing(&jam.id).unwrap();

    crate::jam_clock::tick(&h.db, &hub);
    assert_eq!(
        h.db.jam_by_id(&jam.id)
            .unwrap()
            .unwrap()
            .now_playing_id
            .as_deref(),
        Some(next.as_str()),
        "a skipped stream did not hand the room on"
    );
}

// ── Incognito ───────────────────────────────────────────────────────────────────────────────
