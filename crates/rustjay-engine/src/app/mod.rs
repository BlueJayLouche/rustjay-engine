//! Dual-window application handler implementing winit's ApplicationHandler.

use rustjay_audio::AudioAnalyzer;
use rustjay_control::{MidiManager, MidiState};
#[cfg(feature = "mtc")]
use rustjay_control::MtcReceiver;
use rustjay_control::OscServer;
use rustjay_control::{WebServer, WebConfig, WebCommand as WebServerCommand};
use rustjay_core::EngineState;
use rustjay_gui::{AnyGuiTab, ControlGui, ImGuiRenderer};
use rustjay_io::InputManager;
use rustjay_presets::{PresetBank, default_presets_dir};
use rustjay_render::WgpuEngine;
use crate::config::{AppSettings, ConfigManager};
use rustjay_core::EffectPlugin;

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

pub(crate) fn run_app<P: EffectPlugin>(
    shared_state: Arc<std::sync::Mutex<EngineState>>,
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
) -> Result<()> {
    let event_loop = EventLoop::<WindowAction>::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    #[cfg(target_os = "macos")]
    let proxy = event_loop.create_proxy();

    let mut app = App::new(shared_state, plugin, tabs);

    #[cfg(target_os = "macos")]
    {
        macos::set_proxy(proxy);
        macos::setup_macos_app_delegate();
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

    /// Scratch buffer for dirty MIDI values — cleared and reused each frame to avoid HashMap allocation.
    pub(crate) midi_dirty_scratch: Vec<(String, f32)>,

    /// Cached audio analysis parameters — updated at end of each update_audio so the read
    /// at the top of the next frame can skip a shared_state lock acquisition.
    pub(crate) cached_audio_amplitude: f32,
    pub(crate) cached_audio_smoothing: f32,
    pub(crate) cached_audio_normalize: bool,
    pub(crate) cached_audio_pink_noise: bool,

    // Plugin state
    pub(crate) plugin: Option<P>,
    pub(crate) app_state: P::State,
    pub(crate) custom_tabs: Vec<Box<dyn AnyGuiTab>>,
}

impl<P: EffectPlugin> App<P> {
    pub(crate) fn new(
        shared_state: Arc<std::sync::Mutex<EngineState>>,
        plugin: P,
        tabs: Vec<Box<dyn AnyGuiTab>>,
    ) -> Self {
        let app_name = plugin.app_name().to_string();
        let initial_state = plugin.default_state();
        let config_manager = ConfigManager::new(&app_name);
        if let Ok(mut state) = shared_state.lock() {
            config_manager.settings.apply_to_state(&mut state);
            log::info!("Applied saved settings to state");
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

        let midi_manager = {
            let midi_state = Arc::new(std::sync::Mutex::new(MidiState::default()));
            match MidiManager::new(midi_state) {
                Ok(mut manager) => {
                    manager.refresh_devices();
                    log::info!("MIDI manager initialized");
                    Some(manager)
                }
                Err(e) => {
                    log::warn!("Failed to initialize MIDI manager: {}", e);
                    None
                }
            }
        };

        // Initialize effect-declared parameters
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
            state.param_osc_addresses = descriptors.iter()
                .map(|d| format!("/{}/{}", d.category.name().to_lowercase(), d.id))
                .collect();
        }

        let osc_host = shared_state.lock().unwrap_or_else(|e| e.into_inner()).osc_host.clone();
        let osc_server = {
            let server = OscServer::new(&osc_host, 9000, "/rustjay");
            if let Ok(mut state) = server.state().lock() {
                state.register_default_parameters();
                state.register_parameters(&descriptors);
            }
            log::info!("OSC server initialized");
            Some(server)
        };

        let preset_bank = match default_presets_dir() {
            Ok(presets_dir) => {
                log::info!("Preset bank initialized");
                let bank = PresetBank::new(presets_dir);
                {
                    let names: Vec<String> = bank.presets.iter().map(|p| p.name.clone()).collect();
                    let slot_names: [Option<String>; 8] = std::array::from_fn(|i| {
                        bank.get_slot_name(i + 1).map(|s| s.to_string())
                    });
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

        let (web_host, web_port) = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.web_host.clone(), state.web_port)
        };
        let (web_server, web_command_tx) = {
            let config = WebConfig {
                host: web_host,
                port: web_port,
                app_name: app_name.clone(),
                enabled: false,
            };
            let (mut server, cmd_tx) = WebServer::new(config);
            server.register_default_parameters();
            server.register_parameters(&descriptors);
            log::info!("Web server initialized on port {}", web_port);
            (Some(server), Some(cmd_tx))
        };
        {
            let mut state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.web_app_name = app_name.clone();
        }

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
            input_manager: Some(InputManager::new()),
            second_input_manager: Some(InputManager::new()),
            audio_analyzer: Some(analyzer),
            midi_manager,
            osc_server,
            preset_bank,
            web_server,
            web_command_tx,
            config_manager,
            #[cfg(feature = "link")]
            link_manager: Some(rustjay_sync::LinkManager::new()),
            #[cfg(feature = "prodj")]
            prodj_manager: Some(rustjay_sync::ProDjManager::new()),
            #[cfg(feature = "mtc")]
            mtc_receiver: Some(MtcReceiver::new()),
            shift_pressed: false,
            output_occluded: false,
            control_visible: true,
            last_frame_time: std::time::Instant::now(),
            frame_delta_time: 1.0 / 60.0,
            midi_dirty_scratch: Vec::new(),
            cached_audio_amplitude: 1.0,
            cached_audio_smoothing: 0.5,
            cached_audio_normalize: true,
            cached_audio_pink_noise: false,
            plugin: Some(plugin),
            app_state: initial_state,
            custom_tabs: tabs,
        }
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
        if let Some(ref mut gui) = self.control_gui {
            gui.handle_tap_tempo();
            log::info!("Tap tempo triggered via keyboard");
        }
    }

    pub(crate) fn save_settings(&mut self) {
        if let Ok(state) = self.shared_state.lock() {
            self.config_manager.settings = AppSettings::from_state(&state);
        }
        if let Err(e) = self.config_manager.save() {
            log::error!("Failed to save settings: {}", e);
        } else {
            log::info!("Settings saved");
        }
    }
}

mod commands;
mod events;
mod update;
