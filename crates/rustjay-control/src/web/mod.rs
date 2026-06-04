//! # Web Remote Control Interface
//!
//! WebSocket-based web interface for remote control from phones/tablets.
//! URL: http://[computer-ip]:[port]/[app_name]

// The wire-protocol command/response structs below are self-describing by field name.
#![allow(missing_docs)]
// Router/handler builders thread many params by design.
#![allow(clippy::too_many_arguments)]

use axum::{
    extract::{ws::{WebSocket, Message}, State, WebSocketUpgrade, Query, Json},
    response::IntoResponse,
    routing::{get, post},
    Router, middleware::{self, Next},
    http::{Request, StatusCode, HeaderMap},
    body::Body,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

/// Commands for web server lifecycle control
#[allow(dead_code)] // superseded by `rustjay_core::WebCommand`; kept as the control-layer descriptor
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebControlCommand {
    None,
    Start,
    Stop,
    SetPort(u16),
}

/// Web server configuration
#[derive(Debug, Clone)]
pub struct WebConfig {
    /// Host to bind to (default: 0.0.0.0 — all interfaces)
    pub host: String,
    /// Port to listen on
    pub port: u16,
    /// App name for URL path (e.g., "rustjay")
    pub app_name: String,
    /// Whether server is running
    pub enabled: bool,
    /// When true, clients on the same LAN subnet connect without a token.
    pub lan_trust: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8081,
            app_name: "rustjay-template".to_string(),
            enabled: false,
            lan_trust: false,
        }
    }
}

/// Parameter definition for web UI
#[derive(Debug, Clone, serde::Serialize)]
pub struct WebParameter {
    pub id: String,
    pub name: String,
    pub category: String,
    pub min: f32,
    pub max: f32,
    pub value: f32,
    pub default: f32,
    pub step: f32,
    pub options: Option<Vec<String>>,
}

/// Web server state shared between handlers
/// Shared state for the web server.
pub struct WebServerState {
    /// Server configuration.
    pub config: WebConfig,
    /// All available parameters.
    pub parameters: HashMap<String, WebParameter>,
    /// Channel for broadcasting updates to all connected clients.
    pub broadcast_tx: broadcast::Sender<WebMessage>,
    /// Channel for receiving updates from clients.
    pub command_tx: tokio::sync::mpsc::Sender<WebCommand>,
    /// Per-launch bearer token for auth.
    pub bearer_token: String,
    /// When true, clients on the same LAN subnet skip token auth.
    pub lan_trust: bool,
    /// Preset names for initial broadcast on new WebSocket connect.
    pub preset_names: Vec<String>,
    /// Pending device enumeration result from async Tokio task.
    pub pending_devices: std::sync::Arc<std::sync::Mutex<Option<Vec<rustjay_core::InputDeviceInfo>>>>,
    /// Last time a device refresh was requested (for throttling).
    pub last_refresh: std::time::Instant,
    /// Last-sent input state for hydrating new WebSocket clients immediately.
    pub last_input_state: Option<InputStateJson>,
    /// Last-sent control state for hydrating new WebSocket clients immediately.
    pub last_control_state: Option<ControlStateJson>,
    /// Last-sent modulation state for hydrating new WebSocket clients immediately.
    pub last_modulation_state: Option<ModulationStateJson>,
    /// Active WebSocket connection count (for capping).
    pub active_ws_count: AtomicUsize,
    /// Engine state for API snapshot reads (set by the composition root).
    pub engine_state: Option<Arc<std::sync::Mutex<rustjay_core::EngineState>>>,
}

/// Messages sent from server to web clients
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum WebMessage {
    #[serde(rename = "params")]
    Params { params: Vec<WebParameter> },
    #[serde(rename = "update")]
    Update { id: String, value: f32 },
    #[serde(rename = "connected")]
    Connected { client_count: usize },
    #[serde(rename = "input_state")]
    InputState(InputStateJson),
    #[serde(rename = "control_state")]
    ControlState(ControlStateJson),
    #[serde(rename = "modulation_state")]
    ModulationState(ModulationStateJson),
    #[serde(rename = "preset_state")]
    PresetState(PresetStateJson),
}

/// Input subsystem commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "action")]
pub enum InputWebCommand {
    #[serde(rename = "refresh_devices")]
    RefreshDevices,
    #[serde(rename = "select_device")]
    SelectDevice { index: usize, width: u32, height: u32, fps: u32 },
    #[serde(rename = "stop")]
    StopInput,
}

/// MIDI / OSC subsystem commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "action")]
pub enum ControlWebCommand {
    #[serde(rename = "osc")]
    Osc { enabled: bool },
    #[serde(rename = "osc_set_port")]
    OscSetPort { port: u16 },
    #[serde(rename = "midi_learn")]
    MidiLearn { param_id: String },
    #[serde(rename = "midi_learn_cancel")]
    MidiLearnCancel,
    #[serde(rename = "midi_unlearn")]
    MidiUnlearn { cc: u8, channel: u8 },
    #[serde(rename = "midi_refresh_devices")]
    MidiRefreshDevices,
    #[serde(rename = "midi_select_device")]
    MidiSelectDevice { device: String },
    #[serde(rename = "midi_disconnect")]
    MidiDisconnect,
}

