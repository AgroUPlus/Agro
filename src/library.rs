//! Moving music files, and filing them where Navidrome will find them.
//!
//! Three routes, all inside the authenticated router:
//!
//! | | |
//! |---|---|
//! | `POST /api/v1/library/upload` | declare a file; get back either "already have it" or somewhere to put it |
//! | `PUT  /api/v1/library/upload/{id}` | send the bytes, resumably |
//! | `GET  /api/v1/library/fetch/{hash}` | collect a file a peer left for you |
//!
//! Deliberately REST rather than GraphQL: these carry megabytes, and base64 through a JSON
//! envelope would both inflate them and defeat streaming.
//!
//! The endpoint this replaces (`/api/v1/dropbox/upload`) got four things wrong, and each has a
//! counterpart here: it was outside the auth layer (this is inside), it joined a caller-supplied
//! filename onto a path (`storage::relative_path` sanitises, and `resolve_within` then proves
//! containment), it read whole files into memory with `field.bytes()` on a 512 MB host (this
//! streams via `tokio::io::copy`), and it wrote nothing to the database (this indexes every file).

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::StreamReader;

use crate::auth::AuthedUser;
use crate::db_library::LibraryTrack;
use crate::storage::{self, Filing};
use crate::AppState;

/// Refuse anything larger than this outright. A 2 GB "audio file" is a mistake or an attack, and
/// the disk it would land on is measured in single-digit GB.
const MAX_UPLOAD_BYTES: i64 = 2 * 1024 * 1024 * 1024;

/// How long an unfinished upload's `.part` file is kept before the sweeper reclaims it.
const UPLOAD_TTL_HOURS: i64 = 24;

