//! Mixer tab — crossfader + channel strips.

use rustjay_core::EngineState;
use rustjay_engine::prelude::AnyGuiTab;
use rustjay_mixer::BlendMode;

pub struct MixerTab;

impl AnyGuiTab for MixerTab {
    fn name(&self) -> &str {
        "Mixer"
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        _app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        ui.text("Crossfader");
        {
            let mut crossfader = engine.get_param("crossfader").unwrap_or(0.5);
            if ui.slider_config("##crossfader", 0.0f32, 1.0f32).build(&mut crossfader) {
                engine.set_param_base("crossfader", crossfader);
            }
        }

        ui.separator();

        // Channel A strip
        ui.text("Channel A");
        {
            let mut opacity = engine.get_param("ch_a_opacity").unwrap_or(1.0);
            if ui.slider_config("Opacity A##opa", 0.0f32, 1.0f32).build(&mut opacity) {
                engine.set_param_base("ch_a_opacity", opacity);
            }

            let blend = engine.get_param("ch_a_blend").unwrap_or(0.0) as i32;
            let blend_names: Vec<&str> = BlendMode::all().iter().map(|m| m.short_name()).collect();
            let mut idx = blend as usize;
            if ui.combo_simple_string("Blend A##bla", &mut idx, &blend_names) {
                engine.set_param_base("ch_a_blend", idx as f32);
            }
        }

        ui.separator();

        // Channel B strip
        ui.text("Channel B");
        {
            let mut opacity = engine.get_param("ch_b_opacity").unwrap_or(1.0);
            if ui.slider_config("Opacity B##opb", 0.0f32, 1.0f32).build(&mut opacity) {
                engine.set_param_base("ch_b_opacity", opacity);
            }

            let blend = engine.get_param("ch_b_blend").unwrap_or(0.0) as i32;
            let blend_names: Vec<&str> = BlendMode::all().iter().map(|m| m.short_name()).collect();
            let mut idx = blend as usize;
            if ui.combo_simple_string("Blend B##blb", &mut idx, &blend_names) {
                engine.set_param_base("ch_b_blend", idx as f32);
            }
        }
    }
}