/// LFO / audio-routing subsystem commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "action")]
pub enum ModulationWebCommand {
    #[serde(rename = "lfo_set")]
    LfoSet { slot: usize, config: rustjay_core::lfo::Lfo },
    #[serde(rename = "lfo_enable")]
    LfoEnable { slot: usize, enabled: bool },
    #[serde(rename = "audio_route")]
    AudioRoute { param_id: String, band: rustjay_core::FftBand, depth: f32 },
    #[serde(rename = "audio_unroute")]
    AudioUnroute { param_id: String },
    #[serde(rename = "tap_tempo")]
    TapTempo,
}

/// Preset commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "action")]
pub enum PresetWebCommand {
    #[serde(rename = "list")]
    List,
    #[serde(rename = "save")]
    Save { name: String },
    #[serde(rename = "load")]
    Load { index: usize },
    #[serde(rename = "delete")]
    Delete { index: usize },
}

/// Commands received from web clients
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "type")]
/// Commands received from web clients.
pub enum WebCommand {
    /// Set a parameter value.
    #[serde(rename = "set")]
    Set {
        /// Parameter identifier.
        id: String,
        /// New value.
        value: f32,
    },
    #[serde(rename = "input")]
    Input(InputWebCommand),
    #[serde(rename = "output")]
    Output(OutputWebCommand),
    #[serde(rename = "audio")]
    Audio(AudioWebCommand),
    #[serde(rename = "control")]
    Control(ControlWebCommand),
    #[serde(rename = "modulation")]
    Modulation(ModulationWebCommand),
    #[serde(rename = "preset")]
    Preset(PresetWebCommand),
    #[serde(rename = "link")]
    Link(LinkWebCommand),
    #[serde(rename = "prodj")]
    ProDj(ProDjWebCommand),
}

/// Output subsystem commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
pub enum OutputWebCommand {
    #[serde(rename = "start_ndi")]
    StartNdi,
    #[serde(rename = "stop_ndi")]
    StopNdi,
    #[serde(rename = "start_syphon")]
    StartSyphon,
    #[serde(rename = "stop_syphon")]
    StopSyphon,
    #[serde(rename = "start_spout")]
    StartSpout { sender_name: String },
    #[serde(rename = "stop_spout")]
    StopSpout,
    #[serde(rename = "start_v4l2")]
    StartV4l2 { device_path: String },
    #[serde(rename = "stop_v4l2")]
    StopV4l2,
    #[serde(rename = "resize_output")]
    ResizeOutput,
}

/// Audio subsystem commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
pub enum AudioWebCommand {
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "stop")]
    Stop,
    #[serde(rename = "refresh_devices")]
    RefreshDevices,
    #[serde(rename = "select_device")]
    SelectDevice { device: String },
    #[serde(rename = "set_fft_size")]
    SetFftSize { size: usize },
}

/// Ableton Link commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
pub enum LinkWebCommand {
    #[serde(rename = "enable")]
    Enable,
    #[serde(rename = "disable")]
    Disable,
    #[serde(rename = "set_quantum")]
    SetQuantum { quantum: f64 },
}

/// ProDJ Link commands from web clients.
#[derive(Debug, Clone, serde::Deserialize)]
pub enum ProDjWebCommand {
    #[serde(rename = "start")]
    Start,
    #[serde(rename = "stop")]
    Stop,
}

/// JSON payload for InputState broadcast.
#[derive(Debug, Clone, serde::Serialize)]
pub struct InputStateJson {
    pub devices: Vec<rustjay_core::InputDeviceInfo>,
    pub active_index: Option<usize>,
    pub active_name: String,
    pub width: u32,
    pub height: u32,
    pub fps: f32,
}

/// JSON payload for ControlState broadcast.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ControlStateJson {
    pub osc_enabled: bool,
    pub osc_port: u16,
    pub midi_enabled: bool,
    pub midi_selected_device: Option<String>,
    pub midi_devices: Vec<String>,
    pub midi_mappings: Vec<rustjay_core::MidiMappingSnapshot>,
    pub midi_learn_active: bool,
    pub midi_learning_param_name: Option<String>,
}

/// JSON payload for ModulationState broadcast.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModulationStateJson {
    pub lfos: Vec<rustjay_core::lfo::Lfo>,
    pub audio_routes: Vec<rustjay_core::routing::AudioRoute>,
    pub audio_routing_enabled: bool,
    pub bpm: f32,
    pub tap_tempo_info: String,
}

/// Single preset info for web clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetInfo {
    pub index: usize,
    pub name: String,
}

