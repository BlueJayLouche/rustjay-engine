//! Unified Modulation tab — edits the shared `ModulationEngine` (M5.2).
//!
//! Replaces the legacy 8-slot LFO bank with a source-list + assignment editor.
//! All writes use the clone-Arc-then-drop pattern (F1) so the lock hierarchy
//! `shared_state` → `modulation` is never violated.

use crate::egui_control_gui::EguiControlGui;
use rustjay_core::routing::FftBand;
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
            egui::RichText::new("LFO · ADSR · Step Sequencer · Audio Band · Trigger")
                .size(11.0)
                .color(text_secondary()),
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
        if let Some(ref u) = self.modulation_expanded_source
            && !source_uuids.contains(u) {
                self.modulation_expanded_source = None;
            }

        // Track which source is expanded (persisted in gui state)
        let expanded_uuid = self
            .modulation_expanded_source
            .clone()
            .unwrap_or_default();

        for (uuid, typ, value, enabled) in &sources_snapshot {
            let is_expanded = expanded_uuid == *uuid;
            let header_color = if *enabled { accent_cyan() } else { text_secondary() };

            egui::Frame::group(ui.style())
                .fill(bg_widget())
                .stroke(egui::Stroke::new(1.0_f32, if is_expanded { accent_cyan() } else { border() }))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());

                    // Header row: type + short uuid + value + expand toggle
                    ui.horizontal(|ui| {
                        let label = format!("{}  {}  → {:.2}", typ, &uuid[..4], value);
                        let btn = egui::Button::new(
                            egui::RichText::new(label).color(header_color).strong(),
                        )
                        .fill(if is_expanded { bg_hover() } else { bg_widget() });
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
                    gate_source: None,
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
            if ui.button("+ Trigger").clicked() {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                let uuid = mod_eng.add_source(ModulationSource::AudioTrigger {
                    band: FftBand::Bass,
                    threshold: 1.3,
                    hysteresis: 0.3,
                    min_interval: 0.05,
                    hold: 0.1,
                    armed: true,
                    since_fire: 0.0,
                    hold_left: 0.0,
                    average: 0.0,
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
                            .color(text_secondary()),
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
                            .fill(accent_cyan())
                    } else {
                        egui::Button::new(egui::RichText::new(*name).color(text_primary()))
                            .fill(bg_hover())
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
                    .color(text_secondary()),
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
        let is_adsr = matches!(
            mod_eng.find_source_by_uuid(uuid).map(|e| &e.source),
            Some(ModulationSource::ADSR { .. })
        );
        if is_adsr {
            // What fires this envelope. Without a source it can only be gated by
            // hand from the button beside it, which is no use during a set.
            let triggers: Vec<(String, String)> = mod_eng
                .sources
                .iter()
                .filter(|e| matches!(e.source, ModulationSource::AudioTrigger { .. }))
                .map(|e| (e.uuid.clone(), e.uuid[..4.min(e.uuid.len())].to_string()))
                .collect();
            let current = mod_eng
                .find_source_by_uuid(uuid)
                .and_then(|e| match &e.source {
                    ModulationSource::ADSR { gate_source, .. } => gate_source.clone(),
                    _ => None,
                });
            let mut chosen: Option<Option<String>> = None;
            ui.horizontal(|ui| {
                ui.label("Gated by:");
                egui::ComboBox::from_id_salt(("gatesrc", uuid))
                    .selected_text(match &current {
                        Some(u) => format!("Trigger {}", &u[..4.min(u.len())]),
                        None => "— manual —".to_string(),
                    })
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "— manual —").clicked() {
                            chosen = Some(None);
                        }
                        for (id, tag) in &triggers {
                            if ui
                                .selectable_label(
                                    current.as_deref() == Some(id.as_str()),
                                    format!("Trigger {tag}"),
                                )
                                .clicked()
                            {
                                chosen = Some(Some(id.clone()));
                            }
                        }
                    });
                if triggers.is_empty() {
                    ui.label(
                        egui::RichText::new("add a Trigger source first")
                            .small()
                            .weak(),
                    );
                }
            });
            if let Some(pick) = chosen
                && let Some(ModulationSource::ADSR { gate_source, .. }) = mod_eng.source_mut(uuid)
            {
                *gate_source = pick;
            }
        }
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
        if let Some(ModulationSource::AudioBand {
            freq_low,
            freq_high,
            gain,
            smoothing,
            attack,
            enabled,
            noise_gate,
            ..
        }) = mod_eng.source_mut(uuid)
        {
            ui.checkbox(enabled, "Enabled");
            // The analyser's own bands, the same set the routing grid offers.
            // The range stays editable underneath for anything custom.
            let current = FftBand::all()
                .iter()
                .find(|b| {
                    let (lo, hi) = b.freq_range();
                    (lo - *freq_low).abs() < 0.5 && (hi - *freq_high).abs() < 0.5
                })
                .copied();
            ui.horizontal(|ui| {
                ui.label("Band:");
                egui::ComboBox::from_id_salt(("band", uuid))
                    .selected_text(match current {
                        Some(b) => b.name().to_string(),
                        None => format!("{freq_low:.0}–{freq_high:.0} Hz"),
                    })
                    .show_ui(ui, |ui| {
                        for band in FftBand::all() {
                            if ui
                                .selectable_label(current == Some(*band), band.name())
                                .clicked()
                            {
                                let (lo, hi) = band.freq_range();
                                *freq_low = lo;
                                *freq_high = hi;
                            }
                        }
                    });
            });
            ui.add(
                egui::Slider::new(attack, 0.0..=0.99)
                    .text("Attack")
                    .custom_formatter(|v, _| format!("{v:.2}")),
            )
            .on_hover_text("Rise smoothing — 0 is instant, 0.99 very slow");
            ui.add(egui::Slider::new(smoothing, 0.0..=0.99).text("Release"))
                .on_hover_text("Fall smoothing — 0 is instant, 0.99 very slow");
            ui.add(egui::Slider::new(gain, 0.0..=8.0).text("Gain"));
            ui.add(egui::Slider::new(noise_gate, 0.0..=1.0).text("Noise gate"))
                .on_hover_text("Energy below this counts as silence");
        }

        // ── Audio Trigger ────────────────────────────────────────────────────
        if let Some(ModulationSource::AudioTrigger {
            band,
            threshold,
            hysteresis,
            min_interval,
            hold,
            ..
        }) = mod_eng.source_mut(uuid)
        {
            ui.horizontal(|ui| {
                ui.label("Band:");
                egui::ComboBox::from_id_salt(("trigband", uuid))
                    .selected_text(band.name())
                    .show_ui(ui, |ui| {
                        for b in FftBand::all() {
                            if ui.selectable_label(*band == *b, b.name()).clicked() {
                                *band = *b;
                            }
                        }
                    });
            });
            ui.add(egui::Slider::new(threshold, 1.0..=4.0).text("Threshold"))
                .on_hover_text("How far above the running average a hit must be");
            ui.add(egui::Slider::new(hysteresis, 0.0..=0.9).text("Hysteresis"))
                .on_hover_text("How far it must fall back before firing again");
            ui.add(egui::Slider::new(min_interval, 0.0..=1.0).text("Min gap (s)"));
            ui.add(egui::Slider::new(hold, 0.0..=2.0).text("Hold (s)"))
                .on_hover_text("How long the gate stays open — an envelope's sustain needs this");
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
                    .color(text_secondary()),
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
        ModulationSource::AudioTrigger { .. } => "Trigger".to_string(),
        ModulationSource::ADSR { .. } => "ADSR".to_string(),
        ModulationSource::StepSequencer { .. } => "Step".to_string(),
    }
}

fn source_is_enabled(source: &ModulationSource) -> bool {
    match source {
        ModulationSource::LFO { enabled, .. } => *enabled,
        ModulationSource::AudioBand { .. } => true,
        ModulationSource::AudioTrigger { .. } => true,
        ModulationSource::ADSR { .. } => true,
        ModulationSource::StepSequencer { .. } => true,
    }
}
