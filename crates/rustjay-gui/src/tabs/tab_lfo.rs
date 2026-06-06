//! # LFO Control Tab
//!
//! Dynamically populates LFO targets from effect-declared parameters.

use crate::control_gui::ControlGui;
use rustjay_core::lfo::{beat_division_to_hz, LfoTarget, Waveform};

impl ControlGui {
    /// Legacy LFO bank tab (deprecated — replaced by `build_modulation_tab` in M5.3).
    /// Kept for backward compatibility until M7.3.
    pub(crate) fn build_lfo_tab(&mut self, ui: &imgui::Ui) {
        // Snapshot target list and param name lookup so we can map indices ↔ targets
        let (target_list, hsb_count, param_names) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            // None + HSB targets (backward compatibility); more appended below.
            let mut targets: Vec<LfoTarget> = vec![
                LfoTarget::None,
                LfoTarget::HueShift,
                LfoTarget::Saturation,
                LfoTarget::Brightness,
            ];
            let hsb = targets.len() - 1; // count of non-None HSB targets
            let mut names = std::collections::HashMap::new();
            // Append custom modulatable params
            for d in state.param_descriptors.iter() {
                if d.is_modulatable() {
                    targets.push(LfoTarget::Custom(d.id.clone()));
                    names.insert(d.id.clone(), d.name.clone());
                }
            }
            (targets, hsb, names)
        };

        let target_names: Vec<String> = target_list.iter().map(|t| t.name()).collect();

        let target_refs: Vec<&str> = target_names.iter().map(|s| s.as_str()).collect();

        ui.text("Low Frequency Oscillator Modulation");
        ui.text_disabled("Each LFO can modulate parameters declared by the active effect");
        ui.separator();

        let (bpm, sync_source_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.effective_bpm(), state.effective_sync_source())
        };
        ui.text(format!(
            "Tempo: {:.1} BPM  (source: {})",
            bpm, sync_source_name
        ));
        ui.spacing();

        let waveforms = ["Sine", "Triangle", "Ramp Up", "Ramp Down", "Square"];
        let divisions = ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];

        let lfo_count = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.lfo.bank.lfos.len()
        };
        for i in 0..lfo_count {
            let (
                enabled,
                mut rate,
                mut amplitude,
                waveform_idx,
                tempo_sync,
                current_division,
                phase_offset,
                current_target,
            ) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let bank = &state.lfo.bank.lfos[i];
                (
                    bank.enabled,
                    bank.rate,
                    bank.amplitude,
                    bank.waveform as usize,
                    bank.tempo_sync,
                    bank.division,
                    bank.phase_offset,
                    bank.target.clone(),
                )
            };

            let target_idx = target_list
                .iter()
                .position(|t| *t == current_target)
                .unwrap_or(0);
            let mut division_idx = current_division;

            let _id_token = ui.push_id(format!("lfo_{}", i));

            if ui.collapsing_header(
                format!("LFO {} - {}", i + 1, if enabled { "ON" } else { "OFF" }),
                imgui::TreeNodeFlags::DEFAULT_OPEN,
            ) {
                // Enable/disable
                let mut enabled_mut = enabled;
                if ui.checkbox("Enabled", &mut enabled_mut) && enabled_mut != enabled {
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.lfo.bank.lfos[i].enabled = enabled_mut;
                }

                ui.separator();

                // Rate / beat division
                if tempo_sync {
                    let _width = ui.push_item_width(100.0);
                    if ui.combo_simple_string("Beat Division", &mut division_idx, &divisions)
                        && division_idx != current_division
                    {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.lfo.bank.lfos[i].division = division_idx;
                    }
                } else {
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
                    ui.text_disabled(format!(
                        "= {:.2} Hz",
                        beat_division_to_hz(division_idx, bpm)
                    ));
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
                        let _color =
                            ui.push_style_color(imgui::StyleColor::Button, [0.2, 0.6, 0.8, 1.0]);
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

                // Phase offset — field is stored in degrees; lfo::update() divides by 360.
                let _width = ui.push_item_width(200.0);
                let mut phase_degrees_mut = phase_offset;
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

                // Target dropdown — DYNAMIC
                let _width = ui.push_item_width(150.0);
                let mut tgt_idx = target_idx;
                if ui.combo_simple_string("Target", &mut tgt_idx, &target_refs) {
                    let new_target = target_list.get(tgt_idx).cloned().unwrap_or(LfoTarget::None);
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.lfo.bank.lfos[i].target = new_target;
                }

                // Visual indicator
                if enabled && target_idx > 0 {
                    ui.spacing();
                    let indicator = if target_idx <= hsb_count {
                        match target_idx {
                            1 => "→ Shifts hue".to_string(),
                            2 => "↑↓ Saturation".to_string(),
                            3 => "☀☾ Brightness".to_string(),
                            _ => String::new(),
                        }
                    } else {
                        let name = target_list
                            .get(target_idx)
                            .and_then(|t| t.param_id())
                            .and_then(|id| param_names.get(id))
                            .map(|n| n.as_str())
                            .unwrap_or("parameter");
                        format!("→ Modulating: {}", name)
                    };
                    ui.text_colored([0.8, 0.8, 0.2, 1.0], indicator);
                }
            }
        }

        ui.separator();
        if ui.button("Reset All LFOs") {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.lfo.bank.reset_all();
        }
    }
}
