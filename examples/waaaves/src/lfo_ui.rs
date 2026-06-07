//! LFO dot indicators and right-click assignment helpers (unified engine).
//!
//! M6.1: Rewritten to use `engine.modulation` instead of the deprecated `engine.lfo.bank`.

use rustjay_engine::prelude::{EngineState, GuiTab, ModulationSource};

/// Colour palette for modulation sources (cycles for indices > 8).
pub const LFO_COLORS: [[f32; 4]; 8] = [
    [1.0, 0.2, 0.2, 1.0], // 0 red
    [1.0, 0.6, 0.1, 1.0], // 1 orange
    [1.0, 1.0, 0.2, 1.0], // 2 yellow
    [0.2, 1.0, 0.3, 1.0], // 3 green
    [0.2, 0.9, 0.9, 1.0], // 4 cyan
    [0.3, 0.4, 1.0, 1.0], // 5 blue
    [0.8, 0.3, 1.0, 1.0], // 6 violet
    [1.0, 0.3, 0.8, 1.0], // 7 magenta
];

/// Draw inline coloured dots for every enabled LFO source targeting this param.
pub fn draw_lfo_dots(ui: &imgui::Ui, param_id: &str, engine: &EngineState) {
    let mod_eng = engine.modulation.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mods) = mod_eng.assignments.get(param_id) else {
        return;
    };
    if mods.is_empty() {
        return;
    }

    for m in mods {
        let Some(idx) = mod_eng
            .sources
            .iter()
            .position(|e| e.uuid == m.source_id)
        else {
            continue;
        };
        let source = &mod_eng.sources[idx].source;
        let enabled = match source {
            ModulationSource::LFO { enabled, .. } => *enabled,
            _ => true,
        };
        if !enabled {
            continue;
        }
        let value = mod_eng.current_values().get(idx).copied().unwrap_or(0.0);
        let intensity = value.abs() * 0.7 + 0.3;
        let [r, g, b, _] = LFO_COLORS[idx % LFO_COLORS.len()];
        let color = ui.push_style_color(
            imgui::StyleColor::Text,
            [r * intensity, g * intensity, b * intensity, 1.0],
        );
        ui.same_line();
        ui.text(format!("●{}", idx + 1));
        color.pop();
    }
}

/// Right-click context menu for modulation assignment.
pub fn lfo_context_menu(
    ui: &imgui::Ui,
    param_id: &str,
    param_label: &str,
    engine_state: &mut EngineState,
) {
    let popup_id = format!("mod_menu_{}", param_id);
    if ui.is_item_clicked_with_button(imgui::MouseButton::Right) {
        ui.open_popup(&popup_id);
    }

    if ui.begin_popup(&popup_id).is_some() {
        ui.text_disabled(param_label);
        ui.separator();

        // Snapshot LFO sources and their assignment status for this param
        let lfo_sources: Vec<(String, usize, bool)> = {
            let mod_eng = engine_state.modulation.lock().unwrap_or_else(|e| e.into_inner());
            mod_eng
                .sources
                .iter()
                .enumerate()
                .filter_map(|(i, e)| match &e.source {
                    ModulationSource::LFO { enabled: true, .. } => {
                        let assigned = mod_eng
                            .assignments
                            .get(param_id)
                            .map(|mods| mods.iter().any(|m| m.source_id == e.uuid))
                            .unwrap_or(false);
                        Some((e.uuid.clone(), i, assigned))
                    }
                    _ => None,
                })
                .collect()
        };

        for (uuid, idx, assigned) in &lfo_sources {
            let btn_label = if *assigned {
                format!("Source {}: {} (click to unassign)", idx + 1, param_id)
            } else {
                format!("Source {}: unassigned", idx + 1)
            };
            if ui.selectable_config(&btn_label).selected(false).build() {
                let mut mod_eng = engine_state
                    .modulation
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if *assigned {
                    if let Some(mods) = mod_eng.assignments.get_mut(param_id) {
                        mods.retain(|m| m.source_id != *uuid);
                    }
                } else {
                    mod_eng.assign(param_id, uuid, 1.0, None);
                }
            }
        }

        if lfo_sources.is_empty() {
            ui.text_disabled("No LFO sources — add one in the Modulation tab.");
        }

        ui.separator();
        if ui
            .selectable_config("Jump to Modulation tab")
            .selected(false)
            .build()
        {
            engine_state.current_tab = GuiTab::Modulation;
        }
    }
}
