//! The sender's side of a relay.

use super::*;
use crate::auth::AuthedUser;
use crate::lock::LockExt;
use crate::AppState;
use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;

/// Sender streams bytes into the relay pipe.
#[utoipa::path(
    post,
    path = "/api/v1/relay/{session_id}/send",
    tag = "relay",
    security(("bearer_token" = [])),
    params(("session_id" = String, Path, description = "Session id returned by open_relay")),
    request_body(content_type = "application/octet-stream", description = "Raw audio bytes"),
    responses(
        (status = 200, description = "Streaming finished"),
        (status = 403, description = "Not the sender of this session"),
        (status = 404, description = "No such session, or it expired"),
        (status = 409, description = "A sender is already attached to this session"),
    ),
)]
pub async fn send_relay(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(session_id): AxumPath<String>,
    headers: axum::http::HeaderMap,
    body: Body,
) -> Response {
    let Some(session) = state.relay_hub.get_session(&session_id) else {
        return (StatusCode::NOT_FOUND, "relay session not found or expired").into_response();
    };

    if !session.sender_user.eq_ignore_ascii_case(user.username()) {
        return (
            StatusCode::FORBIDDEN,
            "not the sender of this relay session",
        )
            .into_response();
    }

    // Capture encryption headers if present for E2EE relaying
    if let Some(enc) = headers.get("x-agro-encrypted") {
        if let Ok(val) = enc.to_str() {
            *session.is_encrypted.lock_recover() = val.eq_ignore_ascii_case("true");
        }
    }
    if let Some(nonce) = headers.get("x-agro-nonce") {
        if let Ok(val) = nonce.to_str() {
            *session.nonce.lock_recover() = Some(val.to_string());
        }
    }
    if let Some(fp) = headers.get("x-agro-key-fingerprint") {
        if let Ok(val) = fp.to_str() {
            *session.key_fingerprint.lock_recover() = Some(val.to_string());
        }
    }
    if let Some(sealed) = headers.get("x-agro-sealed-key") {
        if let Ok(val) = sealed.to_str() {
            *session.sealed_key.lock_recover() = Some(val.to_string());
        }
    }

    if let RelayPipe::Fanout {
        tx,
        sender_attached,
    } = &session.pipe
    {
        use std::sync::atomic::Ordering;
        if sender_attached.swap(true, Ordering::SeqCst) {
            return (StatusCode::CONFLICT, "relay sender already connected").into_response();
        }

        let mut stream = body.into_data_stream();
        while let Some(chunk) = stream.next().await {
            let Ok(bytes) = chunk else { break };
            // An error here means nobody is attached *at this instant*, which is not a reason to
            // abandon the upload: a jam listener can attach a moment later, and the host is
            // playing the track regardless.
            let _ = tx.send(bytes);
        }
        // Dropping the hub's session ends every attached listener's stream together.
        state.relay_hub.remove_session(&session_id);
        return (StatusCode::OK, "streaming to relay").into_response();
    }

    let RelayPipe::Direct { tx: pipe_tx, .. } = &session.pipe else {
        unreachable!("fanout handled above");
    };
    let tx = {
        let mut guard = pipe_tx.lock_recover();
        guard.take()
    };

    let Some(tx) = tx else {
        return (StatusCode::CONFLICT, "relay sender already connected").into_response();
    };

    // Pumped inline rather than in a spawned task. Answering first and reading the body afterwards
    // is not something HTTP promises will work: once the response is complete the connection may
    // be reused or closed, and the request body goes with it — which truncates the transfer, or
    // ends it before a single chunk has been read.
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                if tx.send(Ok(bytes)).await.is_err() {
                    break; // Receiver disconnected
                }
            }
            Err(err) => {
                let _ = tx
                    .send(Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        err,
                    )))
                    .await;
                break;
            }
        }
    }
    // Dropping `tx` here ends the receiver's stream, which is what tells it the file is complete.
    drop(tx);

    (StatusCode::OK, "streaming to relay").into_response()
}
