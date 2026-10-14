//! Listen-along and invites.

use super::*;
use crate::db::Db;
use crate::schema::{bounded, caller, require_admin};
use async_graphql::{Context, Object};

#[derive(Default)]
pub struct SocialSharingMutation;

#[Object]
impl SocialSharingMutation {
    /// Tunes the caller in to a friend's playback.
    ///
    /// Gated on the host's now-playing flag, because following someone's playback is a strictly
    /// larger thing than seeing it, and there is no sensible reading in which the smaller one is
    /// closed and the larger one is open.
    async fn start_listen_along(
        &self,
        ctx: &Context<'_>,
        host: String,
        device_id: Option<String>,
    ) -> async_graphql::Result<ListenAlongPayload> {
        let authed = caller(ctx)?;
        let subject = require_visible(ctx, &host, Surface::NowPlaying)?;
        let db = ctx.data::<Db>()?;
        crate::features::Feature::ListenAlong.require(db)?;
        let viewer_device = match device_id.as_deref() {
            Some(raw) => Some(bounded(raw, 128, "deviceId")?),
            None => None,
        };

        if subject.username.eq_ignore_ascii_case(authed.username()) {
            return Err("You are already listening to yourself".into());
        }

        db.set_listen_along(authed.username(), &subject.username)?;
        let ws_hub = ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?;
        let payload = ListenAlongPayload {
            listeners: db.listeners_of(&subject.username)?,
            now_playing: now_playing_for_viewer(
                db,
                ws_hub,
                &subject.username,
                authed.username(),
                viewer_device.as_deref(),
            )?,
            host: subject.username.clone(),
        };

        // The host is told who joined, so their client can show the count without polling.
        ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
            &subject.username,
            "LISTEN_ALONG",
            serde_json::json!({
                "host": subject.username,
                "listeners": payload.listeners,
            }),
        );
        Ok(payload)
    }

    async fn stop_listen_along(&self, ctx: &Context<'_>) -> async_graphql::Result<bool> {
        let authed = caller(ctx)?;
        let db = ctx.data::<Db>()?;

        let host = db.listen_along_of(authed.username())?.map(|s| s.host);
        let stopped = db.clear_listen_along(authed.username())?;
        if let Some(host) = host {
            ctx.data::<std::sync::Arc<crate::ws::WsHub>>()?.notify_user(
                &host,
                "LISTEN_ALONG",
                serde_json::json!({ "host": host, "listeners": db.listeners_of(&host)? }),
            );
        }
        Ok(stopped)
    }

    async fn create_invite(
        &self,
        ctx: &Context<'_>,
        max_uses: Option<i64>,
        ttl_hours: Option<i64>,
    ) -> async_graphql::Result<InvitePayload> {
        let authed = require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        let invite = db.create_invite(authed.username(), max_uses.unwrap_or(1), ttl_hours)?;
        Ok(InvitePayload {
            code: invite.code,
            created_by: invite.created_by,
            created_at: invite.created_at,
            expires_at: invite.expires_at,
            max_uses: invite.max_uses,
            used_count: invite.used_count,
            revoked: invite.revoked,
        })
    }

    /// Deletes an invite outright.
    ///
    /// Revoking and deleting are different acts: revoking stops a code working the moment you
    /// realise it has escaped, and keeps the record. Deleting is the tidying-up afterwards, once a
    /// code is spent or dead and is only taking up room on screen.
    async fn delete_invite(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        let db = ctx.data::<Db>()?;
        Ok(db.delete_invite(code.trim())?)
    }

    async fn revoke_invite(&self, ctx: &Context<'_>, code: String) -> async_graphql::Result<bool> {
        require_admin(ctx)?;
        ctx.data::<Db>()?.revoke_invite(&code).map_err(Into::into)
    }
}
