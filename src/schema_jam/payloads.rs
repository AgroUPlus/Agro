//! The payload types and builders the jam resolvers return.

use super::*;
use crate::db::Db;
use crate::db_jam::{Jam, JamTrackState};
use async_graphql::{Context, SimpleObject};

#[derive(SimpleObject, Clone)]
pub struct JamTrackPayload {
    pub id: String,
    pub added_by: String,
    pub track_uri: String,
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub duration_ms: i64,
    /// Approvals so far, excluding the person who suggested it.
    pub approvals: i64,
    /// Whether *you* have approved it, so the control can show its state.
    pub approved: bool,
    /// How many more approvals it needs. Sent rather than derived, so a client never has to
    /// reimplement the rule and disagree with the server about it.
    pub still_needed: i64,
}

/// The one thing the whole room is hearing.
#[derive(SimpleObject, Clone)]
pub struct JamNowPlayingPayload {
    pub track_id: String,
    pub title: String,
    pub artist: String,
    pub artwork_url: Option<String>,
    pub duration_ms: i64,
    pub started_at: String,
    /// How far in the room is *now*, worked out here from `started_at`. A client seeks to this
    /// rather than starting from zero, which is what lets somebody join late and land in step.
    pub position_ms: i64,
    /// The member who queued it, the device of theirs that holds it, and the bytes it names.
    ///
    /// `content_hash` is `None` for anything queued from a streaming source, which is what sends
    /// the rest of the room back to matching the track by name.
    pub added_by: String,
    pub device_id: Option<String>,
    pub content_hash: Option<String>,
    /// Where to reach that device on this network, and the token to present. Set only when the
    /// server judged the two devices to share one — see [`crate::ws::WsHub::same_network`].
    pub peer_lan_address: Option<String>,
    pub peer_lan_token: Option<String>,
    /// Votes to skip this track so far.
    pub skip_votes: i64,
    /// How many are needed. More than half the room, everybody counted.
    pub skips_needed: i64,
    /// Whether *you* have voted to skip it, so the control shows its state.
    pub you_skipped: bool,
}

#[derive(SimpleObject, Clone)]
pub struct JamPayload {
    pub id: String,
    /// What you give someone so they can join. Shown to members only.
    pub code: String,
    pub host: String,
    /// `open` or `democracy`.
    pub mode: String,
    pub is_host: bool,
    pub members: Vec<String>,
    /// Accepted tracks, in the order they were added.
    pub queue: Vec<JamTrackPayload>,
    /// Suggested tracks the room has not accepted yet. Always empty in `open` mode.
    pub proposals: Vec<JamTrackPayload>,
    /// What everyone is hearing, or null between tracks.
    pub now_playing: Option<JamNowPlayingPayload>,
    /// Approvals a suggestion needs here, so the UI can say "2 to go" without guessing.
    pub approvals_needed: i64,
    /// `code` or `friends` — who can find this jam without being handed the code.
    pub visibility: String,
}

/// A friend's jam, as it appears before you are in it.
///
/// Deliberately thin: the code is not here, and neither is the queue. Being able to see that a
/// friend has a jam open is not the same as being in it.
#[derive(SimpleObject, Clone)]
pub struct FriendJamPayload {
    pub id: String,
    pub host: String,
    pub mode: String,
    pub members: Vec<String>,
    /// What they are playing, so it is worth deciding about.
    pub now_playing_title: Option<String>,
}

/// The jam the caller is in, fully described. The single shape every mutation answers with, so a
/// client never has to stitch a view together from a mutation result and a stale query.
/// Where `viewer` could reach `holder`'s device directly, if anywhere.
///
/// Returns both halves or neither: the address is useless without a grant to present with it, and
/// handing one over without the other would disclose a private address for nothing. A holder who
/// never named a device, or who is not on the viewer's network, yields `(None, None)` and the
/// caller falls through to the relay.
pub(super) fn peer_route(
    db: &Db,
    ws_hub: &crate::ws::WsHub,
    holder: &str,
    holder_device: Option<&str>,
    viewer: &str,
) -> (Option<String>, Option<String>) {
    let Some(device) = holder_device else {
        return (None, None);
    };
    if holder.eq_ignore_ascii_case(viewer) {
        // Your own copy. There is nothing to fetch over the network.
        return (None, None);
    }
    if !ws_hub.shares_network_with_user(holder, device, viewer) {
        return (None, None);
    }
    let Some(address) = ws_hub.get_lan_address(holder, device) else {
        return (None, None);
    };
    let viewer_keys = crate::schema_social::published_keys(db, viewer);
    match ws_hub.grant_p2p_token(holder, device, viewer, &viewer_keys) {
        Some(token) => (Some(address), Some(token)),
        None => (None, None),
    }
}

