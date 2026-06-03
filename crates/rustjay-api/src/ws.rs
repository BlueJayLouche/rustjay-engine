//! WebSocket handler — full state on connect, JSON Patch deltas.

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use futures_util::SinkExt;
use futures_util::stream::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::SharedState;

/// Target delta rate — roughly 30 fps.
const DELTA_INTERVAL_MS: u64 = 33;

/// Client → server command envelope with optional correlation id.
#[derive(Deserialize)]
struct WsCommand {
    id: Option<String>,
    #[serde(flatten)]
    command: rustjay_control::WebCommand,
}

/// Server → client command response envelope.
#[derive(Serialize)]
struct WsCommandResponse {
    id: Option<String>,
    result: WsResultPayload,
}

#[derive(Serialize)]
#[serde(untagged)]
enum WsResultPayload {
    Ok { status: &'static str },
    Err { error: &'static str, message: String },
}

/// `GET /api/ws` — upgrade to WebSocket.
pub async fn ws_upgrade(
    State(state): State<SharedState>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: SharedState) {
    let (mut sink, mut stream) = socket.split();

    // Send full state snapshot on connect.
    let initial_json = match state.engine_snapshot.read().ok().and_then(|g| g.clone()) {
        Some(s) => serde_json::to_value(&s).unwrap_or_default(),
        None => {
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
    };
    if sink.send(Message::Text(initial_json.to_string().into())).await.is_err() {
        return;
    }

    let mut last_json = initial_json;

    // Channel for forwarding incoming commands to the delta loop task.
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<String>();

    // ── Read task: client → server commands ──────────────────────
    let read_handle = tokio::spawn(async move {
        while let Some(Ok(msg)) = stream.next().await {
            match msg {
                Message::Text(text) => {
                    let _ = cmd_tx.send(text.to_string());
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // ── Write task: deltas + command responses ───────────────────
    let write_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(
            std::time::Duration::from_millis(DELTA_INTERVAL_MS),
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Some(current) = state.engine_snapshot.read().ok().and_then(|g| g.clone()) {
                        let current_json = serde_json::to_value(&current).unwrap_or_default();
                        let patch = json_patch::diff(&last_json, &current_json);
                        if !patch.0.is_empty() {
                            let patch_str = serde_json::to_string(&patch).unwrap_or_default();
                            if sink.send(Message::Text(patch_str.into())).await.is_err() {
                                break;
                            }
                            last_json = current_json;
                        }
                    }
                }
                Some(text) = cmd_rx.recv() => {
                    let response = match serde_json::from_str::<WsCommand>(&text) {
                        Ok(ws_cmd) => {
                            let result = state.send_command(ws_cmd.command);
                            let payload = match result {
                                Ok(()) => WsResultPayload::Ok { status: "ok" },
                                Err(msg) => WsResultPayload::Err {
                                    error: "internal",
                                    message: msg.to_string(),
                                },
                            };
                            WsCommandResponse { id: ws_cmd.id, result: payload }
                        }
                        Err(e) => WsCommandResponse {
                            id: None,
                            result: WsResultPayload::Err {
                                error: "invalid_input",
                                message: format!("Invalid command JSON: {e}"),
                            },
                        },
                    };
                    let resp_str = serde_json::to_string(&response).unwrap_or_default();
                    if sink.send(Message::Text(resp_str.into())).await.is_err() {
                        break;
                    }
                }
                else => break,
            }
        }
    });

    // Wait for either task to finish, then abort the other.
    tokio::select! {
        _ = read_handle => {}
        _ = write_handle => {}
    }
}