/// JSON payload for PresetState broadcast.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PresetStateJson {
    pub presets: Vec<PresetInfo>,
}

/// Web server handle.
pub struct WebServer {
    /// Shared server state.
    pub state: Arc<Mutex<WebServerState>>,
    /// Channel receiving commands from web clients.
    pub command_rx: tokio::sync::mpsc::Receiver<WebCommand>,
    handle: Option<std::thread::JoinHandle<()>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    /// Last value broadcast for each parameter.  Used for diff-tracking so
    /// unchanged parameters skip the mutex entirely.
    last_sent: HashMap<String, f32>,
    /// Dirty flags for structural state broadcasts.
    pub input_dirty: bool,
    pub control_dirty: bool,
    pub modulation_dirty: bool,
    pub preset_dirty: bool,
    /// Optional additional router (e.g. REST/OpenAPI) merged under the same
    /// auth layer and listener.
    api_router: Option<Router<Arc<Mutex<WebServerState>>>>,
}

impl WebServer {
    /// Create a new web server and its command channel.
    pub fn new(config: WebConfig) -> (Self, tokio::sync::mpsc::Sender<WebCommand>) {
        let (broadcast_tx, _) = broadcast::channel(100);
        let (command_tx, command_rx) = tokio::sync::mpsc::channel(100);

        let bearer_token = generate_token();
        let lan_trust = config.lan_trust;

        let pending_devices = std::sync::Arc::new(std::sync::Mutex::new(None));

        let state = Arc::new(Mutex::new(WebServerState {
            config,
            parameters: HashMap::new(),
            broadcast_tx,
            command_tx: command_tx.clone(),
            bearer_token,
            lan_trust,
            preset_names: Vec::new(),
            pending_devices,
            last_refresh: std::time::Instant::now() - std::time::Duration::from_secs(10),
            last_input_state: None,
            last_control_state: None,
            last_modulation_state: None,
            active_ws_count: AtomicUsize::new(0),
            engine_state: None,
        }));

        let server = Self {
            state,
            command_rx,
            handle: None,
            shutdown_tx: None,
            last_sent: HashMap::new(),
            input_dirty: false,
            control_dirty: false,
            modulation_dirty: false,
            preset_dirty: false,
            api_router: None,
        };

        (server, command_tx)
    }

    /// Register a parameter for the web UI
    pub fn register_parameter(&mut self, id: &str, name: &str, category: &str, min: f32, max: f32, value: f32, step: f32) {
        // Clear stale diff-tracking entry so the initial broadcast is never skipped.
        self.last_sent.remove(id);
        if let Ok(mut state) = self.state.lock() {
            state.parameters.insert(id.to_string(), WebParameter {
                id: id.to_string(),
                name: name.to_string(),
                category: category.to_string(),
                min,
                max,
                value,
                default: value,
                step,
                options: None,
            });
        }
    }

    /// Register an enum parameter for the web UI (rendered as a select/dropdown)
    pub fn register_enum_parameter(&mut self, id: &str, name: &str, category: &str, options: Vec<String>, value: f32) {
        self.last_sent.remove(id);
        if let Ok(mut state) = self.state.lock() {
            state.parameters.insert(id.to_string(), WebParameter {
                id: id.to_string(),
                name: name.to_string(),
                category: category.to_string(),
                min: 0.0,
                max: (options.len() as f32) - 1.0,
                value,
                default: value,
                step: 1.0,
                options: Some(options),
            });
        }
    }

    /// Register default parameters (color, audio, etc.)
    pub fn register_default_parameters(&mut self) {
        // Color parameters
        self.register_parameter("color/hue_shift", "Hue Shift", "Color", -180.0, 180.0, 0.0, 1.0);
        self.register_parameter("color/saturation", "Saturation", "Color", 0.0, 2.0, 1.0, 0.01);
        self.register_parameter("color/brightness", "Brightness", "Color", 0.0, 2.0, 1.0, 0.01);
        self.register_parameter("color/enabled", "Color Enabled", "Color", 0.0, 1.0, 1.0, 1.0);

        // Audio parameters
        self.register_parameter("audio/amplitude", "Amplitude", "Audio", 0.0, 5.0, 1.0, 0.01);
        self.register_parameter("audio/smoothing", "Smoothing", "Audio", 0.0, 1.0, 0.5, 0.01);
        self.register_parameter("audio/enabled", "Audio Enabled", "Audio", 0.0, 1.0, 1.0, 1.0);
        self.register_parameter("audio/normalize", "Normalize", "Audio", 0.0, 1.0, 1.0, 1.0);
        self.register_parameter("audio/pink_noise", "Pink Noise", "Audio", 0.0, 1.0, 0.0, 1.0);

        // Output parameters
        self.register_parameter("output/fullscreen", "Fullscreen", "Output", 0.0, 1.0, 0.0, 1.0);
    }

