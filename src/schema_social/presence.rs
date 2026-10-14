//! Now-playing presence: what a viewer may see, and the fan-out of it.

use super::*;
use crate::db::Db;

/// How long after its last update a handoff still counts as "now".
///
/// Clients republish every ten seconds while playing, so anything older than this is a session
/// that ended without saying so — the app was killed, the phone lost signal, the battery went.
pub(super) const NOW_PLAYING_STALE_AFTER_SECS: i64 = 300;

/// What a friend is playing *at the moment*, or `None`.
///
/// The handoff row is durable — it is what lets you resume on another device hours later — so it
/// describes the last thing played, not the thing being played. Reading it directly meant a friend
/// who listened days ago sat permanently in "Listening now" with a track long finished.
///
/// Two conditions, both necessary: they were playing rather than paused when they last reported,
/// and they reported recently enough for that to still be true.
pub(super) fn live_now_playing(
    db: &Db,
    username: &str,
) -> async_graphql::Result<Option<FriendNowPlaying>> {
    let Some(now) = now_playing_of(db, username)? else {
        return Ok(None);
    };
    if !now.is_playing {
        return Ok(None);
    }
    let fresh = chrono::DateTime::parse_from_rfc3339(&now.updated_at)
        .map(|seen| {
            (chrono::Utc::now() - seen.with_timezone(&chrono::Utc)).num_seconds()
                < NOW_PLAYING_STALE_AFTER_SECS
        })
        // An unparseable timestamp is treated as stale: showing something that may be days old is
        // the failure this exists to prevent.
        .unwrap_or(false);
    Ok(if fresh { Some(now) } else { None })
}

/// The handoff row as a friend may see it, or `None` when there is nothing to show.
pub(super) fn now_playing_of(
    db: &Db,
    username: &str,
) -> async_graphql::Result<Option<FriendNowPlaying>> {
    let Some(handoff) = db.get_handoff(username)? else {
        return Ok(None);
    };
    Ok(Some(FriendNowPlaying {
        username: username.to_string(),
        track_uri: handoff.track_uri,
        track_title: handoff.track_title,
        artist_name: handoff.artist_name,
        album_name: handoff.album_name,
        artwork_url: handoff.artwork_url,
        position_ms: handoff.position_ms,
        is_playing: handoff.is_playing,
        updated_at: handoff.updated_at,
        device_id: handoff.device_id,
        content_hash: handoff.content_hash,
        // No viewer, no pairwise answer. Whether a direct transfer is possible is a fact about two
        // devices, so a projection that does not know who is asking cannot fill these in. The
        // sealed copy is the same kind of fact and is filled in by the same caller.
        peer_lan_address: None,
        peer_lan_token: None,
        encrypted_presence: None,
    }))
}

/// The same, answered for a specific viewer on a specific device.
///
/// The direct-transfer fields are pairwise — they depend on where *both* devices are — so they can
/// only be filled in once the asker is known. Everything else is identical, and a viewer who
/// cannot be told about this host never reaches here: the caller has already applied the
/// friendship and visibility gates.
/// The identity keys an account has published, for binding into a P2P grant.
///
/// Answers an empty list when the lookup fails rather than propagating. A grant that cannot name
/// the listener's keys is the pre-registry behaviour — the host falls back to the request header —
/// and losing the ability to listen along because a key lookup errored would be the worse failure.
pub(crate) fn published_keys(db: &Db, username: &str) -> Vec<String> {
    db.device_keys_for(username)
        .map(|keys| keys.into_iter().map(|k| k.public_key).collect())
        .unwrap_or_default()
}

/// Hands `now` the sealed copy this viewer's device can open, if there is one.
///
/// Looked up by the exact `(host device, viewer, viewer device)` the row was sealed for, so there
/// is no argument that returns somebody else's copy. A caller that omits the device — an older
/// client, or the dashboard, which holds no device key at all — gets nothing and falls back to the
/// placeholder, which is the correct outcome rather than a degraded one.
///
/// Failure is swallowed for the same reason `fan_out_presence` swallows its own: this decorates a
/// presence line that is already valid without it, and a lookup error must not turn a working feed
/// into an error page.
pub(super) fn attach_sealed_presence(
    db: &Db,
    now: &mut FriendNowPlaying,
    host: &str,
    viewer: &str,
    viewer_device: Option<&str>,
) {
    let Some(device) = viewer_device.map(str::trim).filter(|d| !d.is_empty()) else {
        return;
    };
    now.encrypted_presence = db
        .presence_ciphertext_for(host, &now.device_id, viewer, device)
        .unwrap_or(None);
}

pub(super) fn now_playing_for_viewer(
    db: &Db,
    ws_hub: &crate::ws::WsHub,
    host: &str,
    viewer: &str,
    viewer_device: Option<&str>,
) -> async_graphql::Result<Option<FriendNowPlaying>> {
    let Some(mut now) = now_playing_of(db, host)? else {
        return Ok(None);
    };
    attach_sealed_presence(db, &mut now, host, viewer, viewer_device);
    if ws_hub.shares_network_with_user(host, &now.device_id, viewer) {
        if let Some(lan) = ws_hub.get_lan_address(host, &now.device_id) {
            let viewer_keys = published_keys(db, viewer);
            now.peer_lan_token = ws_hub.grant_p2p_token(host, &now.device_id, viewer, &viewer_keys);
            // The address is only worth handing over alongside a token to use it with.
            if now.peer_lan_token.is_some() {
                now.peer_lan_address = Some(lan);
            }
        }
    }
    Ok(Some(now))
}

