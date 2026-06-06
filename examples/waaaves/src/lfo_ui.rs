//! LFO dot indicators and right-click assignment helpers.

use rustjay_engine::prelude::{EngineState, GuiTab, LfoBank, LfoTarget};

/// Colour palette for the 8 LFO slots.
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

/// Draw inline coloured dots for every enabled LFO targeting this param.
pub fn draw_lfo_dots(ui: &imgui::Ui, param_id: &str, bank: &LfoBank) {
    // Fast path: skip the per-LFO string comparisons when nothing is active.
    if !bank
        .lfos
        .iter()
        .any(|lfo| lfo.enabled && matches!(lfo.target, LfoTarget::Custom(_)))
    {
        return;
    }
    for (i, lfo) in bank.lfos.iter().enumerate() {
        if lfo.enabled {
            if let LfoTarget::Custom(ref target_id) = lfo.target {
                if target_id == param_id {
                    let intensity = lfo.output.abs() * 0.7 + 0.3;
                    let [r, g, b, _] = LFO_COLORS[i];
                    let color = ui.push_style_color(
                        imgui::StyleColor::Text,
                        [r * intensity, g * intensity, b * intensity, 1.0],
                    );
                    ui.same_line();
                    ui.text(format!("●{}", i + 1));
                    color.pop();
                }
            }
        }
    }
}

/// Right-click context menu for LFO assignment.
pub fn lfo_context_menu(
    ui: &imgui::Ui,
    param_id: &str,
    param_label: &str,
    engine_state: &mut EngineState,
) {
    let popup_id = format!("lfo_menu_{}", param_id);
    if ui.is_item_clicked_with_button(imgui::MouseButton::Right) {
        ui.open_popup(&popup_id);
    }

    if ui.begin_popup(&popup_id).is_some() {
        ui.text_disabled(param_label);
        ui.separator();

        for i in 0..engine_state.lfo.bank.lfos.len() {
            let assigned = match &engine_state.lfo.bank.lfos[i].target {
                LfoTarget::Custom(target_id) => target_id.clone(),
                _ => String::new(),
            };
            let is_targeting_us = assigned == param_id;
            let btn_label = if is_targeting_us {
                format!("LFO {}: {} (click to unassign)", i + 1, assigned)
            } else if assigned.is_empty() {
                format!("LFO {}: unassigned", i + 1)
            } else {
                format!("LFO {}: {}", i + 1, assigned)
            };
            if ui.selectable_config(&btn_label).selected(false).build() {
                if is_targeting_us {
                    engine_state.lfo.bank.lfos[i].target = LfoTarget::None;
                } else {
                    engine_state.lfo.bank.lfos[i].target = LfoTarget::Custom(param_id.to_string());
                }
            }
        }

        ui.separator();
        if ui
            .selectable_config("Jump to LFO tab")
            .selected(false)
            .build()
        {
            engine_state.current_tab = GuiTab::Modulation;
        }
    }
}
