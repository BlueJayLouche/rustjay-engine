//! Settings tab — resolution, UI scale, FPS target, performance.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use crate::resolution_presets::{RESOLUTION_PRESETS, preset_dimensions};
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
        if ui
            .add(egui::Slider::new(&mut ui_scale, 0.5..=2.0).trailing_fill(true))
            .changed()
        {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.ui_scale = ui_scale;
        }

        let mut hide_main_output = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.no_primary_output
        };
        if ui
            .checkbox(&mut hide_main_output, "Hide main output window")
            .on_hover_text("Projector, headless, and control outputs keep running.")
            .changed()
        {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.no_primary_output = hide_main_output;
            state.save_settings_requested = true;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // Resolution settings
        ui.label(
            egui::RichText::new("Resolution Settings")
                .color(ACCENT_CYAN)
                .strong(),
        );

        let preset_names: Vec<&str> = RESOLUTION_PRESETS
            .iter()
            .map(|(name, _, _)| *name)
            .collect();

        // Internal Resolution
        ui.label("Internal Resolution (Processing):");
        let mut internal_preset_idx = self.internal_resolution_preset;
        let old_internal = internal_preset_idx;
        egui::ComboBox::from_id_salt("int_res")
            .width(200.0)
            .selected_text(preset_names[internal_preset_idx])
            .show_ui(ui, |ui| {
                for (i, name) in preset_names.iter().enumerate() {
                    if ui
                        .selectable_label(internal_preset_idx == i, *name)
                        .clicked()
                    {
                        internal_preset_idx = i;
                    }
                }
            });
        if internal_preset_idx != old_internal {
            self.internal_resolution_preset = internal_preset_idx;
            if let Some((width, height)) = preset_dimensions(internal_preset_idx) {
                self.pending_internal_width = width;
                self.pending_internal_height = height;
            }
        }

        ui.horizontal(|ui| {
            let mut w = self.pending_internal_width as i32;
            let mut h = self.pending_internal_height as i32;
            ui.add_enabled(
                self.internal_resolution_preset == 0,
                egui::DragValue::new(&mut w).speed(1).range(320..=8192),
            );
            ui.label("×");
            ui.add_enabled(
                self.internal_resolution_preset == 0,
                egui::DragValue::new(&mut h).speed(1).range(240..=4320),
            );
            ui.label("Custom");
            self.pending_internal_width = w.max(320) as u32;
            self.pending_internal_height = h.max(240) as u32;
        });

        // Output Resolution
        ui.add_space(8.0);
        ui.label("Output Resolution (Display/NDI):");
        let mut output_preset_idx = self.output_resolution_preset;
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
        if output_preset_idx != old_output {
            self.output_resolution_preset = output_preset_idx;
            if let Some((width, height)) = preset_dimensions(output_preset_idx) {
                self.pending_output_width = width;
                self.pending_output_height = height;
            }
        }

        ui.horizontal(|ui| {
            let mut ow = self.pending_output_width as i32;
            let mut oh = self.pending_output_height as i32;
            ui.add_enabled(
                self.output_resolution_preset == 0,
                egui::DragValue::new(&mut ow).speed(1).range(320..=8192),
            );
            ui.label("×");
            ui.add_enabled(
                self.output_resolution_preset == 0,
                egui::DragValue::new(&mut oh).speed(1).range(240..=4320),
            );
            ui.label("Custom");
            self.pending_output_width = ow.max(320) as u32;
            self.pending_output_height = oh.max(240) as u32;
        });

        ui.add_space(8.0);
        let apply_btn = egui::Button::new(
            egui::RichText::new("Apply Resolution Changes")
                .strong()
                .color(Color32::BLACK),
        )
        .fill(ACCENT_GREEN);
        if ui.add(apply_btn).clicked() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.resolution.internal_width = self.pending_internal_width;
            state.resolution.internal_height = self.pending_internal_height;
            state.output_width = self.pending_output_width;
            state.output_height = self.pending_output_height;
            state.output_command = OutputCommand::ResizeOutput;
            state.save_settings_requested = true;
            log::info!(
                "Resolution changed - Internal: {}x{}, Output: {}x{}",
                self.pending_internal_width,
                self.pending_internal_height,
                self.pending_output_width,
                self.pending_output_height
            );
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(
            egui::RichText::new("Keyboard Shortcuts:")
                .color(ACCENT_CYAN)
                .strong(),
        );
        ui.label("• Shift+F — Toggle Fullscreen");
        ui.label("• Shift+T — Tap Tempo");
        ui.label("• Escape — Exit Application");
        ui.label("• Shift+F1–F8 — Quick slot presets");

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(
            egui::RichText::new("Performance (Output Window)")
                .color(ACCENT_CYAN)
                .strong(),
        );
        let (fps, frame_time_ms, cpu_update_ms, gpu_encode_ms, present_wait_ms) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let perf = state.performance.lock().unwrap_or_else(|e| e.into_inner());
            (
                perf.fps,
                perf.frame_time_ms,
                perf.cpu_update_ms,
                perf.gpu_encode_ms,
                perf.present_wait_ms,
            )
        };
        ui.label(format!("Output FPS: {:.1}", fps));
        ui.label(format!("Frame Time: {:.2} ms", frame_time_ms));
        ui.collapsing("Frame breakdown", |ui| {
            ui.label(
                egui::RichText::new(format!("CPU update:  {:>5.2} ms", cpu_update_ms))
                    .monospace(),
            );
            ui.label(
                egui::RichText::new(format!("GPU encode:  {:>5.2} ms", gpu_encode_ms))
                    .monospace(),
            );
            ui.label(
                egui::RichText::new(format!("Present wait:{:>5.2} ms", present_wait_ms))
                    .monospace(),
            );
        });

        ui.add_space(8.0);
        ui.label("Target FPS:");
        let fps_options = [24u32, 30, 48, 60, 90, 120];
        let fps_labels = [
            "24 fps",
            "30 fps",
            "48 fps",
            "60 fps (recommended)",
            "90 fps",
            "120 fps",
        ];
        let target_fps_val = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.target_fps
        };
        let mut current_idx = fps_options
            .iter()
            .position(|&f| f == target_fps_val)
            .unwrap_or(3);
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

        ui.horizontal(|ui| {
            ui.label("Present mode:");
            let present_mode = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.present_mode
            };
            let all = rustjay_core::PresentMode::all();
            let current_idx = all.iter().position(|&m| m == present_mode).unwrap_or(0);
            egui::ComboBox::from_id_salt("present_mode")
                .width(200.0)
                .selected_text(all[current_idx].label())
                .show_ui(ui, |ui| {
                    for (i, &mode) in all.iter().enumerate() {
                        if ui.selectable_label(current_idx == i, mode.label()).clicked() {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.present_mode = mode;
                            state.save_settings_requested = true;
                        }
                    }
                });
        });
        ui.label(
            egui::RichText::new(
                "Auto VSync = display-paced (best for target_fps = refresh). \
                 Immediate = software cap only (best for target_fps < refresh or unlocked).",
            )
            .small()
            .color(ui.visuals().weak_text_color()),
        );

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        let save_btn = egui::Button::new(
            egui::RichText::new("💾 Save All Settings")
                .strong()
                .color(Color32::BLACK),
        )
        .fill(ACCENT_CYAN);
        if ui.add(save_btn).clicked() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.save_settings_requested = true;
            log::info!("Save settings requested from GUI");
        }
        ui.label(
            egui::RichText::new("Settings are auto-saved on exit, or manually with this button.")
                .size(11.0)
                .color(TEXT_SECONDARY),
        );
    }
}
