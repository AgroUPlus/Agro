//! Response headers every route gets.

use crate::*;

/// Adds the headers that limit what a compromised page can do.
///
/// The device token lives in `localStorage`, which is reachable by any script that runs on this
/// origin. A strict CSP is what makes "any script" hard to arrange: the dashboard is a
/// self-contained bundle served from this origin, so it needs no third-party sources at all, and
/// saying so closes the ways an injected `<script src>` or an exfiltrating `fetch` would work.
///
/// HSTS is only sent when the request already arrived over TLS. Sending it on a plaintext response
/// is meaningless — anyone able to strip the header is able to strip that too — and on a
/// LAN-only deployment reached by IP it would pin a browser to HTTPS the server does not speak.
pub async fn security_headers(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let was_https = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|proto| proto.eq_ignore_ascii_case("https"));

    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; \
             script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; \
             style-src 'self' 'unsafe-inline'; \
             img-src 'self' data: https:; \
             media-src 'self' blob:; \
             connect-src 'self' ws: wss:; \
             font-src 'self' data:; \
             object-src 'none'; \
             base-uri 'none'; \
             form-action 'self'; \
             frame-ancestors 'none'",
        ),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "referrer-policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );
    if was_https {
        headers.insert(
            "strict-transport-security",
            HeaderValue::from_static("max-age=31536000; includeSubDomains"),
        );
    }
    response
}
