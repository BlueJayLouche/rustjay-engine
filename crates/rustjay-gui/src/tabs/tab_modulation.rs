//! Unified Modulation tab (imgui) — M5.3
//!
//! Provides source-list browsing, add/remove LFO, and basic config editing
//! for the unified ModulationEngine. Less feature-rich than the egui version
//! (M5.2) but functional for waaaves and other imgui-based apps.

use crate::control_gui::ControlGui;
use rustjay_core::modulation::{LFOWaveform, ModulationSource};
use rustjay_core::lfo::{beat_division_to_hz, BEAT_DIVISIONS};

impl ControlGui {
    /// Build the Modulation tab (M5.1 — renamed from LFO).
    pub(crate) fn build_modulation_tab(&mut self, ui: &imgui::Ui) {
        ui.text("Modulation");
        ui.text_disabled("LFO · ADSR · Step Sequencer · Audio Band");
        ui.separator();

        let (mod_arc, bpm) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            (state.modulation.clone(), state.effective_bpm())
        };

        // Snapshot sources
        let sources: Vec<(String, String, f32, bool)> = {
            let mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            mod_eng
                .sources
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let typ = source_type_name(&e.source);
                    let val = mod_eng.current_values().get(i).copied().unwrap_or(0.0);
                    let en = source_is_enabled(&e.source);
                    (e.uuid.clone(), typ, val, en)
                })
                .collect()
        };

        // Source list
        for (uuid, typ, val, enabled) in &sources {
            let token = ui.push_id(&uuid[..4]);
            let color = if *enabled { [0.4, 0.8, 1.0, 1.0] } else { [0.5, 0.5, 0.5, 1.0] };
            ui.text_colored(color, format!("{}  {}  → {:.2}", typ, &uuid[..4], val));
            ui.same_line();
            if ui.small_button("Edit") {
                self.modulation_imgui_expanded = Some(uuid.clone());
            }
            ui.same_line();
            if ui.small_button("Remove") {
                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                mod_eng.remove_source(uuid);
                if self.modulation_imgui_expanded.as_ref() == Some(uuid) {
                    self.modulation_imgui_expanded = None;
                }
            }
            token.pop();
        }

        // Expanded editor
        if let Some(uuid) = self.modulation_imgui_expanded.clone() {
            ui.separator();
            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(source) = mod_eng.source_mut(&uuid) {
                self.draw_modulation_source_imgui(ui, source, bpm);
            }
        }

        ui.separator();
        if ui.button("+ LFO") {
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
            self.modulation_imgui_expanded = Some(uuid);
        }
        ui.same_line();
        if ui.button("+ ADSR") {
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
            self.modulation_imgui_expanded = Some(uuid);
        }
    }

    fn draw_modulation_source_imgui(&mut self, ui: &imgui::Ui, source: &mut ModulationSource, bpm: f32) {
        match source {
            ModulationSource::LFO {
                waveform,
                frequency,
                amplitude,
                bipolar,
                tempo_sync,
                division,
                phase_offset_degrees,
                enabled,
                ..
            } => {
                let mut en = *enabled;
                ui.checkbox("Enabled", &mut en);
                *enabled = en;

                let mut bp = *bipolar;
                ui.checkbox("Bipolar", &mut bp);
                *bipolar = bp;

                let mut ts = *tempo_sync;
                ui.checkbox("Tempo Sync", &mut ts);
                *tempo_sync = ts;

                // Waveform combo
                let wave_labels = ["Sine", "Square", "Triangle", "Sawtooth", "Random"];
                let mut wf_idx = *waveform as usize;
                if ui.combo("Waveform", &mut wf_idx, &wave_labels, |s| (*s).into()) {
                    *waveform = match wf_idx {
                        0 => LFOWaveform::Sine,
                        1 => LFOWaveform::Square,
                        2 => LFOWaveform::Triangle,
                        3 => LFOWaveform::Sawtooth,
                        4 => LFOWaveform::Random,
                        _ => LFOWaveform::Sine,
                    };
                }

                if *tempo_sync {
                    let div_labels = ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];
                    let mut div = *division;
                    if ui.combo("Division", &mut div, &div_labels, |s| (*s).into()) {
                        *division = div;
                    }
                    ui.text_disabled(format!("= {:.2} Hz", beat_division_to_hz(*division, bpm)));
                } else {
                    ui.input_float("Frequency (Hz)", frequency)
                        .step(0.1)
                        .build();
                    *frequency = frequency.clamp(0.01, 20.0);
                }

                ui.input_float("Amplitude", amplitude)
                    .step(0.05)
                    .build();
                *amplitude = amplitude.clamp(0.0, 1.0);

                ui.input_float("Phase Offset (°)", phase_offset_degrees)
                    .step(1.0)
                    .build();
                *phase_offset_degrees = phase_offset_degrees.clamp(0.0, 360.0);
            }
            ModulationSource::ADSR {
                attack, decay, sustain, release, gate, ..
            } => {
                ui.input_float("Attack", attack).step(0.01).build();
                *attack = attack.max(0.001);
                ui.input_float("Decay", decay).step(0.01).build();
                *decay = decay.max(0.001);
                ui.slider("Sustain", 0.0, 1.0, sustain);
                ui.input_float("Release", release).step(0.01).build();
                *release = release.max(0.001);
                let label = if *gate { "Release Gate" } else { "Trigger Gate" };
                if ui.button(label) {
                    *gate = !*gate;
                }
            }
            ModulationSource::StepSequencer { steps, rate, .. } => {
                ui.input_float("Rate", rate).step(0.1).build();
                *rate = rate.max(0.01);
                for (i, step) in steps.iter_mut().enumerate() {
                    ui.slider(format!("Step {}", i + 1), -1.0, 1.0, step);
                }
            }
            ModulationSource::AudioBand { .. } => {
                ui.text_disabled("Audio Band configuration not yet available in imgui tab.");
            }
        }
    }
}

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
