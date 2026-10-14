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
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

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
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
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

mod archive;
mod begin;
mod fetch;
mod put;
mod spool;
mod tags;
use archive::archive;
pub use begin::__path_begin_upload;
pub use begin::begin_upload;
use begin::quota_exceeded;
pub use begin::BeginUpload;
pub use begin::BeginUploadResponse;
pub use fetch::__path_cover;
pub use fetch::__path_fetch;
pub use fetch::cover;
pub use fetch::fetch;
pub use put::__path_put_upload;
pub use put::put_upload;
use spool::spool;
pub use spool::sweep_storage;
use tags::read_tags;
pub use tags::reindex_covers;
use tags::store_cover;
#[cfg(test)]
mod tests;
