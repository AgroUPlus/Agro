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
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    user: Option<axum::Extension<AuthedUser>>,
    Query(query): Query<SocketQuery>,
) -> Response {
    let username = user.map(|u| u.username().to_string());
    let device = query.device;
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
    ws.on_upgrade(move |socket| handle_socket(socket, state.ws_hub, username, device))
}

/// Forwards hub messages this socket is entitled to see.
///
/// Two changes from the original, both of which were holes:
///
/// - **Filtering.** Every socket used to receive every message for every account, so one device's
///   handoff was delivered to a stranger's client.
/// - **No echo.** Inbound frames used to be pushed straight back into the hub, which let any
///   authenticated client forge a `HANDOFF` — or now a `SYNC_OFFER` — to everyone else. Inbound
///   frames are read and discarded; they exist to keep the connection alive, and state changes go
///   through the API where they can be authorised.
async fn handle_socket(
    socket: WebSocket,
    hub: Arc<WsHub>,
    username: Option<String>,
    device: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = hub.tx.subscribe();

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
        hub.clear_lan_address(u, d);
    }
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
