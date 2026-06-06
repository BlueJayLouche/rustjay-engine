//! # rustjay-api — Optional REST/OpenAPI layer for rustjay-engine
//!
//! Provides typed REST endpoints and OpenAPI/Swagger documentation.
//!
//! This crate is a leaf dependency of `rustjay-engine` and is **off by
//! default**. When enabled, its router is merged into the existing
//! `rustjay-control` web server so there is only one HTTP listener.

#![warn(missing_docs)]

pub mod openapi;
pub mod routes;
pub mod ws;

#[cfg(test)]
mod tests;

use axum::Router;
use std::sync::{Arc, Mutex};

/// Re-export the control-layer state type so route handlers can use it
/// directly.  The API is mounted under the same auth layer as the control
/// server, so it shares `Arc<Mutex<WebServerState>>` as its router state.
pub type SharedState = Arc<Mutex<rustjay_control::WebServerState>>;

/// Build the axum router with all API routes.
///
/// The returned router is *stateless* in the type sense: it is a
/// `Router<SharedState>` whose handlers expect `State<SharedState>` but for
/// which no state has been provided yet. `rustjay-control` merges it into its
/// protected router and supplies the single shared state via `.with_state(...)`
/// for the whole tree, so there is exactly one `WebServerState` instance.
///
/// To serve this router standalone (e.g. in tests), call `.with_state(shared)`
/// on the result.
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

/// A serializable snapshot of engine state, built on-demand in handlers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineSnapshot {
    /// Output window state.
    pub output: OutputSnapshot,
    /// Primary video input.
    pub input: InputSnapshot,
    /// Secondary video input.
    pub second_input: InputSnapshot,
    /// Audio analysis state.
    pub audio: AudioSnapshot,
    /// Color / HSB state.
    pub color: ColorSnapshot,
    /// MIDI control state.
    pub midi: MidiSnapshot,
    /// OSC server state.
    pub osc: OscSnapshot,
    /// Preset bank state.
    pub presets: PresetSnapshot,
    /// Web remote state.
    pub web: WebSnapshot,
    /// Ableton Link state.
    pub link: LinkSnapshot,
    /// ProDJ Link state.
    pub prodj: ProDjSnapshot,
    /// MIDI Timecode state.
    pub mtc: MtcSnapshot,
    /// Rendering resolution.
    pub resolution: ResolutionSnapshot,
    /// Frame-time metrics.
    pub performance: PerformanceSnapshot,
    /// Effect-declared parameters and their current values.
    pub params: ParamsSnapshot,
    /// LFO / audio-routing modulation state.
    pub modulation: ModulationSnapshot,
    /// Target render FPS.
    pub target_fps: u32,
    /// Currently selected GUI tab.
    pub current_tab: String,
    /// Opaque JSON snapshot the active app published (or null). Included here
    /// so the WebSocket delta stream carries app state changes.
    pub app_state: serde_json::Value,
}

/// Output subsystem snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OutputSnapshot {
    /// Fullscreen flag.
    pub fullscreen: bool,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
}

/// Video input snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct InputSnapshot {
    /// Active input source type name.
    pub input_type: String,
    /// Source name or identifier.
    pub source_name: String,
    /// Whether the input is streaming.
    pub is_active: bool,
    /// Capture width.
    pub width: u32,
    /// Capture height.
    pub height: u32,
    /// Capture FPS.
    pub fps: f32,
    /// Webcam device index, if applicable.
    pub device_index: Option<usize>,
    /// Discovered devices.
    pub available_devices: Vec<rustjay_core::InputDeviceInfo>,
}

/// Audio analysis snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct AudioSnapshot {
    /// Per-band FFT magnitudes (8 bands, 0–1).
    pub fft: [f32; 8],
    /// Overall volume (0–1).
    pub volume: f32,
    /// Beat detected this frame.
    pub beat: bool,
    /// Estimated BPM.
    pub bpm: f32,
    /// Beat phase (0–1).
    pub beat_phase: f32,
    /// Audio analysis active.
    pub enabled: bool,
    /// Input gain.
    pub amplitude: f32,
    /// Smoothing factor.
    pub smoothing: f32,
    /// Selected device name.
    pub selected_device: Option<String>,
    /// Discovered device names.
    pub available_devices: Vec<String>,
    /// Normalisation enabled.
    pub normalize: bool,
    /// Pink-noise shaping enabled.
    pub pink_noise_shaping: bool,
    /// FFT window size.
    pub fft_size: usize,
    /// Tap-tempo feedback.
    pub tap_tempo_info: String,
}

/// Color / HSB snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ColorSnapshot {
    /// Hue shift (-180 to 180).
    pub hue_shift: f32,
    /// Saturation multiplier (0–2).
    pub saturation: f32,
    /// Brightness multiplier (0–2).
    pub brightness: f32,
    /// Color adjustment enabled.
    pub enabled: bool,
}

