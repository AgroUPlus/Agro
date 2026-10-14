//! The payload types and builders the social resolvers return.

use crate::db_social::{FriendState, Profile};
use crate::schema::{bounded, StatEntry};
use async_graphql::SimpleObject;

#[derive(SimpleObject, Clone)]
pub struct ProfilePayload {
    pub username: String,
    pub display_name: Option<String>,
    pub bio: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: String,
    /// `none`, `pending`, `accepted` or `blocked`, from the viewer's side.
    pub friend_state: String,
    /// True when it was the viewer who sent an unanswered request. Meaningless unless
    /// `friend_state` is `pending`.
    pub outgoing: bool,
    pub show_now_playing: bool,
    pub show_stats: bool,
    pub discoverable: bool,
    pub share_library: bool,
    pub show_activity: bool,
    pub incognito: bool,
    /// Public identity key for E2EE track drops and communications.
    pub public_key: Option<String>,
    /// Whether this account's plays count toward the fleet-wide Popular on Agro chart.
    pub popular_opt_in: bool,
}

/// What a client needs to finish enrolling a second factor. Shown once and never again.
#[derive(SimpleObject, Clone)]
pub struct TotpEnrolmentPayload {
    /// For the QR code.
    pub otpauth_uri: String,
    /// For typing in by hand when the camera will not cooperate.
    pub secret_base32: String,
}

/// The recovery codes, returned once when enrolment is confirmed.
#[derive(SimpleObject, Clone)]
pub struct TotpConfirmedPayload {
    /// Ten single-use codes. The server keeps only their digests and cannot show these again.
    pub recovery_codes: Vec<String>,
    /// How many device tokens were signed out by enrolling. See `confirmTotp`.
    pub devices_signed_out: i64,
}

/// The state of an account's second factor, for the settings screen.
#[derive(SimpleObject, Clone)]
pub struct TotpStatus {
    pub is_enabled: bool,
    /// Unused recovery codes left. Zero means the next lost phone is a lockout.
    pub recovery_codes_remaining: i64,
    /// False when the server has no `AGRO_SECRET_KEY`, so enrolment cannot be offered at all.
    pub is_available: bool,
}

/// What a friend is playing, projected from the one handoff row their account already keeps.
///
/// Presence needs no storage of its own: the handoff a device publishes so its *owner* can pick a
/// session up elsewhere is the same fact a friend is being shown.
#[derive(SimpleObject, Clone)]
pub struct FriendNowPlaying {
    pub username: String,
    pub track_uri: String,
    pub track_title: String,
    pub artist_name: String,
    pub album_name: Option<String>,
    pub artwork_url: Option<String>,
    pub position_ms: i64,
    pub is_playing: bool,
    pub updated_at: String,
    /// Which of the host's devices is playing it.
    ///
    /// A listener needs this to ask for the audio at all: both the direct transfer and the relay
    /// address one device, not an account.
    pub device_id: String,
    /// SHA-256 of the file the host is playing, when they have one. `None` for anything streamed,
    /// which is what sends a listener back to matching by name.
    pub content_hash: Option<String>,
    /// Where to reach the host's device directly, and the token to present when doing so.
    ///
    /// Both are `Some` only when the two devices look like they share a local network *and* the
    /// viewer is allowed to see this account at all. Neither is the authorisation on its own — see
    /// [`crate::ws::WsHub::same_network`] for why the address is a hint and the token is the gate.
    pub peer_lan_address: Option<String>,
    pub peer_lan_token: Option<String>,
    /// This session's real metadata, sealed to the asking device's key.
    ///
    /// Pairwise, like the two fields above, and for a stronger reason: a copy is sealed to one
    /// device's public key, so handing it to anyone else is handing over bytes they cannot open.
    /// `None` whenever the question was asked without naming a device, when the host sealed
    /// nothing, or when the host has not sealed anything to *this* device yet — a friend who has
    /// published no key, or a device added since the last track change.
    ///
    /// When this is set, the plaintext `track_title` and `artist_name` above are a placeholder and
    /// the viewer is expected to open this instead. The server cannot tell the difference; it has
    /// never held the key.
    pub encrypted_presence: Option<String>,
}