    /// Register effect-declared parameters dynamically.
    pub fn register_parameters(&mut self, descriptors: &[rustjay_core::ParameterDescriptor]) {
        for d in descriptors {
            let category = d.category.name();
            let id = format!("{}/{}", category.to_lowercase(), d.id);
            match &d.param_type {
                rustjay_core::ParamType::Enum { variants } => {
                    self.register_enum_parameter(&id, &d.name, &category, variants.clone(), d.default);
                }
                _ => {
                    self.register_parameter(&id, &d.name, &category, d.min, d.max, d.default, d.step);
                }
            }
        }
    }

    /// Merge an external router (e.g. REST/OpenAPI) into the protected router
    /// so it shares the same auth layer and listener.
    pub fn set_api_router(&mut self, router: Router<Arc<Mutex<WebServerState>>>) {
        self.api_router = Some(router);
    }

    /// Provide the engine state for API snapshot reads.
    pub fn set_engine_state(&mut self, engine_state: Arc<std::sync::Mutex<rustjay_core::EngineState>>) {
        if let Ok(mut state) = self.state.lock() {
            state.engine_state = Some(engine_state);
        }
    }

    /// Update a parameter value and broadcast to all clients.
    ///
    /// Uses a fast-path `last_sent` cache so unchanged values skip the
    /// state mutex entirely — this removes ~N mutex acquisitions per frame
    /// where N is the number of registered parameters.
    pub fn update_parameter(&mut self, id: &str, value: f32) {
        const THRESHOLD: f32 = 0.001;

        // NaN/inf would loop forever (abs diff always false); reject at boundary.
        if !value.is_finite() {
            return;
        }

        // Fast path: if we already sent this value, do nothing.
        if let Some(&last) = self.last_sent.get(id) {
            if (value - last).abs() < THRESHOLD {
                return;
            }
        }

        let mut should_broadcast = false;

        if let Ok(mut state) = self.state.lock() {
            if let Some(param) = state.parameters.get_mut(id) {
                // Only update if changed
                if (param.value - value).abs() > 0.0001 {
                    param.value = value;
                    should_broadcast = true;
                }
            }
        }

        if should_broadcast {
            self.last_sent.insert(id.to_string(), value);
            if let Ok(state) = self.state.lock() {
                let _ = state.broadcast_tx.send(WebMessage::Update {
                    id: id.to_string(),
                    value,
                });
            }
        }
    }

    /// Start the web server (creates its own tokio runtime)
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.handle.is_some() {
            return Ok(()); // Already running
        }

        let state = Arc::clone(&self.state);
        let (port, app_name, host, token) = {
            let s = state.lock().unwrap_or_else(|e| e.into_inner());
            (s.config.port, s.config.app_name.clone(), s.config.host.clone(), s.bearer_token.clone())
        };

        let api_router = self.api_router.take();

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        self.shutdown_tx = Some(shutdown_tx);

        let handle = std::thread::spawn(move || {
            // Create a new tokio runtime for this thread
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("Failed to create tokio runtime: {}", e);
                    return;
                }
            };

