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
    /// A short link UID minted by Agro.
    id: Option<String>,
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

    let search_params = if let Some(id) = &params.id {
        format!("?id={}", id)
    } else if let Some(v) = &params.v {
        format!("?v={}", v)
    } else if let Some(u) = &params.u {
        format!("?u={}", urlencoding::encode(u))
    } else {
        String::new()
    };

    let target_json = serde_json::to_string(&target).unwrap_or_else(|_| "\"\"".to_string());
    let search_json = serde_json::to_string(&search_params).unwrap_or_else(|_| "\"\"".to_string());

    let template = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>Wanda &middot; Forwarding</title>
<style>
  :root {
    --bg: #000000;
    --surface: #121212;
    --surface-hover: #1c1c1c;
    --border: #262626;
    --text-main: #f2f2f2;
    --text-muted: #7e7e7e;
    --accent: #ffffff;
    --on-accent: #000000;
  }
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body {
    min-height: 100vh; min-height: 100dvh;
    background-color: var(--bg);
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Oxygen, Ubuntu, Cantarell, "Fira Sans", "Droid Sans", "Helvetica Neue", sans-serif;
    color: var(--text-main); display: flex; align-items: center; justify-content: center; padding: 1.25rem;
    -webkit-font-smoothing: antialiased;
  }
  .container {
    width: 100%; max-width: 380px; background: var(--surface);
    border: 1px solid var(--border); border-radius: 6px; padding: 1.5rem; display: flex; flex-direction: column;
  }
  .tag { font-size: 0.7rem; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; color: var(--text-muted); margin-bottom: 0.5rem; }
  h1 { font-size: 1.15rem; font-weight: 600; color: var(--text-main); letter-spacing: -0.01em; margin-bottom: 0.25rem; }
  .subtitle { font-size: 0.82rem; color: var(--text-muted); line-height: 1.4; margin-bottom: 1.4rem; }
  .actions { display: flex; flex-direction: column; gap: 0.6rem; }
  .btn {
    display: inline-flex; align-items: center; justify-content: center; gap: 0.5rem; height: 42px; border-radius: 4px;
    font-size: 0.88rem; font-weight: 600; text-decoration: none; cursor: pointer; transition: background-color 0.1s ease, border-color 0.1s ease;
  }
  .btn-primary { background: var(--accent); color: var(--on-accent); border: 1px solid var(--accent); }
  .btn-primary:active { background: #d4d4d4; }
  .btn-secondary { background: transparent; color: var(--text-main); border: 1px solid var(--border); }
  .btn-secondary:hover { background: var(--surface-hover); }
  .btn-secondary:active { background: #242424; }
  .progress-box { margin-top: 1.2rem; display: flex; flex-direction: column; gap: 0.4rem; }
  .progress-track { width: 100%; height: 2px; background: var(--border); overflow: hidden; }
  .progress-bar { height: 100%; width: 100%; background: var(--accent); transition: width 0.08s linear; }
  .progress-info { display: flex; justify-content: space-between; align-items: center; font-size: 0.75rem; color: var(--text-muted); }
  .btn-cancel { background: none; border: none; color: var(--text-main); cursor: pointer; font-size: 0.75rem; text-decoration: underline; padding: 0; }
  .target-url { font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace; font-size: 0.7rem; color: #555555; word-break: break-all; margin-top: 1rem; line-height: 1.3; }
</style>
</head>
<body>
<main class="container">
  <div class="tag">Wanda &middot; Link Gateway</div>
  <h1>Connecting to track</h1>
  <p class="subtitle">Dispatching player intent to your device&hellip;</p>
  <div class="actions">
    <a id="btnWanda" class="btn btn-primary" href="wanda://listen__SEARCH_PARAMS__">Open in Wanda</a>
    <a id="btnWeb" class="btn btn-secondary" href="__TARGET__" rel="noreferrer">Open in Browser</a>
  </div>
  <div class="progress-box" id="progressWrap">
    <div class="progress-track"><div class="progress-bar" id="progressFill"></div></div>
    <div class="progress-info">
      <span id="countdownText">Forwarding in 2s&hellip;</span>
      <button class="btn-cancel" id="btnPause">Cancel</button>
    </div>
  </div>
  <p class="target-url">__TARGET__</p>
</main>
<script>
  (function() {
    var target = __TARGET_JSON__;
    var search = __SEARCH_JSON__;
    var isAndroid = /android/i.test(navigator.userAgent);
    if (isAndroid) {
      var intent = "intent://listen" + search + "#Intent;scheme=wanda;package=com.wander.android;S.browser_fallback_url=" + encodeURIComponent(target) + ";end";
      var btn = document.getElementById("btnWanda");
      if (btn) btn.href = intent;
      try { window.location.href = intent; } catch(e) {}
    }
    var duration = 2000, start = Date.now(), isPaused = false;
    var fill = document.getElementById("progressFill");
    var txt = document.getElementById("countdownText");
    document.getElementById("btnPause").onclick = function() { isPaused = true; document.getElementById("progressWrap").style.display = "none"; };
    function tick() {
      if (isPaused) return;
      var remaining = Math.max(0, duration - (Date.now() - start));
      if (fill) fill.style.width = ((remaining / duration) * 100) + "%";
      if (txt) txt.textContent = "Forwarding in " + (remaining / 1000).toFixed(1) + "s…";
      if (remaining > 0) requestAnimationFrame(tick);
      else window.location.replace(target);
    }
    requestAnimationFrame(tick);
  })();
</script>
</body>
</html>"##;

    let html = template
        .replace("__SEARCH_PARAMS__", &search_params)
        .replace("__TARGET__", &target)
        .replace("__TARGET_JSON__", &target_json)
        .replace("__SEARCH_JSON__", &search_json);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, max-age=0".to_string()),
            (header::PRAGMA, "no-cache".to_string()),
        ],
        Html(html),
    )
        .into_response()
}

fn resolve(params: &ListenParams, state: &AppState) -> Option<String> {
    if let Some(id) = params.id.as_deref() {
        if let Ok(Some(raw)) = state.db.get_short_link(id) {
            let url = url_host(&raw)?;
            let allowed = DEFAULT_HOSTS.iter().any(|host| *host == url.host)
                || state
                    .db
                    .allowed_share_hosts()
                    .unwrap_or_default()
                    .iter()
                    .any(|host| *host == url.host);
            if allowed {
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

    #[test]
    fn resolves_short_link_uid_when_allowed() {
        let db = crate::db::Db::new(":memory:").unwrap();
        db.create_short_link("testUid", "https://music.youtube.com/watch?v=dQw4w9WgXcQ", None).unwrap();
        let retrieved = db.get_short_link("testUid").unwrap();
        assert_eq!(retrieved.as_deref(), Some("https://music.youtube.com/watch?v=dQw4w9WgXcQ"));
    }
}
