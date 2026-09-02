//! The jam session API.
//!
//! Authorization here is membership, not friendship: a jam is joined with a code, and the code is
//! the whole credential. That is deliberate — a jam is a room you invite people into, including
//! people you have not added as friends — but it means the code is the only thing standing between
//! a stranger and the queue, so it is minted rather than chosen.
//!
//! One asymmetry: the creator is the host. They set the mode, may drop a track anyone added, and
//! may end the session. Every other action is equal between members, which is the point of the
//! thing — `democracy` mode exists precisely so the queue is not one person's.

use async_graphql::{Context, Object, SimpleObject};

use crate::auth::AuthedUser;
use crate::db::Db;
use crate::db_jam::{Jam, JamMode, JamTrackState, JamVisibility};

fn caller<'a>(ctx: &'a Context<'_>) -> async_graphql::Result<&'a AuthedUser> {
    ctx.data::<AuthedUser>()
        .map_err(|_| async_graphql::Error::new("Unauthorized"))
}

fn forbidden(message: &str) -> async_graphql::Error {
    async_graphql::Error::new(format!("Forbidden: {message}"))
}

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
fn peer_route(
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

fn describe(
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
fn current_jam(ctx: &Context<'_>) -> async_graphql::Result<(Jam, String)> {
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
fn announce(ctx: &Context<'_>, db: &Db, jam: &Jam) {
    let Ok(hub) = ctx.data::<std::sync::Arc<crate::ws::WsHub>>() else {
        return;
    };
    if let Ok(members) = db.jam_members(&jam.id) {
        hub.notify_users(&members, "JAM_UPDATED", serde_json::json!({ "jamId": jam.id }));
    }
}

#[derive(Default)]
pub struct JamQuery;

#[Object]
impl JamQuery {
    /// Jams your friends have opened up, so one can be joined without being handed a code.
    ///
    /// Friendship and the switch, both — the same rule everything else social here follows.
    async fn friend_jams(&self, ctx: &Context<'_>) -> async_graphql::Result<Vec<FriendJamPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let mut open = Vec::new();
        for jam in db.friend_jams(authed.username())? {
            let now_playing_title = db.jam_now_playing(&jam)?.map(|now| now.title);
            open.push(FriendJamPayload {
                id: jam.id.clone(),
                host: jam.host.clone(),
                mode: jam.mode.as_str().to_string(),
                members: db.jam_members(&jam.id)?,
                now_playing_title,
            });
        }
        Ok(open)
    }

    /// The jam this account is in, or null. There is at most one.
    async fn jam(&self, ctx: &Context<'_>) -> async_graphql::Result<Option<JamPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        match db.jam_for_member(authed.username())? {
            Some(jam) => Ok(Some(describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, authed.username())?)),
            None => Ok(None),
        }
    }
}

#[derive(Default)]
pub struct JamMutation;

#[Object]
impl JamMutation {
    /// Opens a jam with this account as its host.
    ///
    /// Leaves whatever jam you were in first: being in two at once would make "the queue" and
    /// "your vote" ambiguous, and there is no interface in which that reads well.
    async fn create_jam(
        &self,
        ctx: &Context<'_>,
        mode: Option<String>,
    ) -> async_graphql::Result<JamPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        if let Some(existing) = db.jam_for_member(authed.username())? {
            db.leave_jam(&existing.id, authed.username())?;
        }

        let mode = mode.as_deref().map(JamMode::parse).unwrap_or(JamMode::Democracy);
        let jam = db.create_jam(authed.username(), mode)?;
        announce(ctx, db, &jam);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, authed.username())
    }

    /// Joins a jam by its code.
    async fn join_jam(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<JamPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        // The same words for a wrong code and an ended one. A code is a credential, and telling
        // the difference apart is how you find out which codes exist.
        let jam = db
            .jam_by_code(&code)?
            .ok_or_else(|| forbidden("no jam is open with that code"))?;

        if let Some(existing) = db.jam_for_member(authed.username())? {
            if existing.id != jam.id {
                db.leave_jam(&existing.id, authed.username())?;
            }
        }
        db.join_jam(&jam.id, authed.username())?;
        announce(ctx, db, &jam);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, authed.username())
    }

    /// Joins a friend's jam by its id, with no code.
    ///
    /// The code is not bypassed so much as unnecessary: the jam has been opened to friends, and
    /// this checks that you are one. A jam that has *not* been opened is refused in the same words
    /// as one that does not exist, so this cannot be used to discover rooms.
    async fn join_friend_jam(
        &self,
        ctx: &Context<'_>,
        jam_id: String,
    ) -> async_graphql::Result<JamPayload> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let jam = db
            .friend_jams(authed.username())?
            .into_iter()
            .find(|jam| jam.id == jam_id.trim())
            .ok_or_else(|| forbidden("no jam is open to you with that id"))?;

        if let Some(existing) = db.jam_for_member(authed.username())? {
            if existing.id != jam.id {
                db.leave_jam(&existing.id, authed.username())?;
            }
        }
        db.join_jam(&jam.id, authed.username())?;
        announce(ctx, db, &jam);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, authed.username())
    }

    /// Opens the jam to friends, or shuts it back to code-only. The creator decides.
    async fn set_jam_visibility(
        &self,
        ctx: &Context<'_>,
        visibility: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        if !jam.host.eq_ignore_ascii_case(&me) {
            return Err(forbidden("only the creator can open the jam up"));
        }
        db.set_jam_visibility(&jam.id, JamVisibility::parse(&visibility))?;
        let updated = db.jam_by_id(&jam.id)?.unwrap_or(jam);
        announce(ctx, db, &updated);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &updated, &me)
    }

    /// Votes to skip whatever is playing.
    ///
    /// One vote per person per track, and the votes die with the track — a skip is about *this*
    /// song, not a standing objection. Once more than half the room has asked, the track is retired
    /// immediately rather than at the end of its duration, and the clock takes the next one.
    async fn vote_skip_jam_track(&self, ctx: &Context<'_>) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;

        let Some(track_id) = jam.now_playing_id.clone() else {
            return Err("Nothing is playing to skip".into());
        };

        if db.vote_skip(&jam.id, &track_id, &me)? {
            // The room has had enough. Retired here rather than left to the clock, which would
            // keep playing it until its duration was up — a skip that takes effect in two minutes
            // is not a skip.
            db.mark_jam_track_played(&jam.id, &track_id)?;
            db.clear_jam_now_playing(&jam.id)?;
        }
        let updated = db.jam_by_id(&jam.id)?.unwrap_or(jam);
        announce(ctx, db, &updated);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &updated, &me)
    }

    /// Leaves the jam, and deletes it once nobody is left.
    ///
    /// The creator leaving ends it for everyone — a room with no one to govern it cannot change
    /// mode or be wound up — and the rows go with it. A jam is a room, not a document: keeping a
    /// dead one means every client has to keep deciding whether it still counts.
    async fn leave_jam(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;

        let creator_left = jam.host.eq_ignore_ascii_case(&me);
        db.leave_jam(&jam.id, &me)?;
        let remaining = db.jam_members(&jam.id)?;

        // Told before it is deleted, or there is nobody left to tell.
        announce(ctx, db, &jam);
        if creator_left || remaining.is_empty() {
            db.delete_jam(&jam.id)?;
        }
        Ok(true)
    }

    /// Suggests a track.
    ///
    /// In `open` mode it joins the queue immediately. In `democracy` it waits as a proposal until a
    /// majority of the *other* members accept it — which is what voting is for here. It sorts
    /// nothing: a track that has been accepted takes its place by when it was added, like any
    /// other.
    ///
    /// `durationMs` matters more than it looks: the server advances the room on that number, so a
    /// track without one would be skipped past the moment it started.
    async fn add_jam_track(
        &self,
        ctx: &Context<'_>,
        track_uri: String,
        title: String,
        artist: String,
        artwork_url: Option<String>,
        duration_ms: Option<i64>,
        // `is_live` marks a stream with no end: it holds the room until someone skips it, rather
        // than being retired by a clock that has no way to know a broadcast is over.
        is_live: Option<bool>,
        // `device_id` and `content_hash` are only meaningful together: they are what lets another
        // member play the queueing member's own copy instead of hunting for the track by name.
        // Omitted for anything queued from a streaming source, which is most of a queue.
        device_id: Option<String>,
        content_hash: Option<String>,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;

        let title = title.trim();
        if title.is_empty() {
            return Err("A track needs a title".into());
        }
        db.add_jam_track(
            &jam.id,
            &me,
            track_uri.trim(),
            title,
            artist.trim(),
            artwork_url.as_deref(),
            duration_ms.unwrap_or(0),
            is_live.unwrap_or(false),
            jam.mode,
            device_id.as_deref(),
            content_hash.as_deref(),
        )?;
        announce(ctx, db, &jam);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, &me)
    }

    /// Accepts somebody's suggestion.
    ///
    /// One-way, unlike the toggle this replaces. Taking an approval back could drop a track out of
    /// the queue after the room had already accepted it — possibly while it was playing — so
    /// somebody who changes their mind removes the track rather than un-approving it.
    ///
    /// Approving your own suggestion is accepted and recorded but never counts. The rule lives here
    /// rather than in each client's decision about whether to show a button.
    async fn approve_jam_track(
        &self,
        ctx: &Context<'_>,
        track_id: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;

        if db.jam_track_owner(&jam.id, &track_id)?.is_none() {
            return Err(forbidden("that track is not in this jam"));
        }
        db.approve_jam_track(&jam.id, &track_id, &me)?;
        announce(ctx, db, &jam);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, &me)
    }

    /// Removes a track. Your own, or anyone's if you are the host.
    async fn remove_jam_track(
        &self,
        ctx: &Context<'_>,
        track_id: String,
    ) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;

        let owner = db
            .jam_track_owner(&jam.id, &track_id)?
            .ok_or_else(|| forbidden("that track is not in this jam"))?;
        let is_host = jam.host.eq_ignore_ascii_case(&me);
        if !is_host && !owner.eq_ignore_ascii_case(&me) {
            return Err(forbidden("only the host can remove someone else's track"));
        }
        db.remove_jam_track(&jam.id, &track_id)?;
        announce(ctx, db, &jam);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &jam, &me)
    }

    // `advanceJam` used to live here. It is gone: the server advances the room on its own clock
    // (see `jam_clock.rs`), which is the only way every device can be hearing the same thing at the
    // same moment. Letting clients advance made whoever finished first decide for everybody, and a
    // client comparing the wrong two ids once drained an entire queue in a single pass.

    /// Switches between `open` and `democracy`. Host only — it is the rule everyone else plays by.
    async fn set_jam_mode(&self, ctx: &Context<'_>, mode: String) -> async_graphql::Result<JamPayload> {
        let (jam, me) = current_jam(ctx)?;
        let db = ctx.data::<Db>()?;
        if !jam.host.eq_ignore_ascii_case(&me) {
            return Err(forbidden("only the host can change the mode"));
        }
        db.set_jam_mode(&jam.id, JamMode::parse(&mode))?;
        let updated = db.jam_by_id(&jam.id)?.unwrap_or(jam);
        announce(ctx, db, &updated);
        describe(db, ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?, &updated, &me)
    }
}
