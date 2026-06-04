//! LFO tab — 8 low-frequency oscillators for parameter modulation.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use egui::{Color32, Stroke};
use rustjay_core::lfo::{LfoTarget, Waveform, beat_division_to_hz};

impl EguiControlGui {
    pub(crate) fn build_lfo_tab(&mut self, ui: &mut egui::Ui) {
        let (target_list, hsb_count, param_names) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let mut targets: Vec<LfoTarget> = vec![
                LfoTarget::None,
                LfoTarget::HueShift,
                LfoTarget::Saturation,
                LfoTarget::Brightness,
            ];
            let hsb = targets.len() - 1;
            let mut names = std::collections::HashMap::new();
            for d in state.param_descriptors.iter() {
                if d.is_modulatable() {
                    targets.push(LfoTarget::Custom(d.id.clone()));
                    names.insert(d.id.clone(), d.name.clone());
                }
            }
            (targets, hsb, names)
        };

        let target_names: Vec<String> = target_list.iter().map(|t| t.name()).collect();

        let (bpm, sync_source_name) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.effective_bpm(), state.effective_sync_source())
        };

        ui.heading("Low Frequency Oscillator Modulation");
        ui.label(egui::RichText::new("Each LFO can modulate parameters declared by the active effect").size(11.0).color(TEXT_SECONDARY));
        ui.add_space(8.0);
        ui.label(format!("Tempo: {:.1} BPM  (source: {})", bpm, sync_source_name));
        ui.add_space(8.0);

        let waveforms = ["Sine", "Triangle", "Ramp Up", "Ramp Down", "Square"];
        let divisions = ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];

        let lfo_count = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.lfo.bank.lfos.len()
        };

        for i in 0..lfo_count {
            let (enabled, mut rate, mut amplitude, waveform_idx,
                 tempo_sync, current_division, phase_offset, current_target) = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let bank = &state.lfo.bank.lfos[i];
                (bank.enabled, bank.rate, bank.amplitude, bank.waveform as usize,
                 bank.tempo_sync, bank.division, bank.phase_offset, bank.target.clone())
            };

            let target_idx = target_list.iter().position(|t| *t == current_target).unwrap_or(0);
            let mut division_idx = current_division;

            let header_text = format!("LFO {} — {}", i + 1, if enabled { "ON" } else { "OFF" });
            let header_color = if enabled { ACCENT_GREEN } else { TEXT_SECONDARY };

            egui::Frame::group(ui.style())
                .fill(BG_WIDGET)
                .stroke(Stroke::new(1.0, if enabled { ACCENT_CYAN } else { BORDER }))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(&header_text).strong().color(header_color));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let mut enabled_mut = enabled;
                            if ui.checkbox(&mut enabled_mut, "Enabled").changed() && enabled_mut != enabled {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                state.lfo.bank.lfos[i].enabled = enabled_mut;
                            }
                        });
                    });

                    ui.separator();

                    // Rate / beat division
                    if tempo_sync {
                        egui::ComboBox::from_id_salt(format!("lfo_div_{}", i))
                            .width(100.0)
                            .selected_text(divisions[division_idx])
                            .show_ui(ui, |ui| {
                                for (j, div) in divisions.iter().enumerate() {
                                    if ui.selectable_label(division_idx == j, *div).clicked() {
                                        division_idx = j;
                                    }
                                }
                            });
                        if division_idx != current_division {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].division = division_idx;
                        }
                    } else {
                        if ui.add(egui::Slider::new(&mut rate, 0.01..=10.0).text("Rate (Hz)").trailing_fill(true)).changed() {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].rate = rate;
                        }
                    }

                    // Tempo sync toggle
                    let mut sync = tempo_sync;
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut sync, "Tempo Sync").changed() && sync != tempo_sync {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].tempo_sync = sync;
                        }
                        if tempo_sync {
                            ui.label(egui::RichText::new(format!("= {:.2} Hz", beat_division_to_hz(division_idx, bpm))).size(11.0).color(TEXT_SECONDARY));
                        }
                    });

                    ui.separator();

                    // Waveform selection
                    ui.label("Waveform:");
                    ui.horizontal(|ui| {
                        for (wf_idx, wf_name) in waveforms.iter().enumerate() {
                            let is_selected = waveform_idx == wf_idx;
                            let btn = if is_selected {
                                egui::Button::new(egui::RichText::new(*wf_name).strong().color(Color32::BLACK))
                                    .fill(ACCENT_CYAN)
                            } else {
                                egui::Button::new(egui::RichText::new(*wf_name).color(TEXT_PRIMARY))
                                    .fill(BG_HOVER)
                            };
                            if ui.add_sized(egui::vec2(70.0, 24.0), btn).clicked() && !is_selected {
                                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                                let lfo = &mut state.lfo.bank.lfos[i];
                                lfo.waveform = match wf_idx {
                                    0 => Waveform::Sine,
                                    1 => Waveform::Triangle,
                                    2 => Waveform::Ramp,
                                    3 => Waveform::Saw,
                                    4 => Waveform::Square,
                                    _ => Waveform::Sine,
                                };
                                // Reset phase so the new waveform starts from the beginning
                                // of its cycle rather than inheriting an arbitrary mid-cycle
                                // phase from the previous waveform (avoids discontinuous jumps).
                                lfo.reset();
                            }
                        }
                    });

                    // Phase offset
                    let mut phase_degrees_mut = phase_offset;
                    ui.horizontal(|ui| {
                        if ui.add(egui::Slider::new(&mut phase_degrees_mut, 0.0..=360.0).text("Phase Offset (°)").trailing_fill(true)).changed() {
                            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.lfo.bank.lfos[i].phase_offset = phase_degrees_mut;
                        }
                        ui.label(egui::RichText::new("(0° = on beat)").size(11.0).color(TEXT_SECONDARY));
                    });

                    // Amplitude
                    if ui.add(egui::Slider::new(&mut amplitude, -1.0..=1.0).text("Amplitude").trailing_fill(true)).changed() {
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.lfo.bank.lfos[i].amplitude = amplitude;
                    }

                    ui.separator();

                    // Target dropdown
                    let mut tgt_idx = target_idx;
                    egui::ComboBox::from_id_salt(format!("lfo_tgt_{}", i))
                        .width(180.0)
                        .selected_text(&target_names[tgt_idx])
                        .show_ui(ui, |ui| {
                            for (j, name) in target_names.iter().enumerate() {
                                if ui.selectable_label(tgt_idx == j, name.as_str()).clicked() {
                                    tgt_idx = j;
                                }
                            }
                        });
                    if tgt_idx != target_idx {
                        let new_target = target_list.get(tgt_idx).cloned().unwrap_or(LfoTarget::None);
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.lfo.bank.lfos[i].target = new_target;
                    }

                    // Visual indicator
                    if enabled && target_idx > 0 {
                        let indicator = if target_idx <= hsb_count {
                            match target_idx {
                                1 => "→ Shifts hue".to_string(),
                                2 => "↑↓ Saturation".to_string(),
                                3 => "☀☾ Brightness".to_string(),
                                _ => String::new(),
                            }
                        } else {
                            let name = target_list.get(target_idx)
                                .and_then(|t| t.param_id())
                                .and_then(|id| param_names.get(id))
                                .map(|n| n.as_str())
                                .unwrap_or("parameter");
                            format!("→ Modulating: {}", name)
                        };
                        ui.colored_label(ACCENT_AMBER, indicator);
                    }
                });

            ui.add_space(8.0);
        }

        ui.add_space(8.0);
        if ui.button("Reset All LFOs").clicked() {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state.lfo.bank.reset_all();
        }
    }
}
