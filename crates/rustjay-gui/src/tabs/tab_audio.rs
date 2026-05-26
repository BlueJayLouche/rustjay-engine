use crate::control_gui::ControlGui;
use rustjay_core::{AudioCommand, SyncSource};

impl ControlGui {
    /// Build the Audio tab
    pub(crate) fn build_audio_tab(&mut self, ui: &imgui::Ui) {
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

        // ── Device ────────────────────────────────────────────────────────────
        if ui.collapsing_header("Input Device", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            if ui.button("Refresh Audio Devices") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio_command = AudioCommand::RefreshDevices;
            }

            ui.spacing();

            if !self.audio_devices.is_empty() {
                let device_names: Vec<&str> = self.audio_devices.iter().map(|s| s.as_str()).collect();

                if let Some(ref current) = selected_device {
                    if let Some(idx) = self.audio_devices.iter().position(|d| d == current) {
                        self.selected_audio_device = idx;
                    }
                }

                if ui.combo_simple_string("Select Audio Device", &mut self.selected_audio_device, &device_names) {
                    let device_name = self.audio_devices.get(self.selected_audio_device).cloned();
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_command = AudioCommand::SelectDevice(device_name.unwrap_or_default());
                }

                if let Some(ref device) = selected_device {
                    ui.text(format!("Active: {}", device));
                }
            } else {
                ui.text_disabled("No audio devices found. Click Refresh.");
            }

            ui.spacing();
            if ui.checkbox("Enable Audio Analysis", &mut enabled) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.enabled = enabled;
                if enabled {
                    state.audio_command = AudioCommand::Start;
                } else {
                    state.audio_command = AudioCommand::Stop;
                }
            }
            ui.spacing();
        }

        // ── Analysis Settings ─────────────────────────────────────────────────
        // Only relevant when audio analysis is running.
        if enabled && ui.collapsing_header("Analysis Settings", imgui::TreeNodeFlags::DEFAULT_OPEN) {
            let (mut normalize, mut pink_noise, current_fft_size) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (state.audio.normalize, state.audio.pink_noise_shaping, state.audio.fft_size)
            };
            ui.text("Amplitude");
            if ui.slider("Amplitude", 0.1, 5.0, &mut amplitude) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.amplitude = amplitude;
            }

            ui.text("Smoothing");
            ui.same_line();
            ui.text_disabled("(0 = instant, 0.99 = very slow)");
            if ui.slider("Smoothing", 0.0, 0.95, &mut smoothing) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.smoothing = smoothing.clamp(0.0, 0.99);
            }

            {
                use rustjay_audio::{FFT_SIZES, FFT_SIZE_LABELS};
                let mut selected_idx = FFT_SIZES.iter().position(|&s| s == current_fft_size).unwrap_or(2);
                let labels: Vec<&str> = FFT_SIZE_LABELS.iter().copied().collect();
                ui.text("FFT Size");
                if ui.combo_simple_string("FFT Size##combo", &mut selected_idx, &labels) {
                    if let Some(&new_size) = FFT_SIZES.get(selected_idx) {
                        if new_size != current_fft_size {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.audio.fft_size = new_size;
                            state.audio_command = AudioCommand::SetFftSize(new_size);
                        }
                    }
                }
            }

            if ui.checkbox("Normalize Bands", &mut normalize) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.normalize = normalize;
            }
            ui.same_line();
            ui.text_disabled("(Auto-gain across all bands)");

            if ui.checkbox("+3dB/Octave Shaping", &mut pink_noise) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.pink_noise_shaping = pink_noise;
            }
            ui.same_line();
            ui.text_disabled("(Compensates for pink noise spectrum)");

            ui.spacing();
        }

        // ── Tempo & Sync ───────────────────────────────────────────────────────
        if ui.collapsing_header("Tempo & Sync", imgui::TreeNodeFlags::DEFAULT_OPEN) {

            let mut sync_source = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.sync_source
            };

            // Source selector
            let sources: &[(&str, SyncSource)] = &[
                ("Audio / Tap Tempo", SyncSource::Audio),
                #[cfg(feature = "link")]
                ("Ableton Link", SyncSource::AbletonLink),
                #[cfg(feature = "prodj")]
                ("ProDJ Link", SyncSource::ProDj),
            ];

            for &(label, variant) in sources {
                let mut selected = sync_source == variant;
                if ui.radio_button(label, &mut selected, true) && selected {
                    sync_source = variant;
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.sync_source = variant;
                    // Route enable/disable commands through the same command path as
                    // before — this lets process_link_commands set link.enabled on the
                    // NEXT frame so link.enable() is never called with the state lock held.
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

            ui.spacing();

            // Per-source live info + tap tempo
            match sync_source {
                SyncSource::Audio => {
                    let (bpm, tap_info) = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        (state.audio.bpm, state.audio.tap_tempo_info.clone())
                    };
                    ui.text(format!("BPM: {:.1}", bpm));

                    let _btn_color = ui.push_style_color(imgui::StyleColor::Button, [0.8, 0.3, 0.3, 1.0]);
                    let _btn_hover = ui.push_style_color(imgui::StyleColor::ButtonHovered, [0.9, 0.4, 0.4, 1.0]);
                    let _btn_active = ui.push_style_color(imgui::StyleColor::ButtonActive, [1.0, 0.5, 0.5, 1.0]);

                    if ui.button_with_size("TAP", [60.0, 30.0]) {
                        self.handle_tap_tempo();
                    }
                    ui.same_line();
                    ui.text_disabled(&tap_info);
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

                    ui.text(format!("BPM: {:.2}  |  Peers: {}  |  Playing: {}", link_bpm, link_peers, if link_playing { "Yes" } else { "No" }));
                    ui.text("Beat phase");
                    imgui::ProgressBar::new(link_phase)
                        .overlay_text(format!("{:.0}%", link_phase * 100.0))
                        .build(ui);

                    let mut quantum_f32 = link_quantum as f32;
                    if ui.slider("Quantum", 1.0_f32, 16.0_f32, &mut quantum_f32) {
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

                    ui.text(format!("Master BPM: {:.2}", prodj_master_bpm));
                    if !prodj_artist.is_empty() || !prodj_title.is_empty() {
                        ui.text(format!("Track: {} - {}", prodj_artist, prodj_title));
                    }
                    if !prodj_devices.is_empty() {
                        for device in &prodj_devices {
                            let master_tag = if device.is_master { " [MASTER]" } else { "" };
                            let playing_tag = if device.is_playing { "▶" } else { "⏸" };
                            ui.text(format!(
                                "{} Deck {}: {}{} | BPM: {:.2}",
                                playing_tag, device.device_id, device.name, master_tag,
                                device.bpm.unwrap_or(0.0)
                            ));
                        }
                    } else {
                        ui.text_disabled("No devices discovered yet...");
                    }
                }

                #[allow(unreachable_patterns)]
                _ => { ui.text_disabled("Selected source is not compiled in."); }
            }

            // MTC — always shown as passive info regardless of sync source
            #[cfg(feature = "mtc")]
            {
                ui.spacing();
                ui.text_colored([0.0, 1.0, 1.0, 1.0], "MIDI Timecode (MTC)");
                let (running, playing, position, source) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (state.mtc.running, state.mtc.playing, state.mtc.position, state.mtc.source_device.clone())
                };
                let (status_color, status_text) = if playing {
                    ([0.0_f32, 1.0, 0.0, 1.0], "Playing")
                } else if running {
                    ([1.0_f32, 0.8, 0.0, 1.0], "Stopped")
                } else {
                    ([0.5_f32, 0.5, 0.5, 1.0], "No signal")
                };
                ui.text("Status:");
                ui.same_line();
                ui.text_colored(status_color, status_text);
                if !source.is_empty() {
                    ui.text(format!("Source:  {}", source));
                }
                ui.text(format!("Position: {}  [{}]", position, position.frame_rate.name()));
                ui.text_disabled("Listening on all MIDI ports automatically.");
            }

            ui.spacing();
        } // end Tempo & Sync

        // ── Frequency Monitor ─────────────────────────────────────────────────
        if enabled && ui.collapsing_header("Frequency Monitor", imgui::TreeNodeFlags::empty()) {
            const BAND_NAMES:   [&str; 8]       = ["Sub", "Bass", "Lo Mid", "Mid", "Hi Mid", "High", "V.High", "Pres"];
            const BAND_COLORS:  [[f32; 4]; 8]   = [
                [0.80, 0.10, 0.10, 1.0], // Sub     — deep red
                [0.90, 0.45, 0.05, 1.0], // Bass    — orange
                [0.85, 0.75, 0.05, 1.0], // Lo Mid  — amber
                [0.40, 0.85, 0.10, 1.0], // Mid     — yellow-green
                [0.05, 0.85, 0.35, 1.0], // Hi Mid  — green
                [0.05, 0.80, 0.90, 1.0], // High    — cyan
                [0.25, 0.40, 0.95, 1.0], // V.High  — blue
                [0.75, 0.20, 0.95, 1.0], // Pres    — violet
            ];

            // Fixed three-column layout: [label | bar | value]
            // row_start_x anchors all rows to the same x regardless of label length.
            let avail_w     = ui.content_region_avail()[0];
            let row_start_x = ui.cursor_pos()[0];
            let label_col   = 50.0_f32; // wide enough for "V.High"
            let val_col     = 34.0_f32; // wide enough for "0.00"
            let gap         = 6.0_f32;
            let bar_w       = (avail_w - label_col - val_col - gap).max(20.0);
            let bar_x       = row_start_x + label_col;
            let val_x       = bar_x + bar_w + gap;

            for (i, (&value, &name)) in fft.iter().zip(BAND_NAMES.iter()).enumerate() {
                let color = BAND_COLORS[i];
                let row_y = ui.cursor_pos()[1];

                // Label — left-aligned, variable width, always readable
                ui.text_colored(color, name);

                // Bar — pinned to fixed column start
                ui.same_line();
                ui.set_cursor_pos([bar_x, row_y]);
                {
                    let _fill = ui.push_style_color(imgui::StyleColor::PlotHistogram, color);
                    let _bg   = ui.push_style_color(imgui::StyleColor::FrameBg, [0.10, 0.10, 0.10, 1.0]);
                    imgui::ProgressBar::new(value)
                        .size([bar_w, 11.0])
                        .overlay_text("")
                        .build(ui);
                }

                // Value — pinned to fixed column start, right of bar
                ui.same_line();
                ui.set_cursor_pos([val_x, row_y]);
                ui.text_colored(color, format!("{:.2}", value));
            }

            ui.spacing();
            ui.text(format!("Volume: {:.2}", volume));
            ui.spacing();
        }

        // ── Audio Routing ─────────────────────────────────────────────────────
        if enabled && ui.collapsing_header("Audio Routing", imgui::TreeNodeFlags::empty()) {
            self.build_audio_routing_section(ui);
            ui.spacing();
        }
    }

    /// Build the audio routing section in the Audio tab
    pub(crate) fn build_audio_routing_section(&mut self, ui: &imgui::Ui) {
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Audio Reactivity Routing");

        let (routing_enabled, show_window, _can_add_route) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let routing = &state.audio_routing;
            (routing.enabled, routing.show_window, routing.matrix.can_add_route())
        };

        let mut enabled = routing_enabled;
        if ui.checkbox("Enable Audio Routing", &mut enabled) {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.enabled = enabled;
        }

        if !enabled {
            ui.text_disabled("Audio routing is disabled");
            return;
        }

        ui.same_line();

        let mut show = show_window;
        if ui.button("Open Routing Matrix") {
            show = !show;
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.show_window = show;
        }

        let route_count = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.matrix.len()
        };

        if route_count > 0 {
            ui.text(format!("Active routes: {}", route_count));

            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            for (i, route) in state.audio_routing.matrix.routes().iter().enumerate() {
                if !route.enabled { continue; }
                ui.text(format!("  {} → {} ({:.0}%)",
                    route.band.short_name(),
                    route.target.name(),
                    route.amount * 100.0
                ));
                if i >= 3 {
                    let remaining = route_count - 4;
                    if remaining > 0 {
                        ui.text_disabled(format!("  ... and {} more", remaining));
                    }
                    break;
                }
            }
        } else {
            ui.text_disabled("No active routes. Click 'Open Routing Matrix' to add.");
        }

        if show {
            self.build_routing_window(ui);
        }
    }

    /// Build the audio routing matrix window
    pub(crate) fn build_routing_window(&mut self, ui: &imgui::Ui) {
        use rustjay_core::routing::{FftBand, ModulationTarget};

        let mut is_open = true;

        let target_list = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            ModulationTarget::all_for(&state.param_descriptors)
        };
        let target_names: Vec<String> = target_list.iter().map(|t| t.name()).collect();
        let target_refs: Vec<&str> = target_names.iter().map(|s| s.as_str()).collect();

        ui.window("Audio Routing Matrix")
            .position([500.0, 100.0], imgui::Condition::FirstUseEver)
            .size([450.0, 550.0], imgui::Condition::FirstUseEver)
            .opened(&mut is_open)
            .build(|| {
                let (can_add, route_count, max_routes) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    let routing = &state.audio_routing;
                    (routing.matrix.can_add_route(), routing.matrix.len(), routing.matrix.max_routes())
                };

                ui.text(format!("Routes: {}/{}", route_count, max_routes));
                ui.same_line();

                if ui.button("Clear All") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.clear();
                }

                ui.separator();
                ui.text_colored([0.0, 1.0, 1.0, 1.0], "Add New Route");

                let (mut band_idx, mut target_idx) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (state.audio_routing.selected_band, state.audio_routing.selected_target)
                };

                if target_idx >= target_list.len() && !target_list.is_empty() {
                    target_idx = target_list.len() - 1;
                }

                let bands: Vec<&str> = FftBand::all().iter().map(|b| b.name()).collect();
                ui.combo_simple_string("Band##new", &mut band_idx, &bands);
                ui.combo_simple_string("Target##new", &mut target_idx, &target_refs);

                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.selected_band = band_idx;
                    state.audio_routing.selected_target = target_idx;
                }

                ui.same_line();

                let can_add = can_add && band_idx < FftBand::all().len() && target_idx < target_list.len();
                if can_add {
                    if ui.button("Add Route") {
                        if let Some(band) = FftBand::from_index(band_idx) {
                            if let Some(target) = target_list.get(target_idx) {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.audio_routing.matrix.add_route(band, target.clone());
                            }
                        }
                    }
                } else {
                    ui.text_disabled("Max routes reached");
                }

                ui.separator();
                ui.text_colored([0.0, 1.0, 1.0, 1.0], "Active Routes");

                let routes_data: Vec<_> = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.routes().iter().map(|r| {
                        (r.id, r.band, r.target.clone(), r.amount, r.attack, r.release, r.enabled, r.current_value)
                    }).collect()
                };

                for (id, band, target, amount, attack, release, enabled, current) in &routes_data {
                    let _id_token = ui.push_id(format!("route_{}", *id));

                    let mut is_enabled = *enabled;
                    if ui.checkbox("##enabled", &mut is_enabled) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                            route.enabled = is_enabled;
                        }
                    }
                    ui.same_line();

                    ui.text(format!("{} → {}", band.short_name(), target.name()));
                    ui.same_line();
                    ui.text_colored([0.0, 1.0, 0.0, 1.0], format!("{:.2}", current));
                    ui.same_line();

                    if ui.button("X") {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.audio_routing.matrix.remove_route(*id);
                    }

                    let mut amt = *amount;
                    if ui.slider("Amount", -1.0, 1.0, &mut amt) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                            route.amount = amt;
                        }
                    }

                    ui.columns(2, "attack_release", false);
                    let mut atk = *attack;
                    if ui.slider("Attack", 0.001, 1.0, &mut atk) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                            route.attack = atk;
                        }
                    }
                    ui.next_column();
                    let mut rel = *release;
                    if ui.slider("Release", 0.001, 1.0, &mut rel) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                            route.release = rel;
                        }
                    }
                    ui.columns(1, "", false);

                    ui.separator();
                }

                if routes_data.is_empty() {
                    ui.text_disabled("No routes configured. Add one above.");
                }
            });

        if !is_open {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.show_window = false;
        }
    }

    /// Handle tap tempo button press
    pub fn handle_tap_tempo(&mut self) {
        use std::time::{SystemTime, UNIX_EPOCH};

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());

        let is_first_tap = now - state.audio.last_tap_time > 2.0;
        if is_first_tap {
            state.audio.tap_times.clear();
            state.audio.tap_tempo_info = "Reset: new tempo sequence".to_string();
            state.lfo.bank.reset_all();
        } else {
            state.audio.tap_tempo_info = format!("{} taps recorded", state.audio.tap_times.len() + 1);
        }

        state.audio.tap_times.push(now);
        state.audio.last_tap_time = now;

        if state.audio.tap_times.len() > 8 {
            state.audio.tap_times.remove(0);
        }

        state.audio.beat_phase = 0.0;

        if state.audio.tap_times.len() >= 4 {
            let mut intervals = Vec::new();
            for i in 1..state.audio.tap_times.len() {
                intervals.push(state.audio.tap_times[i] - state.audio.tap_times[i-1]);
            }

            let avg_interval: f64 = intervals.iter().sum::<f64>() / intervals.len() as f64;

            if avg_interval > 0.1 && avg_interval < 3.0 {
                let new_bpm = (60.0 / avg_interval) as f32;
                state.audio.bpm = new_bpm.clamp(40.0, 200.0);
                state.audio.tap_tempo_info = format!("BPM: {:.1}", state.audio.bpm);
            }
        }
    }
}