/// MIDI snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MidiSnapshot {
    /// MIDI device connected.
    pub enabled: bool,
    /// Selected device name.
    pub selected_device: Option<String>,
    /// Discovered device names.
    pub available_devices: Vec<String>,
    /// CC-learn active.
    pub learn_active: bool,
    /// Parameter being learned.
    pub learning_param_name: Option<String>,
    /// Active mappings.
    pub mappings: Vec<rustjay_core::MidiMappingSnapshot>,
}

/// OSC snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OscSnapshot {
    /// OSC server running.
    pub enabled: bool,
    /// Listen host.
    pub host: String,
    /// Listen port.
    pub port: u16,
    /// Recent message log (address, value, timestamp).
    pub message_log: Vec<(String, f32, f64)>,
}

/// Preset bank snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PresetSnapshot {
    /// Saved preset names.
    pub names: Vec<String>,
    /// Quick-slot names (1–8).
    pub quick_slot_names: Vec<Option<String>>,
}

/// Web remote snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WebSnapshot {
    /// Server running.
    pub enabled: bool,
    /// Listen host.
    pub host: String,
    /// Listen port.
    pub port: u16,
    /// App name path.
    pub app_name: String,
    /// LAN trust mode.
    pub lan_trust: bool,
}

/// Ableton Link snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct LinkSnapshot {
    /// Link enabled.
    pub enabled: bool,
    /// Peers in session.
    pub num_peers: usize,
    /// Session BPM.
    pub bpm: f32,
    /// Beat phase (0–1).
    pub beat_phase: f32,
    /// Musical quantum.
    pub quantum: f64,
    /// Session playing.
    pub is_playing: bool,
}

/// ProDJ Link snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProDjSnapshot {
    /// Discovery active.
    pub enabled: bool,
    /// Discovered devices.
    pub devices: Vec<ProDjDeviceSnapshot>,
    /// Master BPM.
    pub master_bpm: f32,
    /// Master beat phase.
    pub master_beat_phase: f32,
    /// Current track artist.
    pub current_track_artist: String,
    /// Current track title.
    pub current_track_title: String,
}

/// Single CDJ device snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProDjDeviceSnapshot {
    /// Device ID.
    pub device_id: u32,
    /// Device name.
    pub name: String,
    /// Currently playing.
    pub is_playing: bool,
    /// Tempo master.
    pub is_master: bool,
    /// BPM.
    pub bpm: Option<f32>,
}

/// MTC snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MtcSnapshot {
    /// MTC messages received.
    pub running: bool,
    /// Quarter-frames arriving.
    pub playing: bool,
    /// Timecode position.
    pub position: String,
    /// Source MIDI port.
    pub source_device: String,
}

/// Resolution snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ResolutionSnapshot {
    /// Internal render width.
    pub internal_width: u32,
    /// Internal render height.
    pub internal_height: u32,
    /// Input texture width.
    pub input_width: u32,
    /// Input texture height.
    pub input_height: u32,
}

/// Performance snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PerformanceSnapshot {
    /// Current FPS.
    pub fps: f32,
    /// Average frame time ms.
    pub frame_time_ms: f32,
}

/// Parameter snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ParamsSnapshot {
    /// Parameter descriptors.
    pub descriptors: Vec<rustjay_core::ParameterDescriptor>,
    /// Base (unmodulated) values.
    pub bases: Vec<f32>,
    /// Modulated values.
    pub values: Vec<f32>,
}

/// Modulation snapshot.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ModulationSnapshot {
    /// LFO bank state.
    pub lfos: Vec<rustjay_core::lfo::Lfo>,
    /// Audio routing matrix routes.
    pub audio_routes: Vec<rustjay_core::routing::AudioRoute>,
    /// Audio routing enabled.
    pub audio_routing_enabled: bool,
    /// Current BPM.
    pub bpm: f32,
    /// Tap-tempo feedback.
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
        performance: PerformanceSnapshot {
            fps: state
                .performance
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .fps,
            frame_time_ms: state
                .performance
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .frame_time_ms,
        },
        params: ParamsSnapshot {
            descriptors: (*state.param_descriptors).clone(),
            bases: state.custom_param_bases.clone(),
            values: state.custom_params.clone(),
        },
        modulation: ModulationSnapshot {
            lfos: state.lfo.bank.lfos.clone(),
            audio_routes: state.audio_routing.matrix.routes().to_vec(),
            audio_routing_enabled: state.audio_routing.enabled,
            bpm: state.audio.bpm,
            tap_tempo_info: state.audio.tap_tempo_info.clone(),
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
