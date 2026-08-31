use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::{Query, State},
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::auth::AuthedUser;
use crate::AppState;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WsMessage {
    pub msg_type: String,
    pub payload: serde_json::Value,
    /// The account this concerns. A socket only forwards messages for its own account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// One device, when the message is only that device's business — a sync offer, say. `None`
    /// means every device on the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_device: Option<String>,
}

/// Whether two observed public addresses put their devices behind the same edge.
///
/// IPv4 must match exactly. IPv6 matches on the `/64` prefix, which is the unit a router hands to
/// a LAN; comparing the full address there would answer "no" for every pair of devices on the same
/// network, since each picks its own host portion.
fn same_egress(a: &str, b: &str) -> bool {
    use std::net::IpAddr;
    match (a.parse::<IpAddr>(), b.parse::<IpAddr>()) {
        (Ok(IpAddr::V4(a)), Ok(IpAddr::V4(b))) => a == b,
        (Ok(IpAddr::V6(a)), Ok(IpAddr::V6(b))) => a.octets()[..8] == b.octets()[..8],
        // One of the two is not an address the server could parse. Refusing is the safe answer:
        // it costs a fallback to the relay, and guessing costs a private address.
        _ => false,
    }
}

/// What is known about where a connected device sits on the network.
///
/// Both halves are volatile and both are needed together: the LAN address is where a peer would
/// connect, and the egress address is the only evidence the server has about whether connecting
/// could possibly work.
#[derive(Default, Clone)]
struct PeerNetwork {
    /// `host:port` on the device's own local network, as the device reported it.
    lan: Option<String>,
    /// The public address this device's connection arrived from, as seen by the server.
    ///
    /// Never handed to another client. It exists only to be compared with another device's, and a
    /// public address is a far more identifying thing than the RFC1918 address it gates.
    egress: Option<String>,
}

/// How long a peer-to-peer grant stays usable.
///
/// Long enough to cover a listening session's worth of track changes without reminting on every
/// frame, short enough that a token which escapes stops working the same afternoon.
const P2P_GRANT_TTL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Reissue rather than hand out a token about to expire mid-transfer.
const P2P_GRANT_MIN_REMAINING: std::time::Duration = std::time::Duration::from_secs(60);

pub struct WsHub {
    pub tx: broadcast::Sender<WsMessage>,
    /// Live network facts about actively connected devices, held strictly in memory.
    ///
    /// Keyed by `(user_id, device_id)`. Volatile on purpose: when a device disconnects or the
    /// server restarts, the network address is wiped immediately from RAM and leaves zero residue
    /// on disk.
    live_peers: std::sync::RwLock<std::collections::HashMap<(String, String), PeerNetwork>>,
    /// Outstanding peer-to-peer grants, keyed by `(host_user, host_device, listener_user)`.
    ///
    /// A grant is a bearer token the listener presents to the host's local HTTP server. It is
    /// minted here so that neither client has to trust the other, and it is what stops a shared
    /// LAN — a hotel, a campus, a coffee shop — from being a trusted one.
    p2p_grants: std::sync::RwLock<
        std::collections::HashMap<(String, String, String), (String, std::time::Instant)>,
    >,
}