            rt.block_on(async move {
                let app = create_router(state, &app_name, &token, api_router);

                let addr: SocketAddr = match format!("{}:{}", host, port).parse() {
                    Ok(a) => a,
                    Err(e) => {
                        log::error!("Invalid web server bind address {}:{}: {}", host, port, e);
                        return;
                    }
                };

                let listener = match tokio::net::TcpListener::bind(addr).await {
                    Ok(l) => {
                        log::info!("Web server bound to {}", addr);
                        l
                    }
                    Err(e) => {
                        log::error!("Failed to bind web server to {}: {}", addr, e);
                        return;
                    }
                };

                let local_ip = get_local_ip().unwrap_or_else(|| "localhost".to_string());
                eprintln!("Web control token: {token}");
                log::info!("Web server ready:");
                log::info!("  Local:   http://127.0.0.1:{}/{}", port, app_name);
                if host != "127.0.0.1" && host != "localhost" {
                    log::info!("  Network: http://{}:{}/{}", local_ip, port, app_name);
                }

                // Run server with graceful shutdown
                let server = axum::serve(listener, app);

                tokio::select! {
                    result = server => {
                        if let Err(e) = result {
                            log::error!("Web server error: {}", e);
                        }
                    }
                    _ = shutdown_rx => {
                        log::info!("Web server received shutdown signal");
                    }
                }
            });
        });

        self.handle = Some(handle);

        // Update config
        if let Ok(mut state) = self.state.lock() {
            state.config.enabled = true;
        }

        Ok(())
    }

    /// Stop the web server
    pub fn stop(&mut self) {
        // Send shutdown signal
        if let Some(shutdown_tx) = self.shutdown_tx.take() {
            let _ = shutdown_tx.send(());
        }

        // Wait for thread to finish
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
            log::info!("Web server stopped");
        }

        // Update config
        if let Ok(mut state) = self.state.lock() {
            state.config.enabled = false;
        }
    }

    /// Check if server is running
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Get the server URL (no token)
    pub fn get_url(&self) -> String {
        if let Ok(state) = self.state.lock() {
            format!("http://{}:{}/{}",
                state.config.host,
                state.config.port,
                state.config.app_name
            )
        } else {
            String::new()
        }
    }

    /// Get the bearer token.
    pub fn get_token(&self) -> String {
        self.state.lock()
            .map(|s| s.bearer_token.clone())
            .unwrap_or_default()
    }

    /// Get the full access URL including the auth token, using the actual local IP.
    pub fn get_full_url(&self) -> String {
        if let Ok(state) = self.state.lock() {
            let ip = get_local_ip().unwrap_or_else(|| "localhost".to_string());
            format!("http://{}:{}/{}?token={}",
                ip,
                state.config.port,
                state.config.app_name,
                state.bearer_token,
            )
        } else {
            String::new()
        }
    }

    /// Live-update the LAN trust setting without restarting the server.
    pub fn set_lan_trust(&self, enabled: bool) {
        if let Ok(mut state) = self.state.lock() {
            state.lan_trust = enabled;
        }
    }

    /// Broadcast input state to all connected clients.
    pub fn send_input_state(&self, state: &InputStateJson) {
        let msg = WebMessage::InputState(state.clone());
        if let Ok(mut s) = self.state.lock() {
            let _ = s.broadcast_tx.send(msg);
            s.last_input_state = Some(state.clone());
        }
    }

    /// Broadcast control state (OSC + MIDI) to all connected clients.
    pub fn send_control_state(&self, state: &ControlStateJson) {
        let msg = WebMessage::ControlState(state.clone());
        if let Ok(mut s) = self.state.lock() {
            let _ = s.broadcast_tx.send(msg);
            s.last_control_state = Some(state.clone());
        }
    }

    /// Broadcast modulation state (LFOs + audio routes) to all connected clients.
    pub fn send_modulation_state(&self, state: &ModulationStateJson) {
        let msg = WebMessage::ModulationState(state.clone());
        if let Ok(mut s) = self.state.lock() {
            let _ = s.broadcast_tx.send(msg);
            s.last_modulation_state = Some(state.clone());
        }
    }

    /// Broadcast preset state to all connected clients.
    /// Also updates `WebServerState::preset_names` so new WebSocket connections
    /// receive the current list immediately on connect.
    pub fn send_preset_state(&self, state: &PresetStateJson) {
        let msg = WebMessage::PresetState(state.clone());
        if let Ok(mut s) = self.state.lock() {
            let _ = s.broadcast_tx.send(msg);
            s.preset_names = state.presets.iter().map(|p| p.name.clone()).collect();
        }
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// POST /cmd handler — parses JSON WebCommand and forwards to the engine.
async fn cmd_handler(
    State(state): State<Arc<Mutex<WebServerState>>>,
    Json(cmd): Json<WebCommand>,
) -> impl IntoResponse {
    // Intercept RefreshDevices for async enumeration (WR-2.1)
    if let WebCommand::Input(InputWebCommand::RefreshDevices) = &cmd {
        let (should_spawn, pending) = {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            let now = std::time::Instant::now();
            if now.duration_since(s.last_refresh) < std::time::Duration::from_secs(5) {
                (false, None)
            } else {
                s.last_refresh = now;
                (true, Some(Arc::clone(&s.pending_devices)))
            }
        };
        if !should_spawn {
            return StatusCode::TOO_MANY_REQUESTS;
        }
        if let Some(pending) = pending {
            tokio::spawn(async move {
                match tokio::task::spawn_blocking(|| {
                    std::process::Command::new("v4l2-ctl")
                        .args(["--list-devices"])
                        .output()
                }).await {
                    Ok(Ok(output)) => {
                        let stdout = String::from_utf8_lossy(&output.stdout);
                        let devices = parse_v4l2_list_devices(&stdout);
                        if let Ok(mut guard) = pending.lock() {
                            *guard = Some(devices);
                        }
                    }
                    Ok(Err(e)) => {
                        log::warn!("v4l2-ctl --list-devices failed: {}", e);
                    }
                    Err(e) => {
                        log::warn!("v4l2-ctl task panicked or failed: {}", e);
                    }
                }
            });
        }
        return StatusCode::OK;
    }

    let tx = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        s.command_tx.clone()
    };
    match tx.try_send(cmd) {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Parse `v4l2-ctl --list-devices` stdout into [`InputDeviceInfo`] list.
fn parse_v4l2_list_devices(output: &str) -> Vec<rustjay_core::InputDeviceInfo> {
    let mut devices = Vec::new();
    let mut current_name: Option<String> = None;
    for line in output.lines() {
        if line.trim().is_empty() {
            current_name = None;
            continue;
        }
        if line.starts_with('\t') || line.starts_with("    ") {
            // Device path line
            let path = line.trim();
            if path.starts_with("/dev/video") {
                if let Some(ref name) = current_name {
                    // Extract index from /dev/videoN
                    let index = path.strip_prefix("/dev/video")
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(devices.len());
                    devices.push(rustjay_core::InputDeviceInfo {
                        name: name.clone(),
                        path: path.to_string(),
                        index,
                    });
                }
            }
        } else if line.ends_with(':') {
            // Device header line
            current_name = Some(line.trim_end_matches(':').trim().to_string());
        }
    }
    devices
}

/// Create the Axum router
fn create_router(
    state: Arc<Mutex<WebServerState>>,
    app_name: &str,
    token: &str,
    api_router: Option<Router<Arc<Mutex<WebServerState>>>>,
) -> Router {
    let ws_path = format!("/{}/ws", app_name);
    let page_path = format!("/{}", app_name);
    let page_path_slash = format!("/{}/", app_name);
    let page_path_redirect = page_path.clone();
    let page_path_redirect2 = page_path_redirect.clone();
    let cmd_path = format!("/{}/cmd", app_name);
    let input_path = format!("/{}/input", app_name);
    let control_path = format!("/{}/control", app_name);
    let modulation_path = format!("/{}/modulation", app_name);
    let presets_path = format!("/{}/presets", app_name);

    let html_with_token = inject_token_into_html(EMBEDDED_HTML, token, app_name);
    let input_html_with_token = inject_token_into_html(INPUT_HTML, token, app_name);
    let control_html_with_token = inject_token_into_html(CONTROL_HTML, token, app_name);
    let modulation_html_with_token = inject_token_into_html(MODULATION_HTML, token, app_name);
    let presets_html_with_token = inject_token_into_html(PRESETS_HTML, token, app_name);

    // Protected routes: auth required for everything except /health.
    // Auth middleware receives the shared state so it can read `lan_trust` live.
    let mut protected = Router::new()
        .route(&ws_path, get(ws_handler))
        .route(&page_path, get(move || async move {
            index_handler(&html_with_token).await
        }))
        .route(&page_path_slash, get(move || async move {
            axum::response::Redirect::permanent(&page_path_redirect)
        }))
        .route("/", get(move || async move {
            axum::response::Redirect::temporary(&page_path_redirect2)
        }))
        .route(&cmd_path, post(cmd_handler))
        .route(&input_path, get(move || async move {
            index_handler(&input_html_with_token).await
        }))
        .route(&control_path, get(move || async move {
            index_handler(&control_html_with_token).await
        }))
        .route(&modulation_path, get(move || async move {
            index_handler(&modulation_html_with_token).await
        }))
        .route(&presets_path, get(move || async move {
            index_handler(&presets_html_with_token).await
        }));

    // Merge the optional API router (REST/OpenAPI/Swagger) BEFORE applying the
    // auth layer. `route_layer` only wraps routes already present on the router,
    // so merging after it would leave every /api route and /swagger-ui
    // unauthenticated. Merge first, then layer the whole tree.
    if let Some(api) = api_router {
        protected = protected.merge(api);
    }

    let protected = protected.route_layer(middleware::from_fn_with_state(
        Arc::clone(&state),
        auth_middleware,
    ));

    Router::new()
        .route("/health", get(|| async { "OK" }))
        .merge(protected)
        .with_state(state)
}

/// Injects the bearer token and app name into the HTML.
fn inject_token_into_html(html: &str, token: &str, app_name: &str) -> String {
    let script = format!(
        r#"<script>window.RUSTJAY_TOKEN = "{}"; window.APP_NAME = "{}";</script>"#,
        token,
        app_name.to_uppercase()
    );
    let html = html.replacen("<head>", &format!("<head>{}", script), 1);
    html.replace("__APP__", &app_name.to_uppercase())
}

/// Response with proper content type for HTML
async fn index_handler(html: &str) -> impl IntoResponse {
    (
        [
            (axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (axum::http::header::CONNECTION, "keep-alive"),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; style-src 'unsafe-inline'; script-src 'self' 'unsafe-inline'; connect-src 'self' ws: wss: http: https:",
            ),
        ],
        html.to_string()
    )
}

/// Maximum concurrent WebSocket connections.
const MAX_WS_CONNECTIONS: usize = 32;
/// Maximum text frame size from a WebSocket client (bytes).
const MAX_WS_MSG_SIZE: usize = 64 * 1024;
/// Maximum client messages per second per connection.
const MAX_WS_MSG_RATE: u32 = 60;

/// Query parameters for WebSocket upgrade
#[derive(Debug, serde::Deserialize)]
struct WsQuery {
    token: String,
}

/// WebSocket handler
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<Mutex<WebServerState>>>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Verify bearer token from query param (browsers can't set custom headers on WebSocket).
    // Skip when lan_trust is enabled.
    let (valid_token, lan_trust) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        (s.bearer_token.clone(), s.lan_trust)
    };
    if !lan_trust && !constant_time_eq(&query.token, &valid_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    // Verify Origin header.
    // When lan_trust is enabled, any non-empty origin is allowed.
    // Otherwise, the origin must match the request's Host header.
    match headers.get(axum::http::header::ORIGIN) {
        Some(origin) => {
            let origin_str = origin.to_str().unwrap_or("");
            if origin_str.is_empty() {
                return StatusCode::FORBIDDEN.into_response();
            }
            if !lan_trust {
                let host = headers
                    .get(axum::http::header::HOST)
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("");
                let origin_host = origin_str.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
                if !origin_host.eq_ignore_ascii_case(host) {
                    return StatusCode::FORBIDDEN.into_response();
                }
            }
        }
        None => {
            return StatusCode::FORBIDDEN.into_response();
        }
    }

    // Atomically reserve a connection slot under the cap. The counter is only
    // ever read and written while holding the state lock, so this load+store is
    // a single atomic step with respect to other handshakes — closing the
    // check-then-act race where several upgrades could each observe a free slot
    // before any of them incremented. The returned guard owns the reservation
    // and releases it on drop, including if the upgrade future is dropped
    // before `handle_socket` runs.
    let conn_guard = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        let current = s.active_ws_count.load(Ordering::Relaxed);
        if current >= MAX_WS_CONNECTIONS {
            log::warn!("WS connection rejected: max connections ({}) reached", MAX_WS_CONNECTIONS);
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        s.active_ws_count.store(current + 1, Ordering::Relaxed);
        WsConnGuard(Arc::clone(&state))
    };

    ws.on_upgrade(move |socket| handle_socket(socket, state, conn_guard))
}

/// Drop guard that decrements the active WS counter when a connection (or its
/// pending upgrade) ends.
struct WsConnGuard(Arc<Mutex<WebServerState>>);
impl Drop for WsConnGuard {
    fn drop(&mut self) {
        let s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.active_ws_count.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Handle a WebSocket connection. `_conn_guard` owns the connection-cap slot
/// reserved in `ws_handler` and releases it when this future completes.
async fn handle_socket(mut socket: WebSocket, state: Arc<Mutex<WebServerState>>, _conn_guard: WsConnGuard) {
    // Rate-limit state
    let mut msg_count: u32 = 0;
    let mut last_window = std::time::Instant::now();

    // Get initial parameters and preset names in one lock acquisition
    let (params, preset_names) = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = state.parameters.values().cloned().collect::<Vec<_>>();
        p.sort_by(|a, b| a.id.cmp(&b.id));
        let pn = state.preset_names.clone();
        (p, pn)
    };

    // Send initial params list
    let init_msg = WebMessage::Params { params };
    if let Ok(json) = serde_json::to_string(&init_msg) {
        if socket.send(Message::Text(json.into())).await.is_err() {
            return;
        }
    }

    // Send initial preset state so a freshly-opened Presets panel populates immediately
    // without requiring the user to click Refresh (WR-9.6).
    if !preset_names.is_empty() {
        let preset_msg = WebMessage::PresetState(PresetStateJson {
            presets: preset_names.into_iter().enumerate()
                .map(|(i, name)| PresetInfo { index: i, name })
                .collect(),
        });
        if let Ok(json) = serde_json::to_string(&preset_msg) {
            if socket.send(Message::Text(json.into())).await.is_err() {
                return;
            }
        }
    }

    // Send cached structural states so panels populate immediately on connect.
    let (last_input, last_control, last_modulation) = {
        let state = state.lock().unwrap_or_else(|e| e.into_inner());
        (state.last_input_state.clone(), state.last_control_state.clone(), state.last_modulation_state.clone())
    };
    log::debug!("WS connect: sending cached states — input={}, control={}, modulation={}",
        last_input.is_some(), last_control.is_some(), last_modulation.is_some());
    if let Some(s) = last_input {
        match serde_json::to_string(&WebMessage::InputState(s)) {
            Ok(json) => { let _ = socket.send(Message::Text(json.into())).await; }
            Err(e) => log::warn!("WS input_state serialize failed: {}", e),
        }
    }
    if let Some(s) = last_control {
        match serde_json::to_string(&WebMessage::ControlState(s)) {
            Ok(json) => { let _ = socket.send(Message::Text(json.into())).await; }
            Err(e) => log::warn!("WS control_state serialize failed: {}", e),
        }
    }
    if let Some(s) = last_modulation {
        match serde_json::to_string(&WebMessage::ModulationState(s)) {
            Ok(json) => { let _ = socket.send(Message::Text(json.into())).await; }
            Err(e) => log::warn!("WS modulation_state serialize failed: {}", e),
        }
    }

    // Subscribe to broadcasts
    let mut rx = {
        state.lock().unwrap_or_else(|e| e.into_inner()).broadcast_tx.subscribe()
    };

    // Handle messages from client and broadcasts
    loop {
        tokio::select! {
            // Receive broadcast from server
            Ok(msg) = rx.recv() => {
                if let Ok(json) = serde_json::to_string(&msg) {
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break; // Client disconnected
                    }
                }
            }
            // Receive message from client
            Some(Ok(msg)) = socket.recv() => {
                if let Message::Text(text) = msg {
                    // Size limit
                    if text.len() > MAX_WS_MSG_SIZE {
                        log::warn!("WS message too large ({} bytes); dropping connection", text.len());
                        break;
                    }
                    // Rate limit
                    let now = std::time::Instant::now();
                    if now.duration_since(last_window).as_secs_f32() >= 1.0 {
                        last_window = now;
                        msg_count = 0;
                    }
                    msg_count += 1;
                    if msg_count > MAX_WS_MSG_RATE {
                        log::warn!("WS message rate exceeded; dropping connection");
                        break;
                    }
                    if let Ok(cmd) = serde_json::from_str::<WebCommand>(&text) {
                        // Update local parameter cache for Set commands
                        if let WebCommand::Set { ref id, value } = cmd {
                            let id = id.clone();
                            let mut should_broadcast = false;
                            if let Ok(mut state) = state.lock() {
                                if let Some(param) = state.parameters.get_mut(&id) {
                                    if (param.value - value).abs() > 0.0001 {
                                        param.value = value;
                                        should_broadcast = true;
                                    }
                                }
                            }
                            if should_broadcast {
                                if let Ok(state) = state.lock() {
                                    let _ = state.broadcast_tx.send(WebMessage::Update { id, value });
                                }
                            }
                        }
                        // Forward all commands to the engine command channel
                        if let Ok(state) = state.lock() {
                            let _ = state.command_tx.try_send(cmd);
                        }
                    }
                }
            }
            else => break,
        }
    }
}

/// Constant-time string comparison to mitigate timing attacks on the bearer token.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        result |= x ^ y;
    }
    result == 0
}

