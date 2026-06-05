//! GUI — egui tabs for Varda's panels.
//!
//! Each tab is a non-replacing `AnyEguiTab` (it gets its own sidebar button via
//! the engine host) so the built-in tabs (incl. the working LFO/MIDI panels)
//! stay available alongside them. Params are driven through
//! `engine.get_param*` / `set_param_base` (canonical ids) so the GUI stays
//! co-equal with MIDI/OSC/HTTP/LFO. Per-item `ui.push_id(uuid)` scopes avoid
//! egui id collisions across channels/decks/FX.
//!
//! See VARDA_PORT.md §5 and `examples/delta-egui`.

/// Mixer tab — crossfader, per-channel opacity, master FX.
pub struct MixerTab;

/// Deck tab — source picker, opacity/blend/scaling, deck FX.
pub struct DeckTab;

/// Effects / Library tab — registry list + add/enable/reorder.
pub struct EffectsTab;

/// Modulation tab — LFO/audio/ADSR/step assignment + chaining graph.
pub struct ModulationTab;

/// Sequencer tab — transition sequences.
pub struct SequencerTab;

/// MIDI tab — device select, learn/unlearn, mapping table.
pub struct MidiTab;

/// Stage tab — 2D surface editor, warp handles, import.
pub struct StageTab;

/// Outputs tab — window/display/NDI/stream/record assignment.
pub struct OutputsTab;

/// Inspector tab — context panel for selected node.
pub struct InspectorTab;

#[cfg(all(feature = "mixer", feature = "egui"))]
mod egui_impl {
    use super::*;
    use rustjay_engine::prelude::*;
    use rustjay_core::EngineState;
    use rustjay_mixer::BlendMode;
    use crate::graph::DeckCompositor;
    use crate::VardaAppState;

