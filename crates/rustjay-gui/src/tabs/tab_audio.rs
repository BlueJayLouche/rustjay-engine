use crate::control_gui::ControlGui;
use rustjay_core::AudioCommand;

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

        ui.text("Audio Analysis");
        ui.separator();

        // Audio Device Selection
        ui.text_colored([0.0, 1.0, 1.0, 1.0], "Input Device");

        // Refresh button
        if ui.button("Refresh Audio Devices") {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_command = AudioCommand::RefreshDevices;
        }

        ui.spacing();

        // Device dropdown
        if !self.audio_devices.is_empty() {
            let device_names: Vec<&str> = self.audio_devices.iter().map(|s| s.as_str()).collect();

            // Find current selection index
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

            // Show currently selected
            if let Some(ref device) = selected_device {
                ui.text(format!("Active: {}", device));
            }
        } else {
            ui.text_disabled("No audio devices found. Click Refresh.");
        }

        ui.spacing();
        ui.separator();
        ui.spacing();

        // Enable/disable
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

        if enabled {
            // Get additional audio settings
            let (mut normalize, mut pink_noise, current_fft_size) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (state.audio.normalize, state.audio.pink_noise_shaping, state.audio.fft_size)
            };

            // Amplitude
            ui.text("Input Amplitude");
            if ui.slider("Amplitude", 0.1, 5.0, &mut amplitude) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.amplitude = amplitude;
            }

            // Smoothing
            ui.text("Smoothing (0 = instant, 0.99 = very slow)");
            if ui.slider("Smoothing", 0.0, 0.95, &mut smoothing) {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.audio.smoothing = smoothing.clamp(0.0, 0.99);
            }

            ui.spacing();
            ui.separator();
            ui.spacing();

            // FFT Size dropdown
            {
                use rustjay_audio::fft::{FFT_SIZES, FFT_SIZE_LABELS};
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

            ui.spacing();
            ui.separator();
            ui.spacing();

            // Processing options
            ui.text("Processing Options");

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
            ui.separator();
            ui.spacing();

            // Tap Tempo section
            ui.text_colored([0.0, 1.0, 1.0, 1.0], "Tempo");

            let (bpm, tap_info) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                (state.audio.bpm, state.audio.tap_tempo_info.clone())
            };

            ui.text(format!("BPM: {:.1}", bpm));

            // Tap tempo button
            let _btn_color = ui.push_style_color(imgui::StyleColor::Button, [0.8, 0.3, 0.3, 1.0]);
            let _btn_hover = ui.push_style_color(imgui::StyleColor::ButtonHovered, [0.9, 0.4, 0.4, 1.0]);
            let _btn_active = ui.push_style_color(imgui::StyleColor::ButtonActive, [1.0, 0.5, 0.5, 1.0]);

            if ui.button_with_size("TAP", [60.0, 30.0]) {
                self.handle_tap_tempo();
            }

            ui.same_line();
            ui.text_disabled(&tap_info);

            ui.spacing();
            ui.separator();
            ui.spacing();

            // FFT visualization
            ui.text("Frequency Bands");
            let band_names = ["Sub Bass", "Bass", "Low Mid", "Mid", "High Mid", "High", "Very High", "Presence"];
            for (_i, (&value, name)) in fft.iter().zip(band_names.iter()).enumerate() {
                let width = 200.0 * value;
                ui.text(format!("{}", name));
                ui.same_line();
                ui.text(format!(": {:.2}", value));
                // Draw a simple bar
                let draw_list = ui.get_window_draw_list();
                let pos = ui.cursor_screen_pos();
                draw_list.add_rect(
                    pos,
                    [pos[0] + width, pos[1] + 10.0],
                    [0.0, 1.0, 0.0, 1.0],
                ).filled(true).build();
                ui.new_line();
            }

            ui.spacing();
            ui.text(format!("Volume: {:.2}", volume));

            ui.spacing();
            ui.separator();
            ui.spacing();

            // Audio Routing section
            self.build_audio_routing_section(ui);

        } else {
            ui.text_disabled("Audio analysis is disabled");
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

        // Enable/disable toggle
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

        // Open routing window button
        let mut show = show_window;
        if ui.button("Open Routing Matrix") {
            show = !show;
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.show_window = show;
        }

        // Show current routes summary
        let route_count = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.audio_routing.matrix.len()
        };

        if route_count > 0 {
            ui.text(format!("Active routes: {}", route_count));

            // Show a mini list of routes
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            for (i, route) in state.audio_routing.matrix.routes().iter().enumerate() {
                if !route.enabled { continue; }
                ui.text(format!("  {} → {} ({:.0}%)",
                    route.band.short_name(),
                    route.target.name(),
                    route.amount * 100.0
                ));
                if i >= 3 { // Limit to 3 lines
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

        // Build routing window if shown (lock is already dropped at this point)
        if show {
            self.build_routing_window(ui);
        }
    }

    /// Build the audio routing matrix window
    pub(crate) fn build_routing_window(&mut self, ui: &imgui::Ui) {
        use rustjay_audio::routing::{FftBand, ModulationTarget};

        let mut is_open = true;

        ui.window("Audio Routing Matrix")
            .position([500.0, 100.0], imgui::Condition::FirstUseEver)
            .size([450.0, 550.0], imgui::Condition::FirstUseEver)
            .opened(&mut is_open)
            .build(|| {
                // Get current state
                let (can_add, route_count, max_routes) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    let routing = &state.audio_routing;
                    (routing.matrix.can_add_route(), routing.matrix.len(), routing.matrix.max_routes())
                };

                ui.text(format!("Routes: {}/{}", route_count, max_routes));
                ui.same_line();

                // Clear all button
                if ui.button("Clear All") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.clear();
                }

                ui.separator();

                // Add new route section
                ui.text_colored([0.0, 1.0, 1.0, 1.0], "Add New Route");

                // Get selections
                let (mut band_idx, mut target_idx) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (state.audio_routing.selected_band, state.audio_routing.selected_target)
                };

                // Band selection
                let bands: Vec<&str> = FftBand::all().iter().map(|b| b.name()).collect();
                ui.combo_simple_string("Band##new", &mut band_idx, &bands);

                // Target selection
                let targets: Vec<&str> = ModulationTarget::all().iter().map(|t| t.name()).collect();
                ui.combo_simple_string("Target##new", &mut target_idx, &targets);

                // Update selections
                {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.selected_band = band_idx;
                    state.audio_routing.selected_target = target_idx;
                }

                ui.same_line();

                // Add button
                let can_add = can_add && band_idx < FftBand::all().len() && target_idx < ModulationTarget::all().len();
                if can_add {
                    if ui.button("Add Route") {
                        if let Some(band) = FftBand::from_index(band_idx) {
                            if let Some(target) = ModulationTarget::all().get(target_idx) {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.audio_routing.matrix.add_route(band, *target);
                            }
                        }
                    }
                } else {
                    ui.text_disabled("Max routes reached");
                }

                ui.separator();

                // Existing routes list
                ui.text_colored([0.0, 1.0, 1.0, 1.0], "Active Routes");

                // We need to collect route data first to avoid borrow issues
                let routes_data: Vec<_> = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio_routing.matrix.routes().iter().map(|r| {
                        (r.id, r.band, r.target, r.amount, r.attack, r.release, r.enabled, r.current_value)
                    }).collect()
                };

                for (id, band, target, amount, attack, release, enabled, current) in &routes_data {
                    let _id_token = ui.push_id(format!("route_{}", *id));

                    // Enable/disable checkbox
                    let mut is_enabled = *enabled;
                    if ui.checkbox("##enabled", &mut is_enabled) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                            route.enabled = is_enabled;
                        }
                    }
                    ui.same_line();

                    // Route info
                    ui.text(format!("{} → {}", band.short_name(), target.name()));

                    // Current value indicator
                    ui.same_line();
                    ui.text_colored([0.0, 1.0, 0.0, 1.0], format!("{:.2}", current));

                    // Delete button
                    ui.same_line();
                    if ui.button("X") {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.audio_routing.matrix.remove_route(*id);
                    }

                    // Amount slider
                    let mut amt = *amount;
                    if ui.slider("Amount", -1.0, 1.0, &mut amt) {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(route) = state.audio_routing.matrix.get_route_mut(*id) {
                            route.amount = amt;
                        }
                    }

                    // Attack/Release sliders
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
                    // _id_token auto-pops when dropped
                }

                if routes_data.is_empty() {
                    ui.text_disabled("No routes configured. Add one above.");
                }
            });

        // Update window visibility
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

        // Clear taps if it's been too long since last tap (2 seconds)
        if now - state.audio.last_tap_time > 2.0 {
            state.audio.tap_times.clear();
            state.audio.tap_tempo_info = "Reset: new tempo sequence".to_string();
        } else {
            state.audio.tap_tempo_info = format!("{} taps recorded", state.audio.tap_times.len() + 1);
        }

        // Add tap time
        state.audio.tap_times.push(now);
        state.audio.last_tap_time = now;

        // Keep only last 8 taps for average
        if state.audio.tap_times.len() > 8 {
            state.audio.tap_times.remove(0);
        }

        // Reset beat phase on every tap (global sync)
        state.audio.beat_phase = 0.0;

        // Calculate BPM from tap intervals (need at least 4 taps for accuracy)
        if state.audio.tap_times.len() >= 4 {
            let mut intervals = Vec::new();
            for i in 1..state.audio.tap_times.len() {
                intervals.push(state.audio.tap_times[i] - state.audio.tap_times[i-1]);
            }

            // Average interval
            let avg_interval: f64 = intervals.iter().sum::<f64>() / intervals.len() as f64;

            if avg_interval > 0.1 && avg_interval < 3.0 { // Reasonable range (20-600 BPM)
                let new_bpm = (60.0 / avg_interval) as f32;
                state.audio.bpm = new_bpm.clamp(40.0, 200.0);
                state.audio.tap_tempo_info = format!("BPM: {:.1}", state.audio.bpm);
            }
        }
    }
}
