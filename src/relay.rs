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
    pub is_encrypted: Mutex<bool>,
    pub nonce: Mutex<Option<String>>,
    pub key_fingerprint: Mutex<Option<String>>,
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
            is_encrypted: Mutex::new(false),
            nonce: Mutex::new(None),
            key_fingerprint: Mutex::new(None),
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

    // Both device ids come from the request body, and a device id is chosen by the client. Until
    // this check existed the comment here claimed the ownership was verified and nothing verified
    // it: naming another account's device as `fromDevice` made *this* server send that account's
    // device a RELAY_REQUEST for a hash of the caller's choosing, and naming it as `toDevice` aimed
    // the resulting stream at it. The session's `user_id` guard on send/receive did not help — the
    // session was created under the caller's own name, so it looked entirely legitimate.
    for device in [&body.from_device, &body.to_device] {
        if !state
            .db
            .device_belongs_to(user_id, device.trim())
            .unwrap_or(false)
        {
            // One shape for both devices, so the response cannot be used to find out which device
            // ids exist on other accounts.
            return (
                StatusCode::FORBIDDEN,
                "that device does not belong to this account",
            )
                .into_response();
        }
    }

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
    headers: axum::http::HeaderMap,
    body: Body,
) -> Response {
    let Some(session) = state.relay_hub.get_session(&session_id) else {
        return (StatusCode::NOT_FOUND, "relay session not found or expired").into_response();
    };

    if session.user_id != user.username() {
        return (StatusCode::FORBIDDEN, "not your relay session").into_response();
    }

    // Capture encryption headers if present for E2EE relaying
    if let Some(enc) = headers.get("x-agro-encrypted") {
        if let Ok(val) = enc.to_str() {
            *session.is_encrypted.lock().unwrap() = val.eq_ignore_ascii_case("true");
        }
    }
    if let Some(nonce) = headers.get("x-agro-nonce") {
        if let Ok(val) = nonce.to_str() {
            *session.nonce.lock().unwrap() = Some(val.to_string());
        }
    }
    if let Some(fp) = headers.get("x-agro-key-fingerprint") {
        if let Ok(val) = fp.to_str() {
            *session.key_fingerprint.lock().unwrap() = Some(val.to_string());
        }
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

    // Read encryption metadata to pass down in response headers
    let is_encrypted = *session.is_encrypted.lock().unwrap();
    let nonce = session.nonce.lock().unwrap().clone();
    let key_fp = session.key_fingerprint.lock().unwrap().clone();

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
        "application/octet-stream".parse().unwrap(),
    );
    response_headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}.bin\"", session.content_hash)
            .parse()
            .unwrap(),
    );
    if is_encrypted {
        response_headers.insert("x-agro-encrypted", "true".parse().unwrap());
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

    (StatusCode::OK, response_headers, body).into_response()
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

    /// The three tests below exercise the handler rather than the hub, because the bug they pin was
    /// in the handler: `create_session` stamps the session with whatever account asked for it, so a
    /// session opened against someone else's device is indistinguishable from a legitimate one by
    /// the time `send_relay` checks it.
    mod device_ownership {
        use super::*;
        use crate::db::{Db, NodeName};
        use crate::db_identity::{AccountState, Role};

        /// Two accounts, each with one registered device.
        fn two_accounts() -> (AppState, AuthedUser) {
            let db = Db::new_in_memory().unwrap();
            for (user, device) in [("alpha", "alpha-pc"), ("mallory", "mallory-pc")] {
                db.create_account(user, "passphrase", Role::Member, AccountState::Active)
                    .unwrap();
                db.upsert_node(device, user, NodeName::Set(device), "wander", None, None)
                    .unwrap();
            }
            let account = db.account("alpha").unwrap().unwrap();
            let state = crate::login::tests::test_state(db);
            (
                state,
                AuthedUser {
                    account,
                    device_label: "alpha-pc".to_string(),
                },
            )
        }

        async fn open(state: AppState, user: AuthedUser, from: &str, to: &str) -> StatusCode {
            open_relay(
                State(state),
                axum::Extension(user),
                Json(OpenRelayRequest {
                    content_hash: "hash123".to_string(),
                    from_device: from.to_string(),
                    to_device: to.to_string(),
                }),
            )
            .await
            .status()
        }

        #[tokio::test]
        async fn a_relay_between_two_of_my_own_devices_is_allowed() {
            let (state, alpha) = two_accounts();
            state
                .db
                .upsert_node("alpha-phone", "alpha", NodeName::Set("phone"), "wanda", None, None)
                .unwrap();
            assert_eq!(
                open(state, alpha, "alpha-pc", "alpha-phone").await,
                StatusCode::OK
            );
        }

        /// Naming someone else's device as the *sender* made this server send that account's device
        /// a RELAY_REQUEST for a hash of the caller's choosing.
        #[tokio::test]
        async fn i_cannot_make_another_accounts_device_the_sender() {
            let (state, alpha) = two_accounts();
            assert_eq!(
                open(state, alpha, "mallory-pc", "alpha-pc").await,
                StatusCode::FORBIDDEN
            );
        }

        /// And naming it as the *receiver* aimed the stream at it.
        #[tokio::test]
        async fn i_cannot_aim_a_relay_at_another_accounts_device() {
            let (state, alpha) = two_accounts();
            assert_eq!(
                open(state, alpha, "alpha-pc", "mallory-pc").await,
                StatusCode::FORBIDDEN
            );
        }

        /// A device id nobody has registered is refused the same way a device someone else owns is,
        /// so the response cannot be used to enumerate other accounts' device ids.
        #[tokio::test]
        async fn an_unknown_device_is_refused_like_someone_elses() {
            let (state, alpha) = two_accounts();
            assert_eq!(
                open(state, alpha, "alpha-pc", "no-such-device").await,
                StatusCode::FORBIDDEN
            );
        }
    }
}
