//! Dual-window application handler implementing winit's ApplicationHandler.

use crate::config::{AppSettings, ConfigManager};
use rustjay_audio::AudioAnalyzer;
#[cfg(feature = "mtc")]
use rustjay_control::MtcReceiver;
use rustjay_control::OscServer;
use rustjay_control::{MidiManager, MidiMapping, MidiState};
use rustjay_control::{WebCommand as WebServerCommand, WebConfig, WebServer};
use rustjay_core::EffectPlugin;
use rustjay_core::EngineState;
#[cfg(feature = "egui")]
use rustjay_gui::{AnyEguiTab, EguiControlGui, EguiRenderer};
use rustjay_gui::{AnyGuiTab, ControlGui, ImGuiRenderer};
use rustjay_io::InputManager;
use rustjay_presets::{presets_dir_for, PresetBank};
use rustjay_render::WgpuEngine;

use anyhow::Result;
use std::sync::Arc;
use winit::event_loop::{ControlFlow, EventLoop};
use winit::window::Window;

#[derive(Clone, Copy, Debug)]
pub(crate) enum WindowAction {
    RecreateWindows,
}

#[cfg(target_os = "macos")]
mod macos;

#[cfg(feature = "projection")]
pub mod projection;

pub(crate) fn run_app<P: EffectPlugin>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
    nogui: bool,
) -> Result<()> {
    run_app_with_projection(shared_state, plugin, tabs, nogui, |_| {})
}

#[cfg(feature = "projection")]
pub(crate) fn run_app_with_projection<
    P: EffectPlugin,
    F: FnOnce(&mut projection::ProjectionSubsystem),
>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
    nogui: bool,
    projection_setup: F,
) -> Result<()> {
    let event_loop = EventLoop::<WindowAction>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    #[cfg(target_os = "macos")]
    let proxy = event_loop.create_proxy();

    let mut app = App::new(shared_state, plugin, false, tabs, nogui);

    #[cfg(target_os = "macos")]
    {
        macos::set_proxy(proxy);
        macos::setup_macos_app_delegate();
    }

    if let Some(sub) = app.projection_subsystem.as_ref() {
        let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
        projection_setup(&mut sub);
    }

    event_loop.run_app(&mut app)?;

    Ok(())
}

#[cfg(not(feature = "projection"))]
pub(crate) fn run_app_with_projection<P: EffectPlugin>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
    nogui: bool,
    _projection_setup: impl FnOnce(&mut ()),
) -> Result<()> {
    let event_loop = EventLoop::<WindowAction>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    #[cfg(target_os = "macos")]
    let proxy = event_loop.create_proxy();

    let mut app = App::new(shared_state, plugin, false, tabs, nogui);

    #[cfg(target_os = "macos")]
    {
        macos::set_proxy(proxy);
        macos::setup_macos_app_delegate();
    }

    event_loop.run_app(&mut app)?;

    Ok(())
}

#[cfg(feature = "egui")]
pub(crate) fn run_egui_app<P: EffectPlugin>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    tabs: Vec<Box<dyn AnyEguiTab>>,
) -> Result<()> {
    let event_loop = EventLoop::<WindowAction>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    #[cfg(target_os = "macos")]
    let proxy = event_loop.create_proxy();

    let mut app = App::new_with_egui(shared_state, plugin, tabs, false);

    #[cfg(target_os = "macos")]
    {
        macos::set_proxy(proxy);
        macos::setup_macos_app_delegate();
    }

    event_loop.run_app(&mut app)?;

    Ok(())
}

#[cfg(all(feature = "egui", feature = "projection"))]
pub(crate) fn run_egui_app_with_projection<
    P: EffectPlugin,
    F: FnOnce(&mut projection::ProjectionSubsystem),
>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    tabs: Vec<Box<dyn AnyEguiTab>>,
    nogui: bool,
    projection_setup: F,
) -> Result<()> {
    let event_loop = EventLoop::<WindowAction>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    #[cfg(target_os = "macos")]
    let proxy = event_loop.create_proxy();

    let mut app = App::new_with_egui(shared_state, plugin, tabs, nogui);

    #[cfg(target_os = "macos")]
    {
        macos::set_proxy(proxy);
        macos::setup_macos_app_delegate();
    }

    if let Some(sub) = app.projection_subsystem.as_ref() {
        let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
        projection_setup(&mut sub);
    }

    event_loop.run_app(&mut app)?;

    Ok(())
}

