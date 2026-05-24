//! # Control GUI
//!
//! Main ImGui interface for controlling the application.

#![allow(deprecated)]

use rustjay_core::{AudioCommand, GuiTab, InputCommand, EngineState, ParamCategory};
use rustjay_io::InputManager;
use std::sync::{Arc, Mutex};

use crate::AnyGuiTab;

/// Main control GUI
pub struct ControlGui {
    pub(crate) shared_state: Arc<Mutex<EngineState>>,

    // Device lists
    pub(crate) webcam_devices: Vec<String>,
    #[cfg(feature = "ndi")]
    pub(crate) ndi_sources: Vec<String>,
    #[cfg(target_os = "macos")]
    pub(crate) syphon_servers: Vec<rustjay_io::SyphonServerInfo>,
    pub(crate) audio_devices: Vec<String>,

    // Selection state
    pub(crate) selected_webcam: usize,
    #[cfg(feature = "ndi")]
    pub(crate) selected_ndi: usize,
    #[cfg(target_os = "macos")]
    pub(crate) selected_syphon: usize,
    pub(crate) selected_audio_device: usize,

    // NDI output name
    #[cfg(feature = "ndi")]
    pub(crate) ndi_output_name: String,

    // Syphon output name (macOS)
    #[cfg(target_os = "macos")]
    pub(crate) syphon_output_name: String,

    // Spout sender list and selection (Windows)
    #[cfg(target_os = "windows")]
    pub(crate) spout_senders: Vec<rustjay_io::SpoutSenderInfo>,
    #[cfg(target_os = "windows")]
    pub(crate) selected_spout: usize,
    #[cfg(target_os = "windows")]
    pub(crate) spout_output_name: String,

    // V4L2 devices (Linux)
    #[cfg(target_os = "linux")]
    pub(crate) v4l2_capture_devices: Vec<rustjay_io::V4l2DeviceInfo>,
    #[cfg(target_os = "linux")]
    pub(crate) v4l2_output_devices: Vec<rustjay_io::V4l2DeviceInfo>,
    #[cfg(target_os = "linux")]
    pub(crate) selected_v4l2_capture: usize,
    #[cfg(target_os = "linux")]
    pub(crate) selected_v4l2_output: usize,
    #[cfg(target_os = "linux")]
    pub(crate) v4l2_device_path: String,

    // Preview texture IDs
    /// ImGui texture ID for the input preview.
    pub input_preview_texture_id: Option<imgui::TextureId>,
    /// ImGui texture ID for the output preview.
    pub output_preview_texture_id: Option<imgui::TextureId>,

    // Pending resolution changes
    pub(crate) pending_internal_width: u32,
    pub(crate) pending_internal_height: u32,
    pub(crate) pending_output_width: u32,
    pub(crate) pending_output_height: u32,

    // Preset save inline form
    pub(crate) preset_name_buffer: String,
    pub(crate) saving_preset: bool,

    // Custom tabs provided by the active effect plugin
    /// Custom tabs provided by the active effect plugin.
    pub custom_tabs: Vec<Box<dyn AnyGuiTab>>,
    pub(crate) custom_tab_active: Option<usize>,

    // Settings window visibility (toggled via View > Preferences)
    pub(crate) show_settings: bool,
}

impl ControlGui {
    /// Create a new control GUI
    pub fn new(shared_state: Arc<Mutex<EngineState>>) -> anyhow::Result<Self> {
        let (ndi_name, syphon_name) = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            #[cfg(target_os = "macos")]
            let syphon = state.syphon_output.server_name.clone();
            #[cfg(not(target_os = "macos"))]
            let syphon = String::new();
            #[cfg(feature = "ndi")]
            let ndi = state.ndi_output.stream_name.clone();
            #[cfg(not(feature = "ndi"))]
            let ndi = String::new();
            (ndi, syphon)
        };

