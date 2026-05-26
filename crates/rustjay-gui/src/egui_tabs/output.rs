//! Output tab — fullscreen toggle and platform-specific output configuration.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use rustjay_core::OutputCommand;

impl EguiControlGui {
    pub(crate) fn build_output_tab(&mut self, ui: &mut egui::Ui) {
        let fullscreen = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.output_fullscreen
        };

        ui.heading("Output Settings");
        ui.add_space(8.0);

        // Fullscreen toggle
        let mut fs = fullscreen;
        if ui.checkbox(&mut fs, "Fullscreen Output").changed() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.output_fullscreen = fs;
        }
        ui.label(egui::RichText::new("Press Shift+F to toggle fullscreen").size(11.0).color(TEXT_SECONDARY));

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // NDI Output
        #[cfg(feature = "ndi")]
        {
            ui.label(egui::RichText::new("NDI Output").color(ACCENT_GREEN).strong());
            let ndi_active = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.ndi_output.is_active
            };
            ui.text_edit_singleline(&mut self.ndi_output_name);
            if !ndi_active {
                if ui.button("▶ Start NDI Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.ndi_output.stream_name = self.ndi_output_name.clone();
                    state.output_command = OutputCommand::StartNdi;
                }
            } else {
                if ui.button("⏹ Stop NDI Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopNdi;
                }
                self.status_badge(ui, "NDI Active", true);
            }
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
        }

        // Syphon Output (macOS)
        #[cfg(target_os = "macos")]
        {
            ui.label(egui::RichText::new("Syphon Output (macOS)").color(ACCENT_AMBER).strong());
            let syphon_enabled = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.syphon_output.enabled
            };
            ui.text_edit_singleline(&mut self.syphon_output_name);
            if !syphon_enabled {
                if ui.button("▶ Start Syphon Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.syphon_output.server_name = self.syphon_output_name.clone();
                    state.output_command = OutputCommand::StartSyphon;
                }
            } else {
                if ui.button("⏹ Stop Syphon Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopSyphon;
                }
                self.status_badge(ui, "Syphon Active", true);
            }
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
        }

        // Spout Output (Windows)
        #[cfg(target_os = "windows")]
        {
            ui.label(egui::RichText::new("Spout Output (Windows)").color(ACCENT_CYAN).strong());
            let spout_active = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.spout_output.enabled
            };
            ui.text_edit_singleline(&mut self.spout_output_name);
            if !spout_active {
                if ui.button("▶ Start Spout Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StartSpout {
                        sender_name: self.spout_output_name.clone(),
                    };
                }
            } else {
                if ui.button("⏹ Stop Spout Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopSpout;
                }
                self.status_badge(ui, "Spout Active", true);
            }
            ui.add_space(12.0);
            ui.separator();
            ui.add_space(8.0);
        }

        // V4L2 Loopback Output (Linux)
        #[cfg(target_os = "linux")]
        {
            ui.label(egui::RichText::new("V4L2 Loopback Output (Linux)").color(ACCENT_AMBER).strong());
            ui.label(egui::RichText::new("Requires v4l2loopback kernel module").size(11.0).color(TEXT_SECONDARY));

            let v4l2_active = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.v4l2_output.enabled
            };

            if !self.v4l2_output_devices.is_empty() {
                let labels: Vec<String> = self.v4l2_output_devices.iter().map(|d| d.display_name()).collect();
                egui::ComboBox::from_id_salt("v4l2_out_sel")
                    .width(240.0)
                    .selected_text(labels.get(self.selected_v4l2_output).map(|s| s.as_str()).unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in labels.iter().enumerate() {
                            if ui.selectable_label(self.selected_v4l2_output == i, name.as_str()).clicked() {
                                self.selected_v4l2_output = i;
                                if let Some(d) = self.v4l2_output_devices.get(i) {
                                    self.v4l2_device_path = d.path.clone();
                                }
                            }
                        }
                    });
            } else {
                ui.label(egui::RichText::new("No v4l2loopback devices found — see README for setup").color(TEXT_SECONDARY));
                ui.text_edit_singleline(&mut self.v4l2_device_path);
            }

            if !v4l2_active {
                if ui.button("▶ Start V4L2 Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StartV4l2 {
                        device_path: self.v4l2_device_path.clone(),
                    };
                }
            } else {
                if ui.button("⏹ Stop V4L2 Output").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopV4l2;
                }
                self.status_badge(ui, &format!("V4L2 Active: {}", self.v4l2_device_path), true);
            }
        }
    }
}
