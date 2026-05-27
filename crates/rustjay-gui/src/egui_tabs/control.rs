//! Control tab — MIDI learn, OSC server, Web remote.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use rustjay_core::{MidiCommand, MidiMsgKind, OscCommand, WebCommand, ParamCategory};

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

        // ── Device selection ────────────────────────────────────────────────────
        let (enabled, selected_device, available_devices) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.midi_enabled, state.midi_selected_device.clone(), state.midi_available_devices.clone())
        };

        ui.horizontal(|ui| {
            ui.label("Status:");
            if enabled {
                let name = selected_device.as_deref().unwrap_or("unknown");
                self.status_badge(ui, &format!("Connected: {}", name), true);
            } else {
                self.status_badge(ui, "Not connected", false);
            }
        });

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("🔄 Refresh").clicked() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::RefreshDevices;
            }
            if enabled && ui.button("⏹ Disconnect").clicked() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::Disconnect;
            }
        });

        ui.add_space(4.0);
        ui.label(egui::RichText::new("Available Devices").strong());

        if available_devices.is_empty() {
            ui.label(egui::RichText::new("None found — click Refresh").size(11.0).color(TEXT_SECONDARY));
        } else {
            for device in &available_devices {
                let is_selected = selected_device.as_deref() == Some(device.as_str());
                ui.horizontal(|ui| {
                    if is_selected {
                        ui.label(egui::RichText::new("▶").color(ACCENT_CYAN));
                    } else {
                        ui.label("  ");
                    }
                    ui.label(device);
                    if !is_selected && ui.small_button("Connect").clicked() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.midi_command = MidiCommand::SelectDevice(device.clone());
                    }
                });
            }
        }

        let (learn_active, learning_param_name, midi_mappings, descriptors) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.midi_learn_active,
                state.midi_learning_param_name.clone(),
                state.midi_mappings.clone(),
                state.param_descriptors.clone(),
            )
        };

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("MIDI Learn").color(ACCENT_CYAN).strong());

        if learn_active {
            let waiting_text = if let Some(ref name) = learning_param_name {
                format!("⏳ Waiting for CC... ({})", name)
            } else {
                "⏳ Waiting for CC...".to_string()
            };
            ui.label(egui::RichText::new(&waiting_text).color(egui::Color32::YELLOW).strong());
            if ui.button("✖ Cancel Learn").clicked() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::CancelLearn;
            }
        } else {
            ui.label("Click a parameter, then move a MIDI controller to map it.");
            if ui.button("Clear All Mappings").clicked() {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.midi_command = MidiCommand::ClearMappings;
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

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
                            let mapping = midi_mappings.iter().find(|(_, p, _, _, _)| p == &path);
                            ui.horizontal(|ui| {
                                if ui.button(format!("Learn: {}", desc.name)).clicked() {
                                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.midi_command = MidiCommand::StartLearn {
                                        param_path: path,
                                        param_name: desc.name.clone(),
                                        min: desc.min,
                                        max: desc.max,
                                    };
                                }
                                if let Some((_, _, kind, sel, ch)) = mapping {
                                    let label = match kind {
                                        MidiMsgKind::Cc         => format!("CC {} ch{}", sel, ch),
                                        MidiMsgKind::Note       => format!("Note {} ch{}", sel, ch),
                                        MidiMsgKind::Aftertouch => format!("AT ch{}", ch),
                                    };
                                    ui.label(egui::RichText::new(label)
                                        .size(11.0).color(egui::Color32::from_rgb(0, 220, 130)));
                                } else {
                                    ui.label(egui::RichText::new("(unlearned)").size(11.0).color(TEXT_SECONDARY));
                                }
                            });
                        }
                    });
            }
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);
        ui.label("Active Mappings");
        if midi_mappings.is_empty() {
            ui.label(egui::RichText::new("No mappings configured yet — use MIDI Learn above").color(TEXT_SECONDARY));
        } else {
            for (name, _path, kind, sel, ch) in &midi_mappings {
                let binding = match kind {
                    MidiMsgKind::Cc         => format!("CC {} ch{}", sel, ch),
                    MidiMsgKind::Note       => format!("Note {} ch{}", sel, ch),
                    MidiMsgKind::Aftertouch => format!("AT ch{}", ch),
                };
                ui.label(egui::RichText::new(format!("  {} → {}", name, binding))
                    .size(11.0).monospace());
            }
        }
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
            let response = ui.add(egui::DragValue::new(&mut port_i32).speed(1.0).range(1024..=65535));
            ui.label("Receive Port");
            if response.changed() {
                let new_port = port_i32.clamp(1024, 65535) as u16;
                if new_port != port {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.osc_command = OscCommand::SetPort(new_port);
                }
            }
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
        ui.label(egui::RichText::new("OSC is receive-only — Rustjay listens for incoming control messages.").size(11.0).color(TEXT_SECONDARY));

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("Recent Messages").color(ACCENT_CYAN).strong());
        let messages = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.osc_message_log.clone()
        };
        if messages.is_empty() {
            ui.label(egui::RichText::new("No messages received yet.").size(11.0).color(TEXT_SECONDARY));
        } else {
            egui::ScrollArea::vertical().max_height(100.0).show(ui, |ui| {
                for (addr, value, _time) in messages.iter().rev().take(20) {
                    ui.label(egui::RichText::new(format!("{} = {:.3}", addr, value)).monospace().size(12.0));
                }
            });
        }
    }

    pub(crate) fn build_web_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Web Remote Control");
        ui.add_space(8.0);

        let (enabled, port, app_name, full_url, lan_trust) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.web_enabled, state.web_port, state.web_app_name.clone(),
             state.web_full_url.clone(), state.web_lan_trust)
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

        if enabled && !full_url.is_empty() {
            // ── QR code ─────────────────────────────────────────────────────────
            // Determine the URL to encode: with LAN trust the token is omitted
            // from the QR so people can bookmark the plain URL; the token URL
            // still works normally for clients that have it.
            let qr_url = if lan_trust {
                // Strip token from URL for LAN trust mode (clients need no token)
                full_url.split('?').next().unwrap_or(&full_url).to_string()
            } else {
                full_url.clone()
            };

            // Rebuild the matrix only when the URL changes.
            let need_rebuild = self.qr_cache.as_ref().map_or(true, |(u, _)| u != &qr_url);
            if need_rebuild {
                if let Ok(code) = qrcode::QrCode::new(qr_url.as_bytes()) {
                    let width = code.width();
                    let matrix: Vec<Vec<bool>> = (0..width)
                        .map(|row| (0..width)
                            .map(|col| code[(row, col)] == qrcode::Color::Dark)
                            .collect())
                        .collect();
                    self.qr_cache = Some((qr_url.clone(), matrix));
                }
            }

            if let Some((_, matrix)) = &self.qr_cache {
                let qr_size = 200.0_f32;
                let width = matrix.len();
                let cell = qr_size / width as f32;

                // Reserve space and draw the QR code using the painter.
                let (rect, _) = ui.allocate_exact_size(
                    egui::vec2(qr_size, qr_size),
                    egui::Sense::hover(),
                );
                let painter = ui.painter();
                // White background with a quiet zone
                painter.rect_filled(rect, 4.0, egui::Color32::WHITE);
                let top_left = rect.min;
                for (row, cols) in matrix.iter().enumerate() {
                    for (col, &dark) in cols.iter().enumerate() {
                        if dark {
                            let min = top_left + egui::vec2(col as f32 * cell, row as f32 * cell);
                            painter.rect_filled(
                                egui::Rect::from_min_size(min, egui::vec2(cell, cell)),
                                0.0,
                                egui::Color32::BLACK,
                            );
                        }
                    }
                }

                ui.add_space(4.0);
                ui.label(egui::RichText::new("Scan with your phone to connect instantly").size(11.0).color(TEXT_SECONDARY));
            }

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            // ── URL + copy button ────────────────────────────────────────────────
            ui.label(egui::RichText::new("Access URL:").color(ACCENT_CYAN).strong());
            ui.label(egui::RichText::new(&full_url).monospace().size(11.0).color(TEXT_SECONDARY));
            if ui.button("📋 Copy URL").clicked() {
                if let Err(e) = crate::control_gui::copy_to_clipboard(&full_url) {
                    log::warn!("Failed to copy URL to clipboard: {}", e);
                }
            }

        } else if enabled {
            ui.label(egui::RichText::new("Server starting…").color(TEXT_SECONDARY));
        } else {
            ui.label(egui::RichText::new("Start the server to get the QR code and URL.").color(TEXT_SECONDARY));
            // Clear cached QR when server is stopped.
            self.qr_cache = None;
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(8.0);

        // ── LAN Trust toggle ─────────────────────────────────────────────────
        ui.label(egui::RichText::new("Security").color(ACCENT_CYAN).strong());
        let mut lan_trust_mut = lan_trust;
        if ui.checkbox(&mut lan_trust_mut, "Trusted LAN Mode").changed() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.web_command = WebCommand::SetLanTrust(lan_trust_mut);
        }
        if lan_trust {
            ui.label(egui::RichText::new("Any device on your local network can connect without a token.")
                .size(11.0).color(egui::Color32::from_rgb(255, 200, 100)));
        } else {
            ui.label(egui::RichText::new("Token auth required — only devices with the QR code or URL can connect.")
                .size(11.0).color(TEXT_SECONDARY));
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
