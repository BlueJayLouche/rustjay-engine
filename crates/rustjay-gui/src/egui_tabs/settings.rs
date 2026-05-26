//! Settings tab — resolution, UI scale, FPS target, performance.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use egui::Color32;
use rustjay_core::OutputCommand;

impl EguiControlGui {
    pub(crate) fn build_settings_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Application Settings");
        ui.add_space(8.0);

        let mut ui_scale = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.ui_scale
        };
        ui.label("UI Scale:");
        if ui.add(egui::Slider::new(&mut ui_scale, 0.5..=2.0).trailing_fill(true)).changed() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.ui_scale = ui_scale;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // Resolution settings
        ui.label(egui::RichText::new("Resolution Settings").color(ACCENT_CYAN).strong());

        let presets = [
            ("Custom", 0, 0),
            ("480p (640x480)", 640, 480),
            ("720p (1280x720)", 1280, 720),
            ("1080p (1920x1080)", 1920, 1080),
            ("1440p (2560x1440)", 2560, 1440),
            ("4K UHD (3840x2160)", 3840, 2160),
            ("Square 1:1 (1080x1080)", 1080, 1080),
            ("Vertical 9:16 (1080x1920)", 1080, 1920),
        ];
        let preset_names: Vec<&str> = presets.iter().map(|(name, _, _)| *name).collect();

        // Internal Resolution
        ui.label("Internal Resolution (Processing):");
        let (current_internal_w, current_internal_h) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.resolution.internal_width, state.resolution.internal_height)
        };

        let mut internal_preset_idx = 0;
        for (i, (_, w, h)) in presets.iter().enumerate().skip(1) {
            if *w == current_internal_w && *h == current_internal_h {
                internal_preset_idx = i;
                break;
            }
        }
        let old_internal = internal_preset_idx;
        egui::ComboBox::from_id_salt("int_res")
            .width(200.0)
            .selected_text(preset_names[internal_preset_idx])
            .show_ui(ui, |ui| {
                for (i, name) in preset_names.iter().enumerate() {
                    if ui.selectable_label(internal_preset_idx == i, *name).clicked() {
                        internal_preset_idx = i;
                    }
                }
            });
        if internal_preset_idx != old_internal && internal_preset_idx > 0 {
            let (_, w, h) = presets[internal_preset_idx];
            self.pending_internal_width = w;
            self.pending_internal_height = h;
        }

        ui.horizontal(|ui| {
            let mut w = self.pending_internal_width as i32;
            let mut h = self.pending_internal_height as i32;
            ui.add(egui::DragValue::new(&mut w).speed(1).range(320..=8192));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut h).speed(1).range(240..=4320));
            ui.label("Custom");
            self.pending_internal_width = w.max(320) as u32;
            self.pending_internal_height = h.max(240) as u32;
        });

        // Output Resolution
        ui.add_space(8.0);
        ui.label("Output Resolution (Display/NDI):");
        let (current_output_w, current_output_h) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.output_width, state.output_height)
        };

        let mut output_preset_idx = 0;
        for (i, (_, w, h)) in presets.iter().enumerate().skip(1) {
            if *w == current_output_w && *h == current_output_h {
                output_preset_idx = i;
                break;
            }
        }
        let old_output = output_preset_idx;
        egui::ComboBox::from_id_salt("out_res")
            .width(200.0)
            .selected_text(preset_names[output_preset_idx])
            .show_ui(ui, |ui| {
                for (i, name) in preset_names.iter().enumerate() {
                    if ui.selectable_label(output_preset_idx == i, *name).clicked() {
                        output_preset_idx = i;
                    }
                }
            });
        if output_preset_idx != old_output && output_preset_idx > 0 {
            let (_, w, h) = presets[output_preset_idx];
            self.pending_output_width = w;
            self.pending_output_height = h;
        }

        ui.horizontal(|ui| {
            let mut ow = self.pending_output_width as i32;
            let mut oh = self.pending_output_height as i32;
            ui.add(egui::DragValue::new(&mut ow).speed(1).range(320..=8192));
            ui.label("×");
            ui.add(egui::DragValue::new(&mut oh).speed(1).range(240..=4320));
            ui.label("Custom");
            self.pending_output_width = ow.max(320) as u32;
            self.pending_output_height = oh.max(240) as u32;
        });

        ui.add_space(8.0);
        let apply_btn = egui::Button::new(egui::RichText::new("Apply Resolution Changes").strong().color(Color32::BLACK))
            .fill(ACCENT_GREEN);
        if ui.add(apply_btn).clicked()
        {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.resolution.internal_width = self.pending_internal_width;
            state.resolution.internal_height = self.pending_internal_height;
            state.output_width = self.pending_output_width;
            state.output_height = self.pending_output_height;
            state.output_command = OutputCommand::ResizeOutput;
            state.save_settings_requested = true;
            log::info!("Resolution changed - Internal: {}x{}, Output: {}x{}",
                self.pending_internal_width, self.pending_internal_height,
                self.pending_output_width, self.pending_output_height);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("Keyboard Shortcuts:").color(ACCENT_CYAN).strong());
        ui.label("• Shift+F — Toggle Fullscreen");
        ui.label("• Shift+T — Tap Tempo");
        ui.label("• Escape — Exit Application");
        ui.label("• Shift+F1–F8 — Quick slot presets");

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("Performance (Output Window)").color(ACCENT_CYAN).strong());
        let (fps, frame_time_ms) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.performance.fps, state.performance.frame_time_ms)
        };
        ui.label(format!("Output FPS: {:.1}", fps));
        ui.label(format!("Frame Time: {:.2} ms", frame_time_ms));

        ui.add_space(8.0);
        ui.label("Target FPS:");
        let fps_options = [24u32, 30, 48, 60, 90, 120];
        let fps_labels = ["24 fps", "30 fps", "48 fps", "60 fps (recommended)", "90 fps", "120 fps"];
        let target_fps_val = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.target_fps
        };
        let mut current_idx = fps_options.iter().position(|&f| f == target_fps_val).unwrap_or(3);
        egui::ComboBox::from_id_salt("target_fps")
            .width(180.0)
            .selected_text(fps_labels[current_idx])
            .show_ui(ui, |ui| {
                for (i, label) in fps_labels.iter().enumerate() {
                    if ui.selectable_label(current_idx == i, *label).clicked() {
                        current_idx = i;
                    }
                }
            });
        if fps_options[current_idx] != target_fps_val {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.target_fps = fps_options[current_idx];
            state.save_settings_requested = true;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let save_btn = egui::Button::new(egui::RichText::new("💾 Save All Settings").strong().color(Color32::BLACK))
            .fill(ACCENT_CYAN);
        if ui.add(save_btn).clicked()
        {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.save_settings_requested = true;
            log::info!("Save settings requested from GUI");
        }
        ui.label(egui::RichText::new("Settings are auto-saved on exit, or manually with this button.").size(11.0).color(TEXT_SECONDARY));
    }
}
