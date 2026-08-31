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
use tokio::sync::{broadcast, mpsc};

use crate::auth::AuthedUser;
use crate::db_social::FriendState;
use crate::AppState;

const SESSION_TTL: Duration = Duration::from_secs(60);
const CHANNEL_BUFFER_CHUNKS: usize = 8;

/// Chunks held for a jam listener that has fallen behind the others.
///
/// Larger than the direct buffer because the slowest listener sets the pace for nobody: a
/// listener that cannot keep up is dropped from the fanout rather than stalling the host, and
/// this is how much jitter it may absorb before that happens.
const FANOUT_BUFFER_CHUNKS: usize = 16;

/// Where a relay session's bytes go.
pub enum RelayPipe {
    /// One sender to one receiver. The receiving device is named when the session is opened.
    Direct {
        tx: Mutex<Option<mpsc::Sender<Result<Bytes, std::io::Error>>>>,
        rx: Mutex<Option<mpsc::Receiver<Result<Bytes, std::io::Error>>>>,
    },
    /// One sender to every listener in a jam who asks for it.
    ///
    /// The host encrypts and uploads a chunk once; the server copies the ciphertext to whoever is
    /// attached. That is the whole reason this exists: with a session per listener the host would
    /// upload the same bytes once per person, which is the cost a phone cannot pay.
    ///
    /// Nothing is buffered for a listener that has not attached yet. A jam plays in lockstep, so
    /// arriving late means joining the stream in progress, not receiving it from the beginning.
    Fanout {
        tx: broadcast::Sender<Bytes>,
        sender_attached: std::sync::atomic::AtomicBool,
    },
}

