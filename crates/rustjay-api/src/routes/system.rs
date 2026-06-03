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
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(snapshot) => Json(serde_json::to_value(snapshot).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/input`
pub async fn get_state_input(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.input).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/audio`
pub async fn get_state_audio(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.audio).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/midi`
pub async fn get_state_midi(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.midi).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/osc`
pub async fn get_state_osc(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.osc).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/presets`
pub async fn get_state_presets(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.presets).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/modulation`
pub async fn get_state_modulation(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.modulation).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/performance`
pub async fn get_state_performance(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.performance).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/link`
pub async fn get_state_link(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.link).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
}

/// `GET /api/state/prodj`
pub async fn get_state_prodj(State(state): State<SharedState>) -> impl IntoResponse {
    match state.engine_snapshot.read() {
        Ok(guard) => match guard.as_ref() {
            Some(s) => Json(serde_json::to_value(&s.prodj).unwrap()).into_response(),
            None => (StatusCode::SERVICE_UNAVAILABLE, "Engine not yet initialized").into_response(),
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "State lock poisoned").into_response(),
    }
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
    match state.send_command(rustjay_control::WebCommand::Set {
        id: body.id,
        value: body.value,
    }) {
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
    let cmd = rustjay_control::WebCommand::Input(
        rustjay_control::InputWebCommand::SelectDevice {
            index: body.device_index,
            width: body.width,
            height: body.height,
            fps: body.fps,
        },
    );
    match state.send_command(cmd) {
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
    match state.send_command(rustjay_control::WebCommand::Input(
        rustjay_control::InputWebCommand::StopInput,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Input(
        rustjay_control::InputWebCommand::RefreshDevices,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Audio(
        rustjay_control::AudioWebCommand::Start,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Audio(
        rustjay_control::AudioWebCommand::Stop,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Audio(
        rustjay_control::AudioWebCommand::RefreshDevices,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Audio(
        rustjay_control::AudioWebCommand::SelectDevice { device: body.device },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Audio(
        rustjay_control::AudioWebCommand::SetFftSize { size: body.size },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Output(
        rustjay_control::OutputWebCommand::StartNdi,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Output(
        rustjay_control::OutputWebCommand::StopNdi,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Output(
        rustjay_control::OutputWebCommand::ResizeOutput,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::MidiRefreshDevices,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::MidiSelectDevice { device: body.device },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::MidiDisconnect,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::MidiLearn { param_id: body.param_id },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::MidiLearnCancel,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::Osc { enabled: true },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::Osc { enabled: false },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Control(
        rustjay_control::ControlWebCommand::OscSetPort { port: body.port },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Preset(
        rustjay_control::PresetWebCommand::Save { name: body.name },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Preset(
        rustjay_control::PresetWebCommand::Load { index: body.index },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Preset(
        rustjay_control::PresetWebCommand::Delete { index: body.index },
    )) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

// ── Web server ─────────────────────────────────────────────────────

/// `POST /api/web/start`
#[utoipa::path(
    post,
    path = "/api/web/start",
    responses((status = 200, body = CommandOk)),
    tag = "Web"
)]
pub async fn web_start(State(state): State<SharedState>) -> impl IntoResponse {
    match state.send_command(rustjay_control::WebCommand::Set {
        id: "web/start".to_string(),
        value: 1.0,
    }) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}

/// `POST /api/web/stop`
#[utoipa::path(
    post,
    path = "/api/web/stop",
    responses((status = 200, body = CommandOk)),
    tag = "Web"
)]
pub async fn web_stop(State(state): State<SharedState>) -> impl IntoResponse {
    match state.send_command(rustjay_control::WebCommand::Set {
        id: "web/stop".to_string(),
        value: 1.0,
    }) {
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
    match state.send_command(rustjay_control::WebCommand::Link(
        rustjay_control::LinkWebCommand::Enable,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Link(
        rustjay_control::LinkWebCommand::Disable,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Link(
        rustjay_control::LinkWebCommand::SetQuantum { quantum: body.quantum },
    )) {
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
    match state.send_command(rustjay_control::WebCommand::ProDj(
        rustjay_control::ProDjWebCommand::Start,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::ProDj(
        rustjay_control::ProDjWebCommand::Stop,
    )) {
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
    match state.send_command(rustjay_control::WebCommand::Set {
        id: "mixer/crossfader".to_string(),
        value: body.position.clamp(0.0, 1.0),
    }) {
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
    match state.send_command(rustjay_control::WebCommand::Set {
        id: "mixer/master_opacity".to_string(),
        value: body.opacity.clamp(0.0, 1.0),
    }) {
        Ok(()) => command_ok().into_response(),
        Err(m) => command_err(m).into_response(),
    }
}