#[derive(SimpleObject, Clone)]
pub struct FriendEdgePayload {
    pub profile: ProfilePayload,
    pub now_playing: Option<FriendNowPlaying>,
}

/// How much two accounts' listening overlaps.
#[derive(SimpleObject, Clone)]
pub struct TasteMatch {
    /// 0–100. See `taste_score` for what it actually measures.
    pub score: i64,
    pub shared_artists: Vec<StatEntry>,
    pub shared_tracks: Vec<StatEntry>,
}

#[derive(SimpleObject, Clone)]
pub struct ListenAlongPayload {
    pub host: String,
    pub listeners: Vec<String>,
    pub now_playing: Option<FriendNowPlaying>,
}

/// One device's published identity key.
///
/// The device id is opaque to the server and meaningful only within the account — it exists so a
/// sender can seal one copy per device and a device can recognise its own copy on the way back.
#[derive(SimpleObject, Clone, Debug)]
pub struct DeviceKeyPayload {
    pub device_id: String,
    pub public_key: String,
}

/// A friend code as a client renders it.
///
/// [`ttl_seconds`] is sent rather than left for the client to derive from [`expires_at`], because
/// the client's clock is not the server's and a QR panel that re-mints on a drifting timer would
/// either show a dead code or thrash. It counts down from *now* on the device that asked.
#[derive(SimpleObject, Clone, Debug)]
pub struct FriendCodePayload {
    pub code: String,
    pub expires_at: String,
    pub ttl_seconds: i64,
}

/// Short enough that a code photographed off a screen is dead before the photographer gets home,
/// long enough to find the camera and scan it.
pub(super) const FRIEND_CODE_TTL_MINUTES: i64 = 5;

#[derive(SimpleObject, Clone)]
pub struct InvitePayload {
    pub code: String,
    pub created_by: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub max_uses: i64,
    pub used_count: i64,
    pub revoked: bool,
}

pub fn profile_payload(
    profile: &Profile,
    state: Option<FriendState>,
    outgoing: bool,
) -> ProfilePayload {
    ProfilePayload {
        username: profile.username.clone(),
        display_name: profile.display_name.clone(),
        bio: profile.bio.clone(),
        avatar_url: profile.avatar_url.clone(),
        created_at: profile.created_at.clone(),
        friend_state: match state {
            Some(FriendState::Accepted) => "accepted",
            Some(FriendState::Pending) => "pending",
            // A block is never disclosed to the account it was applied to. From their side this is
            // simply somebody they are not friends with.
            Some(FriendState::Blocked) | None => "none",
        }
        .to_string(),
        outgoing,
        show_now_playing: profile.show_now_playing,
        show_stats: profile.show_stats,
        discoverable: profile.discoverable,
        share_library: profile.share_library,
        show_activity: profile.show_activity,
        incognito: profile.incognito,
        public_key: profile.public_key.clone(),
        popular_opt_in: profile.popular_opt_in,
    }
}

/// The names present in both lists, carrying the *caller's* play count for each.
pub(super) fn shared(mine: &[(String, i64)], theirs: &[(String, i64)]) -> Vec<StatEntry> {
    mine.iter()
        .filter(|(name, _)| {
            theirs
                .iter()
                .any(|(other, _)| other.eq_ignore_ascii_case(name))
        })
        .map(|(name, value)| StatEntry {
            name: name.clone(),
            value: *value,
        })
        .collect()
}

/// An avatar has to be a URL this app will actually load, and nothing else.
///
/// `javascript:` and `data:` are the reason this is an allowlist rather than a length check: an
/// avatar is rendered in someone else's client, and a scheme is the cheapest thing to get wrong.
pub(super) fn validated_avatar(raw: &str) -> async_graphql::Result<String> {
    let url = bounded(raw, 2048, "Avatar URL")?;
    if url.is_empty() || url.starts_with("https://") || url.starts_with("http://") {
        Ok(url)
    } else {
        Err("An avatar URL must start with http:// or https://".into())
    }
}
