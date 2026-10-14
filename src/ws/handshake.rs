//! The websocket upgrade and its query parameters.

use super::*;
use crate::auth::AuthedUser;
use crate::AppState;
use axum::{
    extract::ws::WebSocketUpgrade,
    extract::{Query, State},
    response::Response,
};
use serde::Deserialize;

/// Identifies the socket, so messages can be addressed rather than shouted.
///
/// The device id is a query parameter because a WebSocket handshake from a browser cannot carry
/// custom headers — the same reason the token is accepted that way.
#[derive(Deserialize)]
pub struct SocketQuery {
    device: Option<String>,
    /// Where this device can be reached on the local network, `host:port`.
    ///
    /// Carried on the handshake rather than only by `registerNode`, and that is the whole point.
    /// The address lives in memory and is dropped when the socket closes, so something has to put
    /// it back afterwards. `registerNode` cannot: both clients call it exactly once at startup, so
    /// an address cleared by a redeploy or a dropped socket never returned and peer-to-peer
    /// transfers silently fell back to the relay until the app was restarted by hand.
    ///
    /// Arriving with the connection makes the lifetime coherent — the address exists for exactly
    /// as long as the socket does — and self-healing, because both clients already reconnect with
    /// backoff.
    lan: Option<String>,
}

/// The longest `host:port` worth accepting. Enough for an IPv6 literal with a port, and short
/// enough that the field cannot be used as storage.
pub(super) const MAX_LAN_ADDRESS: usize = 64;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: Option<axum::Extension<AuthedUser>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<std::net::SocketAddr>,
    headers: axum::http::HeaderMap,
    Query(query): Query<SocketQuery>,
) -> Response {
    let username = user.map(|u| u.username().to_string());
    let device = query.device;

    // Where this connection actually reached the server from. Recorded for every authenticated
    // socket, not only ones that claim a LAN address, because the comparison needs both ends and
    // only one of them is the device asking. `client_ip` is the same reading the rate limiter
    // trusts, so a reverse proxy does not collapse every device onto one address.
    if let (Some(u), Some(d)) = (&username, &device) {
        state
            .ws_hub
            .set_egress_address(u, d, &crate::login::client_ip(peer, &headers));
    }

    // Before the node upsert, so a device is reachable as soon as its socket is up.
    if let (Some(u), Some(d), Some(lan)) = (&username, &device, query.lan.as_deref()) {
        let lan = lan.trim();
        // Only ever this account's own device, and only a plausible address: this is attacker
        // input, and it is handed to other devices as somewhere to connect to.
        if !lan.is_empty() && lan.len() <= MAX_LAN_ADDRESS && is_plausible_host_port(lan) {
            state.ws_hub.set_lan_address(u, d, lan);
        }
    }

    if let (Some(u), Some(d)) = (&username, &device) {
        // A connect is not a naming event. The invented name is only the fallback for a device
        // that has never been seen, or the socket would rename it on every reconnect — which is
        // every time the server is redeployed.
        let petname = crate::passphrase::generate_random_petname();
        let client_type = crate::db::inferred_client_type(d);
        let _ = state.db.upsert_node(
            d,
            u,
            crate::db::NodeName::KeepOr(&petname),
            client_type,
            None,
            None,
        );
        state.offers.note_archived(u);
    }
    // Sized for what this socket carries: a client only ever sends AUTH and RESUME frames, a few
    // hundred bytes each. The defaults — a 128 KiB read buffer and a 64 MiB message ceiling — are
    // for general-purpose sockets, and at thousands of connections the buffers alone are most of
    // the server's memory.
    ws.read_buffer_size(SOCKET_READ_BUFFER)
        .max_message_size(MAX_INBOUND_FRAME)
        .on_upgrade(move |socket| handle_socket(socket, state, username, device))
}

/// A cheap sanity check on a `host:port` a client says it can be reached at.
///
/// Not a validation of reachability — that is the peer's job, and `PeerReachability` on the client
/// already probes before trusting one. This only rejects the obviously malformed, so that a value
/// handed to another device as a connection target cannot carry a scheme, a path, or a credential.
pub(super) fn is_plausible_host_port(value: &str) -> bool {
    let Some((host, port)) = value.rsplit_once(':') else {
        return false;
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    if port.parse::<u16>().map(|p| p == 0).unwrap_or(true) {
        return false;
    }
    // An IPv6 literal is bracketed; anything else is a bare host or IPv4.
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':' || b == b'_')
}