#[cfg(feature = "gles2")]
pub(crate) fn run_gles2_app<P: EffectPlugin>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    gles2: Box<dyn crate::gles2::Gles2EffectDyn>,
    drm_mode: bool,
) -> Result<()> {
    let event_loop = EventLoop::<WindowAction>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new(shared_state, plugin, false, vec![], true);
    app.gles2_effect = Some(gles2);
    #[cfg(feature = "drm-gles2")]
    {
        app.drm_gles2 = drm_mode;
    }

    event_loop.run_app(&mut app)?;
    Ok(())
}

pub(crate) struct App<P: EffectPlugin> {
    pub(crate) shared_state: Arc<std::sync::Mutex<EngineState>>,

    pub(crate) wgpu_instance: Option<wgpu::Instance>,
    pub(crate) wgpu_adapter: Option<wgpu::Adapter>,
    pub(crate) wgpu_device: Option<Arc<wgpu::Device>>,
    pub(crate) wgpu_queue: Option<Arc<wgpu::Queue>>,

    pub(crate) output_window: Option<Arc<Window>>,
    pub(crate) output_engine: Option<WgpuEngine<P>>,

    pub(crate) control_window: Option<Arc<Window>>,
    pub(crate) control_gui: Option<ControlGui>,
    pub(crate) imgui_renderer: Option<ImGuiRenderer>,

    #[cfg(feature = "egui")]
    pub(crate) egui_control_gui: Option<EguiControlGui>,
    #[cfg(feature = "egui")]
    pub(crate) egui_renderer: Option<EguiRenderer>,

    pub(crate) use_egui: bool,
    pub(crate) nogui: bool,

    pub(crate) input_manager: Option<InputManager>,
    pub(crate) second_input_manager: Option<InputManager>,
    pub(crate) audio_analyzer: Option<AudioAnalyzer>,
    pub(crate) midi_manager: Option<MidiManager>,
    pub(crate) osc_server: Option<OscServer>,
    pub(crate) preset_bank: Option<PresetBank>,
    pub(crate) web_server: Option<WebServer>,
    pub(crate) web_command_tx: Option<tokio::sync::mpsc::Sender<WebServerCommand>>,

    #[cfg(feature = "link")]
    pub(crate) link_manager: Option<rustjay_sync::LinkManager>,
    #[cfg(feature = "prodj")]
    pub(crate) prodj_manager: Option<rustjay_sync::ProDjManager>,
    #[cfg(feature = "mtc")]
    pub(crate) mtc_receiver: Option<MtcReceiver>,

    pub(crate) config_manager: ConfigManager,

    pub(crate) shift_pressed: bool,
    pub(crate) output_occluded: bool,
    pub(crate) control_visible: bool,
    pub(crate) last_frame_time: std::time::Instant,
    pub(crate) frame_delta_time: f32,
    /// Wall-clock start time for the modulation engine. Using a real Instant
    /// ensures LFO dt is based on actual elapsed time, not the clamped
    /// frame_delta_time accumulator (which runs fast under ControlFlow::Poll).
    pub(crate) modulation_start: std::time::Instant,

    /// Last time the control-window UI was rebuilt/rendered. The control UI is
    /// throttled to ~30 Hz independent of the output `target_fps` (perf: avoids
    /// per-frame imgui/egui buffer allocations). See `about_to_wait`.
    pub(crate) last_ui_render: std::time::Instant,
    /// Set when a control-window event arrives so the next frame rebuilds the UI
    /// immediately, keeping interaction (slider drags, tab clicks) responsive.
    pub(crate) ui_needs_redraw: bool,
    /// Last time device discovery was polled. Devices change on a human
    /// timescale, so this is throttled rather than run every frame.
    pub(crate) last_device_poll: std::time::Instant,
    /// Last time an audio stream reconnect was attempted. Reconnects are
    /// throttled to avoid hammering CoreAudio when a device is unavailable.
    pub(crate) last_audio_reconnect_attempt: Option<std::time::Instant>,

    /// Scratch buffer for dirty MIDI values — cleared and reused each frame to avoid HashMap allocation.
    pub(crate) midi_dirty_scratch: Vec<(String, f32)>,

