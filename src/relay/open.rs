//! Opening a relay session.

use crate::auth::AuthedUser;
use crate::db_social::FriendState;
use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenRelayRequest {
    pub content_hash: String,
    pub from_device: String,
    pub to_device: String,
    /// The requesting device's X25519 identity key, so the host can seal a room key to it.
    ///
    /// Passed straight through to the host and never used by the server, which has no business
    /// holding either half of this exchange. Absent means the listener cannot do E2EE, and the
    /// host will relay in the clear as before.
    #[serde(default)]
    pub listener_public_key: Option<String>,
    /// Set to fan one upload out to a jam instead of to a single device.
    ///
    /// `to_device` is ignored when this is present: the audience is whoever is in the jam at the
    /// moment they attach, which is not knowable when the session is opened.
    #[serde(default)]
    pub jam_id: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OpenRelayResponse {
    pub session_id: String,
}

/// Initiates an ephemeral relay session.
#[utoipa::path(
    post,
    path = "/api/v1/relay/open",
    tag = "relay",
    security(("bearer_token" = [])),
    request_body = OpenRelayRequest,
    responses(
        (status = 200, description = "Relay session opened", body = OpenRelayResponse),
        (status = 403, description = "Device or jam authorization failed"),
    ),
)]
pub async fn open_relay(
    State(state): State<AppState>,
    user: axum::Extension<AuthedUser>,
    Json(body): Json<OpenRelayRequest>,
) -> Response {
    let user_id = user.username();

    // A fanout is opened by the host, for its own jam, and is authorised entirely differently:
    // there is no single receiving device to check, and the sender is by definition the caller.
    // Listeners are authorised when they attach, against the jam's membership at that moment.
    if let Some(jam_id) = body
        .jam_id
        .as_deref()
        .map(str::trim)
        .filter(|j| !j.is_empty())
    {
        if !state
            .db
            .device_belongs_to(user_id, body.from_device.trim())
            .unwrap_or(false)
        {
            return (
                StatusCode::FORBIDDEN,
                "source device does not belong to this account",
            )
                .into_response();
        }
        if !state.db.is_jam_member(jam_id, user_id).unwrap_or(false) {
            return (StatusCode::FORBIDDEN, "not a member of this jam").into_response();
        }

        let (session_id, _) = state.relay_hub.create_jam_session(
            jam_id,
            user_id,
            body.from_device.trim(),
            body.content_hash.trim(),
        );
        return Json(OpenRelayResponse { session_id }).into_response();
    }

    // 1. The receiver must be a device belonging to the caller.
    if !state
        .db
        .device_belongs_to(user_id, body.to_device.trim())
        .unwrap_or(false)
    {
        return (
            StatusCode::FORBIDDEN,
            "target device does not belong to this account",
        )
            .into_response();
    }

    // 2. The sender device can belong to caller's account, OR to a friend, OR to a member of the same Jam.
    let sender_user = match state.db.owner_of_device(body.from_device.trim()) {
        Ok(Some(owner)) => owner,
        _ => {
            return (StatusCode::FORBIDDEN, "unknown source device").into_response();
        }
    };

    // `friend_state` answers `Some(..)` for a *pending* request and for a *block* as well as for
    // an accepted friendship, so testing it with `is_some()` would let a stranger who merely sent a
    // request — or someone this account has blocked — name this account's device as the sender, and
    // the `RELAY_REQUEST` below would make that device upload a hash of the caller's choosing.
    // `are_friends` is the accepted-only test, and a block outranks a shared jam.
    let allowed = if sender_user.eq_ignore_ascii_case(user_id) {
        true
    } else if matches!(
        state.db.friend_state(user_id, &sender_user).unwrap_or(None),
        Some(FriendState::Blocked)
    ) {
        false
    } else {
        let is_friend = state.db.are_friends(user_id, &sender_user).unwrap_or(false);
        let is_in_same_jam = match (
            state.db.jam_for_member(user_id),
            state.db.jam_for_member(&sender_user),
        ) {
            (Ok(Some(j1)), Ok(Some(j2))) => j1.id == j2.id,
            _ => false,
        };
        is_friend || is_in_same_jam
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            "source device is not authorized for relay with this account",
        )
            .into_response();
    }

    let (session_id, _) = state.relay_hub.create_session(
        user_id,
        &body.to_device,
        &sender_user,
        &body.from_device,
        &body.content_hash,
    );

    // Notify the holding device over WebSocket that a relay stream is requested
    state.ws_hub.notify_device(
        &sender_user,
        &body.from_device,
        "RELAY_REQUEST",
        serde_json::json!({
            "sessionId": session_id,
            "contentHash": body.content_hash,
            "toDevice": body.to_device,
            "listenerPublicKey": body.listener_public_key,
        }),
    );

    Json(OpenRelayResponse { session_id }).into_response()
}
