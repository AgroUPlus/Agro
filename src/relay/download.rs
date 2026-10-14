//! The receiver's side of a relay.

use super::*;
use crate::auth::AuthedUser;
use crate::lock::LockExt;
use crate::AppState;
use axum::body::Bytes;
use axum::http::HeaderValue;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};

/// Receiver streams bytes out of the relay pipe.
#[utoipa::path(
    get,
    path = "/api/v1/relay/{session_id}/receive",
    tag = "relay",
    security(("bearer_token" = [])),
    params(("session_id" = String, Path, description = "Session id returned by open_relay")),
    responses(
        (status = 200, description = "Raw audio bytes", content_type = "application/octet-stream"),
        (status = 403, description = "Not the receiver of this session, or not a member of the jam"),
        (status = 404, description = "No such session, or it expired"),
        (status = 409, description = "A receiver is already attached to this session"),
    ),
)]
pub async fn receive_relay(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    let Some(session) = state.relay_hub.get_session(&session_id) else {
        return (StatusCode::NOT_FOUND, "relay session not found or expired").into_response();
    };

    // A fanout is addressed to a jam rather than to a device, so membership is what authorises
    // attaching — and it is read now, not when the session was opened, so someone who has left
    // the jam stops being able to receive.
    if let Some(jam_id) = &session.jam_id {
        let is_member = state
            .db
            .is_jam_member(jam_id, user.username())
            .unwrap_or(false);
        if !is_member {
            return (StatusCode::FORBIDDEN, "not a member of this jam").into_response();
        }
    } else if !session.receiver_user.eq_ignore_ascii_case(user.username()) {
        return (
            StatusCode::FORBIDDEN,
            "not the receiver of this relay session",
        )
            .into_response();
    }

    if let RelayPipe::Fanout { tx, .. } = &session.pipe {
        let stream = futures_util::stream::unfold(tx.subscribe(), |mut rx| async move {
            match rx.recv().await {
                Ok(bytes) => Some((Ok::<Bytes, std::io::Error>(bytes), rx)),
                // Closed: the upload finished or the session was swept.
                //
                // Lagged: this listener could not keep up. Ending its stream is the honest
                // answer — the bytes it skipped are gone, and audio resumed past a hole is
                // noise. It falls back to another tier, and the rest of the jam is unaffected.
                Err(_) => None,
            }
        });

        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        if *session.is_encrypted.lock_recover() {
            headers.insert("x-agro-encrypted", HeaderValue::from_static("true"));
        }
        if let Some(nonce) = session.nonce.lock_recover().clone() {
            if let Ok(value) = nonce.parse() {
                headers.insert("x-agro-nonce", value);
            }
        }
        if let Some(fp) = session.key_fingerprint.lock_recover().clone() {
            if let Ok(value) = fp.parse() {
                headers.insert("x-agro-key-fingerprint", value);
            }
        }
        if let Some(sealed) = session.sealed_key.lock_recover().clone() {
            if let Ok(value) = sealed.parse() {
                headers.insert("x-agro-sealed-key", value);
            }
        }
        return (headers, Body::from_stream(stream)).into_response();
    }

    let RelayPipe::Direct { rx: pipe_rx, .. } = &session.pipe else {
        unreachable!("fanout handled above");
    };
    let rx = {
        let mut guard = pipe_rx.lock_recover();
        guard.take()
    };

    let Some(mut rx) = rx else {
        return (StatusCode::CONFLICT, "relay receiver already connected").into_response();
    };

    let db = state.db.clone();
    let user_id = session.receiver_user.clone();
    let receiver_device = session.receiver_device.clone();
    let content_hash = session.content_hash.clone();
    let hub = state.relay_hub.clone();
    let sid = session_id.clone();

    // Read encryption metadata to pass down in response headers
    let is_encrypted = *session.is_encrypted.lock_recover();
    let nonce = session.nonce.lock_recover().clone();
    let key_fp = session.key_fingerprint.lock_recover().clone();
    let sealed_key = session.sealed_key.lock_recover().clone();

    // Cleanup and bookkeeping happen when the stream *ends*, not on a timer.
    let mut transferred: u64 = 0;
    let mut settled = false;
    let stream = futures_util::stream::poll_fn(move |cx| {
        let polled = rx.poll_recv(cx);
        match &polled {
            std::task::Poll::Ready(Some(Ok(bytes))) => transferred += bytes.len() as u64,
            std::task::Poll::Ready(_) if !settled => {
                settled = true;
                // Only a transfer that actually moved bytes counts as a holding.
                if transferred > 0 {
                    let _ = db.upsert_holding(&user_id, &receiver_device, &content_hash, None);
                }
                hub.remove_session(&sid);
            }
            _ => {}
        }
        polled
    });
    let body = Body::from_stream(stream);

    let mut response_headers = axum::http::HeaderMap::new();
    response_headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    // The filename is cosmetic: a hash that is not a valid header value just means no suggestion.
    if let Ok(disposition) = HeaderValue::from_str(&format!(
        "attachment; filename=\"{}.bin\"",
        session.content_hash
    )) {
        response_headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    if is_encrypted {
        response_headers.insert("x-agro-encrypted", HeaderValue::from_static("true"));
    }
    if let Some(n) = nonce {
        if let Ok(v) = n.parse() {
            response_headers.insert("x-agro-nonce", v);
        }
    }
    if let Some(fp) = key_fp {
        if let Ok(v) = fp.parse() {
            response_headers.insert("x-agro-key-fingerprint", v);
        }
    }
    if let Some(sealed) = sealed_key {
        if let Ok(v) = sealed.parse() {
            response_headers.insert("x-agro-sealed-key", v);
        }
    }

    (StatusCode::OK, response_headers, body).into_response()
}
