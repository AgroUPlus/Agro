//! The cloud vault's two byte routes: a device sending a sealed backup, and fetching one back.
//!
//! Bytes travel here rather than over GraphQL because a backup with years of listening in it is
//! larger than the JSON body limit allows, and base64 inside JSON would add a third on top. The
//! label travels beside them in `X-Agro-Vault-Label` — hex of a small JSON object — and is checked
//! field by field before anything is stored, since the dashboard renders it.

use axum::{
    body::Bytes,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::auth::AuthedUser;
use crate::db_vault::{VaultLabel, VaultSection, MAX_SEALED_BYTES};
use crate::AppState;

pub const LABEL_HEADER: &str = "x-agro-vault-label";
pub const SHA256_HEADER: &str = "x-agro-vault-sha256";

/// The most sections one label may name, and the longest a name may be.
const MAX_SECTIONS: usize = 16;
const MAX_SECTION_NAME: usize = 32;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LabelInput {
    device_id: String,
    device_name: Option<String>,
    app_version: Option<String>,
    format: i64,
    plain_bytes: i64,
    sections: Vec<VaultSection>,
}

fn refuse(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// Reads and checks the label. Every string in it is shown on a page, so each is bounded and a
/// section name is plain capitals — a label is a description, never markup.
fn label(headers: &HeaderMap) -> Result<VaultLabel, &'static str> {
    let raw = headers.get(LABEL_HEADER).ok_or("the backup has no label")?;
    let bytes = hex::decode(raw.as_bytes()).map_err(|_| "the label is not hex")?;
    let input: LabelInput =
        serde_json::from_slice(&bytes).map_err(|_| "the label is not a backup label")?;

    let bounded = |value: Option<String>, max: usize| {
        value
            .map(|v| v.trim().chars().take(max).collect::<String>())
            .filter(|v| !v.is_empty())
    };
    let device_id = input.device_id.trim().to_string();
    if device_id.is_empty() || device_id.len() > 128 {
        return Err("the label names no device");
    }
    if !(1..=100).contains(&input.format) || input.plain_bytes < 0 {
        return Err("the label's numbers are out of range");
    }
    if input.sections.is_empty() || input.sections.len() > MAX_SECTIONS {
        return Err("a backup holds between one and sixteen sections");
    }
    let plain_name = |n: &str| {
        !n.is_empty()
            && n.len() <= MAX_SECTION_NAME
            && n.chars().all(|c| c.is_ascii_uppercase() || c == '_')
    };
    if input
        .sections
        .iter()
        .any(|s| !plain_name(&s.name) || s.count < 0)
    {
        return Err("a section name is not one this server shows");
    }
    Ok(VaultLabel {
        device_id,
        device_name: bounded(input.device_name, 64),
        app_version: bounded(input.app_version, 32),
        format: input.format,
        plain_bytes: input.plain_bytes,
        sections: input.sections,
    })
}

/// `PUT /api/v1/vault/backups`: keeps the sealed body under the label in the header.
pub async fn put_backup(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let label = match label(&headers) {
        Ok(label) => label,
        Err(message) => return refuse(StatusCode::BAD_REQUEST, message),
    };
    if body.is_empty() || body.len() > MAX_SEALED_BYTES {
        return refuse(
            StatusCode::PAYLOAD_TOO_LARGE,
            "a backup is between 1 byte and 8 MiB sealed",
        );
    }
    match state.db.store_vault_backup(user.username(), &label, &body) {
        Ok(stored) => (
            StatusCode::CREATED,
            Json(json!({
                "id": stored.id,
                "createdAt": stored.created_at,
                "sealedBytes": stored.sealed_bytes,
                "sha256": stored.sha256,
            })),
        )
            .into_response(),
        Err(err) => {
            tracing::error!("vault: a backup could not be stored: {err}");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the backup could not be stored",
            )
        }
    }
}

/// `GET /api/v1/vault/backups/{id}`: the sealed bytes, with their digest so the device can check.
pub async fn get_backup(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(id): AxumPath<String>,
) -> Response {
    match state.db.vault_backup_blob(user.username(), id.trim()) {
        Ok(Some((backup, sealed))) => (
            StatusCode::OK,
            [
                (
                    axum::http::header::CONTENT_TYPE,
                    "application/octet-stream".to_string(),
                ),
                (axum::http::header::CACHE_CONTROL, "no-store".to_string()),
                (
                    axum::http::HeaderName::from_static(SHA256_HEADER),
                    backup.sha256,
                ),
            ],
            sealed,
        )
            .into_response(),
        // Not found and not yours read the same: an id is not a way to learn whose backups exist.
        Ok(None) => refuse(StatusCode::NOT_FOUND, "no such backup"),
        Err(err) => {
            tracing::error!("vault: a backup could not be read: {err}");
            refuse(
                StatusCode::INTERNAL_SERVER_ERROR,
                "the backup could not be read",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(json: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(LABEL_HEADER, hex::encode(json).parse().unwrap());
        h
    }

    #[test]
    fn a_wellformed_label_is_read_and_its_strings_bounded() {
        let long = "x".repeat(200);
        let l = label(&headers(&format!(
            r#"{{"deviceId":"d1","deviceName":"{long}","format":3,"plainBytes":10,
                "sections":[{{"name":"SETTINGS","count":40}},{{"name":"HISTORY","count":900}}]}}"#
        )))
        .unwrap();
        assert_eq!(l.device_name.unwrap().len(), 64);
        assert_eq!(l.sections[1].count, 900);
    }

    #[test]
    fn a_label_that_could_be_markup_or_nonsense_is_refused() {
        for bad in [
            r#"{"deviceId":"d","format":3,"plainBytes":1,"sections":[{"name":"<b>x</b>","count":1}]}"#,
            r#"{"deviceId":"d","format":3,"plainBytes":1,"sections":[]}"#,
            r#"{"deviceId":"","format":3,"plainBytes":1,"sections":[{"name":"SETTINGS","count":1}]}"#,
            r#"{"deviceId":"d","format":3,"plainBytes":-1,"sections":[{"name":"SETTINGS","count":1}]}"#,
        ] {
            assert!(label(&headers(bad)).is_err(), "accepted: {bad}");
        }
        assert!(label(&HeaderMap::new()).is_err());
    }
}
