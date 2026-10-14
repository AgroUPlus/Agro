//! Archiving a finished upload and running the archive hook.

use super::*;
use crate::storage::{self, Filing};
use crate::AppState;
use axum::{
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::path::{Path, PathBuf};

/// Files the received bytes into the music library.
///
/// Tags are re-read from the file with lofty rather than taken from what the client declared: the
/// file is the thing Navidrome will scan, so the shelf position should agree with it.
pub(super) async fn archive(
    state: &AppState,
    session: &crate::db_library::UploadSession,
    part_path: &PathBuf,
) -> Response {
    let content_hash = session.content_hash.as_str();
    let Some(root) = state.storage.library_root.clone() else {
        return server_error("no library root is configured");
    };

    let mut tags = read_tags(part_path).await;
    // Taken before the rest of `tags` is consumed field by field below.
    let cover = tags.cover.take();
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
    // sync daemon. Ensure standard read permissions for group and others while owner retains write.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) =
            tokio::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).await
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

    // Artwork, so the library can be looked at rather than only listed. Keyed by album, so a
    // twelve-track upload writes one file and skips the other eleven.
    if let (Some(cover), Some(album_name)) = (cover, album.as_deref()) {
        let cover_artist = album_artist.as_deref().unwrap_or(&artist);
        store_cover(state, cover_artist, album_name, cover).await;
    }

    // Addressed to the account, not shouted. `archivedPath` spells out artist, album and title, so
    // a broadcast handed one account's library to every other account's devices.
    state.ws_hub.notify_user(
        &session.user_id,
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
pub(super) const ARCHIVE_HOOK_TIMEOUT_SECS: u64 = 60;

/// Tells whatever else indexes the library that a file arrived.
///
/// Detached on purpose. The bytes are filed and the row is written by the time this runs, so the
/// upload has already succeeded — a hook that fails, hangs or does not exist must not turn that
/// into an error for the client. Failures are logged and nothing else.
///
/// The paths go in the environment rather than into the command string: they are derived from tags
/// a client supplied, and interpolating them into a line handed to `sh` would be an injection.
pub(super) fn run_archive_hook(state: &AppState, relative: &str, absolute: &Path) {
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
