//! Unified Modulation tab — edits the shared `ModulationEngine` (M5.2).
//!
//! Replaces the legacy 8-slot LFO bank with a source-list + assignment editor.
//! All writes use the clone-Arc-then-drop pattern (F1) so the lock hierarchy
//! `shared_state` → `modulation` is never violated.

use crate::egui_control_gui::EguiControlGui;
use crate::egui_theme::colors::*;
use egui::Color32;
use rustjay_core::modulation::{LFOWaveform, ModulationSource};
use rustjay_core::lfo::beat_division_to_hz;

const WAVE_NAMES: &[(&str, LFOWaveform)] = &[
    ("Sine", LFOWaveform::Sine),
    ("Square", LFOWaveform::Square),
    ("Triangle", LFOWaveform::Triangle),
    ("Sawtooth", LFOWaveform::Sawtooth),
    ("Random", LFOWaveform::Random),
];

const DIVISION_LABELS: &[&str] = &["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];

impl EguiControlGui {
    pub(crate) fn build_modulation_tab(&mut self, ui: &mut egui::Ui) {
        ui.heading("Modulation");
        ui.label(
            egui::RichText::new("LFO · ADSR · Step Sequencer · Audio Band")
                .size(11.0)
                .color(TEXT_SECONDARY),
        );
        ui.add_space(12.0);

        // ── Source list + editor ─────────────────────────────────────────────
        let (mod_arc, bpm, param_ids, param_names) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let mod_arc = state.modulation.clone();
            let bpm = state.effective_bpm();
            let mut ids = vec!["hue_shift".to_string(), "saturation".to_string(), "brightness".to_string()];
            let mut names = vec![
                ("hue_shift".to_string(), "Hue Shift".to_string()),
                ("saturation".to_string(), "Saturation".to_string()),
                ("brightness".to_string(), "Brightness".to_string()),
            ];
            for d in state.param_descriptors.iter() {
                if d.is_modulatable() {
                    ids.push(d.id.clone());
                    names.push((d.id.clone(), d.name.clone()));
                }
            }
            (mod_arc, bpm, ids, names.into_iter().collect::<std::collections::HashMap<_, _>>())
        };

        // Snapshot source list for rendering
        let sources_snapshot: Vec<(String, String, f32, bool)> = {
            let mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            mod_eng
                .sources
                .iter()
                .enumerate()
                .map(|(i, entry)| {
                    let typ = source_type_name(&entry.source);
                    let value = mod_eng.current_values().get(i).copied().unwrap_or(0.0);
                    let enabled = source_is_enabled(&entry.source);
                    (entry.uuid.clone(), typ, value, enabled)
                })
                .collect()
        };

        // S5: Guard stale expanded-source UUID (source may have been deleted via
        // ModulationCommand or another tab while this tab was not rendered).
        let source_uuids: std::collections::HashSet<_> = sources_snapshot.iter().map(|(u, _, _, _)| u.clone()).collect();
        if let Some(ref u) = self.modulation_expanded_source {
            if !source_uuids.contains(u) {
                self.modulation_expanded_source = None;
            }
        }

        // Track which source is expanded (persisted in gui state)
        let expanded_uuid = self
            .modulation_expanded_source
            .clone()
            .unwrap_or_default();

        for (uuid, typ, value, enabled) in &sources_snapshot {
            let is_expanded = expanded_uuid == *uuid;
            let header_color = if *enabled { ACCENT_CYAN } else { TEXT_SECONDARY };

            egui::Frame::group(ui.style())
                .fill(BG_WIDGET)
                .stroke(egui::Stroke::new(1.0, if is_expanded { ACCENT_CYAN } else { BORDER }))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    // Header row: type + short uuid + value + expand toggle
                    ui.horizontal(|ui| {
                        let label = format!("{}  {}  → {:.2}", typ, &uuid[..4], value);
                        let btn = egui::Button::new(
                            egui::RichText::new(label).color(header_color).strong(),
                        )
                        .fill(if is_expanded { BG_HOVER } else { BG_WIDGET });
                        if ui.add(btn).clicked() {
                            self.modulation_expanded_source = if is_expanded {
                                None
                            } else {
                                Some(uuid.clone())
                            };
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✕").clicked() {
                                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                                mod_eng.remove_source(uuid);
                                if self.modulation_expanded_source.as_ref() == Some(uuid) {
                                    self.modulation_expanded_source = None;
                                }
                            }
                        });
                    });

                    // Expanded editor
                    if is_expanded {
                        ui.separator();
                        {
                            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                            self.draw_source_editor(ui, &mut mod_eng, uuid, bpm);
                        }
                        // Assignments are drawn outside the mod_eng lock so we can re-lock
                        // for the assignment buttons/sliders (Mutex is not reentrant).
                        self.draw_assignments_for_source(ui, uuid.clone(), &mod_arc, &param_ids, &param_names);
                    }
                });

            ui.add_space(4.0);
        }

        ui.add_space(8.0);

        // ── Add source buttons ───────────────────────────────────────────────
        ui.horizontal(|ui| {
            if ui.button("+ LFO").clicked() {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                let uuid = mod_eng.add_source(ModulationSource::LFO {
                    waveform: LFOWaveform::Sine,
                    frequency: 1.0,
                    phase: 0.0,
                    amplitude: 0.5,
                    bipolar: true,
                    tempo_sync: false,
                    division: 2,
                    phase_offset_degrees: 0.0,
                    enabled: true,
                    last_beat_phase: 0.0,
                });
                self.modulation_expanded_source = Some(uuid);
            }
            if ui.button("+ ADSR").clicked() {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                let uuid = mod_eng.add_source(ModulationSource::ADSR {
                    attack: 0.1,
                    decay: 0.2,
                    sustain: 0.5,
                    release: 0.3,
                    stage: rustjay_core::modulation::ADSRStage::Idle,
                    stage_time: 0.0,
                    gate: false,
                    current_level: 0.0,
                });
                self.modulation_expanded_source = Some(uuid);
            }
            if ui.button("+ Step Seq").clicked() {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                let uuid = mod_eng.add_source(ModulationSource::StepSequencer {
                    steps: vec![0.0, 0.25, 0.5, 0.75],
                    rate: 2.0,
                    interpolation: rustjay_core::modulation::StepInterpolation::None,
                    bipolar: false,
                });
                self.modulation_expanded_source = Some(uuid);
            }
        });
    }

    /// Per-source config editor. `mod_eng` is already locked when this is called.
    fn draw_source_editor(
        &mut self,
        ui: &mut egui::Ui,
        mod_eng: &mut rustjay_core::modulation::ModulationEngine,
        uuid: &str,
        bpm: f32,
    ) {
        // ── LFO ──────────────────────────────────────────────────────────────
        if let Some(ModulationSource::LFO {
            waveform,
            frequency,
            phase: _,
            amplitude,
            bipolar,
            tempo_sync,
            division,
            phase_offset_degrees,
            enabled,
            ..
        }) = mod_eng.source_mut(uuid)
        {
            ui.horizontal(|ui| {
                ui.checkbox(enabled, "Enabled");
                ui.checkbox(bipolar, "Bipolar");
                ui.checkbox(tempo_sync, "Tempo Sync");
                if *tempo_sync {
                    ui.label(
                        egui::RichText::new(format!("BPM: {:.1}", bpm))
                            .size(11.0)
                            .color(TEXT_SECONDARY),
                    );
                }
            });

            // Waveform buttons
            ui.horizontal(|ui| {
                ui.label("Waveform:");
                for (name, wf) in WAVE_NAMES {
                    let selected = *waveform == *wf;
                    let btn = if selected {
                        egui::Button::new(egui::RichText::new(*name).strong().color(Color32::BLACK))
                            .fill(ACCENT_CYAN)
                    } else {
                        egui::Button::new(egui::RichText::new(*name).color(TEXT_PRIMARY))
                            .fill(BG_HOVER)
                    };
                    if ui.add_sized(egui::vec2(64.0, 22.0), btn).clicked() && !selected {
                        *waveform = *wf;
                    }
                }
            });

            // Rate or division
            if *tempo_sync {
                let mut div = *division;
                egui::ComboBox::from_id_salt("mod_div")
                    .width(80.0)
                    .selected_text(DIVISION_LABELS[div.min(DIVISION_LABELS.len() - 1)])
                    .show_ui(ui, |ui| {
                        for (j, label) in DIVISION_LABELS.iter().enumerate() {
                            if ui.selectable_label(div == j, *label).clicked() {
                                div = j;
                            }
                        }
                    });
                if div != *division {
                    *division = div;
                }
                ui.label(
                    egui::RichText::new(format!(
                        "= {:.2} Hz",
                        beat_division_to_hz(*division, bpm)
                    ))
                    .size(11.0)
                    .color(TEXT_SECONDARY),
                );
            } else {
                ui.add(
                    egui::Slider::new(frequency, 0.01..=20.0)
                        .text("Frequency (Hz)")
                        .trailing_fill(true),
                );
            }

            ui.horizontal(|ui| {
                ui.add(
                    egui::Slider::new(phase_offset_degrees, 0.0..=360.0)
                        .text("Phase Offset (°)")
                        .trailing_fill(true),
                );
                ui.add(
                    egui::Slider::new(amplitude, 0.0..=1.0)
                        .text("Amplitude")
                        .trailing_fill(true),
                );
            });
        }

        // ── ADSR ─────────────────────────────────────────────────────────────
        if let Some(ModulationSource::ADSR {
            attack, decay, sustain, release, ..
        }) = mod_eng.source_mut(uuid)
        {
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(attack, 0.001..=5.0).text("Attack").logarithmic(true));
                ui.add(egui::Slider::new(decay, 0.001..=5.0).text("Decay").logarithmic(true));
            });
            ui.horizontal(|ui| {
                ui.add(egui::Slider::new(sustain, 0.0..=1.0).text("Sustain"));
                ui.add(egui::Slider::new(release, 0.001..=5.0).text("Release").logarithmic(true));
            });
        }
        // F3: gate toggle must go through trigger_adsr/release_adsr, not direct mutation.
        let is_gated = mod_eng
            .source_mut(uuid)
            .and_then(|s| {
                if let ModulationSource::ADSR { gate, .. } = s {
                    Some(*gate)
                } else {
                    None
                }
            })
            .unwrap_or(false);
        let gate_label = if is_gated { "Release Gate" } else { "Trigger Gate" };
        if ui.button(gate_label).clicked() {
            if is_gated {
                mod_eng.release_adsr(uuid);
            } else {
                mod_eng.trigger_adsr(uuid);
            }
        }

        // ── Step Sequencer ───────────────────────────────────────────────────
        if let Some(ModulationSource::StepSequencer {
            steps, rate, interpolation: _, bipolar, ..
        }) = mod_eng.source_mut(uuid)
        {
            ui.horizontal(|ui| {
                ui.checkbox(bipolar, "Bipolar");
                ui.add(egui::Slider::new(rate, 0.1..=20.0).text("Rate (steps/s)"));
            });
            ui.horizontal(|ui| {
                for (i, step) in steps.iter_mut().enumerate() {
                    ui.vertical(|ui| {
                        ui.label(format!("{}", i + 1));
                        ui.add(egui::DragValue::new(step).speed(0.01).range(-1.0..=1.0));
                    });
                }
            });
        }

        // ── Audio Band ───────────────────────────────────────────────────────
        if mod_eng.find_source_by_uuid(uuid).is_some()
            && matches!(mod_eng.find_source_by_uuid(uuid).unwrap().source, ModulationSource::AudioBand { .. })
        {
            ui.label("Audio Band configuration coming soon.");
        }
    }

    /// Draw assignment list and "Add assignment" UI for the given source.
    fn draw_assignments_for_source(
        &mut self,
        ui: &mut egui::Ui,
        uuid: String,
        mod_arc: &std::sync::Arc<std::sync::Mutex<rustjay_core::modulation::ModulationEngine>>,
        param_ids: &[String],
        param_names: &std::collections::HashMap<String, String>,
    ) {
        ui.separator();
        ui.label(egui::RichText::new("Assignments").strong());

        // Fetch assignments for this source from the engine
        let assignments = {
            let mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            let mut list = Vec::new();
            for (param_id, mods) in mod_eng.assignments.iter() {
                for m in mods {
                    if m.source_id == uuid {
                        list.push((param_id.clone(), m.amount));
                    }
                }
            }
            list
        };

        for (param_id, amount) in &assignments {
            ui.horizontal(|ui| {
                let name = param_names.get(param_id).map(|s| s.as_str()).unwrap_or(param_id);
                ui.label(format!("{} →", name));
                let mut amt = *amount;
                if ui
                    .add(egui::Slider::new(&mut amt, -1.0..=1.0).text("amount"))
                    .changed()
                {
                    let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(mods) = mod_eng.assignments.get_mut(param_id) {
                        for m in mods.iter_mut() {
                            if m.source_id == uuid {
                                m.amount = amt;
                            }
                        }
                    }
                }
                if ui.small_button("✕").clicked() {
                    let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(mods) = mod_eng.assignments.get_mut(param_id) {
                        mods.retain(|m| m.source_id != uuid);
                    }
                }
            });
        }

        if assignments.is_empty() {
            ui.label(
                egui::RichText::new("No assignments — select a parameter below")
                    .size(11.0)
                    .color(TEXT_SECONDARY),
            );
        }

        // Add assignment
        ui.horizontal(|ui| {
            let mut selected = self.modulation_new_assignment_param.clone().unwrap_or_default();
            egui::ComboBox::from_id_salt("mod_new_assign")
                .width(160.0)
                .selected_text(param_names.get(&selected).map(|s| s.as_str()).unwrap_or("—"))
                .show_ui(ui, |ui| {
                    for pid in param_ids {
                        let name = param_names.get(pid).map(|s| s.as_str()).unwrap_or(pid);
                        if ui.selectable_label(selected == *pid, name).clicked() {
                            selected = pid.clone();
                        }
                    }
                });
            self.modulation_new_assignment_param = Some(selected.clone());

            if ui.button("Assign").clicked() && !selected.is_empty() {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                mod_eng.assign(&selected, &uuid, 0.5, None);
            }
        });
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn source_type_name(source: &ModulationSource) -> String {
    match source {
        ModulationSource::LFO { .. } => "LFO".to_string(),
        ModulationSource::AudioBand { .. } => "Audio".to_string(),
        ModulationSource::ADSR { .. } => "ADSR".to_string(),
        ModulationSource::StepSequencer { .. } => "Step".to_string(),
    }
}

fn source_is_enabled(source: &ModulationSource) -> bool {
    match source {
        ModulationSource::LFO { enabled, .. } => *enabled,
        ModulationSource::AudioBand { .. } => true,
        ModulationSource::ADSR { .. } => true,
        ModulationSource::StepSequencer { .. } => true,
    }
}