impl WsHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            tx,
            live_peers: std::sync::RwLock::new(std::collections::HashMap::new()),
            p2p_grants: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Stores a node's local LAN address in memory for peer-to-peer transfers while online.
    pub fn set_lan_address(&self, user_id: &str, device_id: &str, lan_address: &str) {
        if let Ok(mut map) = self.live_peers.write() {
            map.entry((user_id.to_string(), device_id.to_string()))
                .or_default()
                .lan = Some(lan_address.to_string());
        }
    }

    /// Records where a device's connection reached the server from.
    ///
    /// Separate from the LAN address because the server learns them at different moments and from
    /// different sources: the LAN address is claimed by the client, this is observed.
    pub fn set_egress_address(&self, user_id: &str, device_id: &str, egress: &str) {
        if let Ok(mut map) = self.live_peers.write() {
            map.entry((user_id.to_string(), device_id.to_string()))
                .or_default()
                .egress = Some(egress.to_string());
        }
    }

    /// Looks up a node's active local LAN address from memory.
    pub fn get_lan_address(&self, user_id: &str, device_id: &str) -> Option<String> {
        self.live_peers
            .read()
            .ok()
            .and_then(|map| map.get(&(user_id.to_string(), device_id.to_string())).cloned())
            .and_then(|peer| peer.lan)
    }

    /// Whether two connected devices look like they are on the same local network.
    ///
    /// The test is that both connections reached the server from the same public address, which is
    /// what being behind one NAT looks like from here. It is a heuristic in one direction only:
    ///
    /// - **False positives exist.** Carrier-grade NAT puts thousands of unrelated mobile
    ///   subscribers behind one IPv4 address, as do large campus and office networks. This is why
    ///   the LAN address it unlocks is never the *authorisation* for anything — the grant token is
    ///   — and why the worst case is a connection that is refused rather than a disclosure.
    /// - **False negatives are harmless.** Two devices that really are on one LAN but egress
    ///   differently simply fall through to the relay, which is the tier below.
    ///
    /// IPv6 is compared on the `/64` prefix rather than the whole address. There is no NAT to hide
    /// behind, so every device on a LAN has a distinct global address but shares the prefix its
    /// router advertises — which makes it a stronger signal here than the IPv4 case, not a weaker
    /// one.
    pub fn same_network(
        &self,
        a_user: &str,
        a_device: &str,
        b_user: &str,
        b_device: &str,
    ) -> bool {
        let Ok(map) = self.live_peers.read() else {
            return false;
        };
        let egress_of = |user: &str, device: &str| -> Option<String> {
            map.get(&(user.to_string(), device.to_string()))
                .and_then(|peer| peer.egress.clone())
        };
        let (Some(a), Some(b)) = (egress_of(a_user, a_device), egress_of(b_user, b_device)) else {
            return false;
        };
        same_egress(&a, &b)
    }

    /// Whether *any* device belonging to `viewer_user` shares a network with `host_device`.
    ///
    /// The pairwise question the callers actually have. A listener does not tell the server which
    /// of their devices is asking — and should not have to, since the answer is the same either
    /// way: if any of their connected devices could reach the host directly, the direct tier is
    /// worth offering.
    pub fn shares_network_with_user(
        &self,
        host_user: &str,
        host_device: &str,
        viewer_user: &str,
    ) -> bool {
        let Ok(map) = self.live_peers.read() else {
            return false;
        };
        let Some(host_egress) = map
            .get(&(host_user.to_string(), host_device.to_string()))
            .and_then(|peer| peer.egress.clone())
        else {
            return false;
        };
        map.iter().any(|((user, device), peer)| {
            user == viewer_user
                && !(user == host_user && device == host_device)
                && peer
                    .egress
                    .as_deref()
                    .is_some_and(|egress| same_egress(&host_egress, egress))
        })
    }

    /// A bearer token letting `listener_user` fetch audio from `host_device`'s local server.
    ///
    /// Reused while it has real life left in it, so a listening session does not mint a token per
    /// track change, and pushed to the host the moment it is created — the host cannot accept a
    /// token it has never been told about.
    pub fn grant_p2p_token(
        &self,
        host_user: &str,
        host_device: &str,
        listener_user: &str,
    ) -> Option<String> {
        let key = (
            host_user.to_string(),
            host_device.to_string(),
            listener_user.to_string(),
        );
        let now = std::time::Instant::now();

        if let Ok(grants) = self.p2p_grants.read() {
            if let Some((token, expires)) = grants.get(&key) {
                if expires.saturating_duration_since(now) > P2P_GRANT_MIN_REMAINING {
                    return Some(token.clone());
                }
            }
        }

        // 256 bits from the OS CSPRNG, minted by the same helper as a device token.
        let token = crate::credentials::mint_token().secret;
        {
            let mut grants = self.p2p_grants.write().ok()?;
            grants.retain(|_, (_, expires)| *expires > now);
            grants.insert(key, (token.clone(), now + P2P_GRANT_TTL));
        }

        // The host is told before the listener is, because a grant the host has not seen is one it
        // is obliged to refuse.
        self.notify_device(
            host_user,
            host_device,
            "P2P_GRANT",
            serde_json::json!({
                "token": token,
                "forUser": listener_user,
                "ttlSeconds": P2P_GRANT_TTL.as_secs(),
            }),
        );
        Some(token)
    }

    /// Purges a node's local network facts and its grants when it disconnects.
    pub fn clear_lan_address(&self, user_id: &str, device_id: &str) {
        if let Ok(mut map) = self.live_peers.write() {
            map.remove(&(user_id.to_string(), device_id.to_string()));
        }
        if let Ok(mut grants) = self.p2p_grants.write() {
            grants.retain(|(host_user, host_device, _), _| {
                !(host_user == user_id && host_device == device_id)
            });
        }
    }

    /// Sends to every device on every account. Used for things that are not account-specific.
    pub fn broadcast(&self, msg_type: &str, payload: serde_json::Value) {
        let _ = self.tx.send(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: None,
            target_device: None,
        });
    }

    /// Sends to one account's devices.
    pub fn notify_user(&self, user_id: &str, msg_type: &str, payload: serde_json::Value) {
        let _ = self.tx.send(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: Some(user_id.to_string()),
            target_device: None,
        });
    }

    /// Sends the same message to several accounts.
    ///
    /// One send per recipient rather than a broadcast with a recipient list, because the socket
    /// side filters on `user_id` alone — a list in the payload would arrive at everyone and rely on
    /// each client to ignore what is not theirs, which is not a boundary.
    pub fn notify_users(&self, user_ids: &[String], msg_type: &str, payload: serde_json::Value) {
        for user_id in user_ids {
            self.notify_user(user_id, msg_type, payload.clone());
        }
    }

    /// Sends to exactly one device.
    pub fn notify_device(
        &self,
        user_id: &str,
        device_id: &str,
        msg_type: &str,
        payload: serde_json::Value,
    ) {
        let _ = self.tx.send(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: Some(user_id.to_string()),
            target_device: Some(device_id.to_string()),
        });
    }
}

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
const MAX_LAN_ADDRESS: usize = 64;

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
        let client_type = if d.to_lowercase().contains("android") || d.to_lowercase().contains("wanda") {
            "wanda"
        } else {
            "wander"
        };
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
    ws.on_upgrade(move |socket| handle_socket(socket, state, username, device))
}

