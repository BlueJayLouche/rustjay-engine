//! Optional REST/OpenAPI layer for rustjay-engine.
//!
//! Returns a `Router<SharedState>` without bound state; call `.with_state(shared)`
//! to serve it, or let `rustjay-control` merge it under its protected tree.

pub mod openapi;
pub mod routes;
pub mod ws;

#[cfg(test)]
mod tests;

use axum::Router;
use std::sync::{Arc, Mutex};

/// Shared state type for all API route handlers.
pub type SharedState = Arc<Mutex<rustjay_control::WebServerState>>;

/// Build the axum router with all API routes.
pub fn build_router() -> Router<SharedState> {
    use axum::routing::{get, post, put};
    use utoipa::OpenApi;
    use utoipa_swagger_ui::SwaggerUi;

    Router::new()
        // ── System ──────────────────────────────────────────────
        .route("/api/health", get(routes::system::health))
        .route("/api/state", get(routes::system::get_state))
        .route("/api/state/input", get(routes::system::get_state_input))
        .route("/api/state/audio", get(routes::system::get_state_audio))
        .route("/api/state/midi", get(routes::system::get_state_midi))
        .route("/api/state/osc", get(routes::system::get_state_osc))
        .route("/api/state/presets", get(routes::system::get_state_presets))
        .route(
            "/api/state/modulation",
            get(routes::system::get_state_modulation),
        )
        .route(
            "/api/state/performance",
            get(routes::system::get_state_performance),
        )
        .route("/api/state/link", get(routes::system::get_state_link))
        .route("/api/state/prodj", get(routes::system::get_state_prodj))
        // ── Params ──────────────────────────────────────────────
        .route("/api/params", put(routes::system::set_param))
        // ── Input ───────────────────────────────────────────────
        .route(
            "/api/input/start-webcam",
            post(routes::system::input_start_webcam),
        )
        .route("/api/input/stop", post(routes::system::input_stop))
        .route(
            "/api/input/refresh-devices",
            post(routes::system::input_refresh),
        )
        // ── Output ──────────────────────────────────────────────
        .route(
            "/api/output/start-ndi",
            post(routes::system::output_start_ndi),
        )
        .route(
            "/api/output/stop-ndi",
            post(routes::system::output_stop_ndi),
        )
        .route("/api/output/resize", post(routes::system::output_resize))
        .route(
            "/api/output/start-syphon",
            post(routes::system::output_start_syphon),
        )
        .route(
            "/api/output/stop-syphon",
            post(routes::system::output_stop_syphon),
        )
        .route(
            "/api/output/start-spout",
            post(routes::system::output_start_spout),
        )
        .route(
            "/api/output/stop-spout",
            post(routes::system::output_stop_spout),
        )
        .route(
            "/api/output/start-v4l2",
            post(routes::system::output_start_v4l2),
        )
        .route(
            "/api/output/stop-v4l2",
            post(routes::system::output_stop_v4l2),
        )
        // ── Audio ───────────────────────────────────────────────
        .route("/api/audio/start", post(routes::system::audio_start))
        .route("/api/audio/stop", post(routes::system::audio_stop))
        .route(
            "/api/audio/refresh-devices",
            post(routes::system::audio_refresh),
        )
        .route(
            "/api/audio/select-device",
            post(routes::system::audio_select_device),
        )
        .route(
            "/api/audio/fft-size",
            post(routes::system::audio_set_fft_size),
        )
        // ── MIDI ────────────────────────────────────────────────
        .route(
            "/api/midi/refresh-devices",
            post(routes::system::midi_refresh),
        )
        .route(
            "/api/midi/select-device",
            post(routes::system::midi_select_device),
        )
        .route(
            "/api/midi/disconnect",
            post(routes::system::midi_disconnect),
        )
        .route("/api/midi/learn", post(routes::system::midi_learn))
        .route(
            "/api/midi/learn-cancel",
            post(routes::system::midi_learn_cancel),
        )
        .route("/api/midi/unlearn", post(routes::system::midi_unlearn))
        // ── OSC ─────────────────────────────────────────────────
        .route("/api/osc/start", post(routes::system::osc_start))
        .route("/api/osc/stop", post(routes::system::osc_stop))
        .route("/api/osc/port", post(routes::system::osc_set_port))
        // ── Presets ─────────────────────────────────────────────
        .route("/api/presets/save", post(routes::system::preset_save))
        .route("/api/presets/load", post(routes::system::preset_load))
        .route("/api/presets/delete", post(routes::system::preset_delete))
        // ── Link ────────────────────────────────────────────────
        .route("/api/link/enable", post(routes::system::link_enable))
        .route("/api/link/disable", post(routes::system::link_disable))
        .route("/api/link/quantum", post(routes::system::link_set_quantum))
        // ── ProDJ ───────────────────────────────────────────────
        .route("/api/prodj/start", post(routes::system::prodj_start))
        .route("/api/prodj/stop", post(routes::system::prodj_stop))
        // ── Mixer ───────────────────────────────────────────────
        .route(
            "/api/mixer/crossfader",
            put(routes::system::mixer_crossfader),
        )
        .route(
            "/api/mixer/master-opacity",
            put(routes::system::mixer_master_opacity),
        )
        // ── Modulation (writes) ─────────────────────────────────
        .route(
            "/api/modulation/lfo",
            post(routes::system::modulation_lfo_set),
        )
        .route(
            "/api/modulation/lfo-enable",
            post(routes::system::modulation_lfo_enable),
        )
        .route(
            "/api/modulation/audio-route",
            post(routes::system::modulation_audio_route),
        )
        .route(
            "/api/modulation/audio-unroute",
            post(routes::system::modulation_audio_unroute),
        )
        .route(
            "/api/modulation/tap-tempo",
            post(routes::system::modulation_tap_tempo),
        )
        // ── Generic app routes (app-published state + hierarchical params) ─
        .route("/api/app/state", get(routes::app::get_app_state))
        .route(
            "/api/app/params",
            get(routes::app::list_params).put(routes::app::set_param_by_path),
        )
        // ── WebSocket (JSON-Patch deltas) ─────────────────────────
        .route("/api/ws", axum::routing::get(ws::ws_upgrade))
        // ── OpenAPI / Swagger ───────────────────────────────────
        .merge(SwaggerUi::new("/swagger-ui").url("/api/openapi.json", openapi::ApiDoc::openapi()))
        // ── Middleware ───────────────────────────────────────────
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
}

