//! Generic app routes — app-published state and hierarchical param I/O.
//!
//! These are **app-agnostic**: the engine knows nothing about any concrete app
//! schema. An app (e.g. `examples/vjarda`) publishes an opaque JSON snapshot into
//! [`EngineState::app_state`], which `GET /api/app/state` serves verbatim.
//! Param writes resolve hierarchical paths through
//! [`EngineState::param_resolver`] (when the app installs one) and call
//! `set_param_base` via the same command channel the web UI / MIDI / OSC use;
//! reads go to `get_param` so modulation is visible.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::SharedState;

// ── Helpers ──────────────────────────────────────────────────────────

fn command_ok() -> impl IntoResponse {
    Json(crate::routes::system::CommandOk { status: "ok" })
}

fn command_err(msg: impl Into<String>) -> impl IntoResponse {
    let body = serde_json::json!({"error": "internal", "message": msg.into()});
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body))
}

fn bad_request(msg: impl Into<String>) -> impl IntoResponse {
    let body = serde_json::json!({"error": "bad_request", "message": msg.into()});
    (StatusCode::BAD_REQUEST, Json(body))
}

fn send_command(state: &SharedState, cmd: rustjay_control::WebCommand) -> Result<(), &'static str> {
    let guard = state.lock().map_err(|_| "Server state lock poisoned")?;
    guard
        .command_tx
        .try_send(cmd)
        .map_err(|_| "Engine command channel full")
}

/// Lock engine state out of the shared web-server state.
fn try_engine<F, R>(state: &SharedState, f: F) -> Result<R, (StatusCode, &'static str)>
where
    F: FnOnce(&rustjay_core::EngineState) -> R,
{
    let engine_arc = match state.lock() {
        Ok(guard) => match guard.engine_state.as_ref() {
            Some(engine) => engine.clone(),
            None => {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Engine not yet initialized",
                ))
            }
        },
        Err(_) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Server state lock poisoned",
            ))
        }
    };
    // Bind the result so the MutexGuard drops before `engine_arc`.
    let result = match engine_arc.lock() {
        Ok(e) => Ok(f(&e)),
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Engine state lock poisoned",
        )),
    };
    result
}

/// Resolve a hierarchical path to a canonical flat id using the engine's
/// param resolver, when present. Returns the original path if no resolver.
fn resolve_path(engine: &rustjay_core::EngineState, path: &str) -> Option<String> {
    match engine.param_resolver.as_ref() {
        Some(resolver) => resolver.resolve(path),
        None => Some(path.to_string()),
    }
}

/// Clamp a value to a parameter descriptor's range, if found.
fn clamp_to_descriptor(engine: &rustjay_core::EngineState, id: &str, value: f32) -> f32 {
    engine
        .param_descriptors
        .iter()
        .find(|d| d.id == id)
        .map(|d| value.clamp(d.min, d.max))
        .unwrap_or(value)
}

// ── App state ────────────────────────────────────────────────────────

/// `GET /api/app/state` — the opaque JSON snapshot the active app published,
/// or `null` if none.
#[utoipa::path(
    get,
    path = "/api/app/state",
    responses(
        (status = 200, description = "App-published JSON snapshot (or null)"),
        (status = 503, description = "Engine not yet initialized")
    ),
    tag = "App"
)]
pub async fn get_app_state(State(state): State<SharedState>) -> impl IntoResponse {
    match try_engine(&state, |engine| {
        engine
            .app_state
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or(serde_json::Value::Null)
    }) {
        Ok(value) => Json(value).into_response(),
        Err(resp) => resp.into_response(),
    }
}

// ── Params ───────────────────────────────────────────────────────────

/// `GET /api/app/params` — every declared parameter with its current value.
#[utoipa::path(
    get,
    path = "/api/app/params",
    responses(
        (status = 200, description = "Parameter list with live values"),
        (status = 503, description = "Engine not yet initialized")
    ),
    tag = "App"
)]
pub async fn list_params(State(state): State<SharedState>) -> impl IntoResponse {
    match try_engine(&state, |engine| {
        engine
            .param_descriptors
            .iter()
            .map(|d| {
                serde_json::json!({
                    "id": d.id,
                    "name": d.name,
                    "category": d.category.name(),
                    "min": d.min,
                    "max": d.max,
                    "value": engine.get_param(&d.id).unwrap_or(d.default),
                    "default": d.default,
                    "step": d.step,
                })
            })
            .collect::<Vec<_>>()
    }) {
        Ok(params) => Json(params).into_response(),
        Err(resp) => resp.into_response(),
    }
}

/// Body for the hierarchical-path param write endpoint.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SetParamByPathBody {
    /// Hierarchical path, e.g. `deck/<uuid>/param/opacity`, or a flat canonical id.
    pub path: String,
    /// New value (clamped to the descriptor's min/max by the engine).
    #[schema(example = 0.75)]
    pub value: f32,
}

/// `PUT /api/app/params` — set a parameter by hierarchical path or flat id.
///
/// The path is resolved through the app's `param_resolver` (if installed) to a
/// canonical engine id, clamped, then applied via the standard command channel.
#[utoipa::path(
    put,
    path = "/api/app/params",
    request_body = SetParamByPathBody,
    responses(
        (status = 200, description = "Parameter set"),
        (status = 400, description = "Unknown param path"),
        (status = 503, description = "Engine not yet initialized")
    ),
    tag = "App"
)]
pub async fn set_param_by_path(
    State(state): State<SharedState>,
    Json(body): Json<SetParamByPathBody>,
) -> impl IntoResponse {
    let canonical = match try_engine(&state, |engine| resolve_path(engine, &body.path)) {
        Ok(Some(id)) => id,
        Ok(None) => {
            return bad_request(format!("unknown param path: {}", body.path)).into_response()
        }
        Err(resp) => return resp.into_response(),
    };
    let value = match try_engine(&state, |engine| {
        clamp_to_descriptor(engine, &canonical, body.value)
    }) {
        Ok(v) => v,
        Err(resp) => return resp.into_response(),
    };
    match send_command(
        &state,
        rustjay_control::WebCommand::Set {
            id: canonical,
            value,
        },
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}