/// Forwards hub messages this socket is entitled to see.
///
/// Supports both pre-authenticated connections (via HTTP Authorization header or query parameter)
/// and post-handshake in-band authentication (via an `AUTH` message frame within 5 seconds),
/// preventing bearer tokens from being logged in reverse proxy access logs.
async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    mut username: Option<String>,
    mut device: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.ws_hub.tx.subscribe();

    // If unauthenticated, wait for an in-band AUTH frame within 5 seconds
    if username.is_none() {
        let auth_future = async {
            while let Some(Ok(msg)) = receiver.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        if ws_msg.msg_type.eq_ignore_ascii_case("AUTH") {
                            if let Some(token) = ws_msg.payload.get("token").and_then(|t| t.as_str()) {
                                if let Ok(Some((account, _))) = state.db.account_for_token(token) {
                                    if account.state.is_active() {
                                        let u = account.username.to_string();
                                        let d = ws_msg
                                            .payload
                                            .get("device")
                                            .and_then(|dev| dev.as_str())
                                            .map(str::to_string);
                                        let lan = ws_msg
                                            .payload
                                            .get("lan")
                                            .and_then(|l| l.as_str());

                                        if let (Some(dev_id), Some(lan_addr)) = (&d, lan) {
                                            let lan_addr = lan_addr.trim();
                                            if !lan_addr.is_empty()
                                                && lan_addr.len() <= MAX_LAN_ADDRESS
                                                && is_plausible_host_port(lan_addr)
                                            {
                                                state.ws_hub.set_lan_address(&u, dev_id, lan_addr);
                                            }
                                        }

                                        if let Some(dev_id) = &d {
                                            let petname = crate::passphrase::generate_random_petname();
                                            let client_type = if dev_id.to_lowercase().contains("android")
                                                || dev_id.to_lowercase().contains("wanda")
                                            {
                                                "wanda"
                                            } else {
                                                "wander"
                                            };
                                            let _ = state.db.upsert_node(
                                                dev_id,
                                                &u,
                                                crate::db::NodeName::KeepOr(&petname),
                                                client_type,
                                                None,
                                                None,
                                            );
                                            state.offers.note_archived(&u);
                                        }
                                        return Some((u, d));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            None
        };

        match tokio::time::timeout(std::time::Duration::from_secs(5), auth_future).await {
            Ok(Some((u, d))) => {
                username = Some(u);
                device = d;
                let _ = sender
                    .send(Message::Text(
                        serde_json::json!({
                            "msg_type": "AUTH_SUCCESS",
                            "payload": { "status": "authenticated" }
                        })
                        .to_string()
                        .into(),
                    ))
                    .await;
            }
            _ => {
                return; // Failed to authenticate in-band within timeout
            }
        }
    }

    tokio::select! {
        _ = async {
            while let Ok(msg) = rx.recv().await {
                if !is_for(&msg, username.as_deref(), device.as_deref()) {
                    continue;
                }
                if let Ok(text) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
            }
        } => {},
        _ = async {
            while let Some(Ok(_)) = receiver.next().await {
                // Read and dropped. See above.
            }
        } => {}
    }

    if let (Some(u), Some(d)) = (username.as_deref(), device.as_deref()) {
        state.ws_hub.clear_lan_address(u, d);
    }
}

/// A cheap sanity check on a `host:port` a client says it can be reached at.
///
/// Not a validation of reachability — that is the peer's job, and `PeerReachability` on the client
/// already probes before trusting one. This only rejects the obviously malformed, so that a value
/// handed to another device as a connection target cannot carry a scheme, a path, or a credential.
fn is_plausible_host_port(value: &str) -> bool {
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
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    !host.is_empty()
        && host.bytes().all(|b| {
            b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':' || b == b'_'
        })
}

/// Whether a socket for [`username`]/[`device`] should see [`msg`].
fn is_for(msg: &WsMessage, username: Option<&str>, device: Option<&str>) -> bool {
    if let Some(target_user) = &msg.user_id {
        // An unauthenticated socket only exists during the first-run window, before any account
        // has data worth addressing.
        match username {
            Some(name) if name.eq_ignore_ascii_case(target_user) => {}
            _ => return false,
        }
    }
    match (&msg.target_device, device) {
        (None, _) => true,
        (Some(target), Some(own)) => target == own,
        // Addressed to a specific device by a socket that never said which one it is. Dropping it
        // is the safe answer: a client that wants targeted messages identifies itself.
        (Some(_), None) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(user: Option<&str>, device: Option<&str>) -> WsMessage {
        WsMessage {
            msg_type: "SYNC_OFFER".to_string(),
            payload: json!({}),
            user_id: user.map(str::to_string),
            target_device: device.map(str::to_string),
        }
    }

    #[test]
    fn a_broadcast_reaches_everyone() {
        assert!(is_for(&msg(None, None), Some("alpha"), Some("phone")));
        assert!(is_for(&msg(None, None), None, None));
    }

    #[test]
    fn another_account_never_sees_it() {
        assert!(!is_for(&msg(Some("alpha"), None), Some("beta"), Some("phone")));
        assert!(is_for(&msg(Some("alpha"), None), Some("alpha"), Some("phone")));
    }

    #[test]
    fn a_targeted_message_reaches_only_that_device() {
        let m = msg(Some("alpha"), Some("phone"));
        assert!(is_for(&m, Some("alpha"), Some("phone")));
        assert!(!is_for(&m, Some("alpha"), Some("laptop")));
        assert!(!is_for(&m, Some("alpha"), None));
    }

    /// The same-network test is what decides whether a private address is ever handed to another
    /// account, so both directions of it are pinned here.
    mod same_network {
        use super::*;

        fn hub_with(peers: &[(&str, &str, &str)]) -> WsHub {
            let hub = WsHub::new();
            for (user, device, egress) in peers {
                hub.set_egress_address(user, device, egress);
                hub.set_lan_address(user, device, "192.168.1.50:8702");
            }
            hub
        }

        #[test]
        fn one_public_address_means_one_network() {
            let hub = hub_with(&[
                ("alpha", "phone", "203.0.113.7"),
                ("beta", "laptop", "203.0.113.7"),
            ]);
            assert!(hub.same_network("alpha", "phone", "beta", "laptop"));
            assert!(hub.shares_network_with_user("alpha", "phone", "beta"));
        }

        #[test]
        fn different_public_addresses_mean_different_networks() {
            let hub = hub_with(&[
                ("alpha", "phone", "203.0.113.7"),
                ("beta", "laptop", "198.51.100.4"),
            ]);
            assert!(!hub.same_network("alpha", "phone", "beta", "laptop"));
            assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
        }

        /// A device nobody has heard from has no egress address, and an unknown answer is not a
        /// yes: it falls through to the relay rather than disclosing an address.
        #[test]
        fn an_unseen_device_is_never_on_your_network() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            assert!(!hub.same_network("alpha", "phone", "beta", "laptop"));
            assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
        }

        /// IPv6 has no NAT, so two devices on one LAN never share a full address — they share the
        /// prefix their router advertises. Comparing the whole address would answer "no" for every
        /// real pair.
        #[test]
        fn ipv6_is_compared_on_the_routed_prefix() {
            assert!(same_egress(
                "2001:db8:1:2::1000",
                "2001:db8:1:2::abcd"
            ));
            assert!(!same_egress("2001:db8:1:2::1", "2001:db8:9:9::1"));
        }

        /// Mixing families, or anything unparseable, is refused rather than guessed at.
        #[test]
        fn an_unparseable_address_is_refused() {
            assert!(!same_egress("203.0.113.7", "::ffff:203.0.113.7"));
            assert!(!same_egress("not-an-address", "not-an-address"));
            assert!(!same_egress("", ""));
        }

        /// The token is the gate, so it has to be stable enough to use across a track change and
        /// scoped to the one listener it was minted for.
        #[test]
        fn a_grant_is_reused_for_the_same_pair_and_distinct_per_listener() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            let first = hub.grant_p2p_token("alpha", "phone", "beta").unwrap();
            let again = hub.grant_p2p_token("alpha", "phone", "beta").unwrap();
            let other = hub.grant_p2p_token("alpha", "phone", "gamma").unwrap();
            assert_eq!(first, again);
            assert_ne!(first, other);
        }

        /// A device sharing its own account's network is still a match — the same-account case is
        /// the original point of the LAN transfer — but callers that mean "somebody else" filter
        /// the holder out themselves.
        #[test]
        fn your_own_second_device_is_on_your_network() {
            let hub = hub_with(&[
                ("alpha", "phone", "203.0.113.7"),
                ("alpha", "laptop", "203.0.113.7"),
            ]);
            assert!(hub.shares_network_with_user("alpha", "phone", "alpha"));
        }

        /// The device being asked about is never its own peer, or every host would look reachable
        /// from itself.
        #[test]
        fn a_device_is_not_its_own_peer() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            assert!(!hub.shares_network_with_user("alpha", "phone", "alpha"));
        }

        /// A disconnect takes the address and every grant issued for that device with it.
        #[test]
        fn disconnecting_clears_the_address_and_the_grants() {
            let hub = hub_with(&[("alpha", "phone", "203.0.113.7")]);
            let token = hub.grant_p2p_token("alpha", "phone", "beta").unwrap();
            hub.clear_lan_address("alpha", "phone");
            assert!(hub.get_lan_address("alpha", "phone").is_none());
            assert!(!hub.shares_network_with_user("alpha", "phone", "beta"));
            assert_ne!(
                token,
                hub.grant_p2p_token("alpha", "phone", "beta").unwrap(),
                "a grant must not survive the socket it was minted for"
            );
        }
    }

    /// The regression that made the volatile design worth fixing rather than reverting: an
    /// address set once at startup and cleared on a dropped socket never came back, because
    /// neither client re-registers. The handshake carrying it is what closes that loop, so these
    /// pin the shape of what the handshake is allowed to hand over.
    #[test]
    fn a_plausible_host_port_is_accepted() {
        for good in [
            "192.168.1.50:8702",
            "10.0.0.5:8702",
            "phone.local:8702",
            "[fe80::1]:8702",
        ] {
            assert!(is_plausible_host_port(good), "should accept {good}");
        }
    }

    /// The value is handed to another device as somewhere to connect to, so anything carrying a
    /// scheme, a path or a credential has to be refused rather than forwarded.
    #[test]
    fn a_malformed_or_dangerous_address_is_refused() {
        for bad in [
            "",
            "192.168.1.50",              // no port
            "192.168.1.50:",             // empty port
            "192.168.1.50:0",            // port zero
            "192.168.1.50:notaport",
            "192.168.1.50:99999",        // will not fit a u16
            ":8702",                     // no host
            "http://192.168.1.50:8702",  // a scheme
            "192.168.1.50:8702/steal",   // a path
            "user:pass@10.0.0.1:8702",   // a credential
            // A zone id names an interface on the *sender's* machine, so it means nothing to the
            // peer being told to connect there.
            "[fe80::1%eth0]:8702",
        ] {
            assert!(!is_plausible_host_port(bad), "should refuse {bad:?}");
        }
    }

    #[test]
    fn lan_addresses_are_stored_and_retrieved_in_memory() {
        let hub = WsHub::new();
        assert_eq!(hub.get_lan_address("alpha", "phone"), None);

        hub.set_lan_address("alpha", "phone", "192.168.1.50:8702");
        assert_eq!(
            hub.get_lan_address("alpha", "phone"),
            Some("192.168.1.50:8702".to_string())
        );

        hub.clear_lan_address("alpha", "phone");
        assert_eq!(hub.get_lan_address("alpha", "phone"), None);
    }

    #[test]
    fn lan_addresses_are_scoped_by_user_and_device() {
        let hub = WsHub::new();
        hub.set_lan_address("alpha", "phone", "192.168.1.50:8702");
        hub.set_lan_address("beta", "phone", "10.0.0.5:8702");

        assert_eq!(
            hub.get_lan_address("alpha", "phone"),
            Some("192.168.1.50:8702".to_string())
        );
        assert_eq!(
            hub.get_lan_address("beta", "phone"),
            Some("10.0.0.5:8702".to_string())
        );
        assert_eq!(hub.get_lan_address("alpha", "laptop"), None);
    }
}
