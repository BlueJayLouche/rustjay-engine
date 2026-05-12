use crate::control_gui::ControlGui;
use rustjay_core::InputCommand;

impl ControlGui {
    /// Build the Input tab
    pub(crate) fn build_input_tab(&mut self, ui: &imgui::Ui) {
        let (is_active, _source_type, source_name, is_discovering) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.input.is_active,
                state.input.input_type,
                state.input.source_name.clone(),
                state.input_discovering,
            )
        };

        ui.text("Video Input Source");
        ui.separator();

        // Status
        if is_active {
            ui.text_colored([0.0, 1.0, 0.0, 1.0], &format!("Active: {}", source_name));
        } else {
            ui.text_colored([0.5, 0.5, 0.5, 1.0], "No input active");
        }

        ui.spacing();

        // Refresh Sources button - prominently at the top
        if is_discovering {
            ui.text_colored([1.0, 0.8, 0.2, 1.0], "Discovering sources...");
        } else {
            let _btn_color = ui.push_style_color(imgui::StyleColor::Button, [0.2, 0.6, 0.8, 1.0]);
            let _btn_hover = ui.push_style_color(imgui::StyleColor::ButtonHovered, [0.3, 0.7, 0.9, 1.0]);
            let _btn_active = ui.push_style_color(imgui::StyleColor::ButtonActive, [0.1, 0.5, 0.7, 1.0]);
            if ui.button_with_size("Refresh Sources", [ui.content_region_avail()[0], 30.0]) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.input_command = InputCommand::RefreshDevices;
            }
        }

        ui.spacing();
        ui.separator();
        ui.spacing();

        // Webcam section — on Linux, webcams are shown in the V4L2 section below.
        #[cfg(not(target_os = "linux"))]
        {
            ui.text_colored([0.0, 1.0, 1.0, 1.0], "Webcam");
            if !self.webcam_devices.is_empty() {
                let device_names: Vec<&str> = self.webcam_devices.iter().map(|s| s.as_str()).collect();
                ui.combo_simple_string("Select Webcam", &mut self.selected_webcam, &device_names);

                if ui.button("Start Webcam") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.input_command = InputCommand::StartWebcam {
                        device_index: self.selected_webcam as usize,
                        width: 1920,
                        height: 1080,
                        fps: 30,
                    };
                }
            } else {
                ui.text_disabled("No webcams found");
            }
        }

        // NDI section
        #[cfg(feature = "ndi")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            ui.text_colored([0.0, 1.0, 1.0, 1.0], "NDI");
            if !self.ndi_sources.is_empty() {
                let source_names: Vec<&str> = self.ndi_sources.iter().map(|s| s.as_str()).collect();
                ui.combo_simple_string("Select NDI Source", &mut self.selected_ndi, &source_names);

                if ui.button("Start NDI") {
                    let source_name = self.ndi_sources.get(self.selected_ndi as usize)
                        .cloned()
                        .unwrap_or_default();
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.input_command = InputCommand::StartNdi { source_name };
                }
            } else {
                ui.text_disabled("No NDI sources found");
            }
        }

        // Syphon section (macOS only)
        #[cfg(target_os = "macos")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            ui.text_colored([0.0, 1.0, 1.0, 1.0], "Syphon (macOS)");
            if !self.syphon_servers.is_empty() {
                let server_names: Vec<String> = self.syphon_servers.iter()
                    .map(|s| format!("{} - {}", s.app_name, s.name))
                    .collect();
                let server_name_refs: Vec<&str> = server_names.iter().map(|s| s.as_str()).collect();
                ui.combo_simple_string("Select Syphon Server", &mut self.selected_syphon, &server_name_refs);

                if ui.button("Start Syphon") {
                    let server_info = self.syphon_servers.get(self.selected_syphon).cloned();
                    if let Some(info) = server_info {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.input_command = InputCommand::StartSyphon {
                            server_name: info.display_name().to_string(),
                            server_uuid: info.uuid.clone(),
                        };
                    }
                }
            } else {
                ui.text_disabled("No Syphon servers found");
            }
        }

        // V4L2 section (Linux only)
        #[cfg(target_os = "linux")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            ui.text_colored([0.8, 0.8, 0.2, 1.0], "V4L2 Input (Linux)");
            if !self.v4l2_capture_devices.is_empty() {
                let labels: Vec<String> = self
                    .v4l2_capture_devices
                    .iter()
                    .map(|d| d.display_name())
                    .collect();
                let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                ui.combo_simple_string(
                    "Select V4L2 Device",
                    &mut self.selected_v4l2_capture,
                    &label_refs,
                );

                if ui.button("Start V4L2 Input") {
                    if let Some(info) = self.v4l2_capture_devices.get(self.selected_v4l2_capture) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.input_command = InputCommand::StartV4l2 {
                            device_path: info.path.clone(),
                        };
                    }
                }
            } else {
                ui.text_disabled("No V4L2 capture devices found");
                ui.text_disabled("Click Refresh Sources above to scan");
            }
        }

        // Spout section (Windows only)
        #[cfg(target_os = "windows")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            ui.text_colored([0.0, 1.0, 1.0, 1.0], "Spout (Windows)");
            if !self.spout_senders.is_empty() {
                let sender_names: Vec<&str> = self.spout_senders.iter()
                    .map(|s| s.name.as_str())
                    .collect();
                ui.combo_simple_string("Select Spout Sender", &mut self.selected_spout, &sender_names);

                if ui.button("Start Spout") {
                    let sender_name = self.spout_senders
                        .get(self.selected_spout)
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.input_command = InputCommand::StartSpout { sender_name };
                }
            } else {
                ui.text_disabled("No Spout senders found");
            }
        }

        ui.spacing();
        ui.separator();
        ui.spacing();

        // Stop button
        if is_active && ui.button("Stop Input") {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.input_command = InputCommand::StopInput;
        }
    }

    /// Build the input preview — fills the window with a center-crop
    pub(crate) fn build_input_preview(&mut self, ui: &imgui::Ui) {
        if let Some(texture_id) = self.input_preview_texture_id {
            let (input_width, input_height) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (state.input.width, state.input.height)
            };

            let avail = ui.content_region_avail();
            if avail[0] <= 0.0 || avail[1] <= 0.0 {
                return;
            }

            // UV extent of actual content within the fixed 1920×1080 preview texture
            let content_u = if input_width > 0 { (input_width as f32 / 1920.0).min(1.0) } else { 1.0 };
            let content_v = if input_height > 0 { (input_height as f32 / 1080.0).min(1.0) } else { 1.0 };

            let content_aspect = if input_width > 0 && input_height > 0 {
                input_width as f32 / input_height as f32
            } else {
                16.0 / 9.0
            };
            let container_aspect = avail[0] / avail[1];

            // Center-crop: image fills the container; excess is cropped evenly on each side
            let (uv0, uv1) = if content_aspect > container_aspect {
                // Content is wider → show full height, crop sides
                let visible = container_aspect / content_aspect;
                let pad = (1.0 - visible) / 2.0;
                ([pad * content_u, 0.0], [(1.0 - pad) * content_u, content_v])
            } else {
                // Content is taller → show full width, crop top/bottom
                let visible = content_aspect / container_aspect;
                let pad = (1.0 - visible) / 2.0;
                ([0.0, pad * content_v], [content_u, (1.0 - pad) * content_v])
            };

            imgui::Image::new(texture_id, avail)
                .uv0(uv0)
                .uv1(uv1)
                .build(ui);
        } else {
            ui.text_disabled("No input preview available");
        }
    }

    /// Build the output preview — fills the window with a center-crop
    pub(crate) fn build_output_preview(&mut self, ui: &imgui::Ui) {
        if let Some(texture_id) = self.output_preview_texture_id {
            let (internal_width, internal_height) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (state.resolution.internal_width, state.resolution.internal_height)
            };

            let avail = ui.content_region_avail();
            if avail[0] <= 0.0 || avail[1] <= 0.0 {
                return;
            }

            // UV extent of render_target content within the 1920×1080 preview texture
            let content_u = (internal_width as f32 / 1920.0).min(1.0);
            let content_v = (internal_height as f32 / 1080.0).min(1.0);

            let content_aspect = internal_width as f32 / internal_height as f32;
            let container_aspect = avail[0] / avail[1];

            let (uv0, uv1) = if content_aspect > container_aspect {
                let visible = container_aspect / content_aspect;
                let pad = (1.0 - visible) / 2.0;
                ([pad * content_u, 0.0], [(1.0 - pad) * content_u, content_v])
            } else {
                let visible = content_aspect / container_aspect;
                let pad = (1.0 - visible) / 2.0;
                ([0.0, pad * content_v], [content_u, (1.0 - pad) * content_v])
            };

            imgui::Image::new(texture_id, avail)
                .uv0(uv0)
                .uv1(uv1)
                .build(ui);
        } else {
            ui.text_disabled("No output preview available");
        }
    }
}