    /// Helper: draw a blend-mode combo bound to a canonical engine param key.
    fn blend_combo(ui: &mut egui::Ui, engine: &mut EngineState, key: &str, label: &str) {
        let mut idx = engine.get_param_base(key).unwrap_or(0.0).round() as usize;
        let prev = idx;
        let names: Vec<&str> = BlendMode::all().iter().map(|m| m.short_name()).collect();
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(key)
                .width(ui.available_width())
                .selected_text(*names.get(idx).unwrap_or(&"???"))
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(idx == i, *name).clicked() {
                            idx = i;
                        }
                    }
                });
        });
        if idx != prev {
            engine.set_param_base(key, idx as f32);
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MixerTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for MixerTab {
        fn name(&self) -> &str {
            "Mixer"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_ref::<VardaAppState>()
                .expect("MixerTab expects VardaAppState");

            ui.heading("Mixer");
            ui.separator();
            param_slider(ui, engine, "crossfader", "Crossfader", 0.0, 1.0);
            ui.separator();

            let mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

            for ch in &mixer.channels {
                ui.push_id(&ch.uuid, |ui| {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new(&ch.name).strong());
                        let opacity_key = format!("ch_{}_opacity", ch.uuid);
                        param_slider(ui, engine, &opacity_key, "Opacity", 0.0, 1.0);
                        blend_combo(ui, engine, &format!("ch_{}_blend", ch.uuid), "Blend:");
                    });
                });
            }

            if !mixer.master.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Master FX").strong());
                for slot in &mixer.master {
                    let status = if slot.enabled { "●" } else { "○" };
                    ui.label(format!("{} {}", status, slot.effect.label()));
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // DeckTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for DeckTab {
        fn name(&self) -> &str {
            "Deck"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_ref::<VardaAppState>()
                .expect("DeckTab expects VardaAppState");

            ui.heading("Decks");
            ui.separator();

            let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

            for ch in &mut mixer.channels {
                ui.collapsing(&ch.name, |ui| {
                    let Some(compositor) = ch.effect.as_any_mut() else { return; };
                    let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() else { return; };

                    for deck in &mut compositor.decks {
                        ui.push_id(deck.uuid.clone(), |ui| {
                            ui.group(|ui| {
                                ui.label(egui::RichText::new(&deck.name).strong());
                                param_slider(ui, engine, &deck.opacity_key, "Opacity", 0.0, 1.0);
                                blend_combo(ui, engine, &deck.blend_key, "Blend:");

                                if !deck.chain.is_empty() {
                                    ui.label("FX:");
                                    let mut fx_i = 0;
                                    while fx_i < deck.chain.len() {
                                        let mut enabled = deck.chain[fx_i].enabled;
                                        let fx_label = deck.chain[fx_i].effect.label();
                                        if ui.checkbox(&mut enabled, fx_label).changed() {
                                            deck.set_effect_enabled(fx_i, enabled);
                                        }
                                        fx_i += 1;
                                    }
                                }
                            });
                        });
                    }
                });
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // EffectsTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for EffectsTab {
        fn name(&self) -> &str {
            "Effects"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_ref::<VardaAppState>()
                .expect("EffectsTab expects VardaAppState");

            ui.heading("Effects / Library");
            ui.separator();

            // Library listing (read-only — no device/queue in draw() for runtime creation)
            ui.label(egui::RichText::new("Library").strong());
            egui::ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                for entry in &state.registry.shaders {
                    ui.label(format!("🎨 {}", entry.name));
                }
                for entry in &state.registry.images {
                    ui.label(format!("🖼 {}", entry.name));
                }
                for entry in &state.registry.videos {
                    ui.label(format!("🎬 {}", entry.name));
                }
            });
            ui.separator();

            // Live FX chains
            ui.label(egui::RichText::new("Live FX Chains").strong());

            let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

            for ch in &mut mixer.channels {
                ui.push_id(ch.uuid.clone(), |ui| {
                ui.collapsing(format!("Channel: {}", ch.name), |ui| {
                    // Channel FX
                    if !ch.chain.is_empty() {
                        ui.label("Channel FX:");
                        let mut fx_i = 0;
                        while fx_i < ch.chain.len() {
                            let mut enabled = ch.chain[fx_i].enabled;
                            let fx_label = ch.chain[fx_i].effect.label();
                            if ui.push_id(("ch_fx", fx_i), |ui| ui.checkbox(&mut enabled, fx_label).changed()).inner {
                                ch.chain[fx_i].enabled = enabled;
                            }
                            fx_i += 1;
                        }
                    }

                    // Deck FX
                    let Some(compositor) = ch.effect.as_any_mut() else { return; };
                    let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() else { return; };
                    for deck in &mut compositor.decks {
                        if deck.chain.is_empty() {
                            continue;
                        }
                        ui.push_id(deck.uuid.clone(), |ui| {
                            ui.label(format!("Deck {} FX:", deck.name));
                            let mut fx_i = 0;
                            while fx_i < deck.chain.len() {
                                let mut enabled = deck.chain[fx_i].enabled;
                                let fx_label = deck.chain[fx_i].effect.label();
                                if ui.push_id(fx_i, |ui| ui.checkbox(&mut enabled, fx_label).changed()).inner {
                                    deck.set_effect_enabled(fx_i, enabled);
                                }
                                fx_i += 1;
                            }
                        });
                    }
                });
                });
            }

            if !mixer.master.is_empty() {
                ui.collapsing("Master FX", |ui| {
                    let mut fx_i = 0;
                    while fx_i < mixer.master.len() {
                        let mut enabled = mixer.master[fx_i].enabled;
                        let fx_label = mixer.master[fx_i].effect.label();
                        if ui.push_id(("master_fx", fx_i), |ui| ui.checkbox(&mut enabled, fx_label).changed()).inner {
                            mixer.master[fx_i].enabled = enabled;
                        }
                        fx_i += 1;
                    }
                });
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // ModulationTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for ModulationTab {
        fn name(&self) -> &str {
            "Modulation"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_ref::<VardaAppState>()
                .expect("ModulationTab expects VardaAppState");

            ui.heading("Modulation");
            ui.separator();

            let mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());
            let mod_eng = mixer.modulation.lock().unwrap_or_else(|e| e.into_inner());

            if mod_eng.sources.is_empty() {
                ui.label("No modulation sources active.");
                return;
            }

            ui.label(egui::RichText::new("Sources").strong());
            for src in &mod_eng.sources {
                let kind = match &src.source {
                    rustjay_core::modulation::ModulationSource::LFO { waveform, frequency, .. } => {
                        format!("LFO ({:?}) @ {:.2} Hz", waveform, frequency)
                    }
                    rustjay_core::modulation::ModulationSource::AudioBand { freq_low, freq_high, .. } => {
                        format!("Audio [{:.0}–{:.0} Hz]", freq_low, freq_high)
                    }
                    rustjay_core::modulation::ModulationSource::ADSR { .. } => "ADSR".to_string(),
                    rustjay_core::modulation::ModulationSource::StepSequencer { .. } => {
                        "Step Seq".to_string()
                    }

                };
                ui.label(format!("{} — {}", &src.uuid[..8.min(src.uuid.len())], kind));
            }

            ui.separator();
            ui.label(egui::RichText::new("Assignments").strong());
            if mod_eng.assignments.is_empty() {
                ui.label("No active assignments.");
            } else {
                for (param, mods) in &mod_eng.assignments {
                    for m in mods {
                        ui.label(format!(
                            "{} ← {} @ {:.2}",
                            param,
                            &m.source_id[..8.min(m.source_id.len())],
                            m.amount
                        ));
                    }
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MidiTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for MidiTab {
        fn name(&self) -> &str {
            "MIDI"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("MIDI");
            ui.separator();
            ui.label("MIDI device selection, learn/unlearn, and mapping are managed by the engine's built-in MIDI system.");
            ui.label("Connect a controller and move a knob to auto-learn its mapping to the currently selected parameter.");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Stub tabs (Phase 7+)
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for StageTab {
        fn name(&self) -> &str {
            "Stage"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Stage / Geometry");
            ui.separator();
            ui.label("2D surface editor, warp handles, and SVG/DXF import.");
            ui.label("Coming in Phase 7 (projection mapping).");
        }
    }

    impl AnyEguiTab for OutputsTab {
        fn name(&self) -> &str {
            "Outputs"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Outputs");
            ui.separator();
            ui.label("Multi-output window/display assignment, NDI, streaming, and recording.");
            ui.label("Coming in Phase 8+.");
        }
    }

    impl AnyEguiTab for SequencerTab {
        fn name(&self) -> &str {
            "Sequencer"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Sequencer");
            ui.separator();
            ui.label("Transition sequences and beat-synced scene changes.");
            ui.label("Coming in Phase 12.");
        }
    }

    impl AnyEguiTab for InspectorTab {
        fn name(&self) -> &str {
            "Inspector"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Inspector");
            ui.separator();
            ui.label("Context panel for the selected node.");
            ui.label("Coming in a future phase.");
        }
    }
}
