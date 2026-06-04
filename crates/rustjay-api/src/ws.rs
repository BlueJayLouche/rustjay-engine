//! WebSocket endpoint — full state on connect, JSON Patch (RFC 6902) deltas.
//!
//! Each connection caches its last-sent state and only sends diffs.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};

use crate::SharedState;

/// Target delta rate — roughly 30 fps.
const DELTA_INTERVAL_MS: u64 = 33;

/// Envelope for client → server messages.
#[derive(Deserialize)]
struct WsCommand {
    id: Option<String>,
    #[serde(flatten)]
    command: rustjay_control::WebCommand,
}

/// Envelope for server → client command responses.
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
    let initial_json = {
        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
        match &guard.engine_state {
            Some(engine_arc) => match engine_arc.lock() {
                Ok(engine) => match serde_json::to_value(crate::build_snapshot(&engine)) {
                    Ok(v) => v,
                    Err(e) => {
                        log::warn!("WS initial snapshot serialization failed: {}", e);
                        serde_json::Value::Null
                    }
                },
                Err(_) => serde_json::Value::Null,
            },
            None => serde_json::Value::Null,
        }
    };

    if initial_json.is_null() {
        let _ = sink.send(Message::Close(None)).await;
        return;
    }

    if sink
        .send(Message::Text(initial_json.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // Channel for forwarding incoming commands to the delta loop task.
    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

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
        let mut last_json = initial_json;

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let current_json = {
                        let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                        match &guard.engine_state {
                            Some(engine_arc) => match engine_arc.lock() {
                                Ok(engine) => match serde_json::to_value(crate::build_snapshot(&engine)) {
                                    Ok(v) => v,
                                    Err(e) => {
                                        log::warn!("WS snapshot serialization failed: {}", e);
                                        continue;
                                    }
                                },
                                Err(_) => continue,
                            },
                            None => continue,
                        }
                    };

                    let patch = json_patch::diff(&last_json, &current_json);
                    if !patch.0.is_empty() {
                        let patch_str = match serde_json::to_string(&patch) {
                            Ok(s) => s,
                            Err(e) => {
                                log::warn!("WS patch serialization failed: {}", e);
                                continue;
                            }
                        };
                        if sink.send(Message::Text(patch_str.into())).await.is_err() {
                            break;
                        }
                        last_json = current_json;
                    }
                }
                Some(text) = cmd_rx.recv() => {
                    let response = match serde_json::from_str::<WsCommand>(&text) {
                        Ok(ws_cmd) => {
                            let guard = state.lock().unwrap_or_else(|e| e.into_inner());
                            let result = guard.command_tx.try_send(ws_cmd.command);
                            let payload = match result {
                                Ok(_) => WsResultPayload::Ok { status: "ok" },
                                Err(e) => WsResultPayload::Err {
                                    error: "internal",
                                    message: format!("Failed to forward command: {}", e),
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

#[cfg(test)]
mod tests {

    #[test]
    fn test_json_patch_diff_detects_changes() {
        let old = serde_json::json!({"crossfader": 0.0, "channels": []});
        let new = serde_json::json!({"crossfader": 0.75, "channels": []});
        let patch = json_patch::diff(&old, &new);
        assert!(!patch.0.is_empty());
        let patched = {
            let mut v = old.clone();
            json_patch::patch(&mut v, &patch).unwrap();
            v
        };
        assert_eq!(patched, new);
    }

    #[test]
    fn test_json_patch_no_change_produces_empty_patch() {
        let state = serde_json::json!({"crossfader": 0.5, "channels": [{"name": "A"}]});
        let patch = json_patch::diff(&state, &state);
        assert!(patch.0.is_empty());
    }
}
