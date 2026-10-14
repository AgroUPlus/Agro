//! The page shown when a listen link is refused.

use super::*;

/// Deliberately says nothing about *why*. A page that reports "that host is not allowed" is a tool
/// for finding out which hosts are.
pub(super) fn refusal() -> Response {
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