/// Extract the `token` value from a query string without doing a substring search
/// that would leak timing information.
fn token_from_query(req: &Request<Body>) -> Option<&str> {
    req.uri().query()?.split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next()?;
        if key == "token" {
            parts.next()
        } else {
            None
        }
    })
}

/// Bearer-token auth middleware.
/// Accepts the token via `Authorization: Bearer <token>` header or `?token=<token>` query param.
/// When `lan_trust` is enabled in server state, all requests pass through without a token.
async fn auth_middleware(
    State(state): State<Arc<Mutex<WebServerState>>>,
    req: Request<Body>,
    next: Next,
) -> impl IntoResponse {
    let (token, lan_trust) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        (s.bearer_token.clone(), s.lan_trust)
    };

    if lan_trust {
        return next.run(req).await;
    }

    let auth_header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok());

    let auth_ok = match auth_header {
        Some(header) => {
            let expected = format!("Bearer {}", token);
            constant_time_eq(header, &expected)
        }
        _ => {
            token_from_query(&req).is_some_and(|q| constant_time_eq(q, &token))
        }
    };

    if auth_ok {
        next.run(req).await
    } else {
        StatusCode::UNAUTHORIZED.into_response()
    }
}

/// Get local IP address
fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    // Try to connect to a public DNS server to determine local IP
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return Some(addr.ip().to_string());
            }
        }
    }
    None
}

