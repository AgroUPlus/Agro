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

use crate::lock::LockExt;
use axum::body::Bytes;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};

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
    /// The room key, sealed by the host to the listener's identity key.
    ///
    /// Opaque here. The server carries it from the one device to the other because it is already
    /// carrying their bytes, and it can no more read this than it can read those.
    pub sealed_key: Mutex<Option<String>>,
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
        let mut guard = self.sessions.lock_recover();
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
            sealed_key: Mutex::new(None),
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
        let mut guard = self.sessions.lock_recover();
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
            sealed_key: Mutex::new(None),
            pipe: RelayPipe::Fanout {
                tx,
                sender_attached: std::sync::atomic::AtomicBool::new(false),
            },
        });

        guard.insert(session_id.clone(), channel.clone());
        (session_id, channel)
    }

    pub fn get_session(&self, session_id: &str) -> Option<Arc<RelayChannel>> {
        let mut guard = self.sessions.lock_recover();
        let now = Instant::now();
        let session = guard.get(session_id)?.clone();
        if now.duration_since(session.created_at) >= SESSION_TTL {
            guard.remove(session_id);
            return None;
        }
        Some(session)
    }

    pub fn remove_session(&self, session_id: &str) {
        let mut guard = self.sessions.lock_recover();
        guard.remove(session_id);
    }
}

mod download;
mod open;
mod upload;
pub use download::__path_receive_relay;
pub use download::receive_relay;
pub use open::__path_open_relay;
pub use open::open_relay;
pub use open::OpenRelayRequest;
pub use open::OpenRelayResponse;
pub use upload::__path_send_relay;
pub use upload::send_relay;
#[cfg(test)]
mod tests;
