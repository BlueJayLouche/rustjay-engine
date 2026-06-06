//! Input tab — video source discovery and selection.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use rustjay_core::InputCommand;

impl EguiControlGui {
    pub(crate) fn build_input_tab(&mut self, ui: &mut egui::Ui) {
        let (is_active, source_name, is_discovering, is_active2, source_name2) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.input.is_active,
                state.input.source_name.clone(),
                state.input_discovering,
                state.second_input.is_active,
                state.second_input.source_name.clone(),
            )
        };

        ui.heading("Video Input Sources");
        ui.add_space(8.0);

        // Refresh button
        if is_discovering {
            ui.label(egui::RichText::new("⏳ Discovering sources...").color(ACCENT_AMBER));
        } else if ui.button("🔄 Refresh Sources").clicked() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.input_command = InputCommand::RefreshDevices;
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // Input 1 status
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Input 1").strong().size(14.0));
            self.status_badge(ui, if is_active { "ACTIVE" } else { "OFFLINE" }, is_active);
        });
        if is_active {
            ui.label(format!("Source: {}", source_name));
            if ui.button("⏹ Stop Input 1").clicked() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.input_command = InputCommand::StopInput;
            }
        } else {
            ui.label(egui::RichText::new("No input active").color(TEXT_SECONDARY));
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // Input 2 status
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Input 2").strong().size(14.0));
            self.status_badge(
                ui,
                if is_active2 { "ACTIVE" } else { "OFFLINE" },
                is_active2,
            );
        });
        if is_active2 {
            ui.label(format!("Source: {}", source_name2));
            if ui.button("⏹ Stop Input 2").clicked() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.second_input_command = InputCommand::StopInput;
            }
        } else {
            ui.label(egui::RichText::new("No input active").color(TEXT_SECONDARY));
        }

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        // Source selectors
        self.build_source_selectors(ui);
    }

    fn build_source_selectors(&mut self, ui: &mut egui::Ui) {
        // Webcam
        #[cfg(not(target_os = "linux"))]
        {
            self.section_header(ui, "Webcam");
            if !self.webcam_devices.is_empty() {
                let device_names: Vec<&str> =
                    self.webcam_devices.iter().map(|s| s.as_str()).collect();
                egui::ComboBox::from_id_salt("webcam_sel")
                    .width(240.0)
                    .selected_text(
                        device_names
                            .get(self.selected_webcam)
                            .copied()
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in device_names.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_webcam == i, *name)
                                .clicked()
                            {
                                self.selected_webcam = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("▶ Start Input 1").clicked() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.input_command = InputCommand::StartWebcam {
                            device_index: self.selected_webcam,
                            width: 1920,
                            height: 1080,
                            fps: 30,
                        };
                    }
                    if ui.button("▶ Start Input 2").clicked() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.second_input_command = InputCommand::StartWebcam {
                            device_index: self.selected_webcam,
                            width: 1920,
                            height: 1080,
                            fps: 30,
                        };
                    }
                });
            } else {
                ui.label(egui::RichText::new("No webcams found").color(TEXT_SECONDARY));
            }
        }

        // NDI
        #[cfg(feature = "ndi")]
        {
            self.section_header(ui, "NDI");
            if !self.ndi_sources.is_empty() {
                let names: Vec<&str> = self.ndi_sources.iter().map(|s| s.as_str()).collect();
                egui::ComboBox::from_id_salt("ndi_sel")
                    .width(240.0)
                    .selected_text(names.get(self.selected_ndi).copied().unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            if ui.selectable_label(self.selected_ndi == i, *name).clicked() {
                                self.selected_ndi = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("▶ Start Input 1").clicked() {
                        let source_name = self
                            .ndi_sources
                            .get(self.selected_ndi)
                            .cloned()
                            .unwrap_or_default();
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.input_command = InputCommand::StartNdi { source_name };
                    }
                    if ui.button("▶ Start Input 2").clicked() {
                        let source_name = self
                            .ndi_sources
                            .get(self.selected_ndi)
                            .cloned()
                            .unwrap_or_default();
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.second_input_command = InputCommand::StartNdi { source_name };
                    }
                });
            } else {
                ui.label(egui::RichText::new("No NDI sources found").color(TEXT_SECONDARY));
            }
        }

        // Syphon
        #[cfg(target_os = "macos")]
        {
            self.section_header(ui, "Syphon (macOS)");
            if !self.syphon_servers.is_empty() {
                let server_names: Vec<String> = self
                    .syphon_servers
                    .iter()
                    .map(|s| format!("{} - {}", s.app_name, s.name))
                    .collect();
                egui::ComboBox::from_id_salt("syphon_sel")
                    .width(240.0)
                    .selected_text(
                        server_names
                            .get(self.selected_syphon)
                            .map(|s| s.as_str())
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in server_names.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_syphon == i, name.as_str())
                                .clicked()
                            {
                                self.selected_syphon = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("▶ Start Input 1").clicked() {
                        if let Some(info) = self.syphon_servers.get(self.selected_syphon).cloned() {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.input_command = InputCommand::StartSyphon {
                                server_name: info.display_name().to_string(),
                                server_uuid: info.uuid.clone(),
                            };
                        }
                    }
                    if ui.button("▶ Start Input 2").clicked() {
                        if let Some(info) = self.syphon_servers.get(self.selected_syphon).cloned() {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.second_input_command = InputCommand::StartSyphon {
                                server_name: info.display_name().to_string(),
                                server_uuid: info.uuid.clone(),
                            };
                        }
                    }
                });
            } else {
                ui.label(egui::RichText::new("No Syphon servers found").color(TEXT_SECONDARY));
            }
        }

        // Spout
        #[cfg(target_os = "windows")]
        {
            self.section_header(ui, "Spout (Windows)");
            if !self.spout_senders.is_empty() {
                let names: Vec<&str> = self.spout_senders.iter().map(|s| s.name.as_str()).collect();
                egui::ComboBox::from_id_salt("spout_sel")
                    .width(240.0)
                    .selected_text(names.get(self.selected_spout).copied().unwrap_or("?"))
                    .show_ui(ui, |ui| {
                        for (i, name) in names.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_spout == i, *name)
                                .clicked()
                            {
                                self.selected_spout = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("▶ Start Input 1").clicked() {
                        let sender_name = self
                            .spout_senders
                            .get(self.selected_spout)
                            .map(|s| s.name.clone())
                            .unwrap_or_default();
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.input_command = InputCommand::StartSpout { sender_name };
                    }
                    if ui.button("▶ Start Input 2").clicked() {
                        let sender_name = self
                            .spout_senders
                            .get(self.selected_spout)
                            .map(|s| s.name.clone())
                            .unwrap_or_default();
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.second_input_command = InputCommand::StartSpout { sender_name };
                    }
                });
            } else {
                ui.label(egui::RichText::new("No Spout senders found").color(TEXT_SECONDARY));
            }
        }

        // V4L2
        #[cfg(target_os = "linux")]
        {
            self.section_header(ui, "V4L2 Input (Linux)");
            if !self.v4l2_capture_devices.is_empty() {
                let labels: Vec<String> = self
                    .v4l2_capture_devices
                    .iter()
                    .map(|d| d.display_name())
                    .collect();
                egui::ComboBox::from_id_salt("v4l2_cap_sel")
                    .width(240.0)
                    .selected_text(
                        labels
                            .get(self.selected_v4l2_capture)
                            .map(|s| s.as_str())
                            .unwrap_or("?"),
                    )
                    .show_ui(ui, |ui| {
                        for (i, name) in labels.iter().enumerate() {
                            if ui
                                .selectable_label(self.selected_v4l2_capture == i, name.as_str())
                                .clicked()
                            {
                                self.selected_v4l2_capture = i;
                            }
                        }
                    });
                ui.horizontal(|ui| {
                    if ui.button("▶ Start Input 1").clicked() {
                        if let Some(info) =
                            self.v4l2_capture_devices.get(self.selected_v4l2_capture)
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.input_command = InputCommand::StartV4l2 {
                                device_path: info.path.clone(),
                            };
                        }
                    }
                    if ui.button("▶ Start Input 2").clicked() {
                        if let Some(info) =
                            self.v4l2_capture_devices.get(self.selected_v4l2_capture)
                        {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.second_input_command = InputCommand::StartV4l2 {
                                device_path: info.path.clone(),
                            };
                        }
                    }
                });
            } else {
                ui.label(
                    egui::RichText::new("No V4L2 capture devices found").color(TEXT_SECONDARY),
                );
            }
        }
    }
}
