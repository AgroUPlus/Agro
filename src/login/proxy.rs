//! Working out which client a request really came from.

use axum::http::HeaderMap;
use std::net::{IpAddr, SocketAddr};

/// Whether a peer is allowed to speak for someone else by setting `X-Forwarded-For`.
///
/// Only a reverse proxy may, and the header is worthless from anyone else — it is chosen by the
/// client, so honouring it unconditionally would turn the rate limiter off for anybody who can
/// type a header. Loopback and the private ranges are trusted by default because a request that
/// genuinely came from the internet cannot arrive with a private source address, and `AGRO_TRUSTED_PROXY`
/// overrides that for a proxy on a public address.
pub(super) fn is_trusted_proxy(peer: IpAddr) -> bool {
    if let Ok(configured) = std::env::var("AGRO_TRUSTED_PROXY") {
        return configured
            .split(',')
            .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
            .any(|allowed| allowed == peer);
    }
    match peer {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// The address the request really came from, for rate-limiting purposes.
///
/// Behind a reverse proxy every request shares one peer address — the proxy's — so counting by
/// peer put the entire internet into a single bucket of [`MAX_ATTEMPTS`]. One person mistyping a
/// passphrase locked out everybody, which is a denial of service against your own users and was
/// exactly what a rate limiter is supposed to prevent.
///
/// The *rightmost* untrusted entry is taken, not the leftmost. `X-Forwarded-For` is appended to as
/// it passes through each hop, so the left end is whatever the original client chose to send and
/// only the right end was written by infrastructure that can be trusted.
pub(crate) fn client_ip(peer: SocketAddr, headers: &HeaderMap) -> String {
    if is_trusted_proxy(peer.ip()) {
        if let Some(forwarded) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            let candidate = forwarded
                .split(',')
                .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
                .rev()
                .find(|address| !is_trusted_proxy(*address));
            if let Some(address) = candidate {
                return address.to_string();
            }
        }
    }
    peer.ip().to_string()
}
