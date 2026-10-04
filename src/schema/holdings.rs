//! Which tracks each device holds, as the devices report them.

use crate::db::Db;
use crate::ws::WsHub;
use async_graphql::{Context, InputObject, Object};
use std::sync::Arc;

use super::library_payload::{to_library_payload, LibraryTrackPayload};
use super::sync_offers::MAX_MISSING;
use super::{authorize, require_own_device};

/// What a device reports it holds. Metadata travels with it so the server can index a file it has
/// never been sent — an index-only library still answers "who has what".
#[derive(InputObject)]
pub struct HoldingInput {
    pub content_hash: String,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<i32>,
    pub disc_no: Option<i32>,
    pub year: Option<i32>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub size_bytes: i64,
    pub format: Option<String>,
    pub bitrate_kbps: Option<i32>,
    /// The device's own handle for the file. Stored opaquely and never interpreted.
    pub local_ref: Option<String>,
}

#[derive(Default)]
pub struct HoldingsQuery;

#[Object]
impl HoldingsQuery {
    /// Every content hash a device has reported, for reconciling against what it actually holds.
    async fn device_holdings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
    ) -> async_graphql::Result<Vec<String>> {
        authorize(ctx, &user_id)?;
        require_own_device(ctx, &device_id)?;
        Ok(ctx
            .data::<Db>()?
            .device_holding_hashes(&user_id, &device_id)?)
    }

    /// Tracks this device could delete without losing them: the server holds a filed copy.
    ///
    /// Both the index *and* the disk are consulted. An `archived_path` pointing at a file that is
    /// no longer there would otherwise talk a device into deleting its only copy — the index is a
    /// record of what this server did, not proof of what is on the disk now.
    ///
    /// The size is checked rather than the hash. Re-hashing every candidate would read the whole
    /// library on every call; a size mismatch catches the truncated and half-restored cases, and
    /// the bytes were already verified against the declared hash when they were archived.
    async fn reclaimable(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        limit: Option<i32>,
    ) -> async_graphql::Result<Vec<LibraryTrackPayload>> {
        authorize(ctx, &user_id)?;
        let storage = ctx.data::<crate::storage::Storage>()?;
        // Nothing is reclaimable when the server is not the durable copy.
        let Some(root) = storage.library_root.as_ref() else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(50).clamp(1, MAX_MISSING as i32) as i64;

        Ok(ctx
            .data::<Db>()?
            .reclaimable_on_device(&user_id, &device_id, limit)?
            .into_iter()
            .filter(|track| {
                let Some(relative) = track.archived_path.as_ref() else {
                    return false;
                };
                let Ok(path) = crate::storage::resolve_within(root, std::path::Path::new(relative))
                else {
                    return false;
                };
                std::fs::metadata(&path).is_ok_and(|m| m.len() == track.size_bytes as u64)
            })
            .map(to_library_payload)
            .collect())
    }
}

#[derive(Default)]
pub struct HoldingsMutation;

#[Object]
impl HoldingsMutation {
    /// Records what a device holds.
    ///
    /// Batched and idempotent, so a client sends its whole library once and only deltas after —
    /// re-sending everything is wasteful but never wrong.
    ///
    /// Each entry also indexes the track, so the server knows about files it has never been sent.
    /// That is what makes index-only mode work: the diff needs metadata, not bytes.
    async fn report_holdings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        tracks: Vec<HoldingInput>,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        let db = ctx.data::<Db>()?;

        let mut accepted = 0;
        for input in tracks {
            // A malformed hash would create an index entry nothing can ever match or fetch.
            if input.content_hash.len() != 64
                || !input.content_hash.bytes().all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            let track = crate::db_library::LibraryTrack {
                content_hash: input.content_hash.clone(),
                title: input.title,
                artist: input.artist,
                album: input.album,
                album_artist: input.album_artist,
                track_no: input.track_no.map(i64::from),
                disc_no: input.disc_no.map(i64::from),
                year: input.year.map(i64::from),
                genre: input.genre,
                duration_ms: input.duration_ms,
                size_bytes: input.size_bytes,
                format: input.format,
                bitrate_kbps: input.bitrate_kbps.map(i64::from),
                // Never cleared by a report: only the server decides where it filed something.
                archived_path: None,
            };
            db.upsert_library_track(&track)?;
            db.upsert_holding(
                &user_id,
                &device_id,
                &input.content_hash,
                input.local_ref.as_deref(),
            )?;
            accepted += 1;
        }

        if accepted > 0 {
            if let Ok(offers) = ctx.data::<crate::offers::OfferBatcher>() {
                offers.note_archived(&user_id);
            }
            if let Ok(ws_hub) = ctx.data::<Arc<WsHub>>() {
                ws_hub.notify_user(
                    &user_id,
                    "LIBRARY_UPDATED",
                    serde_json::json!({ "deviceId": device_id, "count": accepted }),
                );
            }
        }
        Ok(accepted)
    }

    /// Forgets holdings a device no longer has — deleted locally, or moved to the server and
    /// removed. The index entry survives: another device may still hold it.
    async fn forget_holdings(
        &self,
        ctx: &Context<'_>,
        user_id: String,
        device_id: String,
        hashes: Vec<String>,
    ) -> async_graphql::Result<i32> {
        authorize(ctx, &user_id)?;
        require_own_device(ctx, &device_id)?;
        Ok(ctx
            .data::<Db>()?
            .forget_holdings(&user_id, &device_id, &hashes)? as i32)
    }
}
