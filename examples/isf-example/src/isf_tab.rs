//! Auto-generated ImGui tab built from the ISF input declarations.

use rustjay_engine::prelude::*;

use crate::isf_effect::IsfState;

pub struct IsfTab {
    pub shader_name: String,
}

impl AnyGuiTab for IsfTab {
    fn name(&self) -> &str { &self.shader_name }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<IsfState>()
            .expect("IsfTab expects IsfState");

        // engine.param_descriptors contains exactly what IsfEffect::parameters() returned.
        let descriptors = engine.param_descriptors.clone();

        if descriptors.is_empty() {
            ui.text("No ISF parameters declared.");
            ui.text("(Image and Audio inputs are handled automatically.)");
            return;
        }

        let _w = ui.push_item_width(220.0);

        for desc in descriptors.iter() {
            match &desc.param_type {
                ParamType::Float => {
                    let mut val = engine.get_param(&desc.id).unwrap_or(desc.default);
                    if ui.slider_config(&desc.name, desc.min, desc.max).build(&mut val) {
                        state.values.insert(desc.id.clone(), val);
                        engine.set_param_base(&desc.id, val);
                    }
                }
                ParamType::Bool => {
                    let current = engine.get_param(&desc.id).unwrap_or(desc.default);
                    let mut checked = current >= 0.5;
                    if ui.checkbox(&desc.name, &mut checked) {
                        let v = if checked { 1.0 } else { 0.0 };
                        state.values.insert(desc.id.clone(), v);
                        engine.set_param_base(&desc.id, v);
                    }
                }
                ParamType::Int => {
                    let current = engine.get_param(&desc.id).unwrap_or(desc.default);
                    let mut val = current as i32;
                    if ui.slider_config(&desc.name, desc.min as i32, desc.max as i32).build(&mut val) {
                        state.values.insert(desc.id.clone(), val as f32);
                        engine.set_param_base(&desc.id, val as f32);
                    }
                }
                ParamType::Enum { variants } => {
                    let current = engine.get_param(&desc.id).unwrap_or(desc.default);
                    let mut idx = current as usize;
                    let names: Vec<&str> = variants.iter().map(|s| s.as_str()).collect();
                    if ui.combo_simple_string(&desc.name, &mut idx, &names) {
                        state.values.insert(desc.id.clone(), idx as f32);
                        engine.set_param_base(&desc.id, idx as f32);
                    }
                }
            }
        }
    }
}