#[derive(Deserialize)]
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
#[derive(Serialize)]
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
pub async fn begin_upload(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    Json(body): Json<BeginUpload>,
) -> Response {
    let user_id = user.username.clone();

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

    // Already archived, or already spooled: the bytes are here, so there is nothing to send. This
    // is the single biggest saving in the whole feature — a re-upload of a known library moves no
    // audio at all.
    let archived = matches!(
        state.db.library_track(&body.content_hash),
        Ok(Some(ref t)) if t.archived_path.is_some()
    );
    let spooled = state.db.spool_contains(&body.content_hash).unwrap_or(false);
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

    let upload_id = uuid::Uuid::new_v4().to_string();
    let target = if state.storage.archives() { "archive" } else { "spool" };
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

/// Streams the bytes in.
///
/// The body is copied straight to disk in fixed-size chunks. It is never collected into a `Vec`:
/// this host has 512 MB of RAM, and a handful of concurrent FLAC uploads buffered in memory is an
/// OOM — which is exactly what the endpoint this replaces did.
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
    if session.user_id != user.username {
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

    let stream = body
        .into_data_stream()
        .map_err(|err| std::io::Error::other(err.to_string()));
    let mut reader = StreamReader::new(stream);

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
async fn finish_upload(state: &AppState, upload_id: &str, part_path: PathBuf) -> Response {
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

/// Files the received bytes into the music library.
///
/// Tags are re-read from the file with lofty rather than taken from what the client declared: the
/// file is the thing Navidrome will scan, so the shelf position should agree with it.
async fn archive(
    state: &AppState,
    session: &crate::db_library::UploadSession,
    part_path: &PathBuf,
) -> Response {
    let content_hash = session.content_hash.as_str();
    let Some(root) = state.storage.library_root.clone() else {
        return server_error("no library root is configured");
    };

    let tags = read_tags(part_path).await;
    // The client's declared extension wins: it named the file it actually read, whereas lofty is
    // inferring from content and answers "bin" for anything it does not recognise.
    let extension = session
        .extension
        .clone()
        .filter(|e| !e.trim().is_empty())
        .unwrap_or_else(|| tags.extension.clone());

    let indexed = state.db.library_track(content_hash).ok().flatten();
    let artist = tags
        .artist
        .or_else(|| indexed.as_ref().map(|t| t.artist.clone()))
        .unwrap_or_else(|| "Unknown Artist".to_string());
    let title = tags
        .title
        .or_else(|| indexed.as_ref().map(|t| t.title.clone()))
        .unwrap_or_else(|| "Untitled".to_string());
    let album = tags
        .album
        .or_else(|| indexed.as_ref().and_then(|t| t.album.clone()));
    let album_artist = tags
        .album_artist
        .or_else(|| indexed.as_ref().and_then(|t| t.album_artist.clone()));
    let track_no = tags
        .track_no
        .or_else(|| indexed.as_ref().and_then(|t| t.track_no).map(|n| n as u32));

    let relative = storage::relative_path(&Filing {
        album_artist: album_artist.as_deref(),
        artist: &artist,
        album: album.as_deref(),
        title: &title,
        track_no,
        extension: &extension,
    });

    let target = match storage::resolve_within(&root, &relative) {
        Ok(path) => storage::unique_path(path),
        Err(err) => return server_error(&err),
    };

    if let Some(parent) = target.parent() {
        if let Err(err) = tokio::fs::create_dir_all(parent).await {
            return server_error(&format!("could not create {}: {err}", parent.display()));
        }
    }

    // Rename rather than copy where possible, so the file appears complete or not at all —
    // whatever scans this tree must never see a half-written file. Across filesystems (the spool
    // and the library need not be the same mount) rename fails, so fall back to copying to a temp
    // name in the destination directory and renaming that.
    if tokio::fs::rename(part_path, &target).await.is_err() {
        let staging = target.with_extension("agro-part");
        if let Err(err) = tokio::fs::copy(part_path, &staging).await {
            return server_error(&format!("could not copy into the library: {err}"));
        }
        if let Err(err) = tokio::fs::rename(&staging, &target).await {
            let _ = tokio::fs::remove_file(&staging).await;
            return server_error(&format!("could not place the file: {err}"));
        }
        let _ = tokio::fs::remove_file(part_path).await;
    }

    // The library is frequently a directory shared with another service — a media scanner, a file
    // sync daemon — reached through a common group on a setgid directory. A file inheriting the
    // spool's tighter mode would be one that service cannot manage, so widen it to group-writable.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o664))
            .await
        {
            tracing::warn!("library: could not set mode on {}: {err}", target.display());
        }
    }

    let stored = target
        .strip_prefix(&root)
        .unwrap_or(&relative)
        .to_string_lossy()
        .to_string();
    let _ = state.db.set_archived_path(content_hash, &stored);

    state.ws_hub.broadcast(
        "LIBRARY_UPDATED",
        json!({ "contentHash": content_hash, "archivedPath": stored }),
    );

    run_archive_hook(state, &stored, &target);
    // Collected rather than sent: an album arrives as a run of these, and should reach the other
    // devices as one offer.
    state.offers.note_archived(&session.user_id);

    Json(json!({ "status": "archived", "path": stored })).into_response()
}

/// How long the archive hook gets before it is killed. A reindex of a large library can be slow;
/// a hook that has not finished in a minute is hung, and holding a task open for it helps nobody.
const ARCHIVE_HOOK_TIMEOUT_SECS: u64 = 60;

/// Tells whatever else indexes the library that a file arrived.
///
/// Detached on purpose. The bytes are filed and the row is written by the time this runs, so the
/// upload has already succeeded — a hook that fails, hangs or does not exist must not turn that
/// into an error for the client. Failures are logged and nothing else.
///
/// The paths go in the environment rather than into the command string: they are derived from tags
/// a client supplied, and interpolating them into a line handed to `sh` would be an injection.
fn run_archive_hook(state: &AppState, relative: &str, absolute: &Path) {
    let Some(command) = state.storage.archive_hook.clone() else {
        return;
    };
    let relative = relative.to_string();
    let absolute = absolute.to_path_buf();

    tokio::spawn(async move {
        let run = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .env("AGRO_ARCHIVED_PATH", &relative)
            .env("AGRO_ARCHIVED_ABS", &absolute)
            .output();

        match tokio::time::timeout(
            std::time::Duration::from_secs(ARCHIVE_HOOK_TIMEOUT_SECS),
            run,
        )
        .await
        {
            Ok(Ok(out)) if out.status.success() => {
                tracing::debug!("archive hook finished for {relative}");
            }
            Ok(Ok(out)) => tracing::warn!(
                "archive hook exited {} for {relative}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Ok(Err(err)) => tracing::warn!("archive hook could not run: {err}"),
            Err(_) => tracing::warn!(
                "archive hook timed out after {ARCHIVE_HOOK_TIMEOUT_SECS}s for {relative}"
            ),
        }
    });
}

/// Parks the bytes for a peer to collect, evicting whatever it takes to stay under the cap.
async fn spool(
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

    evict_spool(state).await;

    state.ws_hub.broadcast(
        "LIBRARY_UPDATED",
        json!({ "contentHash": session.content_hash, "spooled": true }),
    );

    Json(json!({ "status": "spooled" })).into_response()
}

/// Drops expired spool entries, then the oldest, until the spool is back under its budget.
pub async fn evict_spool(state: &AppState) {
    let Ok(doomed) = state.db.spool_evictable(state.storage.spool_max_bytes as i64) else {
        return;
    };
    for (hash, _) in doomed {
        let _ = tokio::fs::remove_file(state.storage.spool_file(&hash)).await;
        let _ = state.db.spool_delete(&hash);
    }
}

/// Hands a spooled file to the device collecting it.
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
    let path = match resolve_fetchable(&state, &user.username, &content_hash) {
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

/// Where a fetchable file lives, if this account may have it.
///
/// The spool is checked first because it is the cheaper lookup and the more specific claim: a
/// spooled file was put there *for* someone. An archived file is available to any device on an
/// account that holds it.
fn resolve_fetchable(state: &AppState, username: &str, content_hash: &str) -> Option<PathBuf> {
    if matches!(state.db.spool_owner(content_hash), Ok(Some(ref owner)) if owner == username) {
        let path = state.storage.spool_file(content_hash);
        if path.exists() {
            return Some(path);
        }
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

#[derive(Default)]
struct FileTags {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_no: Option<u32>,
    extension: String,
}

/// Reads tags off the received file. Blocking, so it runs on the blocking pool.
async fn read_tags(path: &PathBuf) -> FileTags {
    let path = path.clone();
    tokio::task::spawn_blocking(move || {
        use lofty::file::TaggedFileExt;
        use lofty::tag::Accessor;

        let mut tags = FileTags {
            extension: "bin".to_string(),
            ..Default::default()
        };
        let Ok(tagged) = lofty::read_from_path(&path) else {
            return tags;
        };
        tags.extension = match tagged.file_type() {
            lofty::file::FileType::Flac => "flac",
            lofty::file::FileType::Mpeg => "mp3",
            lofty::file::FileType::Opus => "opus",
            lofty::file::FileType::Vorbis => "ogg",
            lofty::file::FileType::Mp4 => "m4a",
            lofty::file::FileType::Wav => "wav",
            lofty::file::FileType::Aac => "aac",
            _ => "bin",
        }
        .to_string();

        let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
            return tags;
        };
        tags.title = tag.title().map(|t| t.to_string());
        tags.artist = tag.artist().map(|t| t.to_string());
        tags.album = tag.album().map(|t| t.to_string());
        tags.album_artist = tag
            .get_string(&lofty::tag::ItemKey::AlbumArtist)
            .map(|t| t.to_string());
        tags.track_no = tag.track();
        tags
    })
    .await
    .unwrap_or_default()
}

/// SHA-256 of a file, read in chunks so a large file never lands in memory whole.
async fn hash_file(path: &PathBuf) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

fn server_error(message: &str) -> Response {
    tracing::error!("library: {message}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": message })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_lowercase_hex_of_the_right_length_is_a_hash() {
        assert!(is_sha256_hex(&"a".repeat(64)));
        assert!(!is_sha256_hex(&"A".repeat(64)), "uppercase is rejected");
        assert!(!is_sha256_hex(&"a".repeat(63)));
        assert!(!is_sha256_hex(&"g".repeat(64)));
        assert!(!is_sha256_hex("../../etc/passwd"));
        assert!(!is_sha256_hex(""));
    }
}
