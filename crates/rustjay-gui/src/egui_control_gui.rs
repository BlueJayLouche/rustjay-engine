//! Egui control panel — professional dark-themed GUI with full pipeline coverage.

use std::sync::Arc;
use egui::{Color32, Stroke};
use rustjay_core::{EngineState, GuiTab, ParameterDescriptor, ParamCategory, ParamType};

/// Egui-based control GUI.
pub struct EguiControlGui {
    pub(crate) shared_state: Arc<std::sync::Mutex<EngineState>>,

    // Preview textures
    pub input_preview_texture_id: Option<egui::TextureId>,
    pub second_input_preview_texture_id: Option<egui::TextureId>,
    pub output_preview_texture_id: Option<egui::TextureId>,

    // Custom tabs
    pub custom_tabs: Vec<Box<dyn crate::AnyEguiTab>>,
    pub(crate) custom_tab_active: Option<usize>,

    // Window toggles
    pub(crate) show_preferences: bool,
    pub(crate) show_routing_window: bool,

    // ── Device lists (cached from InputManager) ──────────────────────────────
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

    // Output names
    #[cfg(feature = "ndi")]
    pub(crate) ndi_output_name: String,
    #[cfg(target_os = "macos")]
    pub(crate) syphon_output_name: String,
    #[cfg(target_os = "windows")]
    pub(crate) spout_senders: Vec<rustjay_io::SpoutSenderInfo>,
    #[cfg(target_os = "windows")]
    pub(crate) selected_spout: usize,
    #[cfg(target_os = "windows")]
    pub(crate) spout_output_name: String,

    // V4L2 (Linux)
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

    // Pending resolution changes
    pub(crate) pending_internal_width: u32,
    pub(crate) pending_internal_height: u32,
    pub(crate) pending_output_width: u32,
    pub(crate) pending_output_height: u32,

    // Preset save form
    pub(crate) preset_name_buffer: String,
    pub(crate) saving_preset: bool,

    // Active sidebar tab
    pub(crate) active_tab: GuiTab,
}

