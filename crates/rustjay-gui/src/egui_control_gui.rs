//! Egui control panel — professional dark-themed GUI with full pipeline coverage.

// Internal panel fields and per-section build_* helpers; not part of a documented API.
#![allow(missing_docs)]

use rustjay_core::{
    EngineState, GuiTab, NotificationLevel, ParamCategory, ParamType, ParameterDescriptor,
};
use std::sync::Arc;

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
    #[allow(dead_code)] // read only by the non-Linux webcam picker
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

    // QR code cache: (url that was encoded, matrix of dark/light modules)
    pub(crate) qr_cache: Option<(String, Vec<Vec<bool>>)>,

    // ── Modulation tab state (M5.2) ──────────────────────────────────────────
    /// UUID of the currently-expanded source in the Modulation tab.
    pub(crate) modulation_expanded_source: Option<String>,
    /// Param id selected in the "Add assignment" dropdown.
    pub(crate) modulation_new_assignment_param: Option<String>,

    // ── Sidebar section collapse state ───────────────────────────────────────
    /// Collapse state for sidebar sections: [SIGNAL, PARAMS, CONTROL, MANAGE, APP].
    /// `true` = collapsed, `false` = expanded.
    pub(crate) sidebar_collapsed: [bool; 5],
}

impl EguiControlGui {
    /// Create a new control GUI.
    pub fn new(shared_state: Arc<std::sync::Mutex<EngineState>>) -> anyhow::Result<Self> {
        // `ndi_name` is only read by the `ndi`-gated UI below.
        #[cfg_attr(not(feature = "ndi"), allow(unused_variables))]
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
            qr_cache: None,
            modulation_expanded_source: None,
            modulation_new_assignment_param: None,
            sidebar_collapsed: [false; 5],
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
        // Publish the raw id so custom egui tabs (e.g. vjarda's Stage tab) can
        // draw the live master output as a canvas background. They reconstruct
        // it with `egui::TextureId::User(id)`.
        let raw = match id {
            egui::TextureId::Managed(n) | egui::TextureId::User(n) => n,
        };
        if let Ok(mut state) = self.shared_state.lock() {
            state.stage_preview_texture_id = Some(raw);
        }
    }

    // ── Tap tempo ────────────────────────────────────────────────────────────

    pub fn handle_tap_tempo(&mut self) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
        let is_first_tap = now - state.audio.last_tap_time > 2.0;
        if is_first_tap {
            state.audio.tap_times.clear();
            let mut mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
            for entry in mod_eng.sources.iter_mut() {
                if let rustjay_core::modulation::ModulationSource::LFO { phase, last_beat_phase, .. } = &mut entry.source {
                    *phase = 0.0;
                    *last_beat_phase = 0.0;
                }
            }
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
        if let Ok(state) = self.shared_state.lock() {
            self.audio_devices = state.audio.available_devices.clone();
        }
    }

    // ── Main UI entry point ──────────────────────────────────────────────────

