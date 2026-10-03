//! What a device sends when it reports playback, and the limits on it.

use async_graphql::InputObject;

#[derive(InputObject, Clone, serde::Serialize, serde::Deserialize)]
pub struct HandoffTrackInput {
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
}

#[derive(InputObject)]
pub struct HandoffInput {
    pub user_id: String,
    /// Optional so an older client is still a valid sender; omitted reads as "did not say", which
    /// leaves whatever length is already stored alone.
    pub duration_ms: Option<i64>,
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub device_id: String,
    /// Optional so a heartbeat can refresh position without re-sending the whole queue; when it is
    /// omitted the stored queue is kept as-is.
    pub queue: Option<Vec<HandoffTrackInput>>,
    pub queue_index: Option<i32>,
    /// SHA-256 of the file being played, when this device has one and has hashed it.
    ///
    /// Optional for the same reason `duration_ms` is: an older client is still a valid sender, and
    /// omitting it leaves whatever hash the track change already established alone rather than
    /// erasing it on every heartbeat.
    pub content_hash: Option<String>,
    /// End-to-end encrypted envelope containing sealed metadata (track_uri, title, artist, album).
    /// When present, Agro acts purely as a blind forwarder over WebSocket without logging or learning the music.
    pub encrypted_payload: Option<String>,
    /// The same session sealed once per friend device, so friends can still see it.
    ///
    /// `encrypted_payload` is sealed to this account's own vault key and no friend can open it, so
    /// a sealed session showed up in the social feed as a placeholder. These are the copies that
    /// fix that: one per device key published by a friend, each openable only by that device.
    ///
    /// Three states, and they are not interchangeable. Omitted means *leave the stored copies
    /// alone* — a heartbeat repeats metadata that has not changed, and re-sealing it per device
    /// ten times a minute is work with no result. Empty means *drop them*: the session ended, or
    /// stopped being sealed. Non-empty replaces the set, which is what a track change sends.
    pub presence_ciphertexts: Option<Vec<PresenceCiphertextInput>>,
}

/// One copy of a sealed session, and the friend device it was sealed to.
#[derive(async_graphql::InputObject)]
pub struct PresenceCiphertextInput {
    pub recipient_user_id: String,
    pub recipient_device_id: String,
    pub ciphertext: String,
}

/// See `update_handoff`.
pub(super) const MAX_QUEUE_TRACKS: usize = 100;

/// The most sealed copies one handoff may carry.
///
/// A copy per friend device, so this is a friend count times a device count. Rejected rather than
/// capped, unlike the queue: a truncated queue is a shorter queue, but a truncated set of copies is
/// a set of friends who silently stop seeing the session, and the sender has no way to notice.
pub(super) const MAX_PRESENCE_CIPHERTEXTS: usize = 256;

/// The longest one sealed copy may be.
///
/// The envelope holds a title, an artist, an album and an artwork URL, sealed — comfortably inside
/// this. It is a bound rather than a guess at a size: without one, a client could write copies of
/// any length, once per friend device, on every track change, and the only limit anywhere is the
/// 2 MB request body. `encrypted_payload` had exactly that gap and is bounded here too.
pub(super) const MAX_SEALED_PAYLOAD_LEN: usize = 8192;