pub(super) fn describe(
    db: &Db,
    ws_hub: &crate::ws::WsHub,
    jam: &Jam,
    viewer: &str,
) -> async_graphql::Result<JamPayload> {
    let payload = |t: crate::db_jam::JamTrack| JamTrackPayload {
        id: t.id,
        added_by: t.added_by,
        track_uri: t.track_uri,
        title: t.title,
        artist: t.artist,
        artwork_url: t.artwork_url,
        duration_ms: t.duration_ms,
        approvals: t.approvals,
        approved: t.approved,
        still_needed: t.still_needed,
    };

    // Read once, before the payload is built: both halves describe the same track.
    let skips = match jam.now_playing_id.as_deref() {
        Some(track) => db.jam_skip_state(track, viewer)?,
        None => (0, false),
    };
    let skips_needed = db.jam_skips_needed(&jam.id)?;

    Ok(JamPayload {
        id: jam.id.clone(),
        code: jam.code.clone(),
        host: jam.host.clone(),
        mode: jam.mode.as_str().to_string(),
        is_host: jam.host.eq_ignore_ascii_case(viewer),
        members: db.jam_members(&jam.id)?,
        // The playing track is still `queued` until it finishes, so it is filtered out here:
        // `nowPlaying` already describes it, and listing it in both places reads as the same song
        // being queued twice.
        queue: db
            .jam_tracks(&jam.id, JamTrackState::Queued, viewer)?
            .into_iter()
            .filter(|t| Some(&t.id) != jam.now_playing_id.as_ref())
            .map(payload)
            .collect(),
        proposals: db
            .jam_tracks(&jam.id, JamTrackState::Proposed, viewer)?
            .into_iter()
            .map(payload)
            .collect(),
        visibility: jam.visibility.as_str().to_string(),
        now_playing: db.jam_now_playing(jam)?.map(|now| {
            // The member who queued the track holds it, so they are the peer here — the same
            // pairwise question Listen Along asks, with the queueing member in the host's place.
            let (peer_lan_address, peer_lan_token) = peer_route(
                db,
                ws_hub,
                &now.added_by,
                now.added_by_device.as_deref(),
                viewer,
            );
            JamNowPlayingPayload {
                track_id: now.track_id,
                title: now.title,
                artist: now.artist,
                added_by: now.added_by,
                device_id: now.added_by_device,
                content_hash: now.content_hash,
                peer_lan_address,
                peer_lan_token,
                artwork_url: now.artwork_url,
                duration_ms: now.duration_ms,
                started_at: now.started_at,
                position_ms: now.position_ms,
                skip_votes: skips.0,
                skips_needed,
                you_skipped: skips.1,
            }
        }),
        approvals_needed: db.jam_approvals_needed(&jam.id)?,
    })
}

/// The caller's jam, or a refusal. Every mutation but `create`/`join` starts here.
pub(super) fn current_jam(ctx: &Context<'_>) -> async_graphql::Result<(Jam, String)> {
    let authed = caller(ctx)?;
    let db = ctx.data::<Db>()?;
    let jam = db
        .jam_for_member(authed.username())?
        .ok_or_else(|| forbidden("you are not in a jam"))?;
    Ok((jam, authed.username().to_string()))
}

/// Tells everyone in the jam that it changed.
///
/// Every mutation ends with this. A shared queue that only updates when you reload is not shared —
/// the whole point is that other people's additions and votes appear as they happen.
pub(super) fn announce(ctx: &Context<'_>, db: &Db, jam: &Jam) {
    let Ok(hub) = ctx.data::<std::sync::Arc<crate::ws::WsHub>>() else {
        return;
    };
    if let Ok(members) = db.jam_members(&jam.id) {
        hub.notify_users(
            &members,
            "JAM_UPDATED",
            serde_json::json!({ "jamId": jam.id }),
        );
    }
}
