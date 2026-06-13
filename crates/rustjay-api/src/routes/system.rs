//! System routes: health, state, shutdown, resolution, clock, workspace.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::SharedState;

// ── Health ─────────────────────────────────────────────────────────

/// Health check response.
#[derive(serde::Serialize, ToSchema)]
pub struct HealthResponse {
    /// Service status.
    pub status: &'static str,
}

/// `GET /api/health`
#[utoipa::path(
    get,
    path = "/api/health",
    responses((status = 200, body = HealthResponse)),
    tag = "System"
)]
pub async fn health() -> impl IntoResponse {
    Json(HealthResponse { status: "ok" })
}

// ── State reads ────────────────────────────────────────────────────

/// Build an `EngineSnapshot` from the live engine state, or early-return an
/// error response. Clones the engine `Arc` out of the outer `WebServerState`
/// guard first so the inner lock is independent of (and outlives) the outer
/// guard, and drops the engine lock before serialization.
macro_rules! try_engine {
    ($state:expr_2021) => {{
        let engine_arc = match $state.lock() {
            Ok(guard) => match guard.engine_state.as_ref() {
                Some(engine) => engine.clone(),
                None => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "Engine not yet initialized",
                    )
                        .into_response()
                }
            },
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Server state lock poisoned",
                )
                    .into_response()
            }
        };
        let snapshot = match engine_arc.lock() {
            Ok(e) => crate::build_snapshot(&e),
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Engine state lock poisoned",
                )
                    .into_response()
            }
        };
        snapshot
    }};
}

/// `GET /api/state` — full engine snapshot.
#[utoipa::path(
    get,
    path = "/api/state",
    responses(
        (status = 200, description = "Full engine state"),
        (status = 503, description = "Engine not yet initialized")
    ),
    tag = "System"
)]
pub async fn get_state(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot).into_response()
}

/// `GET /api/state/input`
pub async fn get_state_input(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.input).into_response()
}

/// `GET /api/state/audio`
pub async fn get_state_audio(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.audio).into_response()
}

/// `GET /api/state/midi`
pub async fn get_state_midi(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.midi).into_response()
}

/// `GET /api/state/osc`
pub async fn get_state_osc(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.osc).into_response()
}

/// `GET /api/state/presets`
pub async fn get_state_presets(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.presets).into_response()
}

/// `GET /api/state/modulation`
pub async fn get_state_modulation(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.modulation).into_response()
}

/// `GET /api/state/performance`
pub async fn get_state_performance(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.performance).into_response()
}

/// `GET /api/state/link`
pub async fn get_state_link(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.link).into_response()
}

/// `GET /api/state/prodj`
pub async fn get_state_prodj(State(state): State<SharedState>) -> impl IntoResponse {
    let snapshot = try_engine!(state);
    Json(snapshot.prodj).into_response()
}

// ── Write commands ─────────────────────────────────────────────────

/// Generic OK response body for command endpoints.
#[derive(serde::Serialize, ToSchema)]
pub struct CommandOk {
    /// Status string.
    pub status: &'static str,
}

fn command_ok() -> impl IntoResponse {
    Json(CommandOk { status: "ok" })
}

fn command_err(msg: impl Into<String>) -> impl IntoResponse {
    let body = serde_json::json!({"error": "internal", "message": msg.into()});
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body))
}

fn send_command(state: &SharedState, cmd: rustjay_control::WebCommand) -> Result<(), &'static str> {
    let guard = state.lock().map_err(|_| "Server state lock poisoned")?;
    guard
        .command_tx
        .try_send(cmd)
        .map_err(|_| "Engine command channel full")
}

// ── Params ─────────────────────────────────────────────────────────

/// Set a parameter value.
#[derive(Deserialize, ToSchema)]
pub struct SetParamBody {
    /// Parameter identifier (e.g. `"color/hue_shift"`).
    pub id: String,
    /// New value.
    pub value: f32,
}

