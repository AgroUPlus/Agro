//! The jam clock: the server decides what the room is hearing, and when it moves on.
//!
//! This exists because every previous arrangement made some *device* the authority, and a device is
//! the wrong thing to trust with it. Host-driven playback made whoever opened the jam a DJ, which
//! is what voting was meant to replace. Letting every device advance for itself meant the room
//! played the same order at different times, and whichever client finished first decided for
//! everybody.
//!
//! So the server holds one track and one start time. It advances on the track's own duration,
//! pushes the change, and every client mirrors it. Nobody reports anything back, so a paused,
//! crashed or slow device cannot stall or skew the room — it simply falls behind and is put back in
//! step on the next track.
//!
//! The cost, stated plainly: nobody can pause the room. The clock runs whether or not anyone is
//! listening. That is the honest price of having no host, and a room-wide pause would need its own
//! mutation rather than being smuggled in here.

use std::sync::Arc;

use crate::db::Db;
use crate::db_jam::{Jam, JamTrackState};
use crate::ws::WsHub;

/// How often the clock looks. Two seconds is far below anything a listener notices at a track
/// boundary, and cheap: it is one indexed query per live jam.
pub const TICK_SECS: u64 = 2;

/// How long a track of *unknown* length may hold the room before the clock moves on regardless.
///
/// This is for a recording whose duration failed to parse, not for a stream: a stream says so with
/// `is_live` and is left alone entirely. Longer than any ordinary song, so a track that is merely
/// unmeasured still plays to the end.
const UNKNOWN_DURATION_LEASE_MS: i64 = 12 * 60 * 1000;

/// A jam with nobody in it is swept away after this long.
///
/// Not immediately: leaving is also what happens when a phone loses signal mid-song, and deleting
/// the room out from under everyone else's reconnect would be worse than a few minutes of nothing.
const ABANDONED_AFTER_MS: i64 = 5 * 60 * 1000;

/// One pass over every live jam.
pub fn tick(db: &Db, hub: &Arc<WsHub>) {
    let Ok(jams) = db.live_jams() else { return };
    for jam in jams {
        if let Err(err) = advance(db, hub, &jam) {
            // A jam that cannot be advanced must not take the sweep down with it: the others are
            // still playing, and this one will be looked at again in two seconds.
            tracing::warn!("jam {} could not advance: {err}", jam.id);
        }
    }
}

fn advance(db: &Db, hub: &Arc<WsHub>, jam: &Jam) -> rusqlite::Result<()> {
    let members = db.jam_members(&jam.id)?;
    if members.is_empty() {
        if age_ms(&jam.created_at) > ABANDONED_AFTER_MS {
            db.delete_jam(&jam.id)?;
        }
        return Ok(());
    }

    // Still playing: nothing to do until its duration is up.
    if let Some(now) = db.jam_now_playing(jam)? {
        // A track with no duration used to be left alone "until something else moves the room on",
        // on the reasoning that a client sending one is the real fix. Nothing else ever moves the
        // room on — this function is the only thing that can — so the room simply stopped, for
        // good, and the next track never played.
        //
        // Zero duration is not rare either. YouTube Music's radio and queue entries often carry no
        // length, and a livestream has none by definition, so one of those reaching a jam wedged
        // it permanently.
        //
        // The lease is a backstop, not a mechanism: a real duration is still used whenever there
        // is one. It is set well past any ordinary song, because cutting a long track short is a
        // worse failure than a few extra minutes on a stream nobody can time.
        // A radio is endless on purpose, so it holds the room until somebody skips it — which
        // `voteSkipJamTrack` can do, and which is the only thing that should end something with no
        // end of its own. Cutting it off after a fixed lease would be the clock deciding a
        // broadcast was over, which it has no way to know.
        if now.is_live {
            return Ok(());
        }
        let ends_at = if now.duration_ms > 0 {
            now.duration_ms
        } else {
            UNKNOWN_DURATION_LEASE_MS
        };
        if now.position_ms < ends_at {
            return Ok(());
        }
        db.mark_jam_track_played(&jam.id, &now.track_id)?;
    }

    let next = db
        .jam_tracks(&jam.id, JamTrackState::Queued, &jam.host)?
        .into_iter()
        .next();

    match next {
        Some(track) => {
            db.set_jam_now_playing(&jam.id, &track.id)?;
            let now = db.jam_now_playing(&jam)?;
            let (holder, holder_device, content_hash) = now
                .as_ref()
                .map(|n| {
                    (
                        n.added_by.clone(),
                        n.added_by_device.clone(),
                        n.content_hash.clone(),
                    )
                })
                .unwrap_or_default();

            // One frame per member rather than one for the room: whether the track can be fetched
            // directly, and the token to fetch it with, are facts about each member and the one
            // holding the file. A shared frame would hand the whole room the first member's
            // address and bearer token.
            for member in &members {
                let (peer_lan_address, peer_lan_token) = match holder_device.as_deref() {
                    Some(device) if !holder.eq_ignore_ascii_case(member) => {
                        if hub.shares_network_with_user(&holder, device, member) {
                            match (
                                hub.get_lan_address(&holder, device),
                                hub.grant_p2p_token(&holder, device, member),
                            ) {
                                (Some(address), Some(token)) => (Some(address), Some(token)),
                                _ => (None, None),
                            }
                        } else {
                            (None, None)
                        }
                    }
                    _ => (None, None),
                };
                hub.notify_user(
                    member,
                    "JAM_NOW_PLAYING",
                    serde_json::json!({
                        "jamId": jam.id,
                        "trackId": track.id,
                        "title": track.title,
                        "artist": track.artist,
                        "artworkUrl": track.artwork_url,
                        "durationMs": track.duration_ms,
                        // Zero rather than the elapsed time: this frame *is* the start.
                        "positionMs": 0,
                        "addedBy": holder,
                        "deviceId": holder_device,
                        "contentHash": content_hash,
                        "peerLanAddress": peer_lan_address,
                        "peerLanToken": peer_lan_token,
                    }),
                );
            }
        }
        None => {
            // Nothing queued. Said out loud rather than left playing the last track forever.
            if jam.now_playing_id.is_some() {
                db.clear_jam_now_playing(&jam.id)?;
                hub.notify_users(
                    &members,
                    "JAM_NOW_PLAYING",
                    serde_json::json!({ "jamId": jam.id, "stopped": true }),
                );
            }
        }
    }
    Ok(())
}

fn age_ms(timestamp: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|then| {
            (chrono::Utc::now() - then.with_timezone(&chrono::Utc))
                .num_milliseconds()
                .max(0)
        })
        .unwrap_or(0)
}