impl EguiControlGui {
    /// Create a new control GUI.
    pub fn new(shared_state: Arc<std::sync::Mutex<EngineState>>) -> anyhow::Result<Self> {
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
            input_preview_texture_id: None,
            second_input_preview_texture_id: None,
            output_preview_texture_id: None,
            custom_tabs: Vec::new(),
            custom_tab_active: None,
            show_preferences: false,
            show_routing_window: false,
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
            pending_internal_width: internal_w,
            pending_internal_height: internal_h,
            pending_output_width: output_w,
            pending_output_height: output_h,
            preset_name_buffer: String::new(),
            saving_preset: false,
            active_tab: GuiTab::Input,
        })
    }

    // ── Texture setters ──────────────────────────────────────────────────────

    pub fn set_input_preview_texture(&mut self, id: egui::TextureId) {
        self.input_preview_texture_id = Some(id);
    }
    pub fn set_second_input_preview_texture(&mut self, id: egui::TextureId) {
        self.second_input_preview_texture_id = Some(id);
    }
    pub fn set_output_preview_texture(&mut self, id: egui::TextureId) {
        self.output_preview_texture_id = Some(id);
    }

    // ── Tap tempo ────────────────────────────────────────────────────────────

    pub fn handle_tap_tempo(&mut self) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
        if now - state.audio.last_tap_time > 2.0 {
            state.audio.tap_times.clear();
        }
        state.audio.tap_times.push(now);
        state.audio.last_tap_time = now;
        if state.audio.tap_times.len() > 8 {
            state.audio.tap_times.remove(0);
        }
        state.audio.beat_phase = 0.0;
        if state.audio.tap_times.len() >= 4 {
            let mut intervals = Vec::new();
            for i in 1..state.audio.tap_times.len() {
                intervals.push(state.audio.tap_times[i] - state.audio.tap_times[i - 1]);
            }
            let avg_interval: f64 = intervals.iter().sum::<f64>() / intervals.len() as f64;
            if avg_interval > 0.1 && avg_interval < 3.0 {
                state.audio.bpm = (60.0 / avg_interval) as f32;
            }
        }
    }

    // ── Device list sync ─────────────────────────────────────────────────────

    pub fn update_device_lists(&mut self, input_manager: &rustjay_io::InputManager) {
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
        self.audio_devices = input_manager.audio_devices().to_vec();
        if let Ok(mut state) = self.shared_state.lock() {
            state.audio.available_devices = self.audio_devices.clone();
        }
    }

    // ── Main UI entry point ──────────────────────────────────────────────────

    pub fn build_ui(&mut self, ctx: &egui::Context, app_state: &mut dyn std::any::Any) {
        crate::egui_theme::apply_professional_theme(ctx);

        self.build_top_bar(ctx);
        self.build_left_sidebar(ctx);
        self.build_preview_panel(ctx);
        self.build_central_panel(ctx, app_state);

        if self.show_preferences {
            self.build_preferences_window(ctx);
        }
        if self.show_routing_window {
            self.build_routing_window(ctx);
        }
    }

    // ── Top bar ──────────────────────────────────────────────────────────────

    fn build_top_bar(&mut self, ctx: &egui::Context) {
        use crate::egui_theme::colors::*;

        let (app_name, bpm, fps, volume, show_preview) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.web_app_name.clone(),
                state.effective_bpm(),
                state.performance.fps,
                state.audio.volume,
                state.show_preview,
            )
        };

        egui::TopBottomPanel::top("top_bar")
            .exact_height(42.0)
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        egui::RichText::new(format!("▶  {}", app_name))
                            .strong()
                            .size(16.0)
                            .color(ACCENT_CYAN),
                    );

                    ui.add_space(20.0);
                    ui.separator();
                    ui.add_space(12.0);

                    // BPM pill
                    ui.horizontal(|ui| {
                        ui.add_space(4.0);
                        ui.colored_label(ACCENT_AMBER, "●");
                        ui.label(
                            egui::RichText::new(format!("{:.1} BPM", bpm))
                                .monospace()
                                .size(13.0),
                        );
                    });

                    ui.add_space(16.0);

                    // FPS
                    ui.label(
                        egui::RichText::new(format!("{:.0} FPS", fps))
                            .monospace()
                            .size(12.0)
                            .color(TEXT_SECONDARY),
                    );

                    ui.add_space(16.0);

                    // Mini volume bar
                    let vol_w = 60.0;
                    let vol_h = 10.0;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(vol_w, vol_h), egui::Sense::hover());
                    let fill_w = vol_w * volume.clamp(0.0, 1.0);
                    let painter = ui.painter();
                    painter.rect_filled(rect, 2.0, BG_WIDGET);
                    if fill_w > 0.5 {
                        let fill_rect = egui::Rect::from_min_size(rect.min, egui::vec2(fill_w, vol_h));
                        let vol_color = if volume > 0.8 { ACCENT_RED } else { ACCENT_GREEN };
                        painter.rect_filled(fill_rect, 2.0, vol_color);
                    }

                    ui.add_space(8.0);

                    // Right side: global toggles
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let preview = show_preview;
                        if ui
                            .selectable_label(preview, "👁 Previews")
                            .clicked()
                        {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.show_preview = !state.show_preview;
                        }
                        ui.add_space(8.0);
                        if ui.selectable_label(self.show_preferences, "⚙ Prefs").clicked() {
                            self.show_preferences = !self.show_preferences;
                        }
                        ui.add_space(8.0);
                        if ui.button("💾 Save").clicked() {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.save_settings_requested = true;
                        }
                    });
                });
            });
    }

    // ── Left sidebar ─────────────────────────────────────────────────────────

    fn build_left_sidebar(&mut self, ctx: &egui::Context) {
        use crate::egui_theme::colors::*;

        let (hidden_tabs, has_color, has_motion) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let hc = state.param_descriptors.iter().any(|d| d.category == ParamCategory::Color);
            let hm = state.param_descriptors.iter().any(|d| d.category == ParamCategory::Motion);
            (state.hidden_tabs.clone(), hc, hm)
        };

        let vis = |tab: GuiTab| -> bool {
            if hidden_tabs.contains(&tab) { return false; }
            match tab {
                GuiTab::Color => has_color,
                GuiTab::Motion => has_motion,
                _ => true,
            }
        };

        egui::SidePanel::left("sidebar")
            .exact_width(110.0)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("PIPELINE").size(10.0).color(TEXT_SECONDARY).strong());
                });
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Signal group
                self.sidebar_button(ui, GuiTab::Input, "📹  Input", vis(GuiTab::Input));
                self.sidebar_button(ui, GuiTab::Output, "📤  Output", vis(GuiTab::Output));

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("PARAMS").size(10.0).color(TEXT_SECONDARY).strong());
                });
                ui.add_space(4.0);

                // Parameters group
                self.sidebar_button(ui, GuiTab::Color, "🎨  Color", vis(GuiTab::Color));
                self.sidebar_button(ui, GuiTab::Motion, "✦  Motion", vis(GuiTab::Motion));
                self.sidebar_button(ui, GuiTab::Audio, "🎵  Audio", vis(GuiTab::Audio));
                self.sidebar_button(ui, GuiTab::Lfo, "～  LFO", vis(GuiTab::Lfo));

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("CONTROL").size(10.0).color(TEXT_SECONDARY).strong());
                });
                ui.add_space(4.0);

                // Control group
                self.sidebar_button(ui, GuiTab::Midi, "🎛  MIDI", vis(GuiTab::Midi));
                self.sidebar_button(ui, GuiTab::Osc, "📡  OSC", vis(GuiTab::Osc));
                self.sidebar_button(ui, GuiTab::Web, "🌐  Web", vis(GuiTab::Web));

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new("MANAGE").size(10.0).color(TEXT_SECONDARY).strong());
                });
                ui.add_space(4.0);

                // Manage group
                self.sidebar_button(ui, GuiTab::Presets, "💾  Presets", vis(GuiTab::Presets));
                self.sidebar_button(ui, GuiTab::Settings, "⚙  Settings", true);
            });
    }

    fn sidebar_button(&mut self, ui: &mut egui::Ui, tab: GuiTab, label: &str, visible: bool) {
        use crate::egui_theme::colors::*;
        if !visible { return; }
        let active = self.active_tab == tab;
        let btn = if active {
            egui::Button::new(
                egui::RichText::new(label).strong().color(ACCENT_CYAN),
            )
            .fill(BG_ACTIVE)
            .stroke(Stroke::new(1.0, ACCENT_CYAN))
        } else {
            egui::Button::new(egui::RichText::new(label).color(TEXT_PRIMARY))
                .fill(BG_WIDGET)
                .stroke(Stroke::NONE)
        };
        if ui.add_sized(egui::vec2(ui.available_width(), 32.0), btn).clicked() {
            self.active_tab = tab;
            self.custom_tab_active = None;
        }
        ui.add_space(2.0);
    }

    // ── Central panel ────────────────────────────────────────────────────────

    fn build_central_panel(&mut self, ctx: &egui::Context, app_state: &mut dyn std::any::Any) {
        use rustjay_core::GuiTab;

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                match self.active_tab {
                    GuiTab::Input => self.build_input_tab(ui),
                    GuiTab::Output => self.build_output_tab(ui),
                    GuiTab::Color => self.build_param_category_tab(ui, ParamCategory::Color),
                    GuiTab::Motion => self.build_param_category_tab(ui, ParamCategory::Motion),
                    GuiTab::Audio => self.build_audio_tab(ui),
                    GuiTab::Lfo => self.build_lfo_tab(ui),
                    GuiTab::Midi => self.build_midi_tab(ui),
                    GuiTab::Osc => self.build_osc_tab(ui),
                    GuiTab::Web => self.build_web_tab(ui),
                    GuiTab::Presets => self.build_presets_tab(ui),
                    GuiTab::Settings => self.build_settings_tab(ui),
                    GuiTab::Sync => { /* Sync is folded into Audio */ }
                }

                // Custom tabs that replace built-ins
                if let Some((idx, _t)) = self.custom_tabs.iter().enumerate().find(|(_, t)| t.replaces() == Some(self.active_tab)) {
                    if let Some(ct) = self.custom_tabs.get_mut(idx) {
                        if let Ok(mut state) = self.shared_state.lock() {
                            ct.draw(ui, app_state, &mut state);
                        }
                    }
                }
            });
        });
    }

    // ── Preview panel ────────────────────────────────────────────────────────

    fn build_preview_panel(&mut self, ctx: &egui::Context) {
        let show_preview = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.show_preview
        };
        if !show_preview { return; }

        let has_input2 = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.second_input.is_active
        };

        egui::SidePanel::right("previews")
            .default_width(280.0)
            .resizable(true)
            .show(ctx, |ui| {
                let available = ui.available_size();
                let preview_count = if has_input2 { 3.0 } else { 2.0 };
                let preview_height = (available.y / preview_count - 8.0).max(20.0);
                let preview_width = (available.x - 8.0).max(20.0);
                let preview_size = egui::vec2(preview_width, preview_height);

                ui.vertical(|ui| {
                    if has_input2 {
                        self.preview_image(ui, "Input 1", self.input_preview_texture_id, preview_size, |s| {
                            (s.input.width, s.input.height)
                        });
                        self.preview_image(ui, "Input 2", self.second_input_preview_texture_id, preview_size, |s| {
                            (s.second_input.width, s.second_input.height)
                        });
                    } else {
                        self.preview_image(ui, "Input", self.input_preview_texture_id, preview_size, |s| {
                            (s.input.width, s.input.height)
                        });
                    }
                    self.preview_image(ui, "Output", self.output_preview_texture_id, preview_size, |s| {
                        (s.resolution.internal_width, s.resolution.internal_height)
                    });
                });
            });
    }

    fn preview_image(
        &self,
        ui: &mut egui::Ui,
        label: &str,
        texture_id: Option<egui::TextureId>,
        size: egui::Vec2,
        get_size: impl FnOnce(&EngineState) -> (u32, u32),
    ) {
        ui.vertical(|ui| {
            ui.label(egui::RichText::new(label).size(11.0).color(crate::egui_theme::colors::TEXT_SECONDARY));
            let size = egui::vec2(
                (size.x - 8.0).max(10.0),
                (size.y - 4.0).max(10.0),
            );
            if let Some(id) = texture_id {
                let (iw, ih) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    get_size(&state)
                };
                let content_aspect = if iw > 0 && ih > 0 {
                    iw as f32 / ih as f32
                } else {
                    16.0 / 9.0
                };
                let container_aspect = size.x / size.y.max(1.0);
                let (uv0, uv1) = if content_aspect > container_aspect {
                    let visible = container_aspect / content_aspect;
                    let pad = (1.0 - visible) / 2.0;
                    ([pad, 0.0], [1.0 - pad, 1.0])
                } else {
                    let visible = content_aspect / container_aspect;
                    let pad = (1.0 - visible) / 2.0;
                    ([0.0, pad], [1.0, 1.0 - pad])
                };
                let image = egui::Image::new(egui::load::SizedTexture::new(id, size))
                    .uv(egui::Rect::from_min_max(egui::pos2(uv0[0], uv0[1]), egui::pos2(uv1[0], uv1[1])));
                ui.add(image);
            } else {
                let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, crate::egui_theme::colors::BG_WIDGET);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "No preview",
                    egui::FontId::proportional(12.0),
                    crate::egui_theme::colors::TEXT_SECONDARY,
                );
            }
        });
    }

    // ── Preferences window ───────────────────────────────────────────────────

    fn build_preferences_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_preferences;
        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(true)
            .default_size([400.0, 300.0])
            .open(&mut open)
            .show(ctx, |ui| {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                ui.checkbox(&mut state.output_fullscreen, "Fullscreen Output");
                ui.checkbox(&mut state.show_preview, "Show Preview");
            });
        self.show_preferences = open;
    }

    // ── Routing window ───────────────────────────────────────────────────────

    pub(crate) fn build_routing_window(&mut self, ctx: &egui::Context) {
        use rustjay_core::routing::{FftBand, ModulationTarget};

        let mut is_open = self.show_routing_window;
        let target_list = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            ModulationTarget::all_for(&state.param_descriptors)
        };
        let target_names: Vec<String> = target_list.iter().map(|t| t.name()).collect();

        egui::Window::new("Audio Routing Matrix")
            .default_pos([500.0, 100.0])
            .default_size([450.0, 550.0])
            .open(&mut is_open)
            .show(ctx, |ui| {
                let (can_add, route_count, max_routes) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    let routing = &state.audio_routing;
                    (routing.matrix.can_add_route(), routing.matrix.len(), routing.matrix.max_routes())
                };

                ui.label(format!("Routes: {}/{}", route_count, max_routes));

                if ui.button("Clear All").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.clear();
                }

                ui.separator();
                ui.label(egui::RichText::new("Add New Route").color(crate::egui_theme::colors::ACCENT_CYAN).strong());

                let (mut band_idx, mut target_idx) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (state.audio_routing.selected_band, state.audio_routing.selected_target)
                };

                if target_idx >= target_list.len() && !target_list.is_empty() {
                    target_idx = target_list.len() - 1;
                }

                let bands: Vec<&str> = FftBand::all().iter().map(|b| b.name()).collect();
                egui::ComboBox::from_id_salt("route_band")
                    .width(120.0)
                    .selected_text(bands.get(band_idx).copied().unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in bands.iter().enumerate() {
                            if ui.selectable_label(band_idx == i, *name).clicked() {
                                band_idx = i;
                            }
                        }
                    });
                egui::ComboBox::from_id_salt("route_target")
                    .width(180.0)
                    .selected_text(target_names.get(target_idx).map(|s| s.as_str()).unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in target_names.iter().enumerate() {
                            if ui.selectable_label(target_idx == i, name.as_str()).clicked() {
                                target_idx = i;
                            }
                        }
                    });

                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.selected_band = band_idx;
                    state.audio_routing.selected_target = target_idx;
                }

                let can_add = can_add && band_idx < FftBand::all().len() && target_idx < target_list.len();
                if can_add {
                    if ui.button("Add Route").clicked() {
                        if let Some(band) = FftBand::from_index(band_idx) {
                            if let Some(target) = target_list.get(target_idx) {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.audio_routing.matrix.add_route(band, target.clone());
                            }
                        }
                    }
                } else {
                    ui.label(egui::RichText::new("Max routes reached").color(crate::egui_theme::colors::TEXT_SECONDARY));
                }

                ui.separator();
                ui.label(egui::RichText::new("Active Routes").color(crate::egui_theme::colors::ACCENT_CYAN).strong());

                let routes_data: Vec<_> = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.routes().iter().map(|r| {
                        (r.id, r.band, r.target.clone(), r.amount, r.attack, r.release, r.enabled, r.current_value)
                    }).collect()
                };

                egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                    for (id, band, target, amount, attack, release, enabled, current) in &routes_data {
                        ui.group(|ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let mut is_enabled = *enabled;
                                if ui.checkbox(&mut is_enabled, "").changed() {
                                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                                        route.enabled = is_enabled;
                                    }
                                }
                                ui.label(format!("{} → {}", band.short_name(), target.name()));
                                ui.colored_label(crate::egui_theme::colors::ACCENT_GREEN, format!("{:.2}", current));
                                if ui.button("✕").clicked() {
                                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.audio_routing.matrix.remove_route(*id);
                                }
                            });

                            let mut amt = *amount;
                            if ui.add(egui::Slider::new(&mut amt, -1.0..=1.0).text("Amount").trailing_fill(true)).changed() {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                                    route.amount = amt;
                                }
                            }

                            ui.columns(2, |cols| {
                                let mut atk = *attack;
                                if cols[0].add(egui::Slider::new(&mut atk, 0.001..=1.0).text("Attack").trailing_fill(true)).changed() {
                                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                                        route.attack = atk;
                                    }
                                }
                                let mut rel = *release;
                                if cols[1].add(egui::Slider::new(&mut rel, 0.001..=1.0).text("Release").trailing_fill(true)).changed() {
                                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                                        route.release = rel;
                                    }
                                }
                            });
                        });
                    }

                    if routes_data.is_empty() {
                        ui.label(egui::RichText::new("No routes configured. Add one above.").color(crate::egui_theme::colors::TEXT_SECONDARY));
                    }
                });
            });

        self.show_routing_window = is_open;
    }

    // ── Parameter rendering ──────────────────────────────────────────────────

    fn build_param_category_tab(&mut self, ui: &mut egui::Ui, category: ParamCategory) {
        let descriptors: Vec<ParameterDescriptor> = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.param_descriptors.iter().filter(|d| d.category == category).cloned().collect()
        };

        if descriptors.is_empty() {
            ui.label(egui::RichText::new("No parameters declared for this category.").color(crate::egui_theme::colors::TEXT_SECONDARY));
            return;
        }

        ui.heading(format!("{} Parameters", category.name()));
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        egui::Grid::new(format!("param_grid_{}", category.name()))
            .num_columns(2)
            .spacing([12.0, 8.0])
            .min_col_width(120.0)
            .show(ui, |ui| {
                for desc in &descriptors {
                    ui.label(&desc.name);
                    if let Ok(mut state) = self.shared_state.lock() {
                        self.draw_param_control(ui, desc, &mut state);
                    }
                    ui.end_row();
                }
            });

        let has_modulatable = descriptors.iter().any(|d| d.is_modulatable());
        if has_modulatable {
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
            ui.label("LFO Modulation");
            if ui.button("Open LFO Window").clicked() {
                if let Ok(mut state) = self.shared_state.lock() {
                    state.lfo.show_window = true;
                }
            }
            let active_lfos = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.lfo.bank.lfos.iter().filter(|b| b.enabled).count()
            };
            if active_lfos > 0 {
                ui.horizontal(|ui| {
                    ui.colored_label(crate::egui_theme::colors::ACCENT_GREEN, format!("({} active)", active_lfos));
                });
            }
        }
    }

    fn draw_param_control(&self, ui: &mut egui::Ui, desc: &ParameterDescriptor, state: &mut std::sync::MutexGuard<'_, EngineState>) {
        let value = state.get_param_base(&desc.id).unwrap_or(desc.default);
        match desc.param_type {
            ParamType::Float => {
                let mut v = value;
                if ui.add(egui::Slider::new(&mut v, desc.min..=desc.max).show_value(true).trailing_fill(true)).changed() {
                    state.set_param_base(&desc.id, v);
                }
            }
            ParamType::Int => {
                let mut v = value as i32;
                let min = desc.min as i32;
                let max = desc.max as i32;
                if ui.add(egui::Slider::new(&mut v, min..=max).show_value(true).trailing_fill(true)).changed() {
                    state.set_param_base(&desc.id, v as f32);
                }
            }
            ParamType::Bool => {
                let mut v = value > 0.5;
                if ui.checkbox(&mut v, "").changed() {
                    state.set_param_base(&desc.id, if v { 1.0 } else { 0.0 });
                }
            }
            ParamType::Enum { ref variants } => {
                let mut idx = value as usize;
                let names: Vec<&str> = variants.iter().map(|s| s.as_str()).collect();
                egui::ComboBox::from_id_salt(&desc.id)
                    .width(140.0)
                    .selected_text(names.get(idx).copied().unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            if ui.selectable_label(idx == i, *name).clicked() {
                                idx = i;
                            }
                        }
                    });
                if idx != value as usize {
                    state.set_param_base(&desc.id, idx as f32);
                }
            }
        }
    }

    // ── Section header helper ────────────────────────────────────────────────

    pub(crate) fn section_header(&self, ui: &mut egui::Ui, title: &str) {
        use crate::egui_theme::colors::*;
        ui.add_space(8.0);
        ui.label(egui::RichText::new(title).strong().size(14.0).color(ACCENT_CYAN));
        ui.separator();
        ui.add_space(4.0);
    }

    pub(crate) fn status_badge(&self, ui: &mut egui::Ui, text: &str, active: bool) {
        use crate::egui_theme::colors::*;
        let (bg, fg) = if active {
            (ACCENT_GREEN, Color32::BLACK)
        } else {
            (BG_WIDGET, TEXT_SECONDARY)
        };
        let galley = ui.painter().layout(text.to_string(), egui::FontId::proportional(11.0), fg, f32::INFINITY);
        let padding = egui::vec2(8.0, 3.0);
        let desired_size = galley.size() + padding * 2.0;
        let (rect, _) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
        ui.painter().rect_filled(rect, 4.0, bg);
        ui.painter().galley(rect.min + padding, galley, fg);
    }
}
