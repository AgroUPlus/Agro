//! `/listen?pl=<id>`: the public face of a playlist published to this server.
//!
//! **Part of `SHARE_LINKS.md` §3.1, which is normative.** A `wanda://playlist?agro=<id>` link cannot
//! be tapped in a chat, so Wanda shares this page instead and the page hands the app that link.
//!
//! The page says nothing about a playlist that is not public. Who can open one is decided by the
//! server when the app asks for it with the visitor's own account; an unauthenticated page that
//! showed the title of a friends-only or private playlist would publish it to anyone holding the
//! link. So for those the page is byte-for-byte the one an id that does not exist gets.
//!
//! A **public** playlist is the exception: anyone may open it already, so its title and track
//! count go in the page and its link preview. Nothing else does — not the owner, not the tracks.

use crate::listen::escape_html;
use crate::listen::preview::Preview;
use crate::AppState;
use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};

/// Where someone without Wanda can get it. A fixed address, never one taken from the link.
const INSTALL_URL: &str = "https://github.com/AgroUPlus/Wanda/releases/latest";

/// Playlist ids are UUIDs, and nothing else reaches the app: the value is placed in a `wanda://`
/// URL and an `intent://` URL, and the app sends it back to a server.
pub(crate) fn is_playlist_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    let lengths = [8, 4, 4, 4, 12];
    groups.len() == lengths.len()
        && groups
            .iter()
            .zip(lengths)
            .all(|(group, len)| group.len() == len && group.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// What the page may say about playlist `id`: its title and size when it is public, and the same
/// nothing as for an unknown id otherwise.
pub(crate) fn preview(state: &AppState, id: &str) -> Preview {
    let public = state.db.get_playlist(id).unwrap_or_else(|e| {
        tracing::warn!("listen: playlist preview lookup failed: {e}");
        None
    });
    let named = public
        .filter(|playlist| playlist.is_public)
        .map(|playlist| {
            let count = state.db.get_playlist_items(id).map(|items| items.len());
            let subtitle = match count {
                Ok(1) => Some("1 track".to_string()),
                Ok(n) => Some(format!("{n} tracks")),
                Err(e) => {
                    tracing::warn!("listen: playlist size lookup failed: {e}");
                    None
                }
            };
            (playlist.title, subtitle)
        });
    let (title, subtitle) = named.unzip();
    Preview {
        title,
        subtitle: subtitle.flatten(),
        image: None,
        noun: "playlist",
    }
}

/// The page for a well-formed id; the caller refuses anything else.
pub(crate) fn page(id: &str, preview: &Preview) -> Response {
    debug_assert!(is_playlist_id(id));
    let subheading = preview
        .subtitle
        .as_deref()
        .unwrap_or("It opens with your account on this server, if its owner shared it with you.");
    let app_link = format!("wanda://playlist?agro={id}");
    let intent = format!(
        "intent://playlist?agro={id}#Intent;scheme=wanda;package=com.wander.android;S.browser_fallback_url={};end",
        urlencoding::encode(INSTALL_URL)
    );
    let html = TEMPLATE
        .replace("__APP_LINK__", &app_link)
        .replace(
            "__INTENT_JSON__",
            &serde_json::to_string(&intent).expect("a String always serializes"),
        )
        .replace("__INSTALL__", INSTALL_URL)
        .replace("__TITLE__", &escape_html(&preview.page_title()))
        .replace("__PREVIEW_META__", &preview.meta_tags())
        .replace("__HEADING__", &escape_html(preview.heading()))
        .replace("__SUBHEADING__", &escape_html(subheading));

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
            (header::CACHE_CONTROL, "no-store, max-age=0".to_string()),
        ],
        Html(html),
    )
        .into_response()
}

// Nothing interpolated here comes from the visitor except the id, and the id is hex and dashes.
// A public playlist's title is its owner's text, and is escaped like any other.
const TEMPLATE: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>__TITLE__</title>
__PREVIEW_META__
<style>
  * { box-sizing: border-box; margin: 0; padding: 0; }
  body { min-height: 100dvh; display: flex; align-items: center; justify-content: center; padding: 1.25rem;
         background: #000; color: #f2f2f2; -webkit-font-smoothing: antialiased;
         font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif; }
  main { width: 100%; max-width: 380px; background: #121212; border: 1px solid #262626; border-radius: 6px;
         padding: 1.5rem; display: flex; flex-direction: column; gap: 0.6rem; }
  .tag { font-size: 0.7rem; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; color: #7e7e7e; }
  h1 { font-size: 1.15rem; font-weight: 600; }
  p { font-size: 0.82rem; color: #7e7e7e; line-height: 1.4; margin-bottom: 0.8rem; }
  a { display: flex; align-items: center; justify-content: center; height: 42px; border-radius: 4px;
      font-size: 0.88rem; font-weight: 600; text-decoration: none; }
  .primary { background: #fff; color: #000; }
  .secondary { color: #f2f2f2; border: 1px solid #262626; }
</style>
</head>
<body>
<main>
  <div class="tag">Wanda &middot; Shared playlist</div>
  <h1>__HEADING__</h1>
  <p>__SUBHEADING__</p>
  <a id="open" class="primary" href="__APP_LINK__">Open in Wanda</a>
  <a class="secondary" href="__INSTALL__" rel="noreferrer">Get Wanda</a>
</main>
<script>
  if (/android/i.test(navigator.userAgent)) {
    var intent = __INTENT_JSON__;
    document.getElementById("open").href = intent;
    window.location.href = intent;
  }
</script>
</body>
</html>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_a_uuid() {
        assert!(is_playlist_id("3f2b8c1e-9a4d-4e57-8b21-6d0f5c7a9e10"));
        assert!(!is_playlist_id("3f2b8c1e-9a4d-4e57-8b21-6d0f5c7a9e1"));
        assert!(!is_playlist_id("3f2b8c1e-9a4d-4e57-8b21-6d0f5c7a9e10&u=x"));
        assert!(!is_playlist_id("\"><script>-9a4d-4e57-8b21-6d0f5c7a9e10"));
        assert!(!is_playlist_id(""));
    }

    #[tokio::test]
    async fn hands_the_app_its_own_playlist_link() {
        let id = "3f2b8c1e-9a4d-4e57-8b21-6d0f5c7a9e10";
        let preview = Preview {
            title: None,
            subtitle: None,
            image: None,
            noun: "playlist",
        };
        let response = page(id, &preview);
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains(&format!("href=\"wanda://playlist?agro={id}\"")));
        assert!(html.contains("intent://playlist?agro="));
        assert!(!html.contains("__"));
    }
}