    pub fn build_ui(&mut self, ctx: &egui::Context, app_state: &mut dyn std::any::Any) {
        crate::egui_theme::apply_professional_theme(ctx);

        self.build_top_bar(ctx);
        self.build_left_sidebar(ctx);
        self.build_preview_panel(ctx);
        self.build_central_panel(ctx, app_state);
        self.build_toast_overlay(ctx);

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
        use crate::egui_widgets::{status_pill, PillState};

        let (
            app_name,
            bpm,
            fps,
            cpu,
            mem_used,
            mem_total,
            volume,
            show_preview,
            audio_enabled,
            web_enabled,
            web_host,
            web_port,
            osc_enabled,
            osc_port,
            output_sinks,
            lfo_assign_mode,
            midi_learn_mode,
        ) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let perf = state.performance.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.web_app_name.clone(),
                state.effective_bpm(),
                perf.fps,
                perf.cpu_percent,
                perf.mem_used_mb,
                perf.mem_total_mb,
                state.audio.volume,
                state.show_preview,
                state.audio.enabled,
                state.web_enabled,
                state.web_host.clone(),
                state.web_port,
                state.osc_enabled,
                state.osc_port,
                state
                    .output_sinks
                    .lock()
                    .map(|g| g.clone())
                    .unwrap_or_default(),
                state.lfo_assign_mode,
                state.midi_learn_mode,
            )
        };

        // egui 0.34 deprecated top-level `Panel::show` in favour of `show_inside` (which needs a
        // parent Ui); we keep the established top-level layout until that migration is done.
        #[allow(deprecated)]
        egui::Panel::top("top_bar")
            .exact_size(56.0)
            .frame(
                egui::Frame::NONE
                    .fill(SURFACE_2)
                    .stroke(egui::Stroke::new(1.0, HAIR_2)),
            )
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);

                    // App name in the HUD style: BOLD/SLASH form
                    let upper = app_name.to_uppercase();
                    let (head, tail) = upper.split_once(' ').unwrap_or((upper.as_str(), ""));
                    ui.label(
                        egui::RichText::new(head)
                            .strong()
                            .size(18.0)
                            .color(INK)
                            .monospace(),
                    );
                    ui.label(egui::RichText::new("/").color(AMBER).size(18.0).monospace());
                    if !tail.is_empty() {
                        ui.label(egui::RichText::new(tail).size(18.0).color(INK).monospace());
                    }
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("CONTROL · v1.0")
                            .size(10.0)
                            .color(INK_4)
                            .monospace(),
                    );

                    ui.add_space(24.0);

                    // BPM readout — big tabular numerics
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("BPM").size(9.5).color(INK_4));
                        ui.label(
                            egui::RichText::new(format!("{:>5.1}", bpm))
                                .size(15.0)
                                .color(AMBER)
                                .strong()
                                .monospace(),
                        );
                    });

                    ui.add_space(16.0);

                    // FPS readout
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("FPS").size(9.5).color(INK_4));
                        ui.label(
                            egui::RichText::new(format!("{:>4.0}", fps))
                                .size(15.0)
                                .color(INK)
                                .monospace(),
                        );
                    });

                    ui.add_space(16.0);

                    // CPU readout
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("CPU").size(9.5).color(INK_4));
                        ui.label(
                            egui::RichText::new(format!("{:>4.0}%", cpu))
                                .size(15.0)
                                .color(INK)
                                .monospace(),
                        );
                    });

                    ui.add_space(16.0);

                    // Memory readout
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("MEM").size(9.5).color(INK_4));
                        ui.label(
                            egui::RichText::new(format!(
                                "{:.2}/{:.2} GB",
                                mem_used as f32 / 1024.0,
                                mem_total as f32 / 1024.0
                            ))
                            .size(15.0)
                            .color(INK)
                            .monospace(),
                        );
                    });

                    ui.add_space(16.0);

                    // Mini volume meter — flat bar w/ amber fill, square edges
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("VOL").size(9.5).color(INK_4));
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(72.0, 10.0), egui::Sense::hover());
                        let p = ui.painter();
                        p.rect_stroke(
                            rect,
                            0.0,
                            egui::Stroke::new(1.0, HAIR_2),
                            egui::StrokeKind::Inside,
                        );
                        let fill_w = rect.width() * volume.clamp(0.0, 1.0);
                        if fill_w > 0.5 {
                            let fr = egui::Rect::from_min_size(
                                rect.min,
                                egui::vec2(fill_w, rect.height()),
                            );
                            let col = if volume > 0.8 { ALERT } else { SIGNAL };
                            p.rect_filled(fr, 0.0, col);
                        }
                        // Tick marks
                        for i in 1..10 {
                            let x = rect.left() + rect.width() * (i as f32 / 10.0);
                            p.line_segment(
                                [
                                    egui::pos2(x, rect.bottom() - 2.0),
                                    egui::pos2(x, rect.bottom()),
                                ],
                                egui::Stroke::new(1.0, HAIR_3),
                            );
                        }
                    });

                    // Right side: status pill + global toggles
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(12.0);
                        if ui
                            .button(
                                egui::RichText::new("⚙  PREFS")
                                    .size(11.0)
                                    .color(if self.show_preferences { AMBER } else { INK_2 }),
                            )
                            .clicked()
                        {
                            self.show_preferences = !self.show_preferences;
                        }
                        ui.add_space(6.0);
                        // LFO-assign mode: highlight params to bind an LFO source.
                        if ui
                            .button(
                                egui::RichText::new("〰 LFO MAP")
                                    .size(11.0)
                                    .color(if lfo_assign_mode { AMBER } else { INK_2 }),
                            )
                            .on_hover_text("Click a parameter to assign a modulation source")
                            .clicked()
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo_assign_mode = !state.lfo_assign_mode;
                            if state.lfo_assign_mode {
                                state.midi_learn_mode = false;
                            }
                        }
                        ui.add_space(6.0);
                        // MIDI-learn mode: click a param, then move a control to map it.
                        if ui
                            .button(
                                egui::RichText::new("🎹 MIDI MAP")
                                    .size(11.0)
                                    .color(if midi_learn_mode { AMBER } else { INK_2 }),
                            )
                            .on_hover_text("Click a parameter, then move/press a MIDI control")
                            .clicked()
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.midi_learn_mode = !state.midi_learn_mode;
                            if state.midi_learn_mode {
                                state.lfo_assign_mode = false;
                            } else {
                                // Leaving map mode cancels any pending arm.
                                state.midi_command = rustjay_core::MidiCommand::CancelLearn;
                            }
                        }
                        ui.add_space(6.0);
                        if ui
                            .button(
                                egui::RichText::new(if show_preview {
                                    "● PREVIEW"
                                } else {
                                    "○ PREVIEW"
                                })
                                .size(11.0)
                                .color(if show_preview {
                                    AMBER
                                } else {
                                    INK_2
                                }),
                            )
                            .clicked()
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.show_preview = !state.show_preview;
                        }
                        ui.add_space(6.0);
                        if ui
                            .button(egui::RichText::new("🔄 REFRESH").size(11.0).color(INK_2))
                            .clicked()
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.input_command = rustjay_core::InputCommand::RefreshDevices;
                            state.audio_command = rustjay_core::AudioCommand::RefreshDevices;
                            state.midi_command = rustjay_core::MidiCommand::RefreshDevices;
                        }
                        ui.add_space(6.0);
                        if ui
                            .button(egui::RichText::new("💾 SAVE").size(11.0).color(INK_2))
                            .clicked()
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.save_settings_requested = true;
                        }
                        ui.add_space(12.0);
                        status_pill(
                            ui,
                            if audio_enabled { "ONLINE" } else { "OFFLINE" },
                            if audio_enabled {
                                PillState::Online
                            } else {
                                PillState::Offline
                            },
                        );

                        // Services strip (right_to_left): active output sinks,
                        // then OSC and WEB. Labels are fixed so a pill only
                        // changes colour (not width) when it goes active — details
                        // live in the hover tooltip to keep the strip compact.
                        ui.add_space(12.0);
                        for label in &output_sinks {
                            status_pill(ui, label, PillState::Online);
                            ui.add_space(6.0);
                        }
                        status_pill(
                            ui,
                            "OSC",
                            if osc_enabled {
                                PillState::Online
                            } else {
                                PillState::Neutral
                            },
                        )
                        .on_hover_text(if osc_enabled {
                            format!("OSC input listening on port {osc_port}")
                        } else {
                            "OSC input disabled".to_string()
                        });
                        ui.add_space(6.0);
                        status_pill(
                            ui,
                            "WEB",
                            if web_enabled {
                                PillState::Online
                            } else {
                                PillState::Neutral
                            },
                        )
                        .on_hover_text(if web_enabled {
                            format!("Web remote at {web_host}:{web_port}")
                        } else {
                            "Web remote disabled".to_string()
                        });
                    });
                });
            });
    }

    // ── Toast overlay ────────────────────────────────────────────────────────

    fn build_toast_overlay(&mut self, ctx: &egui::Context) {
        use crate::egui_theme::colors::*;

        let now = std::time::Instant::now();
        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
        let notifs: Vec<rustjay_core::Notification> = {
            match state.notifications.lock() { Ok(mut guard) => {
                guard.retain(|n| n.expires_at > now);
                guard.clone()
            } _ => {
                Vec::new()
            }}
        };
        drop(state);

        if notifs.is_empty() {
            return;
        }

        // Ensure toasts vanish on idle windows by requesting a repaint
        // when the soonest notification expires.
        if let Some(soonest) = notifs.iter().map(|n| n.expires_at).min() {
            let remaining = soonest.saturating_duration_since(now);
            ctx.request_repaint_after(remaining);
        }

        egui::Area::new(egui::Id::new("toast_overlay"))
            .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 72.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.vertical(|ui| {
                    for n in notifs {
                        let (bg, text) = match n.level {
                            NotificationLevel::Error => (ALERT, INK),
                            NotificationLevel::Warning => (AMBER, INK),
                            NotificationLevel::Success => (SIGNAL, INK),
                            NotificationLevel::Info => (SURFACE, INK_2),
                        };
                        egui::Frame::NONE
                            .fill(bg)
                            .corner_radius(4.0)
                            .inner_margin(egui::Margin::same(8))
                            .show(ui, |ui| {
                                ui.set_max_width(280.0);
                                ui.label(egui::RichText::new(&n.message).size(12.0).color(text));
                            });
                        ui.add_space(6.0);
                    }
                });
            });
    }

    // ── Left sidebar ─────────────────────────────────────────────────────────

    fn build_left_sidebar(&mut self, ctx: &egui::Context) {
        use crate::egui_theme::colors::*;
        use crate::egui_widgets::hud_collapsible_section_header;

        let (hidden_tabs, has_color, has_motion) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let hc = state
                .param_descriptors
                .iter()
                .any(|d| d.category == ParamCategory::Color);
            let hm = state
                .param_descriptors
                .iter()
                .any(|d| d.category == ParamCategory::Motion);
            (state.hidden_tabs.clone(), hc, hm)
        };
        let vis = |tab: GuiTab| -> bool {
            if hidden_tabs.contains(&tab) {
                return false;
            }
            match tab {
                GuiTab::Color => has_color,
                GuiTab::Motion => has_motion,
                _ => true,
            }
        };

        // See note on top panel: top-level `Panel::show` remains the supported layout path.
        #[allow(deprecated)]
        egui::Panel::left("sidebar")
            .exact_size(150.0)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(BG)
                    .stroke(egui::Stroke::new(1.0, HAIR_2)),
            )
            .show(ctx, |ui| {
                ui.add_space(10.0);

                // ── SIGNAL ───────────────────────────────────────────────────────
                self.sidebar_collapsed[0] = hud_collapsible_section_header(
                    ui,
                    "SIGNAL",
                    Some("02 CH"),
                    self.sidebar_collapsed[0],
                );
                if !self.sidebar_collapsed[0] {
                    self.sidebar_button(ui, GuiTab::Input, "INPUT", vis(GuiTab::Input));
                    self.sidebar_button(ui, GuiTab::Output, "OUTPUT", vis(GuiTab::Output));
                }

                // ── PARAMS ───────────────────────────────────────────────────────
                self.sidebar_collapsed[1] = hud_collapsible_section_header(
                    ui,
                    "PARAMS",
                    Some("04 CH"),
                    self.sidebar_collapsed[1],
                );
                if !self.sidebar_collapsed[1] {
                    self.sidebar_button(ui, GuiTab::Color, "COLOR", vis(GuiTab::Color));
                    self.sidebar_button(ui, GuiTab::Motion, "MOTION", vis(GuiTab::Motion));
                    self.sidebar_button(ui, GuiTab::Audio, "AUDIO", vis(GuiTab::Audio));
                    self.sidebar_button(ui, GuiTab::Modulation, "Modulation", vis(GuiTab::Modulation));
                }

                // ── CONTROL ──────────────────────────────────────────────────────
                self.sidebar_collapsed[2] = hud_collapsible_section_header(
                    ui,
                    "CONTROL",
                    Some("03 CH"),
                    self.sidebar_collapsed[2],
                );
                if !self.sidebar_collapsed[2] {
                    self.sidebar_button(ui, GuiTab::Midi, "MIDI", vis(GuiTab::Midi));
                    self.sidebar_button(ui, GuiTab::Osc, "OSC", vis(GuiTab::Osc));
                    self.sidebar_button(ui, GuiTab::Web, "WEB", vis(GuiTab::Web));
                }

                // ── MANAGE ───────────────────────────────────────────────────────
                self.sidebar_collapsed[3] = hud_collapsible_section_header(
                    ui,
                    "MANAGE",
                    Some("02 CH"),
                    self.sidebar_collapsed[3],
                );
                if !self.sidebar_collapsed[3] {
                    self.sidebar_button(ui, GuiTab::Presets, "PRESETS", vis(GuiTab::Presets));
                    self.sidebar_button(ui, GuiTab::Settings, "SETTINGS", true);
                }

                // App-provided custom tabs that don't replace a builtin get their
                // own sidebar buttons (replacing tabs render in their builtin's
                // slot). Mirrors the imgui host's custom-tab handling.
                let custom: Vec<(usize, String)> = self
                    .custom_tabs
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| t.replaces().is_none())
                    .map(|(i, t)| (i, t.name().to_uppercase()))
                    .collect();
                if !custom.is_empty() {
                    let count = format!("{:02} CH", custom.len());
                    self.sidebar_collapsed[4] = hud_collapsible_section_header(
                        ui,
                        "APP",
                        Some(&count),
                        self.sidebar_collapsed[4],
                    );
                    if !self.sidebar_collapsed[4] {
                        for (idx, label) in &custom {
                            self.custom_sidebar_button(ui, *idx, label);
                        }
                    }
                }
            });
    }

    fn sidebar_button(&mut self, ui: &mut egui::Ui, tab: GuiTab, label: &str, visible: bool) {
        if !visible {
            return;
        }
        // A builtin button reads as active only when no custom tab is open.
        let active = self.active_tab == tab && self.custom_tab_active.is_none();
        if Self::draw_sidebar_button(ui, label, active) {
            self.active_tab = tab;
            self.custom_tab_active = None;
        }
    }

    /// Sidebar button for an app-provided custom tab (selected via
    /// `custom_tab_active`, drawn in the central panel before builtin dispatch).
    fn custom_sidebar_button(&mut self, ui: &mut egui::Ui, idx: usize, label: &str) {
        let active = self.custom_tab_active == Some(idx);
        if Self::draw_sidebar_button(ui, label, active) {
            self.custom_tab_active = Some(idx);
        }
    }

    /// Draw one sidebar button; returns `true` if clicked. Shared by builtin and
    /// custom-tab buttons so they look and behave identically.
    fn draw_sidebar_button(ui: &mut egui::Ui, label: &str, active: bool) -> bool {
        use crate::egui_theme::colors::*;

        let height = 32.0;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );
        let p = ui.painter();

        // Background
        let bg = if active {
            SURFACE_2
        } else if resp.hovered() {
            egui::Color32::from_rgba_premultiplied(8, 12, 16, 24)
        } else {
            egui::Color32::TRANSPARENT
        };
        p.rect_filled(rect, 0.0, bg);

        // Left accent bar — amber on active
        let accent_w = if active { 3.0 } else { 1.0 };
        let accent_color = if active { AMBER } else { HAIR_2 };
        p.rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(accent_w, rect.height())),
            0.0,
            accent_color,
        );

        // Label
        let label_color = if active { INK } else { INK_2 };
        let galley = p.layout_no_wrap(
            label.to_string(),
            egui::FontId::monospace(12.0),
            label_color,
        );
        p.galley(
            egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0),
            galley,
            label_color,
        );

        // Tab index on the right when active
        if active {
            let idx_g = p.layout_no_wrap("▶".to_string(), egui::FontId::monospace(11.0), AMBER);
            p.galley(
                egui::pos2(
                    rect.right() - idx_g.size().x - 10.0,
                    rect.center().y - idx_g.size().y / 2.0,
                ),
                idx_g,
                AMBER,
            );
        }

        resp.clicked()
    }

    // ── Central panel ────────────────────────────────────────────────────────

    fn build_central_panel(&mut self, ctx: &egui::Context, app_state: &mut dyn std::any::Any) {
        use rustjay_core::GuiTab;

        #[allow(deprecated)] // top-level CentralPanel::show; see note on the top panel
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                // A custom tab opened via its own sidebar button (it does not
                // replace a builtin) takes precedence over the builtin dispatch.
                if let Some(idx) = self.custom_tab_active {
                    if let Some(ct) = self.custom_tabs.get_mut(idx)
                        && let Ok(mut state) = self.shared_state.lock() {
                            ct.draw(ui, app_state, &mut state);
                        }
                    return;
                }

                // Check if a custom tab replaces the currently active built-in tab
                let custom_replacement = self
                    .custom_tabs
                    .iter()
                    .enumerate()
                    .find(|(_, t)| t.replaces() == Some(self.active_tab));

                if let Some((idx, _t)) = custom_replacement {
                    if let Some(ct) = self.custom_tabs.get_mut(idx)
                        && let Ok(mut state) = self.shared_state.lock() {
                            ct.draw(ui, app_state, &mut state);
                        }
                } else {
                    match self.active_tab {
                        GuiTab::Input => self.build_input_tab(ui),
                        GuiTab::Output => self.build_output_tab(ui),
                        GuiTab::Color => self.build_param_category_tab(ui, ParamCategory::Color),
                        GuiTab::Motion => self.build_param_category_tab(ui, ParamCategory::Motion),
                        GuiTab::Audio => self.build_audio_tab(ui),
                        GuiTab::Modulation => self.build_modulation_tab(ui),
                        GuiTab::Midi => self.build_midi_tab(ui),
                        GuiTab::Osc => self.build_osc_tab(ui),
                        GuiTab::Web => self.build_web_tab(ui),
                        GuiTab::Presets => self.build_presets_tab(ui),
                        GuiTab::Settings => self.build_settings_tab(ui),
                        GuiTab::Sync => { /* Sync is folded into Audio */ }
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
        if !show_preview {
            return;
        }

        let has_input2 = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.second_input.is_active
        };

        // See note on top panel: top-level `Panel::show` remains the supported layout path.
        #[allow(deprecated)]
        egui::Panel::right("preview_panel_right")
            .exact_size(280.0)
            .show(ctx, |ui| {
                let available = ui.available_size();
                let preview_count = if has_input2 { 3.0 } else { 2.0 };
                let preview_height = (available.y / preview_count - 8.0).max(20.0);
                let preview_width = (available.x - 8.0).max(20.0);
                let preview_size = egui::vec2(preview_width, preview_height);

                ui.vertical(|ui| {
                    if has_input2 {
                        self.preview_image(
                            ui,
                            "Input 1",
                            self.input_preview_texture_id,
                            preview_size,
                            |s| (s.input.width, s.input.height),
                        );
                        self.preview_image(
                            ui,
                            "Input 2",
                            self.second_input_preview_texture_id,
                            preview_size,
                            |s| (s.second_input.width, s.second_input.height),
                        );
                    } else {
                        self.preview_image(
                            ui,
                            "Input",
                            self.input_preview_texture_id,
                            preview_size,
                            |s| (s.input.width, s.input.height),
                        );
                    }
                    self.preview_image(
                        ui,
                        "Output",
                        self.output_preview_texture_id,
                        preview_size,
                        |s| (s.resolution.internal_width, s.resolution.internal_height),
                    );
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
            ui.label(
                egui::RichText::new(label)
                    .size(11.0)
                    .color(crate::egui_theme::colors::TEXT_SECONDARY),
            );

            // Allocate space for the image (label already consumed its portion)
            let image_area = egui::vec2((size.x - 8.0).max(10.0), (size.y - 20.0).max(10.0));
            let (rect, _) = ui.allocate_exact_size(image_area, egui::Sense::hover());

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

                // Fit the entire image inside the allocated rect, preserving aspect ratio
                let container_aspect = rect.width() / rect.height().max(1.0);
                let display_size = if content_aspect > container_aspect {
                    egui::vec2(rect.width(), rect.width() / content_aspect)
                } else {
                    egui::vec2(rect.height() * content_aspect, rect.height())
                };

                // Center the image in the allocated rect
                let image_rect = egui::Rect::from_center_size(rect.center(), display_size);
                let image = egui::Image::new(egui::load::SizedTexture::new(id, display_size));
                ui.put(image_rect, image);
            } else {
                ui.painter()
                    .rect_filled(rect, 4.0, crate::egui_theme::colors::BG_WIDGET);
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
                    (
                        routing.matrix.can_add_route(),
                        routing.matrix.len(),
                        routing.matrix.max_routes(),
                    )
                };

                ui.label(format!("Routes: {}/{}", route_count, max_routes));

                if ui.button("Clear All").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.clear();
                }

                ui.separator();
                ui.label(
                    egui::RichText::new("Add New Route")
                        .color(crate::egui_theme::colors::ACCENT_CYAN)
                        .strong(),
                );

                let (mut band_idx, mut target_idx) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        state.audio_routing.selected_band,
                        state.audio_routing.selected_target,
                    )
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
                    .selected_text(
                        target_names
                            .get(target_idx)
                            .map(|s| s.as_str())
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in target_names.iter().enumerate() {
                            if ui
                                .selectable_label(target_idx == i, name.as_str())
                                .clicked()
                            {
                                target_idx = i;
                            }
                        }
                    });

                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.selected_band = band_idx;
                    state.audio_routing.selected_target = target_idx;
                }

                let can_add =
                    can_add && band_idx < FftBand::all().len() && target_idx < target_list.len();
                if can_add {
                    if ui.button("Add Route").clicked()
                        && let Some(band) = FftBand::from_index(band_idx)
                            && let Some(target) = target_list.get(target_idx) {
                                let mut state =
                                    self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.audio_routing.matrix.add_route(band, target.clone());
                            }
                } else {
                    ui.label(
                        egui::RichText::new("Max routes reached")
                            .color(crate::egui_theme::colors::TEXT_SECONDARY),
                    );
                }

                ui.separator();
                ui.label(
                    egui::RichText::new("Active Routes")
                        .color(crate::egui_theme::colors::ACCENT_CYAN)
                        .strong(),
                );

                let routes_data: Vec<_> = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .audio_routing
                        .matrix
                        .routes()
                        .iter()
                        .map(|r| {
                            (
                                r.id,
                                r.band,
                                r.target.clone(),
                                r.amount,
                                r.attack,
                                r.release,
                                r.enabled,
                                r.current_value,
                            )
                        })
                        .collect()
                };

                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for (id, band, target, amount, attack, release, enabled, current) in
                            &routes_data
                        {
                            ui.group(|ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let mut is_enabled = *enabled;
                                    if ui.checkbox(&mut is_enabled, "").changed() {
                                        let mut state = self
                                            .shared_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        if let Some(route) =
                                            state.audio_routing.matrix.get_route_mut(*id)
                                        {
                                            route.enabled = is_enabled;
                                        }
                                    }
                                    ui.label(format!("{} → {}", band.short_name(), target.name()));
                                    ui.colored_label(
                                        crate::egui_theme::colors::ACCENT_GREEN,
                                        format!("{:.2}", current),
                                    );
                                    if ui.button("✕").clicked() {
                                        let mut state = self
                                            .shared_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        state.audio_routing.matrix.remove_route(*id);
                                    }
                                });

                                let mut amt = *amount;
                                if ui
                                    .add(
                                        egui::Slider::new(&mut amt, -1.0..=1.0)
                                            .text("Amount")
                                            .trailing_fill(true),
                                    )
                                    .changed()
                                {
                                    let mut state =
                                        self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    if let Some(route) =
                                        state.audio_routing.matrix.get_route_mut(*id)
                                    {
                                        route.amount = amt;
                                    }
                                }

                                ui.columns(2, |cols| {
                                    let mut atk = *attack;
                                    if cols[0]
                                        .add(
                                            egui::Slider::new(&mut atk, 0.001..=1.0)
                                                .text("Attack")
                                                .trailing_fill(true),
                                        )
                                        .changed()
                                    {
                                        let mut state = self
                                            .shared_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        if let Some(route) =
                                            state.audio_routing.matrix.get_route_mut(*id)
                                        {
                                            route.attack = atk;
                                        }
                                    }
                                    let mut rel = *release;
                                    if cols[1]
                                        .add(
                                            egui::Slider::new(&mut rel, 0.001..=1.0)
                                                .text("Release")
                                                .trailing_fill(true),
                                        )
                                        .changed()
                                    {
                                        let mut state = self
                                            .shared_state
                                            .lock()
                                            .unwrap_or_else(|e| e.into_inner());
                                        if let Some(route) =
                                            state.audio_routing.matrix.get_route_mut(*id)
                                        {
                                            route.release = rel;
                                        }
                                    }
                                });
                            });
                        }

                        if routes_data.is_empty() {
                            ui.label(
                                egui::RichText::new("No routes configured. Add one above.")
                                    .color(crate::egui_theme::colors::TEXT_SECONDARY),
                            );
                        }
                    });
            });

        self.show_routing_window = is_open;
    }

    // ── Parameter rendering ──────────────────────────────────────────────────

    fn build_param_category_tab(&mut self, ui: &mut egui::Ui, category: ParamCategory) {
        let descriptors: Vec<ParameterDescriptor> = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .param_descriptors
                .iter()
                .filter(|d| d.category == category)
                .cloned()
                .collect()
        };

        if descriptors.is_empty() {
            ui.label(
                egui::RichText::new("No parameters declared for this category.")
                    .color(crate::egui_theme::colors::TEXT_SECONDARY),
            );
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
            ui.label("Modulation");
            if ui.button("Open Modulation Tab").clicked() {
                self.active_tab = GuiTab::Modulation;
            }
            let active_lfos = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let mod_eng = state.modulation.lock().unwrap_or_else(|e| e.into_inner());
                mod_eng.sources.iter().filter(|e| {
                    matches!(e.source, rustjay_core::modulation::ModulationSource::LFO { enabled: true, .. })
                }).count()
            };
            if active_lfos > 0 {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        crate::egui_theme::colors::ACCENT_GREEN,
                        format!("({} active)", active_lfos),
                    );
                });
            }
        }
    }

    fn draw_param_control(
        &self,
        ui: &mut egui::Ui,
        desc: &ParameterDescriptor,
        state: &mut std::sync::MutexGuard<'_, EngineState>,
    ) {
        let value = state.get_param_base(&desc.id).unwrap_or(desc.default);
        log::trace!(
            "GUI draw {}: {} (base={:?})",
            desc.id,
            value,
            state.get_param_base(&desc.id)
        );
        match desc.param_type {
            ParamType::Float => {
                let mut v = value;
                let id_tag = format!("{}/{}", desc.category.name().to_lowercase(), desc.id);
                if state.midi_learn_mode || state.lfo_assign_mode {
                    let scope = ui.scope(|ui| {
                        ui.disable();
                        crate::egui_widgets::parameter_card_f32(
                            ui,
                            &desc.name,
                            &id_tag,
                            &mut v,
                            desc.min..=desc.max,
                            "",
                        )
                    });
                    apply_param_map_overlay(
                        ui,
                        state,
                        scope.response.rect,
                        &desc.id,
                        &desc.name,
                        &format!("{}/{}", desc.category.name().to_lowercase(), desc.id),
                        desc.min,
                        desc.max,
                    );
                } else {
                    let (changed, reset) = crate::egui_widgets::parameter_card_f32(
                        ui,
                        &desc.name,
                        &id_tag,
                        &mut v,
                        desc.min..=desc.max,
                        "",
                    );
                    if reset {
                        state.set_param_base(&desc.id, desc.default);
                    } else if changed {
                        state.set_param_base(&desc.id, v);
                    }
                }
            }
            ParamType::Int => {
                let mut v = value as i32;
                let min = desc.min as i32;
                let max = desc.max as i32;
                log::trace!("Int slider {}: {} (range {}..{})", desc.id, v, min, max);
                if state.midi_learn_mode || state.lfo_assign_mode {
                    let scope = ui.scope(|ui| {
                        ui.disable();
                        ui.add(
                            egui::Slider::new(&mut v, min..=max)
                                .show_value(true)
                                .trailing_fill(true),
                        )
                    });
                    apply_param_map_overlay(
                        ui,
                        state,
                        scope.response.rect,
                        &desc.id,
                        &desc.name,
                        &format!("{}/{}", desc.category.name().to_lowercase(), desc.id),
                        desc.min,
                        desc.max,
                    );
                } else if ui
                    .add(
                        egui::Slider::new(&mut v, min..=max)
                            .show_value(true)
                            .trailing_fill(true),
                    )
                    .changed()
                {
                    state.set_param_base(&desc.id, v as f32);
                }
            }
            ParamType::Bool => {
                let mut v = value > 0.5;
                if crate::egui_widgets::segmented_toggle(ui, &desc.id, &mut v, ("OFF", "ON")) {
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
        crate::egui_widgets::hud_section_header(ui, title, None);
    }

    pub(crate) fn status_badge(&self, ui: &mut egui::Ui, text: &str, active: bool) {
        use crate::egui_widgets::{status_pill, PillState};
        status_pill(
            ui,
            text,
            if active {
                PillState::Online
            } else {
                PillState::Offline
            },
        );
    }
}

/// True when either top-bar map mode (MIDI learn / LFO assign) is active.
///
/// Custom egui tabs that render their own param sliders should check this and,
/// when true, render the slider disabled and call [`apply_param_map_overlay`]
/// so their params participate in the map modes like the built-in tabs do.
pub fn map_mode_active(engine: &EngineState) -> bool {
    engine.midi_learn_mode || engine.lfo_assign_mode
}

/// Draw the active map-mode outline over `rect` and handle clicks on a param.
///
/// In MIDI-learn mode a click arms `StartLearn` for the param; in LFO-assign
/// mode a click opens a popup listing modulation sources to bind. `midi_path`
/// is the learn target path (conventionally `"<category>/<id>"`), `id` is the
/// engine param id (also the modulation-assignment key). No-op when neither
/// mode is active.
#[allow(deprecated)] // egui::Popup builder migration is project-wide; tracked separately
#[allow(clippy::too_many_arguments)] // overlay needs full param context; bundling adds no clarity
pub fn apply_param_map_overlay(
    ui: &mut egui::Ui,
    engine: &mut EngineState,
    rect: egui::Rect,
    id: &str,
    name: &str,
    midi_path: &str,
    min: f32,
    max: f32,
) {
    use crate::egui_theme::colors::*;

    if engine.midi_learn_mode {
        let armed = engine.midi_learn_active
            && engine.midi_learning_param_name.as_deref() == Some(name);
        let color = if armed {
            egui::Color32::from_rgb(255, 120, 50) // armed: orange
        } else {
            egui::Color32::from_rgb(180, 80, 220) // mappable: purple
        };
        ui.painter().rect_stroke(
            rect.expand(2.0),
            0.0,
            egui::Stroke::new(2.0, color),
            egui::StrokeKind::Outside,
        );
        let click_id = ui.make_persistent_id(("midi_map_click", id));
        if ui.interact(rect, click_id, egui::Sense::click()).clicked() {
            engine.midi_command = rustjay_core::MidiCommand::StartLearn {
                param_path: midi_path.to_string(),
                param_name: name.to_string(),
                min,
                max,
            };
        }
        return;
    }

    if !engine.lfo_assign_mode {
        return;
    }

    // LFO-assign mode.
    let (assigned, sources) = {
        let mod_eng = engine.modulation.lock().unwrap_or_else(|e| e.into_inner());
        let assigned = mod_eng.assignments.get(id).is_some_and(|v| !v.is_empty());
        let sources: Vec<(String, &'static str)> = mod_eng
            .sources
            .iter()
            .map(|e| (e.uuid.clone(), mod_source_short(&e.source)))
            .collect();
        (assigned, sources)
    };
    let color = if assigned { ACCENT_GREEN } else { ACCENT_CYAN };
    ui.painter().rect_stroke(
        rect.expand(2.0),
        0.0,
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Outside,
    );
    let click_id = ui.make_persistent_id(("lfo_map_click", id));
    let resp = ui.interact(rect, click_id, egui::Sense::click());
    let popup_id = ui.make_persistent_id(("lfo_assign_popup", id));
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    egui::popup_below_widget(
        ui,
        popup_id,
        &resp,
        egui::PopupCloseBehavior::CloseOnClick,
        |ui| {
            ui.set_min_width(150.0);
            ui.label(egui::RichText::new(format!("Modulate {name}")).small().strong());
            if sources.is_empty() {
                ui.label(
                    egui::RichText::new("No sources — add one in Modulation")
                        .small()
                        .weak(),
                );
            }
            for (uuid, ty) in &sources {
                let tag = &uuid[..4.min(uuid.len())];
                if ui.button(format!("+ {ty} {tag}")).clicked() {
                    let mut mod_eng =
                        engine.modulation.lock().unwrap_or_else(|e| e.into_inner());
                    // Single active source per param: replace any existing.
                    mod_eng.assignments.remove(id);
                    mod_eng.assign(id, uuid, 0.5, None);
                }
            }
            if assigned {
                ui.separator();
                if ui.button(egui::RichText::new("✕ Clear").small()).clicked() {
                    let mut mod_eng =
                        engine.modulation.lock().unwrap_or_else(|e| e.into_inner());
                    mod_eng.assignments.remove(id);
                }
            }
        },
    );
}

/// Short label for a modulation source, used by the LFO-assign popup.
fn mod_source_short(src: &rustjay_core::modulation::ModulationSource) -> &'static str {
    use rustjay_core::modulation::ModulationSource as S;
    match src {
        S::LFO { .. } => "LFO",
        S::AudioBand { .. } => "Audio",
        S::ADSR { .. } => "ADSR",
        S::StepSequencer { .. } => "Step",
    }
}
