//! Share-link forwarding: the public half of the custom share domain.
//!
//! **This route implements `SHARE_LINKS.md`, which is normative.** Wanda mints these links and
//! frwd.top forwards them too; the parameter names and bounds below are duplicated in Kotlin and
//! JavaScript, and nothing in either toolchain will notice them drifting apart.
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

use crate::AppState;
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use serde::Deserialize;

/// Public, unauthenticated, and logs nothing — see the module docs. **Implements
/// `SHARE_LINKS.md`, which is normative and shared with external Kotlin/JS clients: the
/// parameter names and bounds here are frozen.**
#[utoipa::path(
    get,
    path = "/listen",
    tag = "sharing",
    params(ListenParams),
    responses(
        (status = 200, description = "HTML page that forwards the browser to the resolved track link client-side", content_type = "text/html"),
        (status = 404, description = "No usable target (unknown short link, or a host not on the allow-list)", content_type = "text/html"),
    ),
)]
pub async fn listen_handler(
    Query(params): Query<ListenParams>,
    State(state): State<AppState>,
) -> Response {
    // Exactly one target (§3.1): a playlist id beside a track target is refused, not picked from.
    if let Some(playlist) = params.pl.as_deref() {
        let alone = params.id.is_none() && params.v.is_none() && params.u.is_none();
        return if alone && crate::listen_playlist::is_playlist_id(playlist) {
            crate::listen_playlist::page(playlist)
        } else {
            refusal()
        };
    }

    let target = match resolve(&params, &state) {
        Some(target) => target,
        // A link swept for going unused says so (§6a); anything else gets the refusal that says
        // nothing (§6). A failed lookup is the refusal too — it must not read as "deleted".
        None => {
            let retired = params.id.as_deref().is_some_and(|id| {
                state.db.short_link_retired(id).unwrap_or_else(|e| {
                    tracing::warn!("listen: tombstone lookup failed: {e}");
                    false
                })
            });
            return if retired {
                crate::listen_retired::page()
            } else {
                refusal()
            };
        }
    };

    let mut search_params = if let Some(id) = &params.id {
        format!("?id={}", id)
    } else if let Some(v) = &params.v {
        format!("?v={}", v)
    } else if let Some(u) = &params.u {
        format!("?u={}", urlencoding::encode(u))
    } else {
        String::new()
    };

    // Rebuilt rather than passed through, so only known parameters within known bounds reach the
    // app's URL. Forwarding the raw query string would put whatever a stranger appended into a
    // `wanda://` link, which is the same open-redirect mistake this route avoids for `u`.
    if !search_params.is_empty() {
        search_params.push_str(&playback_suffix(params.s, params.p));
    }

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
    var btnPause = document.getElementById("btnPause");
    if (btnPause) {
      btnPause.onclick = function() {
        isPaused = true;
        var wrap = document.getElementById("progressWrap");
        if (wrap) wrap.style.display = "none";
      };
    }
    function tick() {
      if (isPaused) return;
      var remaining = Math.max(0, duration - (Date.now() - start));
      if (fill) fill.style.width = ((remaining / duration) * 100) + "%";
      if (txt) txt.textContent = "Forwarding in " + (remaining / 1000).toFixed(1) + "s…";
      if (remaining > 0) {
        requestAnimationFrame(tick);
      } else if (target && target.length > 0) {
        window.location.replace(target);
      }
    }
    requestAnimationFrame(tick);
  })();

</script>

</body>

</html>"##;

    let html = template
        .replace("__SEARCH_PARAMS__", &escape_html(&search_params))
        .replace("__TARGET__", &escape_html(&target))
        .replace("__TARGET_JSON__", &escape_script_json(&target_json))
        .replace("__SEARCH_JSON__", &escape_script_json(&search_json));

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
            (
                header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate, max-age=0".to_string(),
            ),
            (header::PRAGMA, "no-cache".to_string()),
        ],
        Html(html),
    )
        .into_response()
}

#[cfg(test)]
mod escaping_tests;

#[cfg(test)]
mod playback_tests;

mod escaping;
mod params;
mod refusal;
mod target;
pub(crate) use escaping::escape_html;
use escaping::escape_script_json;
use params::*;
use refusal::refusal;
use target::resolve;
#[cfg(test)]
mod tests;
