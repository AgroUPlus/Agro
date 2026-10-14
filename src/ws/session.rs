//! One connected socket: frames in, frames out, resume.

use super::*;
use crate::AppState;
use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;

pub(super) const SOCKET_READ_BUFFER: usize = 8 * 1024;

pub(super) const MAX_INBOUND_FRAME: usize = 64 * 1024;

/// Forwards hub messages this socket is entitled to see.
///
/// Supports both pre-authenticated connections (via HTTP Authorization header or query parameter)
/// and post-handshake in-band authentication (via an `AUTH` message frame within 5 seconds),
/// preventing bearer tokens from being logged in reverse proxy access logs.
pub(super) async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    mut username: Option<String>,
    mut device: Option<String>,
) {
    let (mut sender, mut receiver) = socket.split();
    // Subscribed before authenticating, so nothing addressed to everyone is missed meanwhile. The
    // account's own channel can only be joined once the account is known.
    let mut global_rx = state.ws_hub.channels.subscribe_global();

    // If unauthenticated, wait for an in-band AUTH frame within 5 seconds
    if username.is_none() {
        let auth_future = async {
            while let Some(Ok(msg)) = receiver.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        if ws_msg.msg_type.eq_ignore_ascii_case("AUTH") {
                            if let Some(token) =
                                ws_msg.payload.get("token").and_then(|t| t.as_str())
                            {
                                if let Ok(Some((account, _))) = state.db.account_for_token(token) {
                                    if account.state.is_active() {
                                        let u = account.username.to_string();
                                        let d = ws_msg
                                            .payload
                                            .get("device")
                                            .and_then(|dev| dev.as_str())
                                            .map(str::to_string);
                                        let lan =
                                            ws_msg.payload.get("lan").and_then(|l| l.as_str());

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
                                            let petname =
                                                crate::passphrase::generate_random_petname();
                                            let client_type =
                                                crate::db::inferred_client_type(dev_id);
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

    // Both loops below need to write to the socket — the live stream, and a RESUME being answered
    // — so the sink is owned by one task and fed through a channel rather than shared behind a
    // lock. A bounded channel also means a socket that stops reading applies backpressure here
    // instead of growing a queue.
    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<String>(64);
    let writer = tokio::spawn(async move {
        while let Some(text) = out_rx.recv().await {
            if sender.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    let mut user_rx = username
        .as_deref()
        .map(|name| state.ws_hub.channels.subscribe_user(name));

    let hub = state.ws_hub.clone();
    let (user_for_recv, device_for_recv) = (username.clone(), device.clone());
    let resume_tx = out_tx.clone();

    tokio::select! {
        _ = async {
            loop {
                let received = match user_rx.as_mut() {
                    Some(user_rx) => tokio::select! {
                        r = global_rx.recv() => r,
                        r = user_rx.recv() => r,
                    },
                    None => global_rx.recv().await,
                };
                let text = match received {
                    Ok(msg) => {
                        if !is_for(&msg, username.as_deref(), device.as_deref()) {
                            continue;
                        }
                        let Ok(text) = serde_json::to_string(&msg) else { continue };
                        text
                    }
                    // The socket fell behind and the channel dropped what it had not taken. This
                    // used to end the loop, and with it the connection, so a burst disconnected
                    // exactly the slow clients and their reconnects made the burst worse. The
                    // client already knows how to recover from a gap: refetch and carry on.
                    Err(broadcast::error::RecvError::Lagged(_)) => resumed_frame(0, 0, true),
                    Err(broadcast::error::RecvError::Closed) => break,
                };
                if out_tx.send(text).await.is_err() {
                    break;
                }
            }
        } => {},
        _ = async {
            while let Some(Ok(msg)) = receiver.next().await {
                let Message::Text(text) = msg else { continue };
                let Ok(frame) = serde_json::from_str::<WsMessage>(&text) else { continue };
                if !frame.msg_type.eq_ignore_ascii_case("RESUME") {
                    // Every other inbound frame is still read and dropped. See above.
                    continue;
                }

                // A client that has never seen a message sends 0 and is caught up by definition.
                let after = frame.payload.get("last_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                let missed = hub.replay_after(
                    after,
                    user_for_recv.as_deref(),
                    device_for_recv.as_deref(),
                );

                // The answer goes first, so the client knows whether what follows is the rest of
                // its stream or the start of a new one before it reads any of it.
                let answer = resumed_frame(
                    after,
                    missed.as_ref().map(Vec::len).unwrap_or(0),
                    missed.is_none(),
                );
                if resume_tx.send(answer).await.is_err() {
                    break;
                }

                for msg in missed.unwrap_or_default() {
                    let Ok(text) = serde_json::to_string(&msg) else { continue };
                    if resume_tx.send(text).await.is_err() {
                        return;
                    }
                }
            }
        } => {}
    }

    writer.abort();

    if let (Some(u), Some(d)) = (username.as_deref(), device.as_deref()) {
        state.ws_hub.clear_lan_address(u, d);
    }
}

/// Tells a socket where its stream stands after a resume, or after it fell behind.
///
/// `resync_required` means the client must refetch state rather than trust its own: the gap is
/// longer than the server can account for.
pub(super) fn resumed_frame(from: u64, replayed: usize, resync_required: bool) -> String {
    serde_json::json!({
        "msg_type": "RESUMED",
        "payload": {
            "from": from,
            "replayed": replayed,
            "resync_required": resync_required,
        }
    })
    .to_string()
}

/// Whether a socket for [`username`]/[`device`] should see [`msg`].
pub(super) fn is_for(msg: &WsMessage, username: Option<&str>, device: Option<&str>) -> bool {
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
