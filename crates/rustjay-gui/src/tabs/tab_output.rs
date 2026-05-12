use crate::control_gui::ControlGui;
use rustjay_core::OutputCommand;

impl ControlGui {
    /// Build the Output tab
    pub(crate) fn build_output_tab(&mut self, ui: &imgui::Ui) {
        let fullscreen = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.output_fullscreen
        };
        #[cfg(feature = "ndi")]
        let ndi_active = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.ndi_output.is_active
        };

        ui.text("Output Settings");
        ui.separator();

        // Fullscreen toggle
        let mut fs = fullscreen;
        if ui.checkbox("Fullscreen Output", &mut fs) {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.output_fullscreen = fs;
        }

        ui.text_disabled("Press Shift+F to toggle fullscreen");

        // NDI Output
        #[cfg(feature = "ndi")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            ui.text_colored([0.0, 1.0, 0.5, 1.0], "NDI Output");
            ui.input_text("Stream Name", &mut self.ndi_output_name).build();

            if !ndi_active {
                if ui.button("Start NDI Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.ndi_output.stream_name = self.ndi_output_name.clone();
                    state.output_command = OutputCommand::StartNdi;
                }
            } else {
                if ui.button("Stop NDI Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopNdi;
                }
                ui.text_colored([0.0, 1.0, 0.0, 1.0], "NDI Active");
            }
        }

        // Syphon Output (macOS)
        #[cfg(target_os = "macos")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            let syphon_enabled = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.syphon_output.enabled
            };

            ui.text_colored([1.0, 0.5, 0.0, 1.0], "Syphon Output (macOS)");
            ui.input_text("Server Name", &mut self.syphon_output_name).build();

            if !syphon_enabled {
                if ui.button("Start Syphon Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.syphon_output.server_name = self.syphon_output_name.clone();
                    state.output_command = OutputCommand::StartSyphon;
                }
            } else {
                if ui.button("Stop Syphon Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopSyphon;
                }
                ui.text_colored([0.0, 1.0, 0.0, 1.0], "Syphon Active");
            }
        }

        // Spout Output (Windows)
        #[cfg(target_os = "windows")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            let spout_active = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.spout_output.enabled
            };

            ui.text_colored([0.3, 0.6, 1.0, 1.0], "Spout Output (Windows)");
            ui.input_text("Spout Sender Name##out", &mut self.spout_output_name).build();

            if !spout_active {
                if ui.button("Start Spout Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StartSpout {
                        sender_name: self.spout_output_name.clone(),
                    };
                }
            } else {
                if ui.button("Stop Spout Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopSpout;
                }
                ui.text_colored([0.0, 1.0, 0.0, 1.0], "Spout Active");
            }
        }

        // V4L2 Loopback Output (Linux)
        #[cfg(target_os = "linux")]
        {
            ui.spacing();
            ui.separator();
            ui.spacing();

            let v4l2_active = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.v4l2_output.enabled
            };

            ui.text_colored([0.8, 0.8, 0.2, 1.0], "V4L2 Loopback Output (Linux)");
            ui.text_disabled("Requires v4l2loopback kernel module");

            if !self.v4l2_output_devices.is_empty() {
                let labels: Vec<String> = self
                    .v4l2_output_devices
                    .iter()
                    .map(|d| d.display_name())
                    .collect();
                let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                if ui.combo_simple_string(
                    "Loopback Device",
                    &mut self.selected_v4l2_output,
                    &label_refs,
                ) {
                    if let Some(d) = self.v4l2_output_devices.get(self.selected_v4l2_output) {
                        self.v4l2_device_path = d.path.clone();
                    }
                }
            } else {
                ui.text_disabled("No v4l2loopback devices found — see README for setup");
                ui.input_text("Device Path", &mut self.v4l2_device_path).build();
            }

            if !v4l2_active {
                if ui.button("Start V4L2 Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StartV4l2 {
                        device_path: self.v4l2_device_path.clone(),
                    };
                }
            } else {
                if ui.button("Stop V4L2 Output") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.output_command = OutputCommand::StopV4l2;
                }
                ui.same_line();
                ui.text_colored(
                    [0.0, 1.0, 0.0, 1.0],
                    &format!("V4L2 Active: {}", self.v4l2_device_path),
                );
            }
        }
    }
}
