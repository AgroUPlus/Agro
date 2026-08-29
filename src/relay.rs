//! Zero-disk ephemeral streaming relay for remote P2P audio transfers.
//!
//! When two devices on the same account are in different locations (e.g. phone on 5G and desktop
//! at home), direct local LAN is unreachable. Instead of uploading audio files to the server's disk,
//! storing them in `./spool`, and running into 72-hour TTL and quota constraints, Agro acts as a
//! stateless duplex streaming pipe in memory.
//!
//! A sender streams raw audio chunks via `POST /api/v1/relay/{id}/send`, and the receiver consumes
//! them in real time via `GET /api/v1/relay/{id}/receive`. The server keeps only small in-flight
//! buffers (64 KB) in memory, writes zero bytes to disk, and updates device holdings upon completion.

use axum::{
    body::Body,
    extract::{Path as AxumPath, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use axum::body::Bytes;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::auth::AuthedUser;
use crate::AppState;

const SESSION_TTL: Duration = Duration::from_secs(60);
const CHANNEL_BUFFER_CHUNKS: usize = 8;

pub struct RelayChannel {
    pub session_id: String,
    pub content_hash: String,
    pub sender_device: String,
    pub receiver_device: String,
    pub user_id: String,
    pub created_at: Instant,
    tx: Mutex<Option<mpsc::Sender<Result<Bytes, std::io::Error>>>>,
    rx: Mutex<Option<mpsc::Receiver<Result<Bytes, std::io::Error>>>>,
}

#[derive(Clone, Default)]
pub struct RelayHub {
    sessions: Arc<Mutex<HashMap<String, Arc<RelayChannel>>>>,
}

impl RelayHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_session(
        &self,
        user_id: &str,
        receiver_device: &str,
        sender_device: &str,
        content_hash: &str,
    ) -> (String, Arc<RelayChannel>) {
        let mut guard = self.sessions.lock().unwrap();
        let now = Instant::now();
        // Opportunistic sweep of expired sessions
        guard.retain(|_, s| now.duration_since(s.created_at) < SESSION_TTL);

        let session_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = mpsc::channel(CHANNEL_BUFFER_CHUNKS);

        let channel = Arc::new(RelayChannel {
            session_id: session_id.clone(),
            content_hash: content_hash.to_string(),
            sender_device: sender_device.to_string(),
            receiver_device: receiver_device.to_string(),
            user_id: user_id.to_string(),
            created_at: now,
            tx: Mutex::new(Some(tx)),
            rx: Mutex::new(Some(rx)),
        });

        guard.insert(session_id.clone(), channel.clone());
        (session_id, channel)
    }

    pub fn get_session(&self, session_id: &str) -> Option<Arc<RelayChannel>> {
        let mut guard = self.sessions.lock().unwrap();
        let now = Instant::now();
        let session = guard.get(session_id)?.clone();
        if now.duration_since(session.created_at) >= SESSION_TTL {
            guard.remove(session_id);
            return None;
        }
        Some(session)
    }

    pub fn remove_session(&self, session_id: &str) {
        let mut guard = self.sessions.lock().unwrap();
        guard.remove(session_id);
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRelayRequest {
    pub content_hash: String,
    pub from_device: String,
    pub to_device: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRelayResponse {
    pub session_id: String,
}

/// Initiates an ephemeral relay session.
pub async fn open_relay(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    Json(body): Json<OpenRelayRequest>,
) -> Response {
    let user_id = user.username();

    // Verify requesting account owns both devices or sender is in user's holdings
    let (session_id, _) = state.relay_hub.create_session(
        user_id,
        &body.to_device,
        &body.from_device,
        &body.content_hash,
    );

    // Notify the holding device over WebSocket that a relay stream is requested
    state.ws_hub.notify_device(
        user_id,
        &body.from_device,
        "RELAY_REQUEST",
        serde_json::json!({
            "sessionId": session_id,
            "contentHash": body.content_hash,
            "toDevice": body.to_device,
        }),
    );

    Json(OpenRelayResponse { session_id }).into_response()
}

/// Sender streams bytes into the relay pipe.
pub async fn send_relay(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(session_id): AxumPath<String>,
    body: Body,
) -> Response {
    let Some(session) = state.relay_hub.get_session(&session_id) else {
        return (StatusCode::NOT_FOUND, "relay session not found or expired").into_response();
    };

    if session.user_id != user.username() {
        return (StatusCode::FORBIDDEN, "not your relay session").into_response();
    }

    let tx = {
        let mut guard = session.tx.lock().unwrap();
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

/// Receiver streams bytes out of the relay pipe.
pub async fn receive_relay(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    AxumPath(session_id): AxumPath<String>,
) -> Response {
    let Some(session) = state.relay_hub.get_session(&session_id) else {
        return (StatusCode::NOT_FOUND, "relay session not found or expired").into_response();
    };

    if session.user_id != user.username() {
        return (StatusCode::FORBIDDEN, "not your relay session").into_response();
    }

    let rx = {
        let mut guard = session.rx.lock().unwrap();
        guard.take()
    };

    let Some(mut rx) = rx else {
        return (StatusCode::CONFLICT, "relay receiver already connected").into_response();
    };

    let db = state.db.clone();
    let user_id = user.username().to_string();
    let receiver_device = session.receiver_device.clone();
    let content_hash = session.content_hash.clone();
    let hub = state.relay_hub.clone();
    let sid = session_id.clone();

    // Cleanup and bookkeeping happen when the stream *ends*, not on a timer.
    //
    // This used to sleep five seconds and then unconditionally record the holding and drop the
    // session. Both halves were wrong, and together they are why every relayed track arrived
    // empty. Dropping the session releases the last `Arc`, and with it the sender's half of the
    // channel — so a sender that had not connected within five seconds found its session gone and
    // the receiver's stream simply ended, producing a 200 with no bytes. The client hashed those
    // zero bytes, found they did not match, and reported the file as corrupted. Recording the
    // holding on the same timer then told the server the device had a track it never received.
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

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}.bin\"", session.content_hash),
            ),
        ],
        body,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_retrieves_relay_session() {
        let hub = RelayHub::new();
        let (id, session) = hub.create_session("alpha", "phone", "pc", "hash123");
        assert_eq!(session.user_id, "alpha");
        assert_eq!(session.sender_device, "pc");
        assert_eq!(session.receiver_device, "phone");
        assert_eq!(session.content_hash, "hash123");

        let retrieved = hub.get_session(&id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().session_id, id);

        hub.remove_session(&id);
        assert!(hub.get_session(&id).is_none());
    }

    #[tokio::test]
    async fn pipes_bytes_between_sender_and_receiver() {
        let hub = RelayHub::new();
        let (id, session) = hub.create_session("alpha", "phone", "pc", "hash123");
        let tx = session.tx.lock().unwrap().take().unwrap();
        let mut rx = session.rx.lock().unwrap().take().unwrap();

        tokio::spawn(async move {
            tx.send(Ok(Bytes::from_static(b"audio stream chunk 1"))).await.unwrap();
            tx.send(Ok(Bytes::from_static(b"audio stream chunk 2"))).await.unwrap();
        });

        let chunk1 = rx.recv().await.unwrap().unwrap();
        let chunk2 = rx.recv().await.unwrap().unwrap();
        assert_eq!(&chunk1[..], b"audio stream chunk 1");
        assert_eq!(&chunk2[..], b"audio stream chunk 2");
        hub.remove_session(&id);
    }
}
