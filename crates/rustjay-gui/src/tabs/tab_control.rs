//! # MIDI / OSC / Web Control Tabs
//!
//! Dynamically generates parameter controls from effect-declared descriptors.

use crate::control_gui::ControlGui;
use rustjay_core::{MidiCommand, MidiMsgKind, OscCommand, WebCommand, ParamCategory};

/// Return a sort order for standard categories (lower = earlier).
/// Custom categories always sort after standard ones.
fn category_order(cat: &ParamCategory) -> u8 {
    match cat {
        ParamCategory::Color => 0,
        ParamCategory::Motion => 1,
        ParamCategory::Audio => 2,
        ParamCategory::Output => 3,
        ParamCategory::Settings => 4,
        ParamCategory::Custom(_) => 5,
    }
}

/// Collect unique categories from descriptors and sort them: standard first,
/// then custom ones alphabetically.
fn sorted_categories(descriptors: &[rustjay_core::ParameterDescriptor]) -> Vec<ParamCategory> {
    let mut cats: Vec<_> = descriptors.iter()
        .map(|d| d.category.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();
    cats.sort_by(|a, b| {
        category_order(a).cmp(&category_order(b)).then_with(|| a.name().cmp(&b.name()))
    });
    cats
}

impl ControlGui {
    /// Build the MIDI tab with dynamically-generated learn buttons.
    pub(crate) fn build_midi_tab(&mut self, ui: &imgui::Ui) {
        ui.text("MIDI Control");
        ui.separator();

        // ── Device selection ────────────────────────────────────────────────────
        let (enabled, selected_device, available_devices) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.midi_enabled, state.midi_selected_device.clone(), state.midi_available_devices.clone())
        };

        let status_color = if enabled { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.5, 0.0, 1.0] };
        let status_text = if let Some(ref name) = selected_device {
            format!("Connected: {}", name)
        } else {
            "Not connected".to_string()
        };
        ui.text_colored(status_color, &status_text);

        ui.same_line();
        if ui.button("Refresh") {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.midi_command = MidiCommand::RefreshDevices;
        }

        if enabled {
            ui.same_line();
            if ui.button("Disconnect") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::Disconnect;
            }
        }

        ui.separator();
        ui.text("Available Devices:");

        if available_devices.is_empty() {
            ui.text_disabled("  (none found — click Refresh)");
        } else {
            for device in &available_devices {
                let is_selected = selected_device.as_deref() == Some(device.as_str());
                let label = if is_selected {
                    format!("* {}", device)
                } else {
                    format!("  {}", device)
                };
                ui.text(&label);
                ui.same_line();
                let btn_label = format!("Connect##{}", device);
                if ui.button(&btn_label) && !is_selected {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.midi_command = MidiCommand::SelectDevice(device.clone());
                }
            }
        }

        ui.separator();

        let (learn_active, learning_param_name, midi_mappings, descriptors) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.midi_learn_active,
                state.midi_learning_param_name.clone(),
                state.midi_mappings.clone(),
                state.param_descriptors.clone(),
            )
        };

        ui.text_colored([0.0, 1.0, 1.0, 1.0], "MIDI Learn");
        if learn_active {
            let waiting_label = if let Some(ref name) = learning_param_name {
                format!("Waiting for CC... (learning: {})", name)
            } else {
                "Waiting for CC...".to_string()
            };
            ui.text_colored([1.0, 1.0, 0.0, 1.0], &waiting_label);
            if ui.button("Cancel Learn") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::CancelLearn;
            }
        } else {
            ui.text("Click a parameter, then move a MIDI controller to map it.");
            if ui.button("Clear All Mappings") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::ClearMappings;
            }
        }

        ui.separator();

        if descriptors.is_empty() {
            ui.text_disabled("No effect-declared parameters.");
        } else {
            for cat in &sorted_categories(&descriptors) {
                let cat_params: Vec<_> = descriptors.iter().filter(|d| d.category == *cat).collect();
                if cat_params.is_empty() { continue; }

                let flags = if *cat == ParamCategory::Color || *cat == ParamCategory::Motion {
                    imgui::TreeNodeFlags::DEFAULT_OPEN
                } else {
                    imgui::TreeNodeFlags::empty()
                };

                if ui.collapsing_header(cat.name(), flags) {
                    ui.indent();
                    for desc in &cat_params {
                        let path = format!("{}/{}", cat.name().to_lowercase(), desc.id);
                        let mapping = midi_mappings.iter().find(|m| m.param_path == path);

                        let label = format!("Learn: {}##{}", desc.name, desc.id);
                        if ui.button(&label) {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.midi_command = MidiCommand::StartLearn {
                                param_path: path,
                                param_name: desc.name.clone(),
                                min: desc.min,
                                max: desc.max,
                            };
                        }
                        ui.same_line();
                        if let Some(m) = mapping {
                            let label = match m.kind {
                                MidiMsgKind::Cc         => format!("CC {} ch{}", m.selector, m.channel),
                                MidiMsgKind::Note       => format!("Note {} ch{}", m.selector, m.channel),
                                MidiMsgKind::Aftertouch => format!("AT ch{}", m.channel),
                            };
                            ui.text_colored([0.0, 1.0, 0.5, 1.0], &label);
                        } else {
                            ui.text_disabled("(unlearned)");
                        }
                    }
                    ui.unindent();
                }
            }
        }

        ui.separator();
        ui.text("Active Mappings");
        if midi_mappings.is_empty() {
            ui.text_disabled("No mappings configured yet — use MIDI Learn above");
        } else {
            for m in &midi_mappings {
                let binding = match m.kind {
                    MidiMsgKind::Cc         => format!("CC {} ch{}", m.selector, m.channel),
                    MidiMsgKind::Note       => format!("Note {} ch{}", m.selector, m.channel),
                    MidiMsgKind::Aftertouch => format!("AT ch{}", m.channel),
                };
                ui.text(format!("  {} -> {}", m.name, binding));
            }
        }
    }

    /// Build the OSC tab with dynamically-generated addresses.
    pub(crate) fn build_osc_tab(&mut self, ui: &imgui::Ui) {
        ui.text("OSC Control");
        ui.separator();

        let (running, port, _app_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.osc_enabled, state.osc_port, state.web_app_name.clone())
        };

        let status_color = if running { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
        let status_text = if running { "Running" } else { "Stopped" };

        ui.text("Server Status: ");
        ui.same_line();
        ui.text_colored(status_color, status_text);

        ui.separator();

        ui.text("Receive Port:");
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

        ui.text_colored([0.0, 1.0, 1.0, 1.0], "OSC Addresses");
        ui.text("Send OSC messages to control parameters:");

        // Get descriptors grouped by category
        let descriptors = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.param_descriptors.clone()
        };

        if descriptors.is_empty() {
            ui.text_disabled("No effect-declared parameters.");
        } else {
            for cat in &sorted_categories(&descriptors) {
                let cat_params: Vec<_> = descriptors.iter().filter(|d| d.category == *cat).collect();
                if cat_params.is_empty() { continue; }

                let flags = if *cat == ParamCategory::Color || *cat == ParamCategory::Motion {
                    imgui::TreeNodeFlags::DEFAULT_OPEN
                } else {
                    imgui::TreeNodeFlags::empty()
                };

                if ui.collapsing_header(cat.name(), flags) {
                    ui.indent();
                    for desc in &cat_params {
                        let addr = format!("/rustjay/{}/{}", cat.name().to_lowercase(), desc.id);
                        ui.text(&addr);
                        ui.text_disabled(format!("  Range: {} to {} (step: {})", desc.min, desc.max, desc.step));
                    }
                    ui.unindent();
                }
            }
        }

        // Always show engine-level addresses
        if ui.collapsing_header("Audio", imgui::TreeNodeFlags::empty()) {
            ui.indent();
            ui.text("/rustjay/audio/amplitude");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 0 to 5)");
            ui.text("/rustjay/audio/smoothing");
            ui.text_disabled("  Range: 0.0 - 1.0");
            ui.text("/rustjay/audio/enabled");
            ui.text_disabled("  Range: 0.0 or 1.0");
            ui.unindent();
        }

        if ui.collapsing_header("Output", imgui::TreeNodeFlags::empty()) {
            ui.indent();
            ui.text("/rustjay/output/fullscreen");
            ui.text_disabled("  Range: 0.0 or 1.0");
            ui.text("/rustjay/output/width");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 320 to 4096)");
            ui.text("/rustjay/output/height");
            ui.text_disabled("  Range: 0.0 - 1.0 (maps to 240 to 2160)");
            ui.unindent();
        }

        ui.separator();
        ui.text_disabled("Send an OSC message to the address above to confirm connectivity");
        ui.text_disabled("OSC is receive-only — Rustjay listens for incoming control messages.");

        ui.separator();
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Recent Messages");
        let messages = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.osc_message_log.clone()
        };
        if messages.is_empty() {
            ui.text_disabled("No messages received yet.");
        } else {
            ui.child_window("osc_messages")
                .size([0.0, 100.0])
                .build(|| {
                    for (addr, value, _time) in messages.iter().rev().take(20) {
                        ui.text(format!("{} = {:.3}", addr, value));
                    }
                });
        }
    }

    /// Build the Web tab.
    pub(crate) fn build_web_tab(&mut self, ui: &imgui::Ui) {
        ui.text("Web Remote Control");
        ui.separator();

        let (enabled, port, app_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.web_enabled, state.web_port, state.web_app_name.clone())
        };

        let status_color = if enabled { [0.0, 1.0, 0.0, 1.0] } else { [1.0, 0.0, 0.0, 1.0] };
        let status_text = if enabled { "Running" } else { "Stopped" };

        ui.text("Server Status: ");
        ui.same_line();
        ui.text_colored(status_color, status_text);

        ui.separator();

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

        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Features:");
        ui.bullet_text("Real-time bidirectional sync");
        ui.bullet_text("Works on any device with a browser");
        ui.bullet_text("Mobile-optimized touch interface");
        ui.bullet_text("Auto-generated controls for all parameters");
        ui.bullet_text("Multiple clients can connect simultaneously");
    }
}