    /// Cached audio analysis parameters — updated at end of each update_audio so the read
    /// at the top of the next frame can skip a shared_state lock acquisition.
    pub(crate) cached_audio_amplitude: f32,
    pub(crate) cached_audio_smoothing: f32,
    pub(crate) cached_audio_normalize: bool,
    pub(crate) cached_audio_pink_noise: bool,
    /// Reusable FFT scratch buffer (S1) — avoids per-frame allocation.
    pub(crate) cached_fft: Vec<f32>,
    /// Reusable spectrum scratch buffer (S4) — avoids per-frame allocation.
    pub(crate) cached_spectrum: Vec<f32>,
    /// Last-broadcast MIDI mapping snapshot for change detection (WR-3.3 / WR-6).
    pub(crate) last_broadcast_mappings: Vec<rustjay_core::MidiMappingSnapshot>,

    // Plugin state
    pub(crate) plugin: Option<P>,
    /// Cached `plugin.input_count()`, captured at construction. `plugin` is moved
    /// into the engine during `resumed()`, so the per-frame input path reads this
    /// instead of the (then-`None`) `plugin` field.
    pub(crate) plugin_input_count: u32,
    pub(crate) app_state: P::State,
    pub(crate) custom_tabs_imgui: Vec<Box<dyn AnyGuiTab>>,
    #[cfg(feature = "egui")]
    pub(crate) custom_tabs_egui: Vec<Box<dyn AnyEguiTab>>,

    // Optional GLES 2.0 render path (replaces WgpuEngine on hardware that lacks GLES 3.0)
    #[cfg(feature = "gles2")]
    pub(crate) gles2_effect: Option<Box<dyn crate::gles2::Gles2EffectDyn>>,
    #[cfg(feature = "gles2")]
    pub(crate) gles2_state: Option<crate::gles2::Gles2State>,
    /// When true, use DRM/GBM directly — skip window creation and weston entirely.
    #[cfg(feature = "drm-gles2")]
    pub(crate) drm_gles2: bool,

    /// Optional projection-mapping subsystem (extra projector windows + stage chains).
    #[cfg(feature = "projection")]
    pub(crate) projection_subsystem: Option<projection::ProjectionSubsystemHandle>,
}