/// `PUT /api/params`
#[utoipa::path(
    put,
    path = "/api/params",
    request_body = SetParamBody,
    responses((status = 200, body = CommandOk)),
    tag = "System"
)]
pub async fn set_param(
    State(state): State<SharedState>,
    Json(body): Json<SetParamBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Set {
            id: body.id,
            value: body.value,
        },
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Input ──────────────────────────────────────────────────────────

/// Start webcam request.
#[derive(Deserialize, ToSchema)]
pub struct StartWebcamBody {
    /// Device index.
    pub device_index: usize,
    /// Capture width.
    pub width: u32,
    /// Capture height.
    pub height: u32,
    /// Capture FPS.
    pub fps: u32,
}

/// `POST /api/input/start-webcam`
#[utoipa::path(
    post,
    path = "/api/input/start-webcam",
    request_body = StartWebcamBody,
    responses((status = 200, body = CommandOk)),
    tag = "Input"
)]
pub async fn input_start_webcam(
    State(state): State<SharedState>,
    Json(body): Json<StartWebcamBody>,
) -> impl IntoResponse {
    let cmd = rustjay_control::WebCommand::Input(rustjay_control::InputWebCommand::SelectDevice {
        index: body.device_index,
        width: body.width,
        height: body.height,
        fps: body.fps,
    });
    match send_command(&state, cmd) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/input/stop`
#[utoipa::path(
    post,
    path = "/api/input/stop",
    responses((status = 200, body = CommandOk)),
    tag = "Input"
)]
pub async fn input_stop(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Input(rustjay_control::InputWebCommand::StopInput),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/input/refresh-devices`
#[utoipa::path(
    post,
    path = "/api/input/refresh-devices",
    responses((status = 200, body = CommandOk)),
    tag = "Input"
)]
pub async fn input_refresh(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Input(rustjay_control::InputWebCommand::RefreshDevices),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Audio ──────────────────────────────────────────────────────────

/// `POST /api/audio/start`
#[utoipa::path(
    post,
    path = "/api/audio/start",
    responses((status = 200, body = CommandOk)),
    tag = "Audio"
)]
pub async fn audio_start(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Audio(rustjay_control::AudioWebCommand::Start),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/audio/stop`
#[utoipa::path(
    post,
    path = "/api/audio/stop",
    responses((status = 200, body = CommandOk)),
    tag = "Audio"
)]
pub async fn audio_stop(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Audio(rustjay_control::AudioWebCommand::Stop),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/audio/refresh-devices`
#[utoipa::path(
    post,
    path = "/api/audio/refresh-devices",
    responses((status = 200, body = CommandOk)),
    tag = "Audio"
)]
pub async fn audio_refresh(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Audio(rustjay_control::AudioWebCommand::RefreshDevices),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Select audio device request.
#[derive(Deserialize, ToSchema)]
pub struct SelectAudioDeviceBody {
    /// Device name.
    pub device: String,
}

/// `POST /api/audio/select-device`
#[utoipa::path(
    post,
    path = "/api/audio/select-device",
    request_body = SelectAudioDeviceBody,
    responses((status = 200, body = CommandOk)),
    tag = "Audio"
)]
pub async fn audio_select_device(
    State(state): State<SharedState>,
    Json(body): Json<SelectAudioDeviceBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Audio(rustjay_control::AudioWebCommand::SelectDevice {
            device: body.device,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Set FFT size request.
#[derive(Deserialize, ToSchema)]
pub struct SetFftSizeBody {
    /// Window size.
    pub size: usize,
}

/// `POST /api/audio/fft-size`
#[utoipa::path(
    post,
    path = "/api/audio/fft-size",
    request_body = SetFftSizeBody,
    responses((status = 200, body = CommandOk)),
    tag = "Audio"
)]
pub async fn audio_set_fft_size(
    State(state): State<SharedState>,
    Json(body): Json<SetFftSizeBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Audio(rustjay_control::AudioWebCommand::SetFftSize {
            size: body.size,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Output ─────────────────────────────────────────────────────────

/// `POST /api/output/start-ndi`
#[utoipa::path(
    post,
    path = "/api/output/start-ndi",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_start_ndi(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StartNdi),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/output/stop-ndi`
#[utoipa::path(
    post,
    path = "/api/output/stop-ndi",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_stop_ndi(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StopNdi),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/output/resize`
#[utoipa::path(
    post,
    path = "/api/output/resize",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_resize(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::ResizeOutput),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/output/start-syphon` (macOS)
#[utoipa::path(
    post,
    path = "/api/output/start-syphon",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_start_syphon(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StartSyphon),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/output/stop-syphon` (macOS)
#[utoipa::path(
    post,
    path = "/api/output/stop-syphon",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_stop_syphon(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StopSyphon),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Start Spout sender request (Windows).
#[derive(Deserialize, ToSchema)]
pub struct StartSpoutBody {
    /// Spout sender name.
    pub sender_name: String,
}

/// `POST /api/output/start-spout` (Windows)
#[utoipa::path(
    post,
    path = "/api/output/start-spout",
    request_body = StartSpoutBody,
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_start_spout(
    State(state): State<SharedState>,
    Json(body): Json<StartSpoutBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StartSpout {
            sender_name: body.sender_name,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/output/stop-spout` (Windows)
#[utoipa::path(
    post,
    path = "/api/output/stop-spout",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_stop_spout(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StopSpout),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Start V4L2 loopback sink request (Linux).
#[derive(Deserialize, ToSchema)]
pub struct StartV4l2Body {
    /// V4L2 loopback device path (e.g. `/dev/video10`).
    pub device_path: String,
}

/// `POST /api/output/start-v4l2` (Linux)
#[utoipa::path(
    post,
    path = "/api/output/start-v4l2",
    request_body = StartV4l2Body,
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_start_v4l2(
    State(state): State<SharedState>,
    Json(body): Json<StartV4l2Body>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StartV4l2 {
            device_path: body.device_path,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/output/stop-v4l2` (Linux)
#[utoipa::path(
    post,
    path = "/api/output/stop-v4l2",
    responses((status = 200, body = CommandOk)),
    tag = "Output"
)]
pub async fn output_stop_v4l2(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Output(rustjay_control::OutputWebCommand::StopV4l2),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── MIDI ───────────────────────────────────────────────────────────

/// `POST /api/midi/refresh-devices`
#[utoipa::path(
    post,
    path = "/api/midi/refresh-devices",
    responses((status = 200, body = CommandOk)),
    tag = "MIDI"
)]
pub async fn midi_refresh(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(
            rustjay_control::ControlWebCommand::MidiRefreshDevices,
        ),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Select MIDI device request.
#[derive(Deserialize, ToSchema)]
pub struct SelectMidiDeviceBody {
    /// Device name.
    pub device: String,
}

/// `POST /api/midi/select-device`
#[utoipa::path(
    post,
    path = "/api/midi/select-device",
    request_body = SelectMidiDeviceBody,
    responses((status = 200, body = CommandOk)),
    tag = "MIDI"
)]
pub async fn midi_select_device(
    State(state): State<SharedState>,
    Json(body): Json<SelectMidiDeviceBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(
            rustjay_control::ControlWebCommand::MidiSelectDevice {
                device: body.device,
            },
        ),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/midi/disconnect`
#[utoipa::path(
    post,
    path = "/api/midi/disconnect",
    responses((status = 200, body = CommandOk)),
    tag = "MIDI"
)]
pub async fn midi_disconnect(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::MidiDisconnect),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// MIDI learn request.
#[derive(Deserialize, ToSchema)]
pub struct MidiLearnBody {
    /// Parameter path to learn.
    pub param_id: String,
}

/// `POST /api/midi/learn`
#[utoipa::path(
    post,
    path = "/api/midi/learn",
    request_body = MidiLearnBody,
    responses((status = 200, body = CommandOk)),
    tag = "MIDI"
)]
pub async fn midi_learn(
    State(state): State<SharedState>,
    Json(body): Json<MidiLearnBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::MidiLearn {
            param_id: body.param_id,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/midi/learn-cancel`
#[utoipa::path(
    post,
    path = "/api/midi/learn-cancel",
    responses((status = 200, body = CommandOk)),
    tag = "MIDI"
)]
pub async fn midi_learn_cancel(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::MidiLearnCancel),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Remove a learned MIDI mapping by CC and channel.
#[derive(Deserialize, ToSchema)]
pub struct MidiUnlearnBody {
    /// MIDI continuous-controller number.
    pub cc: u8,
    /// MIDI channel (0–15).
    pub channel: u8,
}

/// `POST /api/midi/unlearn`
#[utoipa::path(
    post,
    path = "/api/midi/unlearn",
    request_body = MidiUnlearnBody,
    responses((status = 200, body = CommandOk)),
    tag = "MIDI"
)]
pub async fn midi_unlearn(
    State(state): State<SharedState>,
    Json(body): Json<MidiUnlearnBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::MidiUnlearn {
            cc: body.cc,
            channel: body.channel,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── OSC ────────────────────────────────────────────────────────────

/// `POST /api/osc/start`
#[utoipa::path(
    post,
    path = "/api/osc/start",
    responses((status = 200, body = CommandOk)),
    tag = "OSC"
)]
pub async fn osc_start(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::Osc {
            enabled: true,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/osc/stop`
#[utoipa::path(
    post,
    path = "/api/osc/stop",
    responses((status = 200, body = CommandOk)),
    tag = "OSC"
)]
pub async fn osc_stop(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::Osc {
            enabled: false,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Set OSC port request.
#[derive(Deserialize, ToSchema)]
pub struct OscPortBody {
    /// New port.
    pub port: u16,
}

/// `POST /api/osc/port`
#[utoipa::path(
    post,
    path = "/api/osc/port",
    request_body = OscPortBody,
    responses((status = 200, body = CommandOk)),
    tag = "OSC"
)]
pub async fn osc_set_port(
    State(state): State<SharedState>,
    Json(body): Json<OscPortBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Control(rustjay_control::ControlWebCommand::OscSetPort {
            port: body.port,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Presets ────────────────────────────────────────────────────────

/// Save preset request.
#[derive(Deserialize, ToSchema)]
pub struct SavePresetBody {
    /// Preset name.
    pub name: String,
}

/// `POST /api/presets/save`
#[utoipa::path(
    post,
    path = "/api/presets/save",
    request_body = SavePresetBody,
    responses((status = 200, body = CommandOk)),
    tag = "Presets"
)]
pub async fn preset_save(
    State(state): State<SharedState>,
    Json(body): Json<SavePresetBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Preset(rustjay_control::PresetWebCommand::Save {
            name: body.name,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Load preset request.
#[derive(Deserialize, ToSchema)]
pub struct LoadPresetBody {
    /// Preset index.
    pub index: usize,
}

/// `POST /api/presets/load`
#[utoipa::path(
    post,
    path = "/api/presets/load",
    request_body = LoadPresetBody,
    responses((status = 200, body = CommandOk)),
    tag = "Presets"
)]
pub async fn preset_load(
    State(state): State<SharedState>,
    Json(body): Json<LoadPresetBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Preset(rustjay_control::PresetWebCommand::Load {
            index: body.index,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Delete preset request.
#[derive(Deserialize, ToSchema)]
pub struct DeletePresetBody {
    /// Preset index.
    pub index: usize,
}

/// `POST /api/presets/delete`
#[utoipa::path(
    post,
    path = "/api/presets/delete",
    request_body = DeletePresetBody,
    responses((status = 200, body = CommandOk)),
    tag = "Presets"
)]
pub async fn preset_delete(
    State(state): State<SharedState>,
    Json(body): Json<DeletePresetBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Preset(rustjay_control::PresetWebCommand::Delete {
            index: body.index,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Link ───────────────────────────────────────────────────────────

/// `POST /api/link/enable`
#[utoipa::path(
    post,
    path = "/api/link/enable",
    responses((status = 200, body = CommandOk)),
    tag = "Link"
)]
pub async fn link_enable(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Link(rustjay_control::LinkWebCommand::Enable),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/link/disable`
#[utoipa::path(
    post,
    path = "/api/link/disable",
    responses((status = 200, body = CommandOk)),
    tag = "Link"
)]
pub async fn link_disable(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Link(rustjay_control::LinkWebCommand::Disable),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Set Link quantum request.
#[derive(Deserialize, ToSchema)]
pub struct LinkQuantumBody {
    /// Musical quantum (e.g. 4.0 for a 4/4 bar).
    pub quantum: f64,
}

/// `POST /api/link/quantum`
#[utoipa::path(
    post,
    path = "/api/link/quantum",
    request_body = LinkQuantumBody,
    responses((status = 200, body = CommandOk)),
    tag = "Link"
)]
pub async fn link_set_quantum(
    State(state): State<SharedState>,
    Json(body): Json<LinkQuantumBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Link(rustjay_control::LinkWebCommand::SetQuantum {
            quantum: body.quantum,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── ProDJ ──────────────────────────────────────────────────────────

/// `POST /api/prodj/start`
#[utoipa::path(
    post,
    path = "/api/prodj/start",
    responses((status = 200, body = CommandOk)),
    tag = "ProDJ"
)]
pub async fn prodj_start(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::ProDj(rustjay_control::ProDjWebCommand::Start),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/prodj/stop`
#[utoipa::path(
    post,
    path = "/api/prodj/stop",
    responses((status = 200, body = CommandOk)),
    tag = "ProDJ"
)]
pub async fn prodj_stop(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::ProDj(rustjay_control::ProDjWebCommand::Stop),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Mixer (parameter proxy) ────────────────────────────────────────

/// Set crossfader request.
#[derive(Deserialize, ToSchema)]
pub struct CrossfaderBody {
    /// Position 0.0 (left) to 1.0 (right).
    pub position: f32,
}

/// `PUT /api/mixer/crossfader`
#[utoipa::path(
    put,
    path = "/api/mixer/crossfader",
    request_body = CrossfaderBody,
    responses((status = 200, body = CommandOk)),
    tag = "Mixer"
)]
pub async fn mixer_crossfader(
    State(state): State<SharedState>,
    Json(body): Json<CrossfaderBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Set {
            id: "mixer/crossfader".to_string(),
            value: body.position.clamp(0.0, 1.0),
        },
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Set master opacity request.
#[derive(Deserialize, ToSchema)]
pub struct MasterOpacityBody {
    /// Opacity 0.0 to 1.0.
    pub opacity: f32,
}

/// `PUT /api/mixer/master-opacity`
#[utoipa::path(
    put,
    path = "/api/mixer/master-opacity",
    request_body = MasterOpacityBody,
    responses((status = 200, body = CommandOk)),
    tag = "Mixer"
)]
pub async fn mixer_master_opacity(
    State(state): State<SharedState>,
    Json(body): Json<MasterOpacityBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Set {
            id: "mixer/master_opacity".to_string(),
            value: body.opacity.clamp(0.0, 1.0),
        },
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Modulation (writes) ────────────────────────────────────────────

/// Configure an LFO slot.
#[derive(Deserialize, ToSchema)]
pub struct LfoSetBody {
    /// LFO slot index.
    pub slot: usize,
    /// Full LFO configuration (waveform, rate, depth, target, etc.).
    #[schema(value_type = Object)]
    pub config: rustjay_core::lfo::Lfo,
}

/// `POST /api/modulation/lfo`
#[utoipa::path(
    post,
    path = "/api/modulation/lfo",
    request_body = LfoSetBody,
    responses((status = 200, body = CommandOk)),
    tag = "Modulation"
)]
pub async fn modulation_lfo_set(
    State(state): State<SharedState>,
    Json(body): Json<LfoSetBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Modulation(rustjay_control::ModulationWebCommand::LfoSet {
            slot: body.slot,
            config: body.config,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Enable or disable an LFO slot.
#[derive(Deserialize, ToSchema)]
pub struct LfoEnableBody {
    /// LFO slot index.
    pub slot: usize,
    /// Whether the slot is active.
    pub enabled: bool,
}

/// `POST /api/modulation/lfo-enable`
#[utoipa::path(
    post,
    path = "/api/modulation/lfo-enable",
    request_body = LfoEnableBody,
    responses((status = 200, body = CommandOk)),
    tag = "Modulation"
)]
pub async fn modulation_lfo_enable(
    State(state): State<SharedState>,
    Json(body): Json<LfoEnableBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Modulation(rustjay_control::ModulationWebCommand::LfoEnable {
            slot: body.slot,
            enabled: body.enabled,
        }),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Route an FFT band to a parameter.
#[derive(Deserialize, ToSchema)]
pub struct AudioRouteBody {
    /// Target parameter identifier.
    pub param_id: String,
    /// Source FFT band (e.g. `"Bass"`, `"HighMid"`).
    #[schema(value_type = String)]
    pub band: rustjay_core::FftBand,
    /// Modulation depth.
    pub depth: f32,
}

/// `POST /api/modulation/audio-route`
#[utoipa::path(
    post,
    path = "/api/modulation/audio-route",
    request_body = AudioRouteBody,
    responses((status = 200, body = CommandOk)),
    tag = "Modulation"
)]
pub async fn modulation_audio_route(
    State(state): State<SharedState>,
    Json(body): Json<AudioRouteBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Modulation(
            rustjay_control::ModulationWebCommand::AudioRoute {
                param_id: body.param_id,
                band: body.band,
                depth: body.depth,
            },
        ),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// Remove an audio route from a parameter.
#[derive(Deserialize, ToSchema)]
pub struct AudioUnrouteBody {
    /// Target parameter identifier.
    pub param_id: String,
}

/// `POST /api/modulation/audio-unroute`
#[utoipa::path(
    post,
    path = "/api/modulation/audio-unroute",
    request_body = AudioUnrouteBody,
    responses((status = 200, body = CommandOk)),
    tag = "Modulation"
)]
pub async fn modulation_audio_unroute(
    State(state): State<SharedState>,
    Json(body): Json<AudioUnrouteBody>,
) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Modulation(
            rustjay_control::ModulationWebCommand::AudioUnroute {
                param_id: body.param_id,
            },
        ),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/modulation/tap-tempo`
#[utoipa::path(
    post,
    path = "/api/modulation/tap-tempo",
    responses((status = 200, body = CommandOk)),
    tag = "Modulation"
)]
pub async fn modulation_tap_tempo(State(state): State<SharedState>) -> impl IntoResponse {
    match send_command(
        &state,
        rustjay_control::WebCommand::Modulation(rustjay_control::ModulationWebCommand::TapTempo),
    ) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}
