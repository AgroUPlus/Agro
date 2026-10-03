//! `/listen?id=<id>` for a short link that was deleted for going unused.
//!
//! **`SHARE_LINKS.md` §6a.** The ordinary refusal says nothing about why, so it cannot be used to
//! probe the allowlist. This page is the one exception, and it gives away only what the visitor
//! already knows — that they hold a link — plus that it lapsed. It names no track, no owner and no
//! date, because the tombstone keeps none of them.

use axum::{
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};

use crate::db_short_links::IDLE_DAYS;

/// The page. `410 Gone` rather than the refusal's `404`: the link did exist and will not return.
pub(crate) fn page() -> Response {
    (
        StatusCode::GONE,
        [
            (header::REFERRER_POLICY, "no-referrer"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        Html(TEMPLATE.replace("__DAYS__", &IDLE_DAYS.to_string())),
    )
        .into_response()
}

const TEMPLATE: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex, nofollow">
<meta name="referrer" content="no-referrer">
<title>Link deleted</title>
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
  <h1>This link has been deleted</h1>
  <p>Nobody opened it for __DAYS__ days, so it was removed. Ask whoever sent it for a new one.</p>
</main>
</body>
</html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_gone_and_how_long_a_link_lasts_unused() {
        let response = page();
        assert_eq!(response.status(), StatusCode::GONE);
        assert!(TEMPLATE.contains("__DAYS__"));
        assert!(!TEMPLATE.replace("__DAYS__", "30").contains("__"));
    }
}
