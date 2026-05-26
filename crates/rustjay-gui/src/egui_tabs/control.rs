//! Control tab — MIDI learn, OSC server, Web remote.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use rustjay_core::{MidiCommand, OscCommand, WebCommand, ParamCategory};

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

impl EguiControlGui {
    pub(crate) fn build_midi_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("MIDI Control");
        ui.add_space(8.0);

        if ui.button("🔄 Refresh Devices").clicked() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.midi_command = MidiCommand::RefreshDevices;
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("MIDI Learn").color(ACCENT_CYAN).strong());
        ui.label("Click a parameter, then move a MIDI controller to map it.");
        if ui.button("Clear All Mappings").clicked() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.midi_command = MidiCommand::ClearMappings;
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let descriptors = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.param_descriptors.clone()
        };

        if descriptors.is_empty() {
            ui.label(egui::RichText::new("No effect-declared parameters.").color(TEXT_SECONDARY));
        } else {
            for cat in &sorted_categories(&descriptors) {
                let cat_params: Vec<_> = descriptors.iter().filter(|d| d.category == *cat).collect();
                if cat_params.is_empty() { continue; }

                egui::CollapsingHeader::new(cat.name())
                    .default_open(false)
                    .show(ui, |ui| {
                        for desc in &cat_params {
                            let path = format!("{}/{}", cat.name().to_lowercase(), desc.id);
                            if ui.button(format!("Learn: {}", desc.name)).clicked() {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.midi_command = MidiCommand::StartLearn {
                                    param_path: path,
                                    param_name: desc.name.clone(),
                                };
                            }
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("(CC: --)").size(11.0).color(TEXT_SECONDARY));
                            });
                        }
                    });
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label("Active Mappings");
        ui.label(egui::RichText::new("No mappings configured yet — use MIDI Learn above").color(TEXT_SECONDARY));
    }

    pub(crate) fn build_osc_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("OSC Control");
        ui.add_space(8.0);

        let (running, port, _app_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.osc_enabled, state.osc_port, state.web_app_name.clone())
        };

        ui.horizontal(|ui| {
            ui.label("Server Status:");
            if running {
                self.status_badge(ui, "Running", true);
            } else {
                self.status_badge(ui, "Stopped", false);
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let mut port_i32 = port as i32;
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut port_i32).speed(1.0).range(1024..=65535));
            ui.label("Port");
            if running {
                if ui.button("⏹ Stop Server").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.osc_command = OscCommand::Stop;
                }
            } else {
                if ui.button("▶ Start Server").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.osc_command = OscCommand::Start;
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("OSC Addresses").color(ACCENT_CYAN).strong());
        ui.label("Send OSC messages to control parameters:");

        let descriptors = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.param_descriptors.clone()
        };

        if descriptors.is_empty() {
            ui.label(egui::RichText::new("No effect-declared parameters.").color(TEXT_SECONDARY));
        } else {
            for cat in &sorted_categories(&descriptors) {
                let cat_params: Vec<_> = descriptors.iter().filter(|d| d.category == *cat).collect();
                if cat_params.is_empty() { continue; }

                egui::CollapsingHeader::new(cat.name())
                    .default_open(false)
                    .show(ui, |ui| {
                        for desc in &cat_params {
                            let addr = format!("/rustjay/{}/{}", cat.name().to_lowercase(), desc.id);
                            ui.label(egui::RichText::new(&addr).monospace().size(12.0));
                            ui.label(egui::RichText::new(format!("  Range: {} to {} (step: {})", desc.min, desc.max, desc.step)).size(11.0).color(TEXT_SECONDARY));
                        }
                    });
            }
        }

        egui::CollapsingHeader::new("Audio")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("/rustjay/audio/amplitude").monospace().size(12.0));
                ui.label(egui::RichText::new("  Range: 0.0 - 1.0 (maps to 0 to 5)").size(11.0).color(TEXT_SECONDARY));
                ui.label(egui::RichText::new("/rustjay/audio/smoothing").monospace().size(12.0));
                ui.label(egui::RichText::new("  Range: 0.0 - 1.0").size(11.0).color(TEXT_SECONDARY));
                ui.label(egui::RichText::new("/rustjay/audio/enabled").monospace().size(12.0));
                ui.label(egui::RichText::new("  Range: 0.0 or 1.0").size(11.0).color(TEXT_SECONDARY));
            });

        egui::CollapsingHeader::new("Output")
            .default_open(false)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("/rustjay/output/fullscreen").monospace().size(12.0));
                ui.label(egui::RichText::new("  Range: 0.0 or 1.0").size(11.0).color(TEXT_SECONDARY));
                ui.label(egui::RichText::new("/rustjay/output/width").monospace().size(12.0));
                ui.label(egui::RichText::new("  Range: 0.0 - 1.0 (maps to 320 to 4096)").size(11.0).color(TEXT_SECONDARY));
                ui.label(egui::RichText::new("/rustjay/output/height").monospace().size(12.0));
                ui.label(egui::RichText::new("  Range: 0.0 - 1.0 (maps to 240 to 2160)").size(11.0).color(TEXT_SECONDARY));
            });

        ui.add_space(8.0);
        ui.label(egui::RichText::new("Send an OSC message to the address above to confirm connectivity").size(11.0).color(TEXT_SECONDARY));
    }

    pub(crate) fn build_web_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Web Remote Control");
        ui.add_space(8.0);

        let (enabled, port, app_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.web_enabled, state.web_port, state.web_app_name.clone())
        };

        ui.horizontal(|ui| {
            ui.label("Server Status:");
            if enabled {
                self.status_badge(ui, "Running", true);
            } else {
                self.status_badge(ui, "Stopped", false);
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        let mut port_i32 = port as i32;
        ui.horizontal(|ui| {
            ui.add(egui::DragValue::new(&mut port_i32).speed(1.0).range(1024..=65535));
            ui.label("Port");
            if enabled {
                if ui.button("⏹ Stop Server").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.web_command = WebCommand::Stop;
                }
            } else {
                if ui.button("▶ Start Server").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.web_command = WebCommand::Start;
                }
            }
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        if enabled {
            ui.label(egui::RichText::new("Access URL:").color(ACCENT_CYAN).strong());
            let local_ip = crate::control_gui::get_local_ip().unwrap_or_else(|| "localhost".to_string());
            let url = format!("http://{}:{}/{}", local_ip, port, app_name);
            ui.label(egui::RichText::new(&url).monospace().size(13.0));

            if ui.button("📋 Copy URL to Clipboard").clicked() {
                if let Err(e) = crate::control_gui::copy_to_clipboard(&url) {
                    log::warn!("Failed to copy URL to clipboard: {}", e);
                }
            }

            ui.add_space(8.0);
            ui.label("Scan with your phone or open in a browser on the same network.");
            ui.label(egui::RichText::new("The web interface provides real-time control of all parameters.").size(11.0).color(TEXT_SECONDARY));
        } else {
            ui.label(egui::RichText::new("Start the server to get the access URL.").color(TEXT_SECONDARY));
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("Features:").color(ACCENT_CYAN).strong());
        ui.label("• Real-time bidirectional sync");
        ui.label("• Works on any device with a browser");
        ui.label("• Mobile-optimized touch interface");
        ui.label("• Auto-generated controls for all parameters");
        ui.label("• Multiple clients can connect simultaneously");
    }
}
