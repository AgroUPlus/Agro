//! Reading the friend graph, presence and listen-along.

use super::*;
use crate::db::Db;
use crate::db_social::FriendState;
use crate::schema::{bounded, caller};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct SocialFriendsQuery;

#[Object]
impl SocialFriendsQuery {
    /// Accepted friends, each with what they are playing when they allow that to be seen.
    /// `device_id` carries the same meaning it does on [`Self::friends_now_playing`]: the sealed
    /// copy of a session is addressed to one device's key, so it can only be picked once the asking
    /// device is named.
    ///
    /// It matters here and not only there because this query *seeds the presence feed* — a client
    /// refreshing the graph gets each friend's session in the same answer. Without a device id the
    /// sessions in that answer are placeholders, and a sealed friend would show as "Private
    /// Session" from every foreground refresh until a socket frame happened to replace it.
    async fn friends(
        &self,
        ctx: &Context<'_>,
        device_id: Option<String>,
    ) -> async_graphql::Result<Vec<FriendEdgePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        let viewer_device = match device_id.as_deref() {
            Some(raw) => Some(bounded(raw, 128, "deviceId")?),
            None => None,
        };

        let mut edges = Vec::new();
        for profile in db.friends(authed.username())? {
            // Not an error when it is closed — a friend who has not opted in is simply a friend
            // with nothing showing, which is different from a friend who is offline.
            let now_playing = if profile.shows_now_playing() {
                live_now_playing(db, &profile.username)?.map(|mut now| {
                    attach_sealed_presence(
                        db,
                        &mut now,
                        &profile.username,
                        authed.username(),
                        viewer_device.as_deref(),
                    );
                    now
                })
            } else {
                None
            };
            edges.push(FriendEdgePayload {
                profile: profile_payload(&profile, Some(FriendState::Accepted), false),
                now_playing,
            });
        }
        Ok(edges)
    }

    /// Requests in both directions: ones to answer, and ones already sent.
    async fn friend_requests(
        &self,
        ctx: &Context<'_>,
    ) -> async_graphql::Result<Vec<ProfilePayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let mut payloads: Vec<ProfilePayload> = db
            .incoming_requests(authed.username())?
            .iter()
            .map(|e| profile_payload(&e.profile, Some(FriendState::Pending), false))
            .collect();
        payloads.extend(
            db.outgoing_requests(authed.username())?
                .iter()
                .map(|e| profile_payload(&e.profile, Some(FriendState::Pending), true)),
        );
        Ok(payloads)
    }

    /// Just the presence feed: friends who are playing something and allow it to be seen.
    ///
    /// `device_id` names the device asking, so a friend running a sealed session can be handed the
    /// copy sealed to that device's key. It is optional because the device token does not carry
    /// one — the id is chosen by the client and published to the key registry — and because the
    /// dashboard has no device key to open a copy with. Omitting it costs nothing but the sealed
    /// metadata, which an asker with no key could not have read anyway.
    async fn friends_now_playing(
        &self,
        ctx: &Context<'_>,
        device_id: Option<String>,
    ) -> async_graphql::Result<Vec<FriendNowPlaying>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::FriendPresence.require(db)?;
        let viewer_device = match device_id.as_deref() {
            Some(raw) => Some(bounded(raw, 128, "deviceId")?),
            None => None,
        };

        let mut playing = Vec::new();
        for profile in db.friends(authed.username())? {
            // Unchanged, and still the only gate that matters: sealing decides what the *server*
            // can read, never who is allowed to look. A friend who has not opted in is absent from
            // this list whether or not a copy was sealed for them.
            if !profile.shows_now_playing() {
                continue;
            }
            if let Some(mut now) = live_now_playing(db, &profile.username)? {
                attach_sealed_presence(
                    db,
                    &mut now,
                    &profile.username,
                    authed.username(),
                    viewer_device.as_deref(),
                );
                playing.push(now);
            }
        }
        Ok(playing)
    }

    /// How much the caller's listening overlaps a friend's. Gated on the friend's stats flag.
    async fn taste_match(
        &self,
        ctx: &Context<'_>,
        username: String,
    ) -> async_graphql::Result<TasteMatch> {
        let authed = caller(ctx)?;
        let subject = require_visible(ctx, &username, Surface::Stats)?;
        let db = ctx.data::<Db>()?;
        let now = chrono::Utc::now().timestamp();

        let mine =
            crate::stats::compute(&db.scrobble_rows(authed.username(), None, None)?, 50, now);
        let theirs =
            crate::stats::compute(&db.scrobble_rows(&subject.username, None, None)?, 50, now);

        Ok(taste_match(&mine, &theirs))
    }

    /// Who the caller is currently tuned in to, if anyone.
    ///
    /// `device_id` serves the same purpose as it does on `friends_now_playing`: following a sealed
    /// session means being handed the copy sealed to this device, or the placeholder.
    async fn listen_along(
        &self,
        ctx: &Context<'_>,
        device_id: Option<String>,
    ) -> async_graphql::Result<Option<ListenAlongPayload>> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::ListenAlong.require(db)?;
        let viewer_device = match device_id.as_deref() {
            Some(raw) => Some(bounded(raw, 128, "deviceId")?),
            None => None,
        };

        let Some(session) = db.listen_along_of(authed.username())? else {
            return Ok(None);
        };
        // The friendship may have been withdrawn since the session started, in which case the row
        // is stale and the session is over.
        if require_visible(ctx, &session.host, Surface::NowPlaying).is_err() {
            db.clear_listen_along(authed.username())?;
            return Ok(None);
        }
        let ws_hub = ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?;
        Ok(Some(ListenAlongPayload {
            listeners: db.listeners_of(&session.host)?,
            now_playing: now_playing_for_viewer(
                db,
                ws_hub,
                &session.host,
                authed.username(),
                viewer_device.as_deref(),
            )?,
            host: session.host,
        }))
    }
}
