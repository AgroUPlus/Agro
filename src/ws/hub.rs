//! Publishing to sockets, with a numbered replay window.

use super::*;

impl WsHub {
    pub fn new() -> Self {
        Self {
            channels: crate::ws_channels::Channels::new(),
            live_peers: std::sync::RwLock::new(std::collections::HashMap::new()),
            p2p_grants: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Every ordered message leaves through here, so the sequence has no gaps and the buffers can
    /// never disagree with what was actually sent.
    pub(super) fn publish(&self, msg: WsMessage) {
        self.channels.publish(msg);
    }

    /// Messages after [`after_seq`] that this socket should have seen, oldest first.
    ///
    /// `None` means the buffers cannot answer — the client has been gone longer than the replay
    /// window or a burst pushed its position out — and the caller must tell it to resynchronise
    /// rather than hand it a prefix with a hole at the front, which would be worse than admitting
    /// the gap.
    ///
    /// `pub(crate)` for the boundary suites as well as the resume path: what a socket is *sent* is
    /// half of what one account can learn about another, and a frame carrying somebody else's
    /// sealed copy would not be visible in any query-level assertion.
    pub(crate) fn replay_after(
        &self,
        after_seq: u64,
        username: Option<&str>,
        device: Option<&str>,
    ) -> Option<Vec<WsMessage>> {
        self.channels
            .replay_after(after_seq, username, |m| is_for(m, username, device))
    }

    /// Sends to every device on every account. Used for things that are not account-specific.
    pub fn broadcast(&self, msg_type: &str, payload: serde_json::Value) {
        self.publish(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: None,
            target_device: None,
            seq: None,
        });
    }

    /// Sends to one account's devices.
    pub fn notify_user(&self, user_id: &str, msg_type: &str, payload: serde_json::Value) {
        self.publish(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: Some(user_id.to_string()),
            target_device: None,
            seq: None,
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
        self.publish(WsMessage {
            msg_type: msg_type.to_string(),
            payload,
            user_id: Some(user_id.to_string()),
            target_device: Some(device_id.to_string()),
            seq: None,
        });
    }
}
