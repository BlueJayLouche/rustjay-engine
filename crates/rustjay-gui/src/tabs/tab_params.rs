//! # Auto-generated Parameter Category Tab
//!
//! Renders ImGui controls for all parameters in a given category,
//! driven by `EngineState::param_descriptors`.

use crate::control_gui::ControlGui;
use rustjay_core::{ParamCategory, ParamType};

impl ControlGui {
    /// Build an auto-generated parameter tab for a given category.
    ///
    /// This renders sliders, checkboxes, and dropdowns for every
    /// `ParameterDescriptor` whose category matches.
    pub(crate) fn build_param_category_tab(&mut self, ui: &imgui::Ui, category: ParamCategory) {
        let descriptors = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .param_descriptors
                .iter()
                .filter(|d| d.category == category)
                .cloned()
                .collect::<Vec<_>>()
        };

        if descriptors.is_empty() {
            ui.text_disabled("No parameters declared for this category.");
            return;
        }

        ui.text(format!("{} Parameters", category.name()));
        ui.separator();

        for desc in &descriptors {
            match &desc.param_type {
                ParamType::Float => {
                    let mut value = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.get_param_base(&desc.id).unwrap_or(desc.default)
                    };
                    if ui
                        .slider_config(&desc.name, desc.min, desc.max)
                        .build(&mut value)
                    {
                        let mut state =
                            self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.set_param_base(&desc.id, value);
                    }
                }
                ParamType::Int => {
                    let mut value = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.get_param_base(&desc.id).unwrap_or(desc.default) as i32
                    };
                    if ui
                        .slider_config(&desc.name, desc.min as i32, desc.max as i32)
                        .build(&mut value)
                    {
                        let mut state =
                            self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.set_param_base(&desc.id, value as f32);
                    }
                }
                ParamType::Bool => {
                    let mut value = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.get_param_base(&desc.id).unwrap_or(desc.default) >= 0.5
                    };
                    if ui.checkbox(&desc.name, &mut value) {
                        let mut state =
                            self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.set_param_base(&desc.id, if value { 1.0 } else { 0.0 });
                    }
                }
                ParamType::Enum { variants } => {
                    let mut idx = {
                        let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state
                            .get_param_base(&desc.id)
                            .unwrap_or(desc.default)
                            .clamp(0.0, (variants.len() - 1) as f32) as usize
                    };
                    if ui.combo_simple_string(&desc.name, &mut idx, variants) {
                        let mut state =
                            self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        state.set_param_base(&desc.id, idx as f32);
                    }
                }
            }

            ui.spacing();
        }

        // Show LFO opener if any params in this category are modulatable
        let has_modulatable = descriptors.iter().any(|d| d.is_modulatable());
        if has_modulatable {
            ui.separator();
            ui.spacing();
            ui.text("LFO Modulation");
            if ui.button("Open LFO Window") {
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.lfo.show_window = true;
            }

            let active_lfos = {
                let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                state.lfo.bank.lfos.iter().filter(|b| b.enabled).count()
            };
            if active_lfos > 0 {
                ui.same_line();
                ui.text_colored([0.2, 0.8, 0.2, 1.0], format!("({} active)", active_lfos));
            }
        }
    }
}
