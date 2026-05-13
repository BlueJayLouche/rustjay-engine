use crate::control_gui::ControlGui;
use rustjay_core::{MidiCommand, OscCommand, WebCommand};

impl ControlGui {
    /// Build the MIDI tab
    pub(crate) fn build_midi_tab(&mut self, ui: &imgui::Ui) {
        ui.text("MIDI Control");
        ui.separator();

        // Device selection - TODO: Get from MidiState
        // For now, these are placeholders until MidiState is integrated with GUI
        let _enabled = false;
        let _device: Option<String> = None;

        ui.text("Device:");
        ui.same_line();

        // TODO: Device dropdown from MidiState
        if ui.button("Refresh Devices") {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.midi_command = MidiCommand::RefreshDevices;
        }

        ui.separator();

        // Learn mode section
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "MIDI Learn");
        ui.text("Click a parameter below, then move a MIDI controller to map it.");

        if ui.button("Clear All Mappings") {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.midi_command = MidiCommand::ClearMappings;
        }

        ui.separator();

        // Mappable parameters
        ui.text("Parameters");

        // Color parameters
        if ui.collapsing_header("Color", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            ui.indent();

            if ui.button("Learn: Hue Shift") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::StartLearn {
                    param_path: "color/hue_shift".to_string(),
                    param_name: "Hue Shift".to_string(),
                };
            }
            ui.same_line();
            ui.text_disabled("(CC: --)");