/// Generate a random 16-byte hex token.
fn generate_token() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
        s
    })
}

/// Embedded HTML/JS/CSS for the web UI
const EMBEDDED_HTML: &str = include_str!("ui.html");
const INPUT_HTML: &str = include_str!("input.html");
const CONTROL_HTML: &str = include_str!("control.html");
const MODULATION_HTML: &str = include_str!("modulation.html");
const PRESETS_HTML: &str = include_str!("presets.html");

#[cfg(test)]
mod auth_merge_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower::util::ServiceExt;

    // Build the real create_router with a dummy API route merged in, exactly as
    // the engine wires rustjay-api. Verifies the merged /api routes inherit the
    // auth layer (regression guard for the route_layer-then-merge ordering).
    fn router_with_api() -> Router {
        let (server, _tx) = WebServer::new(WebConfig::default());
        {
            let mut s = server.state.lock().unwrap();
            s.bearer_token = "secret".to_string();
            s.lan_trust = false;
        }
        let api = Router::new().route("/api/health", get(|| async { "ok" }));
        create_router(Arc::clone(&server.state), "app", "secret", Some(api))
    }

    #[tokio::test]
    async fn merged_api_route_requires_auth() {
        let app = router_with_api();
        let resp = app
            .oneshot(Request::get("/api/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED,
            "merged /api route must be behind auth");
    }

    #[tokio::test]
    async fn merged_api_route_passes_with_token() {
        let app = router_with_api();
        let resp = app
            .oneshot(
                Request::get("/api/health")
                    .header("Authorization", "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