impl<P: EffectPlugin> App<P> {
    pub(crate) fn new(
        shared_state: Arc<std::sync::Mutex<EngineState>>,
        plugin: P,
        use_egui: bool,
        tabs_imgui: Vec<Box<dyn AnyGuiTab>>,
        nogui: bool,
    ) -> Self {
        let app_name = plugin.app_name().to_string();
        let initial_state = plugin.default_state();
        let config_manager = ConfigManager::new(&app_name);
        if let Ok(mut state) = shared_state.lock() {
            config_manager.settings.apply_to_state(&mut state);
            log::info!("Applied saved settings to state");
        }

        if nogui {
            if let Ok(mut state) = shared_state.lock() {
                state.output_fullscreen = true;
                if state.target_fps > 30 {
                    state.target_fps = 30;
                }
            }
            log::info!("Headless mode: fullscreen output, target_fps capped at 30");
        }

        let mut analyzer = AudioAnalyzer::new();
        let (saved_fft_size, saved_device) = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.audio.fft_size, state.audio.selected_device.clone())
        };
        analyzer.set_fft_size(saved_fft_size);
        match analyzer.start_with_device(saved_device.as_deref()) {
            Ok(actual_name) => {
                let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.selected_device = Some(actual_name);
            }
            Err(e) => log::warn!("Failed to start audio analyzer: {}", e),
        }

        let mut midi_manager = {
            let midi_state = Arc::new(std::sync::Mutex::new(MidiState::default()));
            match MidiManager::new(midi_state) {
                Ok(mut manager) => {
                    let devices = manager.refresh_devices();
                    log::info!("MIDI manager initialized with {} devices", devices.len());
                    {
                        let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.midi_available_devices = devices;
                    }
                    Some(manager)
                }
                Err(e) => {
                    log::warn!("Failed to initialize MIDI manager: {}", e);
                    None
                }
            }
        };

        // Restore the saved MIDI device connection + learned mappings into the
        // *live* MidiState. `apply_to_state` only populated the EngineState
        // snapshot, so without this the device reads "connected" but no input
        // callback runs (needing a manual reconnect) and mappings are dropped.
        if let Some(ref mut manager) = midi_manager {
            let (saved_device, saved_mappings) = {
                let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (
                    state.midi_selected_device.clone(),
                    state.midi_mappings.clone(),
                )
            };
            if !saved_mappings.is_empty()
                && let Ok(mut ms) = manager.state().lock() {
                    ms.mappings = saved_mappings
                        .iter()
                        .map(|s| {
                            MidiMapping::new(
                                s.kind,
                                s.selector,
                                s.channel,
                                &s.name,
                                &s.param_path,
                                s.min_value,
                                s.max_value,
                            )
                        })
                        .collect();
                    log::info!("Restored {} saved MIDI mappings", ms.mappings.len());
                }
            let connected = match saved_device {
                Some(ref dev) => match manager.connect(dev) {
                    Ok(()) => true,
                    Err(e) => {
                        log::warn!("MIDI: could not reconnect saved device '{dev}': {e}");
                        false
                    }
                },
                None => false,
            };
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.midi_enabled = connected;
        }

        let descriptors = plugin.parameters();
        let hidden = plugin.hidden_tabs();
        {
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.param_descriptors = Arc::new(descriptors.clone());
            state.hidden_tabs = hidden;
            state.custom_param_bases.resize(descriptors.len(), 0.0);
            state.custom_params.resize(descriptors.len(), 0.0);
            for (i, d) in descriptors.iter().enumerate() {
                state.custom_param_bases[i] = d.default;
                state.custom_params[i] = d.default;
            }
            state.registered_param_ids = descriptors.iter().map(|d| d.id.clone()).collect();
            state.param_osc_addresses = descriptors
                .iter()
                .map(|d| format!("/rustjay/{}/{}", d.category.name().to_lowercase(), d.id))
                .collect();
        }

        let (osc_host, osc_port) = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.osc_host.clone(), state.osc_port)
        };
        let osc_server = {
            let server = OscServer::new(&osc_host, osc_port, "/rustjay");
            if let Ok(mut state) = server.state().lock() {
                state.register_default_parameters();
                state.register_parameters(&descriptors);
            }
            log::info!("OSC server initialized");
            Some(server)
        };

        let preset_bank = match presets_dir_for(&app_name) {
            Ok(presets_dir) => {
                log::info!("Preset bank initialized at {}", presets_dir.display());
                let mut bank = PresetBank::new(presets_dir);
                {
                    // Rebind quick slots from names saved in config (state.preset_quick_slot_names
                    // was populated by apply_to_state before this point). Silently drops slots
                    // whose preset was deleted since last run.
                    let saved = {
                        let s = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        s.preset_quick_slot_names.clone()
                    };
                    for (i, maybe_name) in saved.iter().enumerate() {
                        if let Some(name) = maybe_name
                            && let Some(idx) = bank.presets.iter().position(|p| &p.name == name) {
                                let _ = bank.assign_to_slot(idx, i + 1);
                            }
                    }
                    let names: Vec<String> = bank.presets.iter().map(|p| p.name.clone()).collect();
                    let slot_names: [Option<String>; 8] =
                        std::array::from_fn(|i| bank.get_slot_name(i + 1).map(|s| s.to_string()));
                    let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.preset_names = names;
                    state.preset_quick_slot_names = slot_names;
                }
                Some(bank)
            }
            Err(e) => {
                log::warn!("Failed to initialize preset bank: {}", e);
                None
            }
        };

        let (web_host, web_port, web_lan_trust, web_token) = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.web_host.clone(),
                state.web_port,
                state.web_lan_trust,
                state.web_token.clone(),
            )
        };
        let (web_server, web_command_tx) = {
            let config = WebConfig {
                host: web_host.clone(),
                port: web_port,
                app_name: app_name.clone(),
                enabled: false,
                lan_trust: web_lan_trust,
                token: if web_token.is_empty() {
                    None
                } else {
                    Some(web_token)
                },
            };
            let (mut server, cmd_tx) = WebServer::new(config);
            server.register_default_parameters();
            server.register_parameters(&descriptors);
            // Mount the optional REST/OpenAPI router under the same listener and
            // auth layer as the control web UI (no separate port or runtime).
            #[cfg(feature = "api")]
            {
                server.set_api_router(rustjay_api::build_router());
                server.set_engine_state(Arc::clone(&shared_state));
                log::info!("API routes mounted under /api on web port {}", web_port);
            }
            log::info!("Web server initialized on port {}", web_port);
            (Some(server), Some(cmd_tx))
        };
        {
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.web_app_name = app_name.clone();
        }

        #[cfg(feature = "projection")]
        let shared_state_for_projection = Arc::clone(&shared_state);

        Self {
            shared_state,
            wgpu_instance: None,
            wgpu_adapter: None,
            wgpu_device: None,
            wgpu_queue: None,
            output_window: None,
            output_engine: None,
            control_window: None,
            control_gui: None,
            imgui_renderer: None,
            #[cfg(feature = "egui")]
            egui_control_gui: None,
            #[cfg(feature = "egui")]
            egui_renderer: None,
            use_egui,
            nogui,
            input_manager: Some(InputManager::new()),
            second_input_manager: Some(InputManager::new()),
            audio_analyzer: Some(analyzer),
            midi_manager,
            osc_server,
            preset_bank,
            web_server,
            web_command_tx,
            config_manager,
            // Constructed lazily on enable (see update_link/update_prodj):
            // their background threads otherwise burn idle CPU — and ProDJ
            // would join the Pro DJ Link network — for features nobody enabled.
            #[cfg(feature = "link")]
            link_manager: None,
            #[cfg(feature = "prodj")]
            prodj_manager: None,
            #[cfg(feature = "mtc")]
            mtc_receiver: Some(MtcReceiver::new()),
            shift_pressed: false,
            output_occluded: false,
            control_visible: true,
            last_frame_time: std::time::Instant::now(),
            frame_delta_time: 1.0 / 60.0,
            modulation_start: std::time::Instant::now(),
            last_ui_render: std::time::Instant::now(),
            ui_needs_redraw: true,
            last_device_poll: std::time::Instant::now(),
            last_audio_reconnect_attempt: None,
            midi_dirty_scratch: Vec::new(),
            cached_audio_amplitude: 1.0,
            cached_audio_smoothing: 0.5,
            cached_audio_normalize: true,
            cached_audio_pink_noise: false,
            cached_fft: Vec::new(),
            cached_spectrum: Vec::new(),
            last_broadcast_mappings: Vec::new(),
            plugin_input_count: plugin.input_count(),
            plugin: Some(plugin),
            app_state: initial_state,
            custom_tabs_imgui: tabs_imgui,
            #[cfg(feature = "egui")]
            custom_tabs_egui: Vec::new(),
            #[cfg(feature = "gles2")]
            gles2_effect: None,
            #[cfg(feature = "gles2")]
            gles2_state: None,
            #[cfg(feature = "drm-gles2")]
            drm_gles2: false,
            #[cfg(feature = "projection")]
            projection_subsystem: {
                let sub = Arc::new(std::sync::Mutex::new(projection::ProjectionSubsystem::new()));
                if let Ok(mut state) = shared_state_for_projection.lock() {
                    state.projection_handle =
                        Some(Arc::clone(&sub) as Arc<std::sync::Mutex<dyn std::any::Any + Send>>);
                }
                Some(sub)
            },
        }
    }

    #[cfg(feature = "egui")]
    pub(crate) fn new_with_egui(
        shared_state: Arc<std::sync::Mutex<EngineState>>,
        plugin: P,
        tabs_egui: Vec<Box<dyn AnyEguiTab>>,
        nogui: bool,
    ) -> Self {
        let mut app = Self::new(shared_state, plugin, true, Vec::new(), nogui);
        app.custom_tabs_egui = tabs_egui;
        app
    }

    pub(crate) fn toggle_fullscreen(&mut self) {
        if let Some(ref output_window) = self.output_window {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.toggle_fullscreen();
            let fullscreen_mode = if state.output_fullscreen {
                Some(winit::window::Fullscreen::Borderless(None))
            } else {
                None
            };
            output_window.set_fullscreen(fullscreen_mode);
            output_window.set_cursor_visible(false);
            log::info!("Fullscreen: {}", state.output_fullscreen);
        }
    }

    pub(crate) fn trigger_tap_tempo(&mut self) {
        if self.use_egui {
            #[cfg(feature = "egui")]
            if let Some(ref mut gui) = self.egui_control_gui {
                gui.handle_tap_tempo();
            }
        } else if let Some(ref mut gui) = self.control_gui {
            gui.handle_tap_tempo();
        }
        log::info!("Tap tempo triggered via keyboard");
    }

    pub(crate) fn save_settings(&mut self) {
        if let Ok(state) = self.shared_state.lock() {
            self.config_manager.settings = AppSettings::from_state(&state);
        }
        match self.config_manager.save() { Err(e) => {
            log::error!("Failed to save settings: {}", e);
        } _ => {
            log::info!("Settings saved");
        }}
    }
}

mod commands;
mod events;
mod update;
