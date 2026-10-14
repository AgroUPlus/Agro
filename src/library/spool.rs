//! Spooling, eviction and the storage sweep.

use super::*;
use crate::AppState;
use axum::{
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::path::PathBuf;

/// Parks the bytes for a peer to collect, evicting whatever it takes to stay under the cap.
pub(super) async fn spool(
    state: &AppState,
    session: &crate::db_library::UploadSession,
    part_path: &PathBuf,
) -> Response {
    let target = state.storage.spool_file(&session.content_hash);
    if tokio::fs::rename(part_path, &target).await.is_err() {
        if let Err(err) = tokio::fs::copy(part_path, &target).await {
            return server_error(&format!("could not spool the file: {err}"));
        }
        let _ = tokio::fs::remove_file(part_path).await;
    }

    if let Err(err) = state.db.spool_insert(
        &session.content_hash,
        session.size_bytes,
        &session.device_id,
        &session.user_id,
        state.storage.spool_ttl_hours,
    ) {
        return server_error(&format!("could not record the spooled file: {err}"));
    }

    evict_spool(state, &session.user_id).await;

    state.ws_hub.notify_user(
        &session.user_id,
        "LIBRARY_UPDATED",
        json!({ "contentHash": session.content_hash, "spooled": true }),
    );

    Json(json!({ "status": "spooled" })).into_response()
}

/// Drops expired spool entries, then the oldest, until the spool is back under its budget.
pub async fn evict_spool(state: &AppState, user_id: &str) {
    // The account's own quota is the budget, falling back to the deployment-wide setting for an
    // uncapped account — the admin, who is bounded by the disk rather than by a number.
    let budget = state
        .db
        .account(user_id)
        .ok()
        .flatten()
        .and_then(|a| a.effective_quota())
        .unwrap_or(state.storage.spool_max_bytes as i64);

    let Ok(doomed) = state.db.spool_evictable_for(user_id, budget) else {
        return;
    };
    for (hash, _) in doomed {
        let _ = tokio::fs::remove_file(state.storage.spool_file(&hash)).await;
        let _ = state.db.spool_delete(&hash);
    }
}

/// Sweeps every account's spool, and reclaims uploads that were started and abandoned.
///
/// Eviction only ever ran as a side effect of a successful spool write, which meant it never ran
/// at all on a deployment with a library root — every upload is archived there, so nothing was
/// spooled and nothing was swept. Abandoned `.part` files were worse: `expired_uploads` existed
/// with no callers whatsoever, so a client that started an upload and vanished left bytes on disk
/// that nothing would ever attribute or remove.
pub async fn sweep_storage(state: &AppState) {
    for user_id in state.db.spool_users().unwrap_or_default() {
        evict_spool(state, &user_id).await;
    }

    for upload in state.db.expired_uploads().unwrap_or_default() {
        let _ = tokio::fs::remove_file(state.storage.part_file(&upload)).await;
        let _ = state.db.delete_upload(&upload);
    }
}
