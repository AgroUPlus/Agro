//! Share-link forwarding: the public half of the custom share domain.
//!
//! A player rewrites its share links onto the domain the user set — `frwd.top/listen?v=<id>` for a
//! YouTube Music track, `?u=<url>` for anything else — and this route sends whoever opens one on
//! to the track. Point the domain's DNS at this server and the whole feature is Agro's; leave the
//! domain unset and the players share their backends' own links, which is what happens with no
//! Agro at all.
//!
//! Two rules this route does not break:
//!
//! 1. It forwards only to hosts an account here has allowed. A forwarder that will send a visitor
//!    to any address handed to it is an open redirect — a phishing URL wearing the user's domain,
//!    with that domain's reputation paying for it.
//! 2. It records nothing. No log line, no counter, no cookie. A shortener is a redirect log by
//!    construction, and the players this serves are built on the premise that nobody keeps one.

use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

use crate::AppState;

/// Hosts that need no configuring: the ones the players mint links for out of the box.
const DEFAULT_HOSTS: &[&str] = &[
    "music.youtube.com",
    "youtube.com",
    "www.youtube.com",
    "youtu.be",
];

/// A YouTube id is a fixed eleven characters of URL-safe base64. Checking it is what stops a
/// decorated value from being pasted into a `Location` header.
const VIDEO_ID_LEN: usize = 11;

#[derive(Deserialize)]
pub struct ListenParams {
    /// A YouTube video id, carried in the open: it is public already, and it is what makes a
    /// shared link readable.
    v: Option<String>,
    /// Any other track link, percent-encoded. Checked against the allowlist before it is used.
    u: Option<String>,
}

pub async fn listen_handler(
    Query(params): Query<ListenParams>,
    State(state): State<AppState>,
) -> Response {
    let target = match resolve(&params, &state) {
        Some(target) => target,
        None => return refusal(),
    };

    // 302 rather than 301: a permanent redirect is cached by the browser, and a link that has been
    // cached cannot later be changed or withdrawn.
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, target),
            // Nothing about the visitor travels on to the destination.
            (header::REFERRER_POLICY, "no-referrer".to_string()),
            (header::CACHE_CONTROL, "no-store".to_string()),
        ],
    )
        .into_response()
}

fn resolve(params: &ListenParams, state: &AppState) -> Option<String> {
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
            .iter()
            .any(|host| *host == url.host);

    allowed.then(|| url.full)
}

struct TargetUrl {
    host: String,
    full: String,
}

/// The host of an `https` URL, or nothing.
///
/// Parsed by hand rather than by pulling in a URL crate for one field: this only has to recognise
/// `https://host[:port]/…`, and anything it cannot recognise is refused rather than guessed at.
/// `http` is refused too — a downgrade the recipient never agreed to.
fn url_host(raw: &str) -> Option<TargetUrl> {
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

fn is_video_id(value: &str) -> bool {
    value.len() == VIDEO_ID_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Deliberately says nothing about *why*. A page that reports "that host is not allowed" is a tool
/// for finding out which hosts are.
fn refusal() -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(
            r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>Nothing to open</title>
<style>
  body { margin:0; min-height:100dvh; display:grid; place-items:center; padding:24px;
         background:#14181d; color:#d5dae1;
         font:16px/1.6 system-ui, -apple-system, "Segoe UI", Roboto, sans-serif; }
  main { max-width:420px; text-align:center; }
  h1 { font-size:1.25rem; margin:0 0 8px; color:#f0f3f6; }
  p { color:#8d97a3; margin:0; }
</style>
</head>
<body>
<main>
  <h1>Nothing to open</h1>
  <p>This link does not carry a track this server will forward to.</p>
</main>
</body>
</html>"#,
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_well_formed_video_id() {
        assert!(is_video_id("dQw4w9WgXcQ"));
    }

    #[test]
    fn rejects_ids_that_are_not_ids() {
        assert!(!is_video_id("short"));
        assert!(!is_video_id("dQw4w9WgXcQ&x=1"));
        assert!(!is_video_id("../../etc/passwd"));
    }

    #[test]
    fn reads_the_host_of_an_https_url() {
        let parsed = url_host("https://Music.Example.com/rest?x=1").unwrap();
        assert_eq!(parsed.host, "music.example.com");
    }

    #[test]
    fn refuses_urls_that_disguise_their_host() {
        // The authority here is `evil.example`, however much it reads as youtube.com.
        assert!(url_host("https://music.youtube.com@evil.example/x").is_none());
        assert!(url_host("http://music.example.com/x").is_none());
        assert!(url_host("javascript:alert(1)").is_none());
        assert!(url_host("https://localhost/x").is_none());
    }
}