/// Tells the people allowed to know that `user` has started playing something else.
///
/// Called from `updateHandoff`, which is the only place a change of track exists. Two audiences,
/// deliberately separate: friends who are merely watching get a presence line, and listeners who
/// are following along get the position too, because their player has to act on it.
///
/// Every failure here is swallowed. This is a notification about somebody else's playback; it may
/// not be allowed to fail the handoff of the account that triggered it.
pub fn fan_out_presence(db: &Db, ws_hub: &crate::ws::WsHub, user: &str) {
    let Ok(Some(now)) = now_playing_of_quiet(db, user) else {
        return;
    };

    // The subject's own flag decides. A friend who has not opted in is not merely omitted from a
    // list here — nothing about them is sent at all.
    if crate::features::Feature::FriendPresence.is_on(db)
        && db
            .profile(user)
            .ok()
            .flatten()
            .is_some_and(|p| p.shows_now_playing())
    {
        if let Ok(friends) = db.friends(user) {
            // One frame per friend rather than one broadcast, for the same reason the listener
            // loop below sends one at a time: a sealed copy is addressed to a single device's key.
            // Broadcasting one payload would hand every friend the first one's ciphertext — bytes
            // none of the rest can open — and quietly replace a working feed with a locked one.
            //
            // An unsealed session has nothing pairwise in it and does not need this. It costs a
            // frame each either way, and one code path is worth more than the saving.
            for friend in friends {
                let sealed = db
                    .presence_ciphertexts_to(user, &friend.username)
                    .unwrap_or_default();
                let copies: Vec<serde_json::Value> = sealed
                    .into_iter()
                    .map(|copy| {
                        serde_json::json!({
                            "deviceId": copy.recipient_device_id,
                            "ciphertext": copy.ciphertext,
                        })
                    })
                    .collect();
                ws_hub.notify_user(
                    &friend.username,
                    "FRIEND_PRESENCE",
                    serde_json::json!({
                        "username": now.username,
                        "trackTitle": now.track_title,
                        "artistName": now.artist_name,
                        "albumName": now.album_name,
                        "artworkUrl": now.artwork_url,
                        "isPlaying": now.is_playing,
                        "updatedAt": now.updated_at,
                        // Every copy this friend can hold, not one: a friend may be signed in on
                        // several devices and the socket frame does not know which is listening.
                        // Each device opens the one sealed to it and ignores the rest, exactly as
                        // the drops inbox already does.
                        "encryptedPresence": copies,
                    }),
                );
            }
        }
    }

    // `listeners_of` re-checks the friendship, so someone who was unfriended mid-session stops
    // receiving frames whether or not their row was cleaned up.
    //
    // Sent one listener at a time rather than as a single fan-out, because two of these fields are
    // *pairwise*: whether the host is directly reachable, and the token to reach them with, are
    // facts about this listener and this host together. Broadcasting one payload would hand every
    // listener the first one's answer — including a LAN address and a bearer token that were never
    // theirs.
    if !crate::features::Feature::ListenAlong.is_on(db) {
        return;
    }
    if let Ok(listeners) = db.listeners_of(user) {
        for listener in listeners {
            let mut peer_lan_address = None;
            let mut peer_lan_token = None;
            if ws_hub.shares_network_with_user(user, &now.device_id, &listener) {
                if let Some(lan) = ws_hub.get_lan_address(user, &now.device_id) {
                    let listener_keys = published_keys(db, &listener);
                    peer_lan_token =
                        ws_hub.grant_p2p_token(user, &now.device_id, &listener, &listener_keys);
                    if peer_lan_token.is_some() {
                        peer_lan_address = Some(lan);
                    }
                }
            }
            // The same sealed copies the friends feed gets. A listener is following the session,
            // not merely watching it, so a frame carrying only the placeholder leaves them unable
            // to name the track *or* resolve the file — the plaintext columns hold neither under a
            // sealed session. This loop is already per-listener, so the copy is free to address.
            let sealed = db
                .presence_ciphertexts_to(user, &listener)
                .unwrap_or_default();
            let copies: Vec<serde_json::Value> = sealed
                .into_iter()
                .map(|copy| {
                    serde_json::json!({
                        "deviceId": copy.recipient_device_id,
                        "ciphertext": copy.ciphertext,
                    })
                })
                .collect();
            ws_hub.notify_user(
                &listener,
                "LISTEN_ALONG",
                serde_json::json!({
                    "host": user,
                    "trackUri": now.track_uri,
                    "trackTitle": now.track_title,
                    "artistName": now.artist_name,
                    "albumName": now.album_name,
                    "artworkUrl": now.artwork_url,
                    "positionMs": now.position_ms,
                    "isPlaying": now.is_playing,
                    "updatedAt": now.updated_at,
                    "deviceId": now.device_id,
                    "contentHash": now.content_hash,
                    "peerLanAddress": peer_lan_address,
                    "peerLanToken": peer_lan_token,
                    "encryptedPresence": copies,
                }),
            );
        }
    }
}

/// [`now_playing_of`] without the GraphQL error type, for the push path.
pub(super) fn now_playing_of_quiet(
    db: &Db,
    username: &str,
) -> Result<Option<FriendNowPlaying>, ()> {
    now_playing_of(db, username).map_err(|_| ())
}
