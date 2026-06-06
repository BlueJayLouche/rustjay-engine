//! Audio tab — device selection, analysis settings, FFT monitor, tempo/sync, routing.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use rustjay_core::{AudioCommand, SyncSource};

impl EguiControlGui {
    pub(crate) fn build_audio_tab(&mut self, ui: &mut egui::Ui) {
        let (mut enabled, mut amplitude, mut smoothing, fft, volume, selected_device) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.audio.enabled,
                state.audio.amplitude,
                state.audio.smoothing,
                state.audio.fft,
                state.audio.volume,
                state.audio.selected_device.clone(),
            )
        };

        // ── Input Device ──────────────────────────────────────────────────────
        egui::CollapsingHeader::new("🎤 Input Device")
            .default_open(true)
            .show(ui, |ui| {
                if ui.button("🔄 Refresh Audio Devices").clicked() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_command = AudioCommand::RefreshDevices;
                }
                ui.add_space(8.0);

                if !self.audio_devices.is_empty() {
                    let device_names: Vec<&str> =
                        self.audio_devices.iter().map(|s| s.as_str()).collect();
                    if let Some(ref current) = selected_device {
                        if let Some(idx) = self.audio_devices.iter().position(|d| d == current) {
                            self.selected_audio_device = idx;
                        }
                    }
                    egui::ComboBox::from_id_salt("audio_dev")
                        .width(240.0)
                        .selected_text(
                            device_names
                                .get(self.selected_audio_device)
                                .copied()
                                .unwrap_or("?"),
                        )
                        .show_ui(ui, |ui| {
                            for (i, name) in device_names.iter().enumerate() {
                                if ui
                                    .selectable_label(self.selected_audio_device == i, *name)
                                    .clicked()
                                {
                                    self.selected_audio_device = i;
                                    let device_name = self.audio_devices.get(i).cloned();
                                    let mut state =
                                        self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                    state.audio_command =
                                        AudioCommand::SelectDevice(device_name.unwrap_or_default());
                                }
                            }
                        });
                    if let Some(ref device) = selected_device {
                        ui.label(format!("Active: {}", device));
                    }
                } else {
                    ui.label(
                        egui::RichText::new("No audio devices found. Click Refresh.")
                            .color(TEXT_SECONDARY),
                    );
                }

                ui.add_space(8.0);
                if ui.checkbox(&mut enabled, "Enable Audio Analysis").changed() {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio.enabled = enabled;
                    state.audio_command = if enabled {
                        AudioCommand::Start
                    } else {
                        AudioCommand::Stop
                    };
                }
            });

        ui.add_space(8.0);

        // ── Analysis Settings ─────────────────────────────────────────────────
        if enabled {
            egui::CollapsingHeader::new("🔧 Analysis Settings")
                .default_open(true)
                .show(ui, |ui| {
                    let (mut normalize, mut pink_noise, current_fft_size) = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        (
                            state.audio.normalize,
                            state.audio.pink_noise_shaping,
                            state.audio.fft_size,
                        )
                    };

                    ui.label("Amplitude");
                    if ui
                        .add(egui::Slider::new(&mut amplitude, 0.1..=5.0).trailing_fill(true))
                        .changed()
                    {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.audio.amplitude = amplitude;
                    }

                    ui.horizontal(|ui| {
                        ui.label("Smoothing");
                        ui.label(
                            egui::RichText::new("(0 = instant, 0.99 = very slow)")
                                .size(11.0)
                                .color(TEXT_SECONDARY),
                        );
                    });
                    if ui
                        .add(egui::Slider::new(&mut smoothing, 0.0..=0.95).trailing_fill(true))
                        .changed()
                    {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.audio.smoothing = smoothing.clamp(0.0, 0.99);
                    }

                    {
                        use rustjay_audio::{FFT_SIZES, FFT_SIZE_LABELS};
                        let mut selected_idx = FFT_SIZES
                            .iter()
                            .position(|&s| s == current_fft_size)
                            .unwrap_or(2);
                        ui.label("FFT Size");
                        egui::ComboBox::from_id_salt("fft_size")
                            .width(120.0)
                            .selected_text(FFT_SIZE_LABELS[selected_idx])
                            .show_ui(ui, |ui| {
                                for (i, label) in FFT_SIZE_LABELS.iter().enumerate() {
                                    if ui.selectable_label(selected_idx == i, *label).clicked() {
                                        selected_idx = i;
                                    }
                                }
                            });
                        if let Some(&new_size) = FFT_SIZES.get(selected_idx) {
                            if new_size != current_fft_size {
                                let mut state =
                                    self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.audio.fft_size = new_size;
                                state.audio_command = AudioCommand::SetFftSize(new_size);
                            }
                        }
                    }

                    if ui.checkbox(&mut normalize, "Normalize Bands").changed() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.audio.normalize = normalize;
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("(Auto-gain across all bands)")
                                .size(11.0)
                                .color(TEXT_SECONDARY),
                        );
                    });

                    if ui
                        .checkbox(&mut pink_noise, "+3dB/Octave Shaping")
                        .changed()
                    {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.audio.pink_noise_shaping = pink_noise;
                    }
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("(Compensates for pink noise spectrum)")
                                .size(11.0)
                                .color(TEXT_SECONDARY),
                        );
                    });
                });

            ui.add_space(8.0);

            // ── Tempo & Sync ──────────────────────────────────────────────────
            egui::CollapsingHeader::new("⏱ Tempo & Sync")
                .default_open(true)
                .show(ui, |ui| {
                    let mut sync_source = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.sync_source
                    };

                    let sources: &[(&str, SyncSource)] = &[
                        ("Audio / Tap Tempo", SyncSource::Audio),
                        #[cfg(feature = "link")]
                        ("Ableton Link", SyncSource::AbletonLink),
                        #[cfg(feature = "prodj")]
                        ("ProDJ Link", SyncSource::ProDj),
                    ];

                    for &(label, variant) in sources {
                        if ui.radio_value(&mut sync_source, variant, label).changed() {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.sync_source = variant;
                            #[cfg(feature = "link")]
                            {
                                state.link_command = if variant == SyncSource::AbletonLink {
                                    rustjay_core::LinkCommand::Enable
                                } else {
                                    rustjay_core::LinkCommand::Disable
                                };
                            }
                            #[cfg(feature = "prodj")]
                            {
                                state.prodj_command = if variant == SyncSource::ProDj {
                                    rustjay_core::ProDjCommand::Start
                                } else {
                                    rustjay_core::ProDjCommand::Stop
                                };
                            }
                        }
                    }

                    ui.add_space(8.0);

                    match sync_source {
                        SyncSource::Audio => {
                            let (bpm, tap_info) = {
                                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                (state.audio.bpm, state.audio.tap_tempo_info.clone())
                            };
                            ui.horizontal(|ui| {
                                ui.label(format!("BPM: {:.1}", bpm));
                                ui.add_space(16.0);
                                if ui.button(egui::RichText::new("TAP").strong().size(16.0)).clicked() {
                                    self.handle_tap_tempo();
                                }
                                ui.label(egui::RichText::new(&tap_info).size(11.0).color(TEXT_SECONDARY));
                            });
                        }

                        #[cfg(feature = "link")]
                        SyncSource::AbletonLink => {
                            let (link_peers, link_bpm, link_phase, mut link_quantum, link_playing) = {
                                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                (
                                    state.link.num_peers,
                                    state.link.bpm,
                                    state.link.beat_phase,
                                    state.link.quantum,
                                    state.link.is_playing,
                                )
                            };
                            ui.label(format!("BPM: {:.2}  |  Peers: {}  |  Playing: {}", link_bpm, link_peers, if link_playing { "Yes" } else { "No" }));
                            ui.label("Beat phase");
                            let progress = egui::ProgressBar::new(link_phase)
                                .text(format!("{:.0}%", link_phase * 100.0))
                                .desired_width(200.0);
                            ui.add(progress);
                            let mut quantum_f32 = link_quantum as f32;
                            if ui.add(egui::Slider::new(&mut quantum_f32, 1.0..=16.0).text("Quantum").trailing_fill(true)).changed() {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.link.quantum = quantum_f32 as f64;
                                state.link_command = rustjay_core::LinkCommand::SetQuantum(quantum_f32 as f64);
                            }
                        }

                        #[cfg(feature = "prodj")]
                        SyncSource::ProDj => {
                            let (prodj_devices, prodj_master_bpm, prodj_artist, prodj_title) = {
                                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                (
                                    state.prodj.devices.clone(),
                                    state.prodj.master_bpm,
                                    state.prodj.current_track_artist.clone(),
                                    state.prodj.current_track_title.clone(),
                                )
                            };
                            ui.label(format!("Master BPM: {:.2}", prodj_master_bpm));
                            if !prodj_artist.is_empty() || !prodj_title.is_empty() {
                                ui.label(format!("Track: {} - {}", prodj_artist, prodj_title));
                            }
                            if !prodj_devices.is_empty() {
                                for device in &prodj_devices {
                                    let master_tag = if device.is_master { " [MASTER]" } else { "" };
                                    let playing_tag = if device.is_playing { "▶" } else { "⏸" };
                                    ui.label(format!(
                                        "{} Deck {}: {}{} | BPM: {:.2}",
                                        playing_tag, device.device_id, device.name, master_tag,
                                        device.bpm.unwrap_or(0.0)
                                    ));
                                }
                            } else {
                                ui.label(egui::RichText::new("No devices discovered yet...").color(TEXT_SECONDARY));
                            }
                        }

                        #[allow(unreachable_patterns)]
                        _ => {
                            ui.label(egui::RichText::new("Selected source is not compiled in.").color(TEXT_SECONDARY));
                        }
                    }

                    // MTC
                    #[cfg(feature = "mtc")]
                    {
                        ui.add_space(12.0);
                        ui.label(egui::RichText::new("MIDI Timecode (MTC)").color(ACCENT_CYAN).strong());
                        let (running, playing, position, source) = {
                            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            (state.mtc.running, state.mtc.playing, state.mtc.position, state.mtc.source_device.clone())
                        };
                        let (status_color, status_text) = if playing {
                            (ACCENT_GREEN, "Playing")
                        } else if running {
                            (ACCENT_AMBER, "Stopped")
                        } else {
                            (TEXT_SECONDARY, "No signal")
                        };
                        ui.horizontal(|ui| {
                            ui.label("Status:");
                            ui.colored_label(status_color, status_text);
                        });
                        if !source.is_empty() {
                            ui.label(format!("Source: {}", source));
                        }
                        ui.label(format!("Position: {}  [{}]", position, position.frame_rate.name()));
                        ui.label(egui::RichText::new("Listening on all MIDI ports automatically.").size(11.0).color(TEXT_SECONDARY));
                    }
                });

            ui.add_space(8.0);

            // ── Frequency Monitor ─────────────────────────────────────────────
            egui::CollapsingHeader::new("📊 Frequency Monitor")
                .default_open(true)
                .show(ui, |ui| {
                    const BAND_NAMES: [&str; 8] = [
                        "Sub", "Bass", "Lo Mid", "Mid", "Hi Mid", "High", "V.High", "Pres",
                    ];
                    let avail_w = ui.available_width();
                    let label_col = 50.0;
                    let val_col = 34.0;
                    let gap = 6.0;
                    let bar_w = (avail_w - label_col - val_col - gap).max(20.0);

                    for (i, (&value, &name)) in fft.iter().zip(BAND_NAMES.iter()).enumerate() {
                        let color = FFT_BANDS[i];
                        ui.horizontal(|ui| {
                            ui.add_space(4.0);
                            ui.colored_label(color, name);
                            ui.add_space(4.0);
                            let rect = ui.available_rect_before_wrap();
                            let bar_rect = egui::Rect::from_min_size(
                                egui::pos2(rect.min.x, rect.min.y + 3.0),
                                egui::vec2(bar_w * value.clamp(0.0, 1.0), 11.0),
                            );
                            let bg_rect = egui::Rect::from_min_size(
                                egui::pos2(rect.min.x, rect.min.y + 3.0),
                                egui::vec2(bar_w, 11.0),
                            );
                            ui.painter().rect_filled(bg_rect, 2.0, BG_WIDGET);
                            if bar_rect.width() > 0.5 {
                                ui.painter().rect_filled(bar_rect, 2.0, color);
                            }
                            ui.allocate_rect(bg_rect, egui::Sense::hover());
                            ui.add_space(gap);
                            ui.colored_label(color, format!("{:.2}", value));
                        });
                    }

                    ui.add_space(4.0);
                    ui.label(format!("Volume: {:.2}", volume));
                });

            ui.add_space(8.0);

            // ── Audio Routing ─────────────────────────────────────────────────
            egui::CollapsingHeader::new("🔀 Audio Routing")
                .default_open(true)
                .show(ui, |ui| {
                    self.build_audio_routing_section(ui);
                });
        }
    }

    fn build_audio_routing_section(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Audio Reactivity Routing")
                .color(ACCENT_CYAN)
                .strong(),
        );

        let (routing_enabled, _show_window, _can_add) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let routing = &state.audio_routing;
            (
                routing.enabled,
                routing.show_window,
                routing.matrix.can_add_route(),
            )
        };

        let mut enabled = routing_enabled;
        if ui.checkbox(&mut enabled, "Enable Audio Routing").changed() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.enabled = enabled;
        }

        if !enabled {
            ui.label(egui::RichText::new("Audio routing is disabled").color(TEXT_SECONDARY));
            return;
        }

        ui.horizontal(|ui| {
            if ui.button("Open Routing Matrix").clicked() {
                self.show_routing_window = !self.show_routing_window;
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio_routing.show_window = self.show_routing_window;
            }
        });

        let route_count = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.matrix.len()
        };

        if route_count > 0 {
            ui.label(format!("Active routes: {}", route_count));
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            for (i, route) in state.audio_routing.matrix.routes().iter().enumerate() {
                if !route.enabled {
                    continue;
                }
                ui.label(format!(
                    "  {} → {} ({:.0}%)",
                    route.band.short_name(),
                    route.target.name(),
                    route.amount * 100.0
                ));
                if i >= 3 {
                    let remaining = route_count.saturating_sub(4);
                    if remaining > 0 {
                        ui.label(
                            egui::RichText::new(format!("  ... and {} more", remaining))
                                .color(TEXT_SECONDARY),
                        );
                    }
                    break;
                }
            }
        } else {
            ui.label(
                egui::RichText::new("No active routes. Click 'Open Routing Matrix' to add.")
                    .color(TEXT_SECONDARY),
            );
        }
    }
}
