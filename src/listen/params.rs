//! The listen page's query parameters and the playback window suffix.

use super::*;

/// Hosts that need no configuring: the ones the players mint links for out of the box.
pub(super) const DEFAULT_HOSTS: &[&str] = &[
    "music.youtube.com",
    "youtube.com",
    "www.youtube.com",
    "youtu.be",
];

/// A YouTube id is a fixed eleven characters of URL-safe base64. Checking it is what stops a
/// decorated value from being pasted into a `Location` header.
pub(super) const VIDEO_ID_LEN: usize = 11;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ListenParams {
    /// A short link UID minted by Agro.
    pub(super) id: Option<String>,
    /// A YouTube video id, carried in the open: it is public already, and it is what makes a
    /// shared link readable.
    pub(super) v: Option<String>,
    /// Any other track link, percent-encoded. Checked against the allowlist before it is used.
    pub(super) u: Option<String>,
    /// A playlist published to this server, by id. See `listen_playlist`.
    pub(super) pl: Option<String>,
    /// Playback rate the sharer was listening at, so the link plays what they meant.
    pub(super) s: Option<f32>,
    /// Pitch, independent of rate. Both are bounded to what the player will accept.
    pub(super) p: Option<f32>,
}

/// What the players will accept. A link outside it is forwarded without the parameters rather
/// than clamped: a link asking for 40x is not one whose intent can be recovered.
pub(super) const RATE_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;

/// The `&s=…&p=…` a forwarded link carries, or an empty string.
///
/// `SHARE_LINKS.md` §3.2, kept as a pure function so the rule can be tested without standing up a
/// router: both or neither, both within [`RATE_RANGE`], and **dropped rather than clamped** when
/// out of range — a link asking for 40x is not one whose intent can be recovered.
pub(super) fn playback_suffix(speed: Option<f32>, pitch: Option<f32>) -> String {
    match (speed, pitch) {
        (Some(s), Some(p)) if RATE_RANGE.contains(&s) && RATE_RANGE.contains(&p) => {
            format!("&s={}&p={}", s, p)
        }
        _ => String::new(),
    }
}
