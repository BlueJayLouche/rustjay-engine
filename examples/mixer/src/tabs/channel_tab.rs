//! Channel detail tab — per-channel effect parameters.

use rustjay_core::EngineState;
use rustjay_engine::prelude::AnyGuiTab;

pub struct ChannelTab;

impl AnyGuiTab for ChannelTab {
    fn name(&self) -> &str {
        "Channels"
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        _app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        // Channel A: SolidEffect RGB
        ui.text("Channel A — Solid Color");
        {
            let mut red = engine.get_param("ch_a_red").unwrap_or(0.5);
            if ui
                .slider_config("Red##solid_r", 0.0f32, 1.0f32)
                .build(&mut red)
            {
                engine.set_param_base("ch_a_red", red);
            }

            let mut green = engine.get_param("ch_a_green").unwrap_or(0.2);
            if ui
                .slider_config("Green##solid_g", 0.0f32, 1.0f32)
                .build(&mut green)
            {
                engine.set_param_base("ch_a_green", green);
            }

            let mut blue = engine.get_param("ch_a_blue").unwrap_or(0.8);
            if ui
                .slider_config("Blue##solid_b", 0.0f32, 1.0f32)
                .build(&mut blue)
            {
                engine.set_param_base("ch_a_blue", blue);
            }
        }

        ui.separator();

        // Channel B: TintEffect RGB
        ui.text("Channel B — Tint");
        {
            let mut tint_r = engine.get_param("ch_b_tint_r").unwrap_or(1.0);
            if ui
                .slider_config("Tint R##tint_r", 0.0f32, 1.0f32)
                .build(&mut tint_r)
            {
                engine.set_param_base("ch_b_tint_r", tint_r);
            }

            let mut tint_g = engine.get_param("ch_b_tint_g").unwrap_or(1.0);
            if ui
                .slider_config("Tint G##tint_g", 0.0f32, 1.0f32)
                .build(&mut tint_g)
            {
                engine.set_param_base("ch_b_tint_g", tint_g);
            }

            let mut tint_b = engine.get_param("ch_b_tint_b").unwrap_or(1.0);
            if ui
                .slider_config("Tint B##tint_b", 0.0f32, 1.0f32)
                .build(&mut tint_b)
            {
                engine.set_param_base("ch_b_tint_b", tint_b);
            }
        }
    }
}