        // Initialize pending resolutions from current state
        let (internal_w, internal_h, output_w, output_h) = {
            let state = shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.resolution.internal_width,
                state.resolution.internal_height,
                state.output_width,
                state.output_height,
            )
        };

        Ok(Self {
            shared_state,
            webcam_devices: Vec::new(),
            #[cfg(feature = "ndi")]
            ndi_sources: Vec::new(),
            #[cfg(target_os = "macos")]
            syphon_servers: Vec::new(),
            audio_devices: Vec::new(),
            selected_webcam: 0,
            #[cfg(feature = "ndi")]
            selected_ndi: 0,
            #[cfg(target_os = "macos")]
            selected_syphon: 0,
            selected_audio_device: 0,
            #[cfg(feature = "ndi")]
            ndi_output_name: ndi_name,
            #[cfg(target_os = "macos")]
            syphon_output_name: syphon_name,
            #[cfg(target_os = "windows")]
            spout_senders: Vec::new(),
            #[cfg(target_os = "windows")]
            selected_spout: 0,
            #[cfg(target_os = "windows")]
            spout_output_name: "RustJay Template".to_string(),
            #[cfg(target_os = "linux")]
            v4l2_capture_devices: Vec::new(),
            #[cfg(target_os = "linux")]
            v4l2_output_devices: Vec::new(),
            #[cfg(target_os = "linux")]
            selected_v4l2_capture: 0,
            #[cfg(target_os = "linux")]
            selected_v4l2_output: 0,
            #[cfg(target_os = "linux")]
            v4l2_device_path: "/dev/video12".to_string(),
            input_preview_texture_id: None,
            output_preview_texture_id: None,
            pending_internal_width: internal_w,
            pending_internal_height: internal_h,
            pending_output_width: output_w,
            pending_output_height: output_h,
            preset_name_buffer: String::new(),
            saving_preset: false,
            custom_tabs: Vec::new(),
            custom_tab_active: None,
            show_settings: false,
        })
    }

    /// Set input preview texture ID
    pub fn set_input_preview_texture(&mut self, texture_id: imgui::TextureId) {
        self.input_preview_texture_id = Some(texture_id);
    }

    /// Set output preview texture ID
    pub fn set_output_preview_texture(&mut self, texture_id: imgui::TextureId) {
        self.output_preview_texture_id = Some(texture_id);
    }

    /// Update FPS counter (deprecated - FPS now tracked in output engine)
    #[allow(dead_code)]
    pub fn update_fps(&mut self) {
        // FPS is now tracked in WgpuEngine and stored in EngineState.performance
    }

    /// Sync GUI device lists from the current InputManager state.
    ///
    /// Call this after [`poll_device_discovery`](the engine App::poll_device_discovery)
    /// returns `true` (i.e. background discovery has finished).
    pub fn update_device_lists(&mut self, input_manager: &InputManager) {
        self.webcam_devices = input_manager.webcam_devices().to_vec();
        #[cfg(feature = "ndi")]
        {
            self.ndi_sources = input_manager.ndi_sources().to_vec();
        }
        #[cfg(target_os = "macos")]
        {
            self.syphon_servers = input_manager.syphon_servers().to_vec();
        }
        #[cfg(target_os = "windows")]
        {
            self.spout_senders = input_manager.spout_senders().to_vec();
        }
        #[cfg(target_os = "linux")]
        {
            self.v4l2_capture_devices = input_manager.v4l2_capture_devices().to_vec();
            self.v4l2_output_devices = input_manager.v4l2_output_devices().to_vec();
            // If the current v4l2_device_path matches a discovered output device,
            // align the combo selection with it.
            if let Some(pos) = self
                .v4l2_output_devices
                .iter()
                .position(|d| d.path == self.v4l2_device_path)
            {
                self.selected_v4l2_output = pos;
            } else if !self.v4l2_output_devices.is_empty() {
                self.selected_v4l2_output = 0;
                self.v4l2_device_path = self.v4l2_output_devices[0].path.clone();
            }
            if self.selected_v4l2_capture >= self.v4l2_capture_devices.len() {
                self.selected_v4l2_capture = 0;
            }
        }

        // Audio devices were enumerated in the background discovery thread.
        self.audio_devices = input_manager.audio_devices().to_vec();
        log::info!("[GUI] Found {} audio device(s)", self.audio_devices.len());
        for device in &self.audio_devices {
            log::info!("  - {}", device);
        }

        if let Ok(mut state) = self.shared_state.lock() {
            state.audio.available_devices = self.audio_devices.clone();
        }
    }

    /// Build the ImGui UI
    pub fn build_ui(&mut self, ui: &mut imgui::Ui, app_state: &mut dyn std::any::Any) {
        let window_size = ui.io().display_size;

        let window_title = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            format!("{} - Controls", state.web_app_name)
        };

        // Main control window
        ui.window(&window_title)
            .position([10.0, 10.0], imgui::Condition::FirstUseEver)
            .size([400.0, window_size[1] - 20.0], imgui::Condition::FirstUseEver)
            .movable(true)
            .collapsible(false)
            .resizable(true)
            .menu_bar(true)
            .build(|| {
                self.build_menu_bar(ui);
                self.build_tabs(ui, app_state);
            });

        // Preview windows — only rendered when enabled
        let show_preview = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.show_preview
        };

        if show_preview {
            let preview_pos = [420.0, 10.0];
            let preview_size = [
                (window_size[0] - preview_pos[0] - 10.0).max(200.0),
                (window_size[1] / 2.0 - 15.0).max(200.0),
            ];

            ui.window("Input Preview")
                .position(preview_pos, imgui::Condition::FirstUseEver)
                .size(preview_size, imgui::Condition::FirstUseEver)
                .build(|| {
                    self.build_input_preview(ui);
                });

            let output_preview_pos = [420.0, window_size[1] / 2.0 + 5.0];
            let output_preview_size = [
                (window_size[0] - output_preview_pos[0] - 10.0).max(200.0),
                (window_size[1] / 2.0 - 15.0).max(200.0),
            ];

            ui.window("Output Preview")
                .position(output_preview_pos, imgui::Condition::FirstUseEver)
                .size(output_preview_size, imgui::Condition::FirstUseEver)
                .build(|| {
                    self.build_output_preview(ui);
                });
        }

        self.build_settings_window(ui);
    }

    /// Floating Preferences window (opened via View > Preferences).
    fn build_settings_window(&mut self, ui: &imgui::Ui) {
        if !self.show_settings {
            return;
        }
        let window_size = ui.io().display_size;
        let mut still_open = true;
        ui.window("Preferences")
            .size([420.0, 520.0], imgui::Condition::FirstUseEver)
            .position(
                [
                    (window_size[0] / 2.0 - 210.0).max(10.0),
                    (window_size[1] / 2.0 - 260.0).max(10.0),
                ],
                imgui::Condition::FirstUseEver,
            )
            .collapsible(false)
            .opened(&mut still_open)
            .build(|| {
                self.build_settings_tab(ui);
            });
        self.show_settings = still_open;
    }

    /// Build the menu bar
    fn build_menu_bar(&mut self, ui: &imgui::Ui) {
        ui.menu_bar(|| {
            ui.menu("View", || {
                let show_preview = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.show_preview
                };
                if ui.menu_item_config("Show Previews").selected(show_preview).build() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.show_preview = !state.show_preview;
                }
                if ui.menu_item_config("Preferences...").selected(self.show_settings).build() {
                    self.show_settings = !self.show_settings;
                }
                ui.separator();
                if ui.menu_item("Refresh All Devices") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.input_command = InputCommand::RefreshDevices;
                    state.audio_command = AudioCommand::RefreshDevices;
                }
            });
        });
    }

    /// Build the main tabs as four grouped sections: Signal, Parameters, Control, Manage.
    ///
    /// Each group is a top-level tab; individual feature tabs are nested inside.
    /// Tabs are filtered by `EngineState::hidden_tabs`; Color/Motion are only shown
    /// when the effect declares parameters in those categories.
    /// Settings has moved to View > Preferences in the menu bar.
    fn build_tabs(&mut self, ui: &imgui::Ui, app_state: &mut dyn std::any::Any) {
        let (current_tab, hidden_tabs, has_color, has_motion) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let hc = state.param_descriptors.iter().any(|d| d.category == ParamCategory::Color);
            let hm = state.param_descriptors.iter().any(|d| d.category == ParamCategory::Motion);
            (state.current_tab, state.hidden_tabs.clone(), hc, hm)
        };
        let custom_tab_active = self.custom_tab_active;

        let vis = |tab: GuiTab| -> bool {
            if hidden_tabs.contains(&tab) { return false; }
            match tab {
                GuiTab::Color  => has_color,
                GuiTab::Motion => has_motion,
                _ => true,
            }
        };

        // Snapshot custom tab info to avoid borrow conflicts while mutating self.
        let custom_info: Vec<(usize, String, Option<GuiTab>)> = self.custom_tabs
            .iter()
            .enumerate()
            .map(|(i, t)| (i, t.name().to_string(), t.replaces()))
            .collect();

        // Group membership — tabs that belong to each section.
        const SIGNAL:     &[GuiTab] = &[GuiTab::Input, GuiTab::Output];
        const PARAMETERS: &[GuiTab] = &[GuiTab::Color, GuiTab::Motion, GuiTab::Audio, GuiTab::Lfo];
        const CONTROL:    &[GuiTab] = &[GuiTab::Midi,  GuiTab::Osc,    GuiTab::Web];
        const MANAGE:     &[GuiTab] = &[GuiTab::Presets];

        let group_any = |tabs: &[GuiTab]| -> bool {
            tabs.iter().any(|&t| vis(t))
            || custom_info.iter().any(|(_, _, r)| r.map_or(false, |r| tabs.contains(&r)))
        };

        if let Some(_groups) = ui.tab_bar("##groups") {
            // ── Signal ─────────────────────────────────────────────────────
            if group_any(SIGNAL) {
                if let Some(_g) = ui.tab_item("Signal") {
                    if let Some(_inner) = ui.tab_bar("##signal") {
                        for &tab in SIGNAL {
                            self.render_inner_tab(ui, app_state, tab, &custom_info,
                                current_tab, custom_tab_active, vis(tab));
                        }
                    }
                }
            }

            // ── Parameters ─────────────────────────────────────────────────
            if group_any(PARAMETERS) {
                if let Some(_g) = ui.tab_item("Parameters") {
                    if let Some(_inner) = ui.tab_bar("##params") {
                        for &tab in PARAMETERS {
                            self.render_inner_tab(ui, app_state, tab, &custom_info,
                                current_tab, custom_tab_active, vis(tab));
                        }
                    }
                }
            }

            // ── Control ────────────────────────────────────────────────────
            if group_any(CONTROL) {
                if let Some(_g) = ui.tab_item("Control") {
                    if let Some(_inner) = ui.tab_bar("##control") {
                        for &tab in CONTROL {
                            self.render_inner_tab(ui, app_state, tab, &custom_info,
                                current_tab, custom_tab_active, vis(tab));
                        }
                    }
                }
            }

            // ── Manage ─────────────────────────────────────────────────────
            if group_any(MANAGE) {
                if let Some(_g) = ui.tab_item("Manage") {
                    if let Some(_inner) = ui.tab_bar("##manage") {
                        for &tab in MANAGE {
                            self.render_inner_tab(ui, app_state, tab, &custom_info,
                                current_tab, custom_tab_active, vis(tab));
                        }
                    }
                }
            }

            // ── Custom non-replacing tabs appended as peer group tabs ──────
            for (idx, name, replaces) in &custom_info {
                if replaces.is_none() {
                    let idx = *idx;
                    let name = name.clone();
                    if let Some(_g) = ui.tab_item(&name) {
                        if custom_tab_active != Some(idx) {
                            self.custom_tab_active = Some(idx);
                        }
                        if let Some(ct) = self.custom_tabs.get_mut(idx) {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            ct.draw(ui, app_state, &mut state);
                        }
                    }
                }
            }
        }
    }

    /// Render one inner tab slot, honouring custom-tab replacement.
    ///
    /// Content is rendered inline (inside the tab item block) so each group's
    /// inner tab bar is self-contained.
    fn render_inner_tab(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        tab: GuiTab,
        custom_info: &[(usize, String, Option<GuiTab>)],
        current_tab: GuiTab,
        custom_tab_active: Option<usize>,
        is_visible: bool,
    ) {
        if !is_visible { return; }

        // Custom tab replacing this slot?
        if let Some((ci, cname, _)) = custom_info.iter().find(|(_, _, r)| *r == Some(tab)) {
            let ci = *ci;
            let cname = cname.clone();
            if let Some(_t) = ui.tab_item(&cname) {
                if custom_tab_active != Some(ci) {
                    self.custom_tab_active = Some(ci);
                }
                if let Some(ct) = self.custom_tabs.get_mut(ci) {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    ct.draw(ui, app_state, &mut state);
                }
            }
            return;
        }

        // Standard tab.
        if let Some(_t) = ui.tab_item(tab.name()) {
            if current_tab != tab || custom_tab_active.is_some() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.current_tab = tab;
                self.custom_tab_active = None;
            }
            match tab {
                GuiTab::Input   => self.build_input_tab(ui),
                GuiTab::Output  => self.build_output_tab(ui),
                GuiTab::Color   => self.build_param_category_tab(ui, ParamCategory::Color),
                GuiTab::Motion  => self.build_param_category_tab(ui, ParamCategory::Motion),
                GuiTab::Audio   => self.build_audio_tab(ui),
                GuiTab::Lfo     => self.build_lfo_tab(ui),
                GuiTab::Midi    => self.build_midi_tab(ui),
                GuiTab::Osc     => self.build_osc_tab(ui),
                GuiTab::Web     => self.build_web_tab(ui),
                GuiTab::Presets => self.build_presets_tab(ui),
                GuiTab::Settings | GuiTab::Sync => {}
            }
        }
    }
}

/// Returns the local IP address, if available.
pub fn get_local_ip() -> Option<String> {
    use std::net::UdpSocket;
    if let Ok(socket) = UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("8.8.8.8:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                return Some(addr.ip().to_string());
            }
        }
    }
    None
}

/// Copy text to the system clipboard.
pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::process::{Command, Stdio};
    use std::io::Write;

    #[cfg(target_os = "macos")]
    {
        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
    }
    #[cfg(target_os = "linux")]
    {
        // Try wl-copy first (Wayland), then fall back to xclip
        let result = Command::new("wl-copy")
            .stdin(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                if let Some(mut stdin) = child.stdin.take() {
                    stdin.write_all(text.as_bytes())?;
                }
                child.wait()
            });
        if result.is_err() {
            let mut child = Command::new("xclip")
                .args(["-selection", "clipboard"])
                .stdin(Stdio::piped())
                .spawn()?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()?;
        }
    }
    #[cfg(target_os = "windows")]
    {
        let mut child = Command::new("cmd")
            .args(["/C", "clip"])
            .stdin(Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            // Windows clip expects UTF-16LE with BOM for Unicode, but plain ASCII/UTF-8 works for basic text
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
    }
    Ok(())
}

