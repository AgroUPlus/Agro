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

pub struct WsHub {
    pub tx: broadcast::Sender<WsMessage>,
    /// Live LAN addresses of actively connected devices, held strictly in memory.
    ///
    /// Keyed by `(user_id, device_id)`. Volatile on purpose: when a device disconnects or the
    /// server restarts, the network address is wiped immediately from RAM and leaves zero residue
    /// on disk.
    live_lan_addresses: std::sync::RwLock<std::collections::HashMap<(String, String), String>>,
}

impl WsHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self {
            tx,
            live_lan_addresses: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Stores a node's local LAN address in memory for peer-to-peer transfers while online.
    pub fn set_lan_address(&self, user_id: &str, device_id: &str, lan_address: &str) {
        if let Ok(mut map) = self.live_lan_addresses.write() {
            map.insert(
                (user_id.to_string(), device_id.to_string()),
                lan_address.to_string(),
            );
        }
    }

    /// Looks up a node's active local LAN address from memory.
    pub fn get_lan_address(&self, user_id: &str, device_id: &str) -> Option<String> {
        self.live_lan_addresses
            .read()
            .ok()
            .and_then(|map| map.get(&(user_id.to_string(), device_id.to_string())).cloned())
    }

    /// Purges a node's local LAN address when it disconnects.
    pub fn clear_lan_address(&self, user_id: &str, device_id: &str) {
        if let Ok(mut map) = self.live_lan_addresses.write() {
            map.remove(&(user_id.to_string(), device_id.to_string()));
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
    Query(query): Query<SocketQuery>,
) -> Response {
    let username = user.map(|u| u.username().to_string());
    let device = query.device;

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
