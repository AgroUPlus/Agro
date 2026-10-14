//! Beginning a resumable upload and checking quota.

use super::*;
use crate::auth::AuthedUser;
use crate::db_library::LibraryTrack;
use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Refuse anything larger than this outright. A 2 GB "audio file" is a mistake or an attack, and
/// the disk it would land on is measured in single-digit GB.
pub(super) const MAX_UPLOAD_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// How long an unfinished upload's `.part` file is kept before the sweeper reclaims it.
pub(super) const UPLOAD_TTL_HOURS: i64 = 24;

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BeginUpload {
    pub device_id: String,
    /// Lowercase hex SHA-256 of the file, computed by the client and re-verified here.
    pub content_hash: String,
    pub size_bytes: i64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub year: Option<i64>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub format: Option<String>,
    pub bitrate_kbps: Option<i64>,
    /// The client's own handle for the file — a content URI, a path. Stored opaquely so the
    /// device can find its own copy again; never interpreted here.
    pub local_ref: Option<String>,
    /// File extension, used for the archived filename. Taken from the client's filename, then
    /// stripped to alphanumerics by `storage::relative_path`.
    pub extension: Option<String>,
}

/// `rename_all` on the enum renames the *variants*; the per-variant attributes are what rename
/// their fields. Without those the clients receive `upload_id` while every other field on the wire
/// is camelCase.
#[derive(Serialize, utoipa::ToSchema)]
#[serde(tag = "status")]
pub enum BeginUploadResponse {
    /// The server already has these bytes. Nothing is transferred — by far the most common
    /// outcome once a library has been uploaded once.
    #[serde(rename = "exists", rename_all = "camelCase")]
    Exists { content_hash: String },
    /// Send the bytes to `PUT .../upload/{uploadId}`, starting at `offset`.
    #[serde(rename = "upload", rename_all = "camelCase")]
    Upload { upload_id: String, offset: i64 },
}

/// Declares a file and finds out whether it needs sending.
#[utoipa::path(
    post,
    path = "/api/v1/library/upload",
    tag = "library",
    security(("bearer_token" = [])),
    request_body = BeginUpload,
    responses(
        (status = 200, description = "Either \"exists\" (nothing to send) or \"upload\" (send bytes to the PUT route)", body = BeginUploadResponse),
        (status = 400, description = "Invalid content hash, size, or device id", body = crate::openapi::ApiError),
    ),
)]
pub async fn begin_upload(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    Json(body): Json<BeginUpload>,
) -> Response {
    let user_id = user.username().to_string();

    if !is_sha256_hex(&body.content_hash) {
        return bad_request("contentHash must be a lowercase hex SHA-256");
    }
    if body.size_bytes <= 0 || body.size_bytes > MAX_UPLOAD_BYTES {
        return bad_request("sizeBytes is outside the accepted range");
    }
    if body.device_id.trim().is_empty() {
        return bad_request("deviceId is required");
    }

    // Index the metadata regardless of whether the bytes travel: knowing this device holds this
    // recording is the whole point of the index, and is what the diff reads.
    let track = LibraryTrack {
        content_hash: body.content_hash.clone(),
        title: body.title.clone(),
        artist: body.artist.clone(),
        album: body.album.clone(),
        album_artist: body.album_artist.clone(),
        track_no: body.track_no,
        disc_no: body.disc_no,
        year: body.year,
        genre: body.genre.clone(),
        duration_ms: body.duration_ms,
        size_bytes: body.size_bytes,
        format: body.format.clone(),
        bitrate_kbps: body.bitrate_kbps,
        archived_path: None,
    };
    if let Err(err) = state.db.upsert_library_track(&track) {
        return server_error(&format!("could not index that track: {err}"));
    }
    if let Err(err) = state.db.upsert_holding(
        &user_id,
        &body.device_id,
        &body.content_hash,
        body.local_ref.as_deref(),
    ) {
        return server_error(&format!("could not record that holding: {err}"));
    }

    // Already here, so there is nothing to send. This is the single biggest saving in the whole
    // feature — a re-upload of a known library moves no audio at all.
    //
    // Scoped to what *this* account may legitimately have. Both halves used to be global: a guest
    // could declare any hash and be told it already existed, which turned this endpoint into an
    // oracle for the admin's library — and handed the guest a holding row on the admin's file
    // without a byte moving. The archive is the admin's, so only the admin can dedup against it,
    // and a spooled file only counts for the account it was spooled for.
    let archived = user.is_admin()
        && matches!(
            state.db.library_track(&body.content_hash),
            Ok(Some(ref t)) if t.archived_path.is_some()
        );
    let spooled = matches!(
        state.db.spool_owner(&body.content_hash),
        Ok(Some(ref owner)) if owner == &user_id
    );
    if archived || spooled {
        return Json(BeginUploadResponse::Exists {
            content_hash: body.content_hash,
        })
        .into_response();
    }

    // An interrupted transfer of the same file resumes where it stopped rather than restarting —
    // which over a phone connection is the difference between finishing and never finishing.
    if let Ok(Some(existing)) =
        state
            .db
            .resumable_upload(&user_id, &body.device_id, &body.content_hash)
    {
        let on_disk = tokio::fs::metadata(state.storage.part_file(&existing.upload_id))
            .await
            .map(|m| m.len() as i64)
            .unwrap_or(0);
        // Trust the file, not the bookkeeping: a crash between the write and the row update would
        // otherwise have the client resume from the wrong place and corrupt the result.
        let offset = on_disk.min(existing.received_bytes.max(0));
        return Json(BeginUploadResponse::Upload {
            upload_id: existing.upload_id,
            offset,
        })
        .into_response();
    }

    // Archiving is allowed for admins or users with can_archive permission.
    let target = if state.storage.archives() && user.account.can_archive() {
        "archive"
    } else {
        "spool"
    };

    // Checked before a byte is accepted, and again while they arrive: this is only the declared
    // size, and the declaration is the client's.
    if let Some(quota) = user.account.effective_quota() {
        let used = state.db.spool_bytes_for(&user_id).unwrap_or(0);
        if used + body.size_bytes > quota {
            return quota_exceeded(used, quota);
        }
    }

    let upload_id = uuid::Uuid::new_v4().to_string();
    if let Err(err) = state.db.create_upload(
        &upload_id,
        &user_id,
        &body.device_id,
        &body.content_hash,
        body.size_bytes,
        target,
        body.extension.as_deref(),
        UPLOAD_TTL_HOURS,
    ) {
        return server_error(&format!("could not start that upload: {err}"));
    }

    Json(BeginUploadResponse::Upload {
        upload_id,
        offset: 0,
    })
    .into_response()
}

/// Refuses an upload that would not fit, and says by how much.
///
/// A number rather than a bare "no": a client that cannot see the ceiling cannot show the user
/// what to delete.
pub(super) fn quota_exceeded(used: i64, quota: i64) -> Response {
    (
        StatusCode::from_u16(507).unwrap_or(StatusCode::INSUFFICIENT_STORAGE),
        Json(json!({
            "error": "quota exceeded",
            "usedBytes": used,
            "quotaBytes": quota,
        })),
    )
        .into_response()
}
