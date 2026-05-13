use crate::control_gui::ControlGui;
use rustjay_core::lfo::{LfoTarget, Waveform, beat_division_to_hz};

impl ControlGui {
    /// Build the LFO control window
    pub(crate) fn build_lfo_window(&mut self, ui: &imgui::Ui) {
        let mut show_window = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.lfo.show_window
        };
        let mut should_close = false;

        ui.window("LFO Control")
            .size([520.0, 480.0], imgui::Condition::FirstUseEver)
            .opened(&mut show_window)
            .build(|| {
                ui.text("Low Frequency Oscillator Modulation");
                ui.text_disabled("Each LFO can modulate Hue, Saturation, or Brightness");
                ui.separator();

                // Get BPM for display
                let bpm = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio.bpm
                };
                ui.text(&format!("Tempo: {:.1} BPM", bpm));
                ui.spacing();

                let waveforms = ["Sine", "Triangle", "Ramp Up", "Ramp Down", "Square"];
                let targets = ["None", "Hue", "Saturation", "Brightness"];
                let divisions = ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];

                // Iterate through each LFO bank
                for i in 0..3 {
                    let _needs_update = false;

                    // Get current values (store originals for change detection)
                    let (enabled, mut rate, mut amplitude, waveform_idx,
                         tempo_sync, current_division, phase_offset, mut target_idx) = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        let bank = &state.lfo.bank.lfos[i];
                        let wf_idx = bank.waveform as usize;
                        let tgt_idx = match bank.target {
                            LfoTarget::None | LfoTarget::Unknown => 0,
                            LfoTarget::HueShift => 1,
                            LfoTarget::Saturation => 2,
                            LfoTarget::Brightness => 3,
                        };
                        (bank.enabled, bank.rate, bank.amplitude, wf_idx,
                         bank.tempo_sync, bank.division, bank.phase_offset, tgt_idx)
                    };
                    // Local mutable copy for UI
                    let mut division_idx = current_division;

                    let _header_color = if enabled {
                        [0.2, 0.8, 0.2, 1.0]
                    } else {
                        [0.5, 0.5, 0.5, 1.0]
                    };

                    let _id_token = ui.push_id(format!("lfo_{}", i));

                    if ui.collapsing_header(
                        &format!("LFO {} - {}", i + 1, if enabled { "ON" } else { "OFF" }),
                        imgui::TreeNodeFlags::DEFAULT_OPEN
                    ) {
                        // Enable/disable checkbox
                        let mut enabled_mut = enabled;
                        if ui.checkbox("Enabled", &mut enabled_mut) && enabled_mut != enabled {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].enabled = enabled_mut;
                        }

                        ui.separator();

                        // Rate control
                        if tempo_sync {
                            // Beat division dropdown
                            let _width = ui.push_item_width(100.0);
                            if ui.combo_simple_string(
                                "Beat Division",
                                &mut division_idx,
                                &divisions
                            ) && division_idx != current_division {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.lfo.bank.lfos[i].division = division_idx;
                            }
                        } else {
                            // Free rate slider
                            let _width = ui.push_item_width(200.0);
                            if ui.slider("Rate (Hz)", 0.01, 10.0, &mut rate) {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.lfo.bank.lfos[i].rate = rate;
                            }
                        }

                        // Tempo sync toggle
                        let mut sync = tempo_sync;
                        if ui.checkbox("Tempo Sync", &mut sync) && sync != tempo_sync {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].tempo_sync = sync;
                        }
                        ui.same_line();
                        if tempo_sync {
                            ui.text_disabled(&format!("= {:.2} Hz",
                                beat_division_to_hz(division_idx, bpm)));
                        }

                        ui.separator();

                        // Waveform selection
                        ui.text("Waveform:");
                        for (wf_idx, wf_name) in waveforms.iter().enumerate() {
                            if wf_idx > 0 {
                                ui.same_line();
                            }
                            let is_selected = waveform_idx == wf_idx;
                            if is_selected {
                                let _color = ui.push_style_color(
                                    imgui::StyleColor::Button,
                                    [0.2, 0.6, 0.8, 1.0]
                                );
                                ui.button(wf_name);
                            } else if ui.button(wf_name) {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.lfo.bank.lfos[i].waveform = match wf_idx {
                                        0 => Waveform::Sine,
                                        1 => Waveform::Triangle,
                                        2 => Waveform::Ramp,
                                        3 => Waveform::Saw,
                                        4 => Waveform::Square,
                                        _ => Waveform::Sine,
                                    };
                            }
                        }

                        // Phase offset
                        let _width = ui.push_item_width(200.0);
                        let phase_degrees = phase_offset * 360.0;
                        let mut phase_degrees_mut = phase_degrees;
                        if ui.slider("Phase Offset (°)", 0.0, 360.0, &mut phase_degrees_mut) {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].phase_offset = phase_degrees_mut;
                        }
                        ui.same_line();
                        ui.text_disabled("(0° = on beat)");

                        // Amplitude
                        if ui.slider("Amplitude", -1.0, 1.0, &mut amplitude) {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].amplitude = amplitude;
                        }

                        ui.separator();

                        // Target parameter
                        let _width = ui.push_item_width(120.0);
                        if ui.combo_simple_string("Target", &mut target_idx, &targets) {
                            let new_target = match target_idx {
                                1 => LfoTarget::HueShift,
                                2 => LfoTarget::Saturation,
                                3 => LfoTarget::Brightness,
                                _ => LfoTarget::None,
                            };
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].target = new_target;
                        }

                        // Visual indicator of modulation direction
                        if enabled && target_idx > 0 {
                            ui.spacing();
                            let indicator = match (amplitude > 0.0, target_idx) {
                                (true, 1) => "→ Shifts hue RIGHT",
                                (false, 1) => "← Shifts hue LEFT",
                                (true, 2) => "↑ Increases saturation",
                                (false, 2) => "↓ Decreases saturation",
                                (true, 3) => "☀ Increases brightness",
                                (false, 3) => "☾ Decreases brightness",
                                _ => "",
                            };
                            ui.text_colored([0.8, 0.8, 0.2, 1.0], indicator);
                        }
                    }

                    // ID token dropped automatically
                }

                ui.separator();
                if ui.button("Close") {
                    should_close = true;
                }
                ui.same_line();
                if ui.button("Reset All LFOs") {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.lfo.bank.reset_all();
                }
            });

        // Update show_window in state if changed
        if !show_window || should_close {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.lfo.show_window = false;
        }
    }
}
