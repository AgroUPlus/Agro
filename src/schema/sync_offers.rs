//! What a device is missing, and offering to send it.

use crate::db::Db;
use crate::ws::WsHub;
use async_graphql::{Context, Enum, Object};
use std::sync::Arc;

use super::authorize;
use super::library_payload::{to_library_payload_with_sources, LibraryTrackPayload};

/// How this deployment moves music between devices.
///
/// The clients used to decide this individually, from local config that knew nothing about the
/// server — which is why the same account could behave differently on the desktop and the phone.
/// The server holds every fact the decision needs, so it makes the decision and the clients
/// render it.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum SyncMode {
    /// A Navidrome is configured for this account and the server archives. Devices do not need
    /// their own copies — they stream — so downloads are never offered and freeing space is
    /// always safe.
    Navidrome,
    /// The server archives, but there is no Navidrome to stream from. A device that lacks a
    /// recording is offered the file itself.
    PeerToPeer,
    /// No library root: the server keeps the index and relays through the spool, but never keeps
    /// the bytes. It cannot be the durable copy, so it never suggests deleting one.
    IndexOnly,
}

/// Most a diff returns in one go. The offer is a prompt, not a migration plan.
pub(super) const MAX_MISSING: i64 = 200;

#[derive(Default)]
pub struct SyncOffersQuery;

#[Object]
impl SyncOffersQuery {
    /// Tracks another of this account's devices holds that this one does not.
    ///
    /// Matched on the recording rather than the bytes, so owning a different rip of the same song
    /// counts as having it — see `Db::missing_on_device`.
    async fn missing_on_device(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<LibraryTrackPayload>> {
        authorize(ctx, &user_id)?;
        let limit = limit.unwrap_or(50).clamp(1, MAX_MISSING as i32) as i64;
        let db = ctx.data::<Db>()?;
        let ws_hub = ctx.data::<Arc<WsHub>>().ok();
        let tracks = db.missing_on_device(&user_id, &device_id, limit)?;
        Ok(tracks
            .into_iter()
            .map(|t| to_library_payload_with_sources(db, ws_hub.map(|a| a.as_ref()), &user_id, t))
            .collect())
    }

    /// How this account should sync — the one answer both clients branch on.
    ///
    /// Derived rather than configured: a deployment that archives and has a Navidrome address on
    /// file is a streaming setup whether or not anyone said so, and a deployment with no library
    /// root cannot be anything but index-only.
    async fn sync_mode(
        &self,
        ctx: &Context<'_>,
        user_id: String,
    ) -> async_graphql::Result<SyncMode> {
        authorize(ctx, &user_id)?;
        if !ctx.data::<crate::storage::Storage>()?.archives() {
            return Ok(SyncMode::IndexOnly);
        }
        // Presence is the whole question, and it is now the only part of the settings this server
        // can answer: the address itself is inside a blob it has no key for. The client states the
        // bit explicitly when it saves.
        let has_navidrome = ctx
            .data::<Db>()?
            .get_synced_settings(&user_id)?
            .is_some_and(|s| s.has_server_url);

        Ok(if has_navidrome {
            SyncMode::Navidrome
        } else {
            SyncMode::PeerToPeer
        })
    }
}

#[derive(Default)]
pub struct SyncOffersMutation;

#[Object]
impl SyncOffersMutation {
    /// Nudges one device to look at what it is missing.
    ///
    /// Addressed to that device alone rather than broadcast, so the other devices on the account
    /// are not prompted about a library that is not theirs.
    async fn offer_sync(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        crate::features::Feature::LibraryTransfers.require(ctx.data::<Db>()?)?;
        let missing = ctx
            .data::<Db>()?
            .missing_on_device(&user_id, &device_id, MAX_MISSING)?;
        if missing.is_empty() {
            return Ok(0);
        }
        if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
            ws_hub.notify_device(
                &user_id,
                &device_id,
                "SYNC_OFFER",
                serde_json::json!({
                    "count": missing.len(),
                    "sample": missing.iter().take(3)
                        .map(|t| format!("{} — {}", t.artist, t.title))
                        .collect::<Vec<_>>(),
                }),
            );
        }
        Ok(missing.len() as i32)
    }
}