// ── Engine Snapshot DTOs ───────────────────────────────────────────────────

/// Serializable snapshot of engine state, returned by GET /api/state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineSnapshot {
    pub output: OutputSnapshot,
    pub input: InputSnapshot,
    pub second_input: InputSnapshot,
    pub audio: AudioSnapshot,
    pub color: ColorSnapshot,
    pub midi: MidiSnapshot,
    pub osc: OscSnapshot,
    pub presets: PresetSnapshot,
    pub web: WebSnapshot,
    pub link: LinkSnapshot,
    pub prodj: ProDjSnapshot,
    pub mtc: MtcSnapshot,
    pub resolution: ResolutionSnapshot,
    pub performance: PerformanceSnapshot,
    pub params: ParamsSnapshot,
    pub modulation: ModulationSnapshot,
    pub target_fps: u32,
    pub current_tab: String,
    /// Opaque JSON blob the active app published, or null.
    pub app_state: serde_json::Value,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OutputSnapshot {
    pub fullscreen: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InputSnapshot {
    pub input_type: String,
    pub source_name: String,
    pub is_active: bool,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
    pub device_index: Option<usize>,
    pub available_devices: Vec<rustjay_core::InputDeviceInfo>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AudioSnapshot {
    /// 8-band FFT magnitudes (0–1).
    pub fft: [f32; 8],
    pub volume: f32,
    pub beat: bool,
    pub bpm: f32,
    /// Beat phase (0–1).
    pub beat_phase: f32,
    pub enabled: bool,
    pub amplitude: f32,
    pub smoothing: f32,
    pub selected_device: Option<String>,
    pub available_devices: Vec<String>,
    pub normalize: bool,
    pub pink_noise_shaping: bool,
    pub fft_size: usize,
    pub tap_tempo_info: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ColorSnapshot {
    /// Degrees, -180 to 180.
    pub hue_shift: f32,
    /// Multiplier, 0–2.
    pub saturation: f32,
    /// Multiplier, 0–2.
    pub brightness: f32,
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MidiSnapshot {
    pub enabled: bool,
    pub selected_device: Option<String>,
    pub available_devices: Vec<String>,
    pub learn_active: bool,
    pub learning_param_name: Option<String>,
    pub mappings: Vec<rustjay_core::MidiMappingSnapshot>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OscSnapshot {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    /// (address, value, timestamp) tuples.
    pub message_log: Vec<(String, f32, f64)>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PresetSnapshot {
    pub names: Vec<String>,
    /// Quick-slot assignments (indices 0–7).
    pub quick_slot_names: Vec<Option<String>>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WebSnapshot {
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub app_name: String,
    pub lan_trust: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LinkSnapshot {
    pub enabled: bool,
    pub num_peers: usize,
    pub bpm: f32,
    /// Beat phase (0–1).
    pub beat_phase: f32,
    pub quantum: f64,
    pub is_playing: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProDjSnapshot {
    pub enabled: bool,
    pub devices: Vec<ProDjDeviceSnapshot>,
    pub master_bpm: f32,
    pub master_beat_phase: f32,
    pub current_track_artist: String,
    pub current_track_title: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProDjDeviceSnapshot {
    pub device_id: u32,
    pub name: String,
    pub is_playing: bool,
    pub is_master: bool,
    pub bpm: Option<f32>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MtcSnapshot {
    pub running: bool,
    pub playing: bool,
    pub position: String,
    pub source_device: String,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ResolutionSnapshot {
    pub internal_width: u32,
    pub internal_height: u32,
    pub input_width: u32,
    pub input_height: u32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PerformanceSnapshot {
    pub fps: f32,
    pub frame_time_ms: f32,
    /// Zero when the `sysmon` feature is off.
    pub cpu_percent: f32,
    /// Zero when the `sysmon` feature is off.
    pub mem_used_mb: u64,
    /// Zero when the `sysmon` feature is off.
    pub mem_total_mb: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ParamsSnapshot {
    pub descriptors: Vec<rustjay_core::ParameterDescriptor>,
    /// Base (pre-modulation) values, parallel to `descriptors`.
    pub bases: Vec<f32>,
    /// Modulated live values, parallel to `descriptors`.
    pub values: Vec<f32>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModulationSnapshot {
    /// Legacy LFO bank shim.
    pub lfos: Vec<rustjay_core::lfo::Lfo>,
    pub sources: Vec<rustjay_core::modulation::ModulationSourceEntry>,
    pub assignments: std::collections::HashMap<String, Vec<rustjay_core::modulation::ParamModulation>>,
    pub audio_routes: Vec<rustjay_core::routing::AudioRoute>,
    pub audio_routing_enabled: bool,
    pub bpm: f32,
    pub tap_tempo_info: String,
}

/// Build a snapshot DTO from the live engine state.
pub fn build_snapshot(state: &rustjay_core::EngineState) -> EngineSnapshot {
    EngineSnapshot {
        output: OutputSnapshot {
            fullscreen: state.output_fullscreen,
            width: state.output_width,
            height: state.output_height,
        },
        input: input_snapshot(&state.input),
        second_input: input_snapshot(&state.second_input),
        audio: AudioSnapshot {
            fft: state.audio.fft,
            volume: state.audio.volume,
            beat: state.audio.beat,
            bpm: state.audio.bpm,
            beat_phase: state.audio.beat_phase,
            enabled: state.audio.enabled,
            amplitude: state.audio.amplitude,
            smoothing: state.audio.smoothing,
            selected_device: state.audio.selected_device.clone(),
            available_devices: state.audio.available_devices.clone(),
            normalize: state.audio.normalize,
            pink_noise_shaping: state.audio.pink_noise_shaping,
            fft_size: state.audio.fft_size,
            tap_tempo_info: state.audio.tap_tempo_info.clone(),
        },
        color: ColorSnapshot {
            hue_shift: state.hsb_params.hue_shift,
            saturation: state.hsb_params.saturation,
            brightness: state.hsb_params.brightness,
            enabled: state.color_enabled,
        },
        midi: MidiSnapshot {
            enabled: state.midi_enabled,
            selected_device: state.midi_selected_device.clone(),
            available_devices: state.midi_available_devices.clone(),
            learn_active: state.midi_learn_active,
            learning_param_name: state.midi_learning_param_name.clone(),
            mappings: state.midi_mappings.clone(),
        },
        osc: OscSnapshot {
            enabled: state.osc_enabled,
            host: state.osc_host.clone(),
            port: state.osc_port,
            message_log: state.osc_message_log.clone(),
        },
        presets: PresetSnapshot {
            names: state.preset_names.clone(),
            quick_slot_names: state.preset_quick_slot_names.to_vec(),
        },
        web: WebSnapshot {
            enabled: state.web_enabled,
            host: state.web_host.clone(),
            port: state.web_port,
            app_name: state.web_app_name.clone(),
            lan_trust: state.web_lan_trust,
        },
        link: LinkSnapshot {
            enabled: state.link.enabled,
            num_peers: state.link.num_peers,
            bpm: state.link.bpm,
            beat_phase: state.link.beat_phase,
            quantum: state.link.quantum,
            is_playing: state.link.is_playing,
        },
        prodj: ProDjSnapshot {
            enabled: state.prodj.enabled,
            devices: state
                .prodj
                .devices
                .iter()
                .map(|d| ProDjDeviceSnapshot {
                    device_id: d.device_id,
                    name: d.name.clone(),
                    is_playing: d.is_playing,
                    is_master: d.is_master,
                    bpm: d.bpm,
                })
                .collect(),
            master_bpm: state.prodj.master_bpm,
            master_beat_phase: state.prodj.master_beat_phase,
            current_track_artist: state.prodj.current_track_artist.clone(),
            current_track_title: state.prodj.current_track_title.clone(),
        },
        mtc: MtcSnapshot {
            running: state.mtc.running,
            playing: state.mtc.playing,
            position: state.mtc.position.to_string(),
            source_device: state.mtc.source_device.clone(),
        },
        resolution: ResolutionSnapshot {
            internal_width: state.resolution.internal_width,
            internal_height: state.resolution.internal_height,
            input_width: state.resolution.input_width,
            input_height: state.resolution.input_height,
        },
        performance: {
            let perf = state
                .performance
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            PerformanceSnapshot {
                fps: perf.fps,
                frame_time_ms: perf.frame_time_ms,
                cpu_percent: perf.cpu_percent,
                mem_used_mb: perf.mem_used_mb,
                mem_total_mb: perf.mem_total_mb,
            }
        },
        params: ParamsSnapshot {
            descriptors: (*state.param_descriptors).clone(),
            bases: state.custom_param_bases.clone(),
            values: state
                .param_descriptors
                .iter()
                .enumerate()
                .map(|(i, d)| {
                    state.get_param(&d.id)
                        .unwrap_or(state.custom_param_bases.get(i).copied().unwrap_or(d.default))
                })
                .collect(),
        },
        modulation: {
            let mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
            ModulationSnapshot {
                lfos: mod_eng.to_lfo_vec(),
                sources: mod_eng.sources.clone(),
                assignments: mod_eng.assignments.clone(),
                audio_routes: state.audio_routing.matrix.routes().to_vec(),
                audio_routing_enabled: state.audio_routing.enabled,
                bpm: state.audio.bpm,
                tap_tempo_info: state.audio.tap_tempo_info.clone(),
            }
        },
        target_fps: state.target_fps,
        current_tab: state.current_tab.name().to_string(),
        app_state: state
            .app_state
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .unwrap_or(serde_json::Value::Null),
    }
}

fn input_snapshot(input: &rustjay_core::InputState) -> InputSnapshot {
    InputSnapshot {
        input_type: input.input_type.name().to_string(),
        source_name: input.source_name.clone(),
        is_active: input.is_active,
        width: input.width,
        height: input.height,
        fps: input.fps,
        device_index: input.device_index,
        available_devices: input.available_devices.clone(),
    }
}
