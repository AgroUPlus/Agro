use super::*;
use serde_json::json;

pub(super) fn msg(user: Option<&str>, device: Option<&str>) -> WsMessage {
    WsMessage {
        msg_type: "SYNC_OFFER".to_string(),
        payload: json!({}),
        user_id: user.map(str::to_string),
        target_device: device.map(str::to_string),
        seq: None,
    }
}

mod addresses;
mod delivery;
mod replay;
