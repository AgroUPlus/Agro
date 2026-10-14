//! Receiving upload bytes and finishing the upload.

use super::*;
use crate::auth::AuthedUser;
use crate::AppState;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::TryStreamExt;
use serde_json::json;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::StreamReader;

/// Streams the bytes in.
///
/// The body is copied straight to disk in fixed-size chunks. It is never collected into a `Vec`:
/// this host has 512 MB of RAM, and a handful of concurrent FLAC uploads buffered in memory is an
/// OOM — which is exactly what the endpoint this replaces did.
#[utoipa::path(
    put,
    path = "/api/v1/library/upload/{upload_id}",
    tag = "library",
    security(("bearer_token" = [])),
    params(
        ("upload_id" = String, Path, description = "Upload id returned by `begin_upload`'s \"upload\" response"),
        ("x-agro-offset" = Option<i64>, Header, description = "Byte offset to resume from; omit to start from the beginning"),
    ),
    request_body(content_type = "application/octet-stream", description = "Raw file bytes, from the resume offset onward"),
    responses(
        (status = 200, description = "Upload accepted"),
        (status = 400, description = "Resume offset is past what the server holds", body = crate::openapi::ApiError),
        (status = 403, description = "That upload does not belong to the caller"),
        (status = 404, description = "No such upload session"),
    ),
)]
pub async fn put_upload(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(upload_id): AxumPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Ok(Some(session)) = state.db.upload_session(&upload_id) else {
        return (StatusCode::NOT_FOUND, "no such upload").into_response();
    };
    if session.user_id != user.username() {
        return (StatusCode::FORBIDDEN, "that upload is not yours").into_response();
    }

    // Where the client believes it is resuming from. Absent means "from the beginning".
    let offset: i64 = headers
        .get("x-agro-offset")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let part_path = state.storage.part_file(&upload_id);
    if let Some(parent) = part_path.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            return server_error(&format!("could not open the spool directory: {err}"));
        }
    }

    let existing = tokio::fs::metadata(&part_path)
        .await
        .map(|m| m.len() as i64)
        .unwrap_or(0);
    if offset > existing {
        return bad_request("resume offset is past the end of what the server holds");
    }

    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        // Never truncate on open: a resumed upload's whole point is the bytes already on disk.
        // The explicit `set_len` below is the only truncation this path does.
        .truncate(false)
        .open(&part_path)
        .await
    {
        Ok(file) => file,
        Err(err) => return server_error(&format!("could not open the part file: {err}")),
    };
    // Truncating to the agreed offset is what makes a resume safe: anything the previous attempt
    // wrote past that point was never acknowledged and must not survive into the middle of the
    // file.
    if let Err(err) = file.set_len(offset as u64).await {
        return server_error(&format!("could not truncate the part file: {err}"));
    }
    if let Err(err) = file.seek(std::io::SeekFrom::Start(offset as u64)).await {
        return server_error(&format!("could not seek the part file: {err}"));
    }

    // How many more bytes this request may write.
    //
    // The declared `sizeBytes` was the *only* thing ever checked, at the moment the upload was
    // declared, while the copy below was unbounded — so a client could declare ten megabytes and
    // then send ten gigabytes. The ceiling is whichever is tighter: what was declared, or what is
    // left of the account's quota.
    let mut allowance = (session.size_bytes - offset).max(0);
    if let Some(quota) = user.account.effective_quota() {
        let used = state.db.spool_bytes_for(user.username()).unwrap_or(0);
        allowance = allowance.min((quota - used - offset).max(0));
    }

    let stream = body
        .into_data_stream()
        .map_err(|err| std::io::Error::other(err.to_string()));
    // `take` is the enforcement: the reader simply ends at the allowance, so an over-long body is
    // truncated rather than trusted, and the hash check in `finish_upload` then rejects it.
    let mut reader = StreamReader::new(stream).take(allowance as u64 + 1);

    let written = match tokio::io::copy(&mut reader, &mut file).await {
        Ok(n) => n as i64,
        Err(err) => {
            // The part file survives, so the client can resume rather than restart.
            let _ = file.flush().await;
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("transfer interrupted: {err}") })),
            )
                .into_response();
        }
    };

    // One byte past the allowance means the client was sending more than it declared or more than
    // it may hold. The part file goes with it: a truncated body is not something to resume from,
    // and leaving it on disk is the unattributed sink this is meant to prevent.
    if written > allowance {
        let _ = file.flush().await;
        drop(file);
        let _ = tokio::fs::remove_file(&part_path).await;
        let _ = state.db.delete_upload(&upload_id);
        let used = state.db.spool_bytes_for(user.username()).unwrap_or(0);
        return match user.account.effective_quota() {
            Some(quota) => quota_exceeded(used, quota),
            None => bad_request("that upload sent more bytes than it declared"),
        };
    }
    if let Err(err) = file.flush().await {
        return server_error(&format!("could not flush the part file: {err}"));
    }
    drop(file);

    let received = offset + written;
    let _ = state.db.set_upload_received(&upload_id, received);

    if received < session.size_bytes {
        // A partial write is a success, not an error: the client sends the rest.
        return Json(json!({ "status": "partial", "received": received })).into_response();
    }

    finish_upload(&state, &session.upload_id, part_path).await
}

/// Verifies, files and indexes a fully received upload.
pub(super) async fn finish_upload(
    state: &AppState,
    upload_id: &str,
    part_path: PathBuf,
) -> Response {
    let Ok(Some(session)) = state.db.upload_session(upload_id) else {
        return server_error("the upload session vanished mid-transfer");
    };

    // Hash what actually arrived. The client's declared hash is a claim; this is the check that
    // the bytes on disk are the file it said it was sending, and it is what stops a truncated or
    // corrupted transfer being filed as though it were fine.
    let actual = match hash_file(&part_path).await {
        Ok(hash) => hash,
        Err(err) => return server_error(&format!("could not hash the upload: {err}")),
    };
    if actual != session.content_hash {
        let _ = tokio::fs::remove_file(&part_path).await;
        let _ = state.db.delete_upload(upload_id);
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "the uploaded bytes do not match the declared contentHash",
                "expected": session.content_hash,
                "actual": actual,
            })),
        )
            .into_response();
    }

    let response = if session.target == "archive" {
        archive(state, &session, &part_path).await
    } else {
        spool(state, &session, &part_path).await
    };

    let _ = state.db.delete_upload(upload_id);
    response
}
