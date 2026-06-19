use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::broadcast;

use crate::state::{AppState, WsEvent};

/// WebSocket upgrade handler — each client gets a dedicated task.
pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let rx = state.event_tx.subscribe();
    ws.on_upgrade(move |socket| handle_socket(socket, rx))
}

async fn handle_socket(socket: WebSocket, mut rx: broadcast::Receiver<WsEvent>) {
    let (mut sender, mut receiver) = socket.split();

    // Send initial daemon status
    let status_msg = WsEvent::DaemonStatus(crate::state::DaemonStatusPayload {
        scanning: false,
        deduping: false,
        uptime_secs: 0,
    });
    if let Ok(json) = serde_json::to_string(&status_msg) {
        let _ = sender.send(Message::Text(json)).await;
    }

    // Forward broadcast events to the WebSocket client
    let forward_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if let Ok(json) = serde_json::to_string(&event) {
                        if sender.send(Message::Text(json)).await.is_err() {
                            break; // Client disconnected
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    });

    // Keep reading client messages (ping/pong or close)
    while let Some(Ok(msg)) = receiver.next().await {
        if matches!(msg, Message::Close(_)) {
            break;
        }
    }

    forward_task.abort();
}
