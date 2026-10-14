//! Fetching tracks and covers.

use super::*;
use crate::auth::AuthedUser;
use crate::storage::{self};
use crate::AppState;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use std::path::{Path, PathBuf};

/// Hands a spooled file to the device collecting it.
#[utoipa::path(
    get,
    path = "/api/v1/library/fetch/{content_hash}",
    tag = "library",
    security(("bearer_token" = [])),
    params(("content_hash" = String, Path, description = "Lowercase hex SHA-256 of the file")),
    responses(
        (status = 200, description = "Raw file bytes", content_type = "application/octet-stream"),
        (status = 400, description = "Not a content hash", body = crate::openapi::ApiError),
        (status = 404, description = "Nothing to fetch under that hash, or it does not belong to the caller"),
    ),
)]
pub async fn fetch(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(content_hash): AxumPath<String>,
) -> Response {
    if !is_sha256_hex(&content_hash) {
        return bad_request("not a content hash");
    }

    // Two places a file can be, and both are served: the spool (staged for a peer) and the
    // library (already filed). Serving the archive too is what makes this work at all in the
    // common setup — with a library root configured every upload is archived rather than spooled,
    // so a spool-only endpoint would have nothing to hand back and peer sync would never fire.
    let path = match resolve_fetchable(&state, &user, &content_hash) {
        Some(path) => path,
        // One answer for "not yours" and "not here" alike: no point confirming a hash exists to
        // someone probing for it.
        None => return (StatusCode::NOT_FOUND, "nothing to fetch under that hash").into_response(),
    };
    let Ok(file) = tokio::fs::File::open(&path).await else {
        return (StatusCode::NOT_FOUND, "nothing spooled under that hash").into_response();
    };
    let len = file.metadata().await.map(|m| m.len()).unwrap_or(0);

    let stream = tokio_util::io::ReaderStream::new(file);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// Serves an album's cover art.
///
/// Authenticated like every other library route — artwork spells out what is in someone's library
/// just as plainly as a track listing does.
///
/// Aggressively cacheable: the key is a hash of the album's identity and the file under it never
/// changes in place, so a browser that has fetched one never needs to ask for it again. That
/// matters here because the library grid asks for a screenful of these at once.
#[utoipa::path(
    get,
    path = "/api/v1/cover/{album_key}",
    tag = "library",
    security(("bearer_token" = [])),
    params(("album_key" = String, Path, description = "32-character hex key identifying the album")),
    responses(
        (status = 200, description = "Cover art bytes", content_type = "application/octet-stream"),
        (status = 400, description = "Not an album key", body = crate::openapi::ApiError),
        (status = 404, description = "No cover for that album, or caller is not an administrator"),
    ),
)]
pub async fn cover(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(album_key): AxumPath<String>,
) -> Response {
    // Covers belong to archived albums, and the archive is the admin's. The identity used to be
    // bound as `_user` and discarded, which made this a picture-shaped listing of the library for
    // anyone with a token.
    if !user.is_admin() {
        return (StatusCode::NOT_FOUND, "no cover for that album").into_response();
    }

    // The key is generated as hex; anything else is not a key this server minted, and refusing it
    // here is what keeps the filename below from being able to express a path.
    if album_key.len() != 32 || !album_key.bytes().all(|b| b.is_ascii_hexdigit()) {
        return bad_request("not an album key");
    }

    let Ok(Some(extension)) = state.db.cover_extension(&album_key) else {
        return (StatusCode::NOT_FOUND, "no cover for that album").into_response();
    };
    let path = state.storage.cover_file(&album_key, &extension);
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, "no cover for that album").into_response();
    };

    let content_type = if extension == "png" {
        "image/png"
    } else {
        "image/jpeg"
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type.to_string()),
            (header::CACHE_CONTROL, "private, max-age=604800".to_string()),
            (header::ETAG, format!("\"{album_key}\"")),
        ],
        bytes,
    )
        .into_response()
}

/// Where a fetchable file lives, if this account may have it.
///
/// Two stores, with two different rules.
///
/// **The spool** is served to the account it was staged for. That claim is specific — a spooled
/// file was put there *for* someone — and it is what makes peer sync work.
///
/// **The archive is the admin's library, and only the admin may fetch from it.** This used to have
/// no owner check at all: any authenticated account could pull any archived file it could name a
/// hash for. Hashes are not secret — a guest learns them by declaring one and being told it
/// already exists — so that was the whole library, readable by every guest.
pub(super) fn resolve_fetchable(
    state: &AppState,
    user: &AuthedUser,
    content_hash: &str,
) -> Option<PathBuf> {
    if matches!(state.db.spool_owner(content_hash), Ok(Some(ref owner)) if owner == user.username())
    {
        let path = state.storage.spool_file(content_hash);
        if path.exists() {
            return Some(path);
        }
    }

    if !user.is_admin() {
        return None;
    }

    let track = state.db.library_track(content_hash).ok().flatten()?;
    let relative = track.archived_path?;
    let root = state.storage.library_root.as_ref()?;
    // Re-checked rather than trusted: the stored path came from this server, but it is still a
    // path being joined onto a root, and that is the one operation in this file worth being
    // paranoid about twice.
    let path = storage::resolve_within(root, Path::new(&relative)).ok()?;
    path.exists().then_some(path)
}

// ── Helpers ─────────────────────────────────────────────────────────────────────────────────