            if ui.button("Learn: Saturation") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::StartLearn {
                    param_path: "color/saturation".to_string(),
                    param_name: "Saturation".to_string(),
                };
            }
            ui.same_line();
            ui.text_disabled("(CC: --)");

            if ui.button("Learn: Brightness") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::StartLearn {
                    param_path: "color/brightness".to_string(),
                    param_name: "Brightness".to_string(),
                };
            }
            ui.same_line();
            ui.text_disabled("(CC: --)");

            ui.unindent();
        }

        // Audio parameters
        if ui.collapsing_header("Audio", imgui::TreeNodeFlags::empty()) {
            ui.indent();

            if ui.button("Learn: Amplitude") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::StartLearn {
                    param_path: "audio/amplitude".to_string(),
                    param_name: "Audio Amplitude".to_string(),
                };
            }

            if ui.button("Learn: Smoothing") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::StartLearn {
                    param_path: "audio/smoothing".to_string(),
                    param_name: "Audio Smoothing".to_string(),
                };
            }

            ui.unindent();
        }

        ui.separator();

        // Active mappings
        ui.text("Active Mappings");
        ui.text_disabled("MIDI mappings will appear here");
    }

    /// Build the OSC tab
    pub(crate) fn build_osc_tab(&mut self, ui: &imgui::Ui) {
        ui.text("OSC Control");
        ui.separator();

        // Server settings - read from shared state
        let (running, port) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            // For now, we track running status in the state
            // In a full implementation, this would come from OscState
            (state.osc_enabled, state.osc_port)
        };

        // Status indicator
        let status_color = if running { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
        let status_text = if running { "Running" } else { "Stopped" };

        ui.text("Server Status: ");
        ui.same_line();
        ui.text_colored(status_color, status_text);

        ui.separator();

        // Port configuration
        ui.text("Port:");
        ui.same_line();

        let mut port_i32 = port as i32;
        ui.set_next_item_width(100.0);
        if ui.input_int("##osc_port", &mut port_i32).build() {
            let new_port = port_i32.clamp(1024, 65535) as u16;
            if new_port != port {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.osc_command = OscCommand::SetPort(new_port);
            }
        }

        ui.same_line();

        // Start/Stop button
        if running {
            if ui.button("Stop Server") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.osc_command = OscCommand::Stop;
            }
        } else {
            if ui.button("Start Server") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.osc_command = OscCommand::Start;
            }
        }

        ui.separator();

        // Address information
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "OSC Addresses");
        ui.text("Send OSC messages to control parameters:");

        if ui.collapsing_header("Color", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            ui.indent();
            ui.text("/rustjay-template/color/hue_shift");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to -180 to 180)");

            ui.text("/rustjay-template/color/saturation");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 0 to 2)");

            ui.text("/rustjay-template/color/brightness");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 0 to 2)");

            ui.text("/rustjay-template/color/enabled");
            ui.text_disabled("  Range: 0.0 or 1.0");
            ui.unindent();
        }

        if ui.collapsing_header("Audio", imgui::TreeNodeFlags::empty()) {
            ui.indent();
            ui.text("/rustjay-template/audio/amplitude");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 0 to 5)");

            ui.text("/rustjay-template/audio/smoothing");
            ui.text_disabled("  Range: 0.0 - 1.0");

            ui.text("/rustjay-template/audio/enabled");
            ui.text_disabled("  Range: 0.0 or 1.0");
            ui.unindent();
        }

        if ui.collapsing_header("Output", imgui::TreeNodeFlags::empty()) {
            ui.indent();
            ui.text("/rustjay-template/output/fullscreen");
            ui.text_disabled("  Range: 0.0 or 1.0");

            ui.text("/rustjay-template/output/width");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 320 to 4096)");

            ui.text("/rustjay-template/output/height");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 240 to 2160)");
            ui.unindent();
        }

        ui.separator();

        // Message log
        ui.text("Recent Messages");
        ui.text_disabled("(Last 100 OSC messages will appear here)");
    }

    /// Build the Web tab
    pub(crate) fn build_web_tab(&mut self, ui: &imgui::Ui) {
        ui.text("Web Remote Control");
        ui.separator();

        // Get current state
        let (enabled, port, app_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.web_enabled, state.web_port, state.web_app_name.clone())
        };

        // Status indicator
        let status_color = if enabled { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
        let status_text = if enabled { "Running" } else { "Stopped" };

        ui.text("Server Status: ");
        ui.same_line();
        ui.text_colored(status_color, status_text);

        ui.separator();

        // Port configuration
        ui.text("Port:");
        ui.same_line();

        let mut port_i32 = port as i32;
        ui.set_next_item_width(100.0);
        if ui.input_int("##web_port", &mut port_i32).build() {
            let new_port = port_i32.clamp(1024, 65535) as u16;
            if new_port != port {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.web_command = WebCommand::SetPort(new_port);
            }
        }

        ui.same_line();

        // Start/Stop button
        if enabled {
            if ui.button("Stop Server") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.web_command = WebCommand::Stop;
            }
        } else {
            if ui.button("Start Server") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.web_command = WebCommand::Start;
            }
        }

        ui.separator();

        // URL display
        if enabled {
            ui.text_colored([0.0, 1.0, 1.0, 1.0], "Access URL:");

            let local_ip = crate::control_gui::get_local_ip().unwrap_or_else(|| "localhost".to_string());
            let url = format!("http://{}:{}/{}", local_ip, port, app_name);

            ui.text(&url);

            if ui.button("Copy URL to Clipboard") {
                if let Err(e) = crate::control_gui::copy_to_clipboard(&url) {
                    log::warn!("Failed to copy URL to clipboard: {}", e);
                } else {
                    ui.tooltip_text("URL copied!");
                }
            }

            ui.separator();

            ui.text("Scan with your phone or open in a browser on the same network.");
            ui.text_disabled("The web interface provides real-time control of all parameters.");
        } else {
            ui.text_disabled("Start the server to get the access URL.");
        }

        ui.separator();

        // Features list
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Features:");
        ui.bullet_text("Real-time bidirectional sync");
        ui.bullet_text("Works on any device with a browser");
        ui.bullet_text("Mobile-optimized touch interface");
        ui.bullet_text("Auto-generated controls for all parameters");
        ui.bullet_text("Multiple clients can connect simultaneously");
    }
}
