use axum::{
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    extract::State,
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::broadcast;
use serde::{Deserialize, Serialize};
use crate::AppState;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct WsMessage {
    pub msg_type: String,
    pub payload: serde_json::Value,
}

pub struct WsHub {
    pub tx: broadcast::Sender<WsMessage>,
}

impl WsHub {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(100);
        Self { tx }
    }
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state.ws_hub))
}

async fn handle_socket(socket: WebSocket, hub: Arc<WsHub>) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = hub.tx.subscribe();

    tokio::select! {
        _ = async {
            while let Ok(msg) = rx.recv().await {
                if let Ok(text) = serde_json::to_string(&msg) {
                    if sender.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
            }
        } => {},
        _ = async {
            while let Some(Ok(msg)) = receiver.next().await {
                if let Message::Text(text) = msg {
                    if let Ok(ws_msg) = serde_json::from_str::<WsMessage>(&text) {
                        let _ = hub.tx.send(ws_msg);
                    }
                }
            }
        } => {}
    }
}
