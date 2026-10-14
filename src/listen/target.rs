//! Working out where a listen link points, and whether it is allowed.

use super::*;
use crate::AppState;

pub(super) fn resolve(params: &ListenParams, state: &AppState) -> Option<String> {
    if let Some(id) = params.id.as_deref() {
        if let Ok(Some(raw)) = state.db.get_short_link(id) {
            let url = url_host(&raw)?;
            let allowed = DEFAULT_HOSTS.iter().any(|host| *host == url.host)
                || state
                    .db
                    .allowed_share_hosts()
                    .unwrap_or_default()
                    .contains(&url.host);
            if allowed {
                // Aggregate only: the owner learns the link is being used, and nothing is recorded
                // about who used it. See migration 6 in `db.rs`.
                state.db.record_short_link_click(id);
                return Some(url.full);
            }
        }
        return None;
    }

    if let Some(video_id) = params.v.as_deref() {
        if is_video_id(video_id) {
            return Some(format!("https://music.youtube.com/watch?v={video_id}"));
        }
        return None;
    }

    let raw = params.u.as_deref()?;
    let url = url_host(raw)?;
    let allowed = DEFAULT_HOSTS.iter().any(|host| *host == url.host)
        || state
            .db
            .allowed_share_hosts()
            .unwrap_or_default()
            .contains(&url.host);

    allowed.then_some(url.full)
}

pub(super) struct TargetUrl {
    pub(super) host: String,
    pub(super) full: String,
}

/// The host of an `https` URL, or nothing.
///
/// Parsed by hand rather than by pulling in a URL crate for one field: this only has to recognise
/// `https://host[:port]/…`, and anything it cannot recognise is refused rather than guessed at.
/// `http` is refused too — a downgrade the recipient never agreed to.
pub(super) fn url_host(raw: &str) -> Option<TargetUrl> {
    let rest = raw.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    // Credentials in the authority (`user@host`) are how a URL is made to look like one host while
    // resolving to another, which is exactly the trick this route must not forward.
    if authority.contains('@') || authority.is_empty() {
        return None;
    }
    let host = authority.split(':').next()?.to_lowercase();
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(TargetUrl {
        host,
        full: raw.to_string(),
    })
}

pub(super) fn is_video_id(value: &str) -> bool {
    value.len() == VIDEO_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}