pub struct RelayChannel {
    pub session_id: String,
    pub content_hash: String,
    pub sender_user: String,
    pub sender_device: String,
    /// The single authorised receiver, for a [`RelayPipe::Direct`] session. Empty for a fanout,
    /// where membership of [`RelayChannel::jam_id`] decides instead.
    pub receiver_user: String,
    pub receiver_device: String,
    /// Set when this session fans out to a jam. Authorisation reads it on every attach, so a
    /// listener who leaves the jam stops being able to receive.
    pub jam_id: Option<String>,
    pub created_at: Instant,
    pub is_encrypted: Mutex<bool>,
    pub nonce: Mutex<Option<String>>,
    pub key_fingerprint: Mutex<Option<String>>,
    pub pipe: RelayPipe,
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
        receiver_user: &str,
        receiver_device: &str,
        sender_user: &str,
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
            sender_user: sender_user.to_string(),
            sender_device: sender_device.to_string(),
            receiver_user: receiver_user.to_string(),
            receiver_device: receiver_device.to_string(),
            jam_id: None,
            created_at: now,
            is_encrypted: Mutex::new(false),
            nonce: Mutex::new(None),
            key_fingerprint: Mutex::new(None),
            pipe: RelayPipe::Direct {
                tx: Mutex::new(Some(tx)),
                rx: Mutex::new(Some(rx)),
            },
        });

        guard.insert(session_id.clone(), channel.clone());
        (session_id, channel)
    }

    /// Opens a session the whole jam can attach to, fed by one upload from the host.
    pub fn create_jam_session(
        &self,
        jam_id: &str,
        sender_user: &str,
        sender_device: &str,
        content_hash: &str,
    ) -> (String, Arc<RelayChannel>) {
        let mut guard = self.sessions.lock().unwrap();
        let now = Instant::now();
        guard.retain(|_, s| now.duration_since(s.created_at) < SESSION_TTL);

        let session_id = uuid::Uuid::new_v4().to_string();
        let (tx, _) = broadcast::channel(FANOUT_BUFFER_CHUNKS);

        let channel = Arc::new(RelayChannel {
            session_id: session_id.clone(),
            content_hash: content_hash.to_string(),
            sender_user: sender_user.to_string(),
            sender_device: sender_device.to_string(),
            // Named on attach instead: a fanout has as many receivers as the jam has listeners.
            receiver_user: String::new(),
            receiver_device: String::new(),
            jam_id: Some(jam_id.to_string()),
            created_at: now,
            is_encrypted: Mutex::new(false),
            nonce: Mutex::new(None),
            key_fingerprint: Mutex::new(None),
            pipe: RelayPipe::Fanout {
                tx,
                sender_attached: std::sync::atomic::AtomicBool::new(false),
            },
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
    /// Set to fan one upload out to a jam instead of to a single device.
    ///
    /// `to_device` is ignored when this is present: the audience is whoever is in the jam at the
    /// moment they attach, which is not knowable when the session is opened.
    #[serde(default)]
    pub jam_id: Option<String>,
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

    // A fanout is opened by the host, for its own jam, and is authorised entirely differently:
    // there is no single receiving device to check, and the sender is by definition the caller.
    // Listeners are authorised when they attach, against the jam's membership at that moment.
    if let Some(jam_id) = body.jam_id.as_deref().map(str::trim).filter(|j| !j.is_empty()) {
        if !state
            .db
            .device_belongs_to(user_id, body.from_device.trim())
            .unwrap_or(false)
        {
            return (
                StatusCode::FORBIDDEN,
                "source device does not belong to this account",
            )
                .into_response();
        }
        if !state.db.is_jam_member(jam_id, user_id).unwrap_or(false) {
            return (StatusCode::FORBIDDEN, "not a member of this jam").into_response();
        }

        let (session_id, _) = state.relay_hub.create_jam_session(
            jam_id,
            user_id,
            body.from_device.trim(),
            body.content_hash.trim(),
        );
        return Json(OpenRelayResponse { session_id }).into_response();
    }

    // 1. The receiver must be a device belonging to the caller.
    if !state
        .db
        .device_belongs_to(user_id, body.to_device.trim())
        .unwrap_or(false)
    {
        return (
            StatusCode::FORBIDDEN,
            "target device does not belong to this account",
        )
            .into_response();
    }

    // 2. The sender device can belong to caller's account, OR to a friend, OR to a member of the same Jam.
    let sender_user = match state.db.owner_of_device(body.from_device.trim()) {
        Ok(Some(owner)) => owner,
        _ => {
            return (
                StatusCode::FORBIDDEN,
                "unknown source device",
            )
                .into_response();
        }
    };

    // `friend_state` answers `Some(..)` for a *pending* request and for a *block* as well as for
    // an accepted friendship, so testing it with `is_some()` would let a stranger who merely sent a
    // request — or someone this account has blocked — name this account's device as the sender, and
    // the `RELAY_REQUEST` below would make that device upload a hash of the caller's choosing.
    // `are_friends` is the accepted-only test, and a block outranks a shared jam.
    let allowed = if sender_user.eq_ignore_ascii_case(user_id) {
        true
    } else if matches!(
        state.db.friend_state(user_id, &sender_user).unwrap_or(None),
        Some(FriendState::Blocked)
    ) {
        false
    } else {
        let is_friend = state.db.are_friends(user_id, &sender_user).unwrap_or(false);
        let is_in_same_jam = match (
            state.db.jam_for_member(user_id),
            state.db.jam_for_member(&sender_user),
        ) {
            (Ok(Some(j1)), Ok(Some(j2))) => j1.id == j2.id,
            _ => false,
        };
        is_friend || is_in_same_jam
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            "source device is not authorized for relay with this account",
        )
            .into_response();
    }

    let (session_id, _) = state.relay_hub.create_session(
        user_id,
        &body.to_device,
        &sender_user,
        &body.from_device,
        &body.content_hash,
    );

    // Notify the holding device over WebSocket that a relay stream is requested
    state.ws_hub.notify_device(
        &sender_user,
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

    if !session.sender_user.eq_ignore_ascii_case(user.username()) {
        return (StatusCode::FORBIDDEN, "not the sender of this relay session").into_response();
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
        let mut guard = pipe_tx.lock().unwrap();
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
        return (StatusCode::FORBIDDEN, "not the receiver of this relay session").into_response();
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
            "application/octet-stream".parse().unwrap(),
        );
        if *session.is_encrypted.lock().unwrap() {
            headers.insert("x-agro-encrypted", "true".parse().unwrap());
        }
        if let Some(nonce) = session.nonce.lock().unwrap().clone() {
            if let Ok(value) = nonce.parse() {
                headers.insert("x-agro-nonce", value);
            }
        }
        if let Some(fp) = session.key_fingerprint.lock().unwrap().clone() {
            if let Ok(value) = fp.parse() {
                headers.insert("x-agro-key-fingerprint", value);
            }
        }
        return (headers, Body::from_stream(stream)).into_response();
    }

    let RelayPipe::Direct { rx: pipe_rx, .. } = &session.pipe else {
        unreachable!("fanout handled above");
    };
    let rx = {
        let mut guard = pipe_rx.lock().unwrap();
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
        let (id, session) = hub.create_session("alpha", "phone", "alpha", "pc", "hash123");
        assert_eq!(session.receiver_user, "alpha");
        assert_eq!(session.sender_user, "alpha");
        assert_eq!(session.sender_device, "pc");
        assert_eq!(session.receiver_device, "phone");
        assert_eq!(session.content_hash, "hash123");

        let retrieved = hub.get_session(&id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().session_id, id);

        hub.remove_session(&id);
        assert!(hub.get_session(&id).is_none());
    }

    /// One upload, every listener. This is the whole point of the fanout.
    #[tokio::test]
    async fn a_jam_upload_reaches_every_attached_listener() {
        let hub = RelayHub::new();
        let (_, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");

        let RelayPipe::Fanout { tx, .. } = &session.pipe else {
            panic!("create_jam_session makes a fanout pipe");
        };
        let mut first = tx.subscribe();
        let mut second = tx.subscribe();

        tx.send(Bytes::from_static(b"chunk")).expect("listeners attached");

        assert_eq!(first.recv().await.unwrap(), Bytes::from_static(b"chunk"));
        assert_eq!(second.recv().await.unwrap(), Bytes::from_static(b"chunk"));
    }

    /// A listener attaching after the upload started joins in progress; a jam plays in lockstep,
    /// so there is nothing sensible to replay to it.
    #[tokio::test]
    async fn a_late_listener_does_not_receive_earlier_chunks() {
        let hub = RelayHub::new();
        let (_, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");
        let RelayPipe::Fanout { tx, .. } = &session.pipe else {
            panic!("fanout");
        };

        // Nobody attached yet: the send fails and the chunk is gone, which is intended.
        let _ = tx.send(Bytes::from_static(b"early"));
        let mut late = tx.subscribe();
        tx.send(Bytes::from_static(b"later")).expect("listener attached");

        assert_eq!(late.recv().await.unwrap(), Bytes::from_static(b"later"));
    }

    /// The host uploads once. A second upload to the same session is refused rather than
    /// interleaved into the listeners' streams.
    #[test]
    fn a_fanout_accepts_only_one_sender() {
        use std::sync::atomic::Ordering;
        let hub = RelayHub::new();
        let (_, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");
        let RelayPipe::Fanout { sender_attached, .. } = &session.pipe else {
            panic!("fanout");
        };

        assert!(!sender_attached.swap(true, Ordering::SeqCst), "first upload attaches");
        assert!(sender_attached.swap(true, Ordering::SeqCst), "second is refused");
    }

    /// A fanout carries the jam it belongs to, because that is what every attach is checked
    /// against — not a receiver fixed when the session was opened.
    #[test]
    fn a_fanout_is_addressed_to_a_jam_not_a_device() {
        let hub = RelayHub::new();
        let (id, session) = hub.create_jam_session("jam-1", "host", "host-phone", "hash-1");

        assert_eq!(session.jam_id.as_deref(), Some("jam-1"));
        assert!(session.receiver_device.is_empty());
        assert!(hub.get_session(&id).is_some());
    }

    async fn pipes_bytes_between_sender_and_receiver() {
        let hub = RelayHub::new();
        let (id, session) = hub.create_session("alpha", "phone", "alpha", "pc", "hash123");
        let RelayPipe::Direct { tx, rx } = &session.pipe else {
            panic!("create_session makes a direct pipe");
        };
        let tx = tx.lock().unwrap().take().unwrap();
        let mut rx = rx.lock().unwrap().take().unwrap();

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

    /// The tests below exercise the handler rather than the hub, ensuring device ownership and
    /// friend / jam permissions are strictly enforced.
    mod device_ownership {
        use super::*;
        use crate::db::{Db, NodeName};
        use crate::db_identity::{AccountState, Role};

        /// Two accounts, each with one registered device.
        fn two_accounts() -> (AppState, AuthedUser) {
            let db = Db::new_in_memory().unwrap();
            for (user, device) in [("alpha", "alpha-pc"), ("mallory", "mallory-pc"), ("friend", "friend-phone")] {
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
                    token_hash: String::new(),
                },
            )
        }

        async fn open(state: AppState, user: AuthedUser, from: &str, to: &str) -> StatusCode {
            open_relay(
                State(state),
                axum::Extension(user),
                Json(OpenRelayRequest {
                    jam_id: None,
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

        #[tokio::test]
        async fn a_relay_from_a_friend_is_allowed() {
            let (state, alpha) = two_accounts();
            // Accept friend relation
            state.db.send_friend_request("alpha", "friend").unwrap();
            state.db.accept_friend_request("friend", "alpha").unwrap();
            assert_eq!(
                open(state, alpha, "friend-phone", "alpha-pc").await,
                StatusCode::OK
            );
        }

        /// A request that has been *sent* and not answered is not a friendship. Treating any
        /// `friend_state` row as permission let a stranger who merely asked make this account's
        /// device upload a hash of their choosing.
        #[tokio::test]
        async fn a_relay_from_a_pending_request_is_refused() {
            let (state, alpha) = two_accounts();
            state.db.send_friend_request("friend", "alpha").unwrap();
            assert_eq!(
                open(state, alpha, "friend-phone", "alpha-pc").await,
                StatusCode::FORBIDDEN
            );
        }

        /// And a block is the strongest answer there is, including over a shared jam.
        #[tokio::test]
        async fn a_relay_from_a_blocked_account_is_refused() {
            let (state, alpha) = two_accounts();
            state.db.send_friend_request("alpha", "friend").unwrap();
            state.db.accept_friend_request("friend", "alpha").unwrap();
            state.db.block_user("alpha", "friend").unwrap();
            assert_eq!(
                open(state, alpha, "friend-phone", "alpha-pc").await,
                StatusCode::FORBIDDEN
            );
        }

        /// Naming an unrelated stranger's device as the sender is forbidden.
        #[tokio::test]
        async fn i_cannot_make_a_strangers_device_the_sender() {
            let (state, alpha) = two_accounts();
            assert_eq!(
                open(state, alpha, "mallory-pc", "alpha-pc").await,
                StatusCode::FORBIDDEN
            );
        }

        /// And naming someone else's device as the *receiver* is always forbidden.
        #[tokio::test]
        async fn i_cannot_aim_a_relay_at_another_accounts_device() {
            let (state, alpha) = two_accounts();
            assert_eq!(
                open(state, alpha, "alpha-pc", "mallory-pc").await,
                StatusCode::FORBIDDEN
            );
        }

        /// A device id nobody has registered is refused.
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
