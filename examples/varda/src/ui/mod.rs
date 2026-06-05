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
pub struct StageTab {
    #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
    import_path: String,
}

impl Default for StageTab {
    fn default() -> Self {
        Self::new()
    }
}

impl StageTab {
    pub fn new() -> Self {
        Self {
            #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
            import_path: String::new(),
        }
    }
}

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
    // StageTab — 2D surface editor
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
            ui.heading("Stage");
            ui.separator();

            #[cfg(feature = "projection")]
            {
                let state = _app_state
                    .downcast_mut::<VardaAppState>()
                    .expect("StageTab expects VardaAppState");
                Self::draw_stage_tab(ui, state, &mut self.import_path);
            }

            #[cfg(not(feature = "projection"))]
            {
                ui.label("Projection feature is not enabled. Enable it to use surfaces.");
            }
        }
    }

    #[cfg(feature = "projection")]
    impl StageTab {
        fn draw_stage_tab(ui: &mut egui::Ui, state: &mut VardaAppState, import_path: &mut String) {
            use crate::stage::{SurfaceSource, VardaSurface};
            use egui::{Color32, Rect, Pos2, Vec2, Stroke, CornerRadius};

            // Collect channel/deck names for source selector
            let source_options: Vec<(String, SurfaceSource)> = {
                let mut opts = vec![("Master".to_string(), SurfaceSource::Master)];
                if let Ok(mixer) = state.mixer.try_lock() {
                    for ch in &mixer.channels {
                        opts.push((
                            format!("{} ({})", ch.name, &ch.uuid[..ch.uuid.len().min(4)]),
                            SurfaceSource::Channel(ch.uuid.clone()),
                        ));
                        if let Some(compositor) = ch.effect.as_any() {
                            if let Some(compositor) = compositor.downcast_ref::<crate::graph::DeckCompositor>() {
                                for deck in &compositor.decks {
                                    opts.push((
                                        format!("  {} ({})", deck.name, &deck.uuid[..deck.uuid.len().min(4)]),
                                        SurfaceSource::Deck {
                                            channel_uuid: ch.uuid.clone(),
                                            deck_uuid: deck.uuid.clone(),
                                        },
                                    ));
                                }
                            }
                        }
                    }
                }
                opts.push(("Domemaster".to_string(), SurfaceSource::Domemaster));
                opts
            };

            // Set when a surface warp is edited this frame, so we publish to the
            // projector only on change (avoids per-frame mesh rebuilds).
            let mut warp_dirty = false;

            // Left panel: surface list
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(180.0);
                    ui.label(egui::RichText::new("Surfaces").strong());
                    ui.separator();

                    let mut to_remove: Option<usize> = None;
                    for (i, surf) in state.stage.surfaces.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(surf.name.to_string());
                            if ui.small_button("✖").clicked() {
                                to_remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = to_remove {
                        state.stage.surfaces.remove(i);
                    }

                    if ui.button("+ Add Rectangle").clicked() {
                        let idx = state.stage.surfaces.len() + 1;
                        state.stage.surfaces.push(VardaSurface::full_frame(
                            format!("Surface {}", idx),
                            format!("surf{}", idx),
                        ));
                    }
                    if ui.button("+ Add Circle").clicked() {
                        let idx = state.stage.surfaces.len() + 1;
                        state.stage.surfaces.push(VardaSurface::circle(
                            format!("Circle {}", idx),
                            format!("circle{}", idx),
                            [0.5, 0.5],
                            0.25,
                        ));
                    }
                    ui.separator();
                    ui.label("Import:");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(import_path).hint_text("Path to SVG/DXF..."));
                        if ui.button("Import").clicked() && !import_path.is_empty() {
                            let path = std::path::Path::new(&*import_path);
                            let params = rustjay_projection::surface_import::DetectionParams::default();
                            match rustjay_projection::surface_import::detect_from_file(path, &params) {
                                Ok(result) => {
                                    for (i, contour) in result.contours.iter().enumerate() {
                                        let s = contour.to_surface(i);
                                        state.stage.surfaces.push(VardaSurface {
                                            name: s.name,
                                            uuid: format!("import{}", i),
                                            vertices: s.vertices,
                                            is_circular: s.is_circular,
                                            radius: contour.circle_fit.map(|(_, r)| r).unwrap_or(0.1),
                                            source: SurfaceSource::Master,
                                            warp: rustjay_projection::WarpMode::identity(),
                                        });
                                    }
                                    log::info!("Imported {} contours from {}", result.contours.len(), import_path);
                                }
                                Err(e) => {
                                    log::warn!("Import failed for {}: {}", import_path, e);
                                }
                            }
                        }
                    });
                });

                ui.separator();

                // Center: 2D canvas
                let canvas_rect = ui.available_rect_before_wrap();
                let canvas_size = canvas_rect.size().min_elem();
                let canvas_rect = Rect::from_min_size(
                    canvas_rect.min,
                    Vec2::splat(canvas_size),
                );

                let response = ui.interact(canvas_rect, ui.id().with("stage_canvas"), egui::Sense::click());
                let painter = ui.painter_at(canvas_rect);

                // Background
                painter.rect_filled(canvas_rect, CornerRadius::ZERO, Color32::from_gray(30));
                painter.rect_stroke(canvas_rect, CornerRadius::ZERO, Stroke::new(1.0, Color32::from_gray(80)), egui::StrokeKind::Inside);

                // Grid
                for i in 0..=10 {
                    let t = i as f32 / 10.0;
                    let x = canvas_rect.min.x + t * canvas_rect.width();
                    let y = canvas_rect.min.y + t * canvas_rect.height();
                    painter.line_segment(
                        [Pos2::new(x, canvas_rect.min.y), Pos2::new(x, canvas_rect.max.y)],
                        Stroke::new(0.5, Color32::from_gray(50)),
                    );
                    painter.line_segment(
                        [Pos2::new(canvas_rect.min.x, y), Pos2::new(canvas_rect.max.x, y)],
                        Stroke::new(0.5, Color32::from_gray(50)),
                    );
                }

                // Draw surfaces
                for surf in state.stage.surfaces.iter() {
                    let color = if response.clicked_by(egui::PointerButton::Primary) {
                        Color32::from_rgb(100, 150, 255)
                    } else {
                        Color32::from_rgb(200, 100, 100)
                    };

                    if surf.is_circular && !surf.vertices.is_empty() {
                        let center = surf.vertices[0];
                        let cx = canvas_rect.min.x + center[0] * canvas_rect.width();
                        let cy = canvas_rect.min.y + center[1] * canvas_rect.height();
                        let r = surf.radius * canvas_rect.width();
                        painter.circle_stroke(Pos2::new(cx, cy), r, Stroke::new(2.0, color));
                        painter.text(
                            Pos2::new(cx, cy),
                            egui::Align2::CENTER_CENTER,
                            &surf.name,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                    } else if surf.vertices.len() >= 3 {
                        let points: Vec<Pos2> = surf.vertices.iter().map(|v| {
                            Pos2::new(
                                canvas_rect.min.x + v[0] * canvas_rect.width(),
                                canvas_rect.min.y + v[1] * canvas_rect.height(),
                            )
                        }).collect();
                        painter.add(egui::Shape::convex_polygon(
                            points.clone(),
                            color.linear_multiply(0.3),
                            Stroke::new(2.0, color),
                        ));
                        // Label at centroid
                        let centroid_x: f32 = surf.vertices.iter().map(|v| v[0]).sum::<f32>() / surf.vertices.len() as f32;
                        let centroid_y: f32 = surf.vertices.iter().map(|v| v[1]).sum::<f32>() / surf.vertices.len() as f32;
                        painter.text(
                            Pos2::new(
                                canvas_rect.min.x + centroid_x * canvas_rect.width(),
                                canvas_rect.min.y + centroid_y * canvas_rect.height(),
                            ),
                            egui::Align2::CENTER_CENTER,
                            &surf.name,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                    }
                }

                ui.allocate_rect(canvas_rect, egui::Sense::hover());

                ui.separator();

                // Right panel: selected surface properties
                ui.vertical(|ui| {
                    ui.set_width(220.0);
                    ui.label(egui::RichText::new("Properties").strong());
                    ui.separator();

                    if let Some(surf) = state.stage.surfaces.first_mut() {
                        ui.label("Name:");
                        ui.text_edit_singleline(&mut surf.name);
                        ui.separator();

                        ui.label("Source:");
                        let current_label = surf.source.label();
                        egui::ComboBox::from_id_salt("source_sel")
                            .selected_text(&current_label)
                            .show_ui(ui, |ui| {
                                for (label, src) in &source_options {
                                    if ui.selectable_label(surf.source == *src, label).clicked() {
                                        surf.source = src.clone();
                                    }
                                }
                            });
                        ui.separator();

                        ui.label("Warp Mode:");
                        let warp_label = match &surf.warp {
                            rustjay_projection::WarpMode::CornerPin { .. } => "Corner Pin",
                            rustjay_projection::WarpMode::Mesh(_) => "Mesh",
                        };
                        egui::ComboBox::from_id_salt("warp_mode")
                            .selected_text(warp_label)
                            .show_ui(ui, |ui| {
                                if ui.selectable_label(matches!(surf.warp, rustjay_projection::WarpMode::CornerPin { .. }), "Corner Pin").clicked() {
                                    surf.warp = rustjay_projection::WarpMode::corner_pin([
                                        [0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0],
                                    ]);
                                    warp_dirty = true;
                                }
                                if ui.selectable_label(matches!(surf.warp, rustjay_projection::WarpMode::Mesh(_)), "Mesh").clicked() {
                                    surf.warp = rustjay_projection::WarpMode::Mesh(
                                        rustjay_projection::WarpMesh::identity(4, 4),
                                    );
                                    warp_dirty = true;
                                }
                            });

                        if let rustjay_projection::WarpMode::CornerPin { corners } = &mut surf.warp {
                            ui.label("Corners (normalized):");
                            for (i, corner) in corners.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label(["TL", "TR", "BR", "BL"][i]);
                                    if ui.add(egui::DragValue::new(&mut corner[0]).speed(0.01).range(0.0..=1.0)).changed() {
                                        warp_dirty = true;
                                    }
                                    if ui.add(egui::DragValue::new(&mut corner[1]).speed(0.01).range(0.0..=1.0)).changed() {
                                        warp_dirty = true;
                                    }
                                });
                            }
                        }
                    } else {
                        ui.label("No surfaces. Add one from the list.");
                    }
                });
            });

            // Push warp edits to the projector's live warp stage (version-bumped,
            // so the projector re-applies only on an actual change).
            if warp_dirty {
                state.stage.publish_warp();
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Stub tabs (Phase 8+)
    // ─────────────────────────────────────────────────────────────────────────

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
