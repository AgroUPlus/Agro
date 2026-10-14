//! Input validation helpers for drops.

use super::*;
use crate::db_drops::DeviceCiphertext;
use crate::schema::bounded;

/// Clamps a page request. A page size is a hint from a caller, not an instruction.
pub(super) fn page(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    (
        limit.unwrap_or(50).clamp(1, MAX_PAGE),
        offset.unwrap_or(0).max(0),
    )
}

/// [`bounded`], but for a field that may legitimately be absent. An empty string is treated as
/// absent rather than stored, so a client that sends `""` for "no album" does not produce a row
/// that renders as a blank album name.
/// Bounds and normalises the per-device sealed copies a sender supplied.
///
/// Each entry is checked exactly as the single `noteCiphertext` is, and the *list* is bounded
/// separately — one argument that can repeat is one argument that can be repeated too often.
/// Duplicate device ids collapse to the last one given, because the storage key is
/// `(drop_id, device_id)` and a client that sent two copies for one device has already decided
/// which it meant last.
pub(super) fn sealed_copies(
    raw: Option<Vec<DeviceCiphertextPayload>>,
) -> async_graphql::Result<Vec<crate::db_drops::DeviceCiphertext>> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.len() > MAX_SEALED_COPIES {
        return Err(format!("A note may be sealed to at most {MAX_SEALED_COPIES} devices").into());
    }

    let mut seen: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for copy in raw {
        let device_id = bounded(&copy.device_id, MAX_FIELD_LEN, "deviceId")?;
        if device_id.is_empty() {
            return Err("A sealed copy needs a device id".into());
        }
        let ciphertext = bounded(&copy.ciphertext, MAX_CIPHERTEXT_LEN, "ciphertext")?;
        if ciphertext.is_empty() {
            return Err("A sealed copy needs a ciphertext".into());
        }
        if seen.insert(device_id.clone(), ciphertext).is_none() {
            order.push(device_id);
        }
    }

    Ok(order
        .into_iter()
        .map(|device_id| {
            let ciphertext = seen[&device_id].clone();
            DeviceCiphertext {
                device_id,
                ciphertext,
            }
        })
        .collect())
}

pub(super) fn optional(
    raw: Option<&str>,
    max: usize,
    field: &str,
) -> async_graphql::Result<Option<String>> {
    match raw {
        None => Ok(None),
        Some(value) => {
            let clean = bounded(value, max, field)?;
            Ok(if clean.is_empty() { None } else { Some(clean) })
        }
    }
}
