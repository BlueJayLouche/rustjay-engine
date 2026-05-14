//! Delta — RGB spatial delay / motion extraction.
//!
//! Demonstrates effect-declared parameters with LFO, audio routing,
//! OSC, MIDI, and web remote control.

use rustjay_engine::prelude::*;

struct DeltaEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DeltaUniforms {
    delay_r: f32,
    delay_g: f32,
    delay_b: f32,
    mix_amount: f32,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct DeltaState {
    delay_r: f32,
    delay_g: f32,
    delay_b: f32,
    mix_amount: f32,
}

impl EffectPlugin for DeltaEffect {
    type State = DeltaState;
    type Uniforms = DeltaUniforms;

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/delta.wgsl")
    }

    /// Declare motion parameters that drive UI, LFO targets, audio routing,
    /// and OSC/MIDI/Web control mappings.
    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float(
                "delay_r", "Red Offset", ParamCategory::Motion,
                -10.0, 10.0, 0.0, 0.1,
            ),
            ParameterDescriptor::float(
                "delay_g", "Green Offset", ParamCategory::Motion,
                -10.0, 10.0, 0.0, 0.1,
            ),
            ParameterDescriptor::float(
                "delay_b", "Blue Offset", ParamCategory::Motion,
                -10.0, 10.0, 0.0, 0.1,
            ),
            ParameterDescriptor::float(
                "mix_amount", "Mix Amount", ParamCategory::Motion,
                0.0, 1.0, 0.5, 0.01,
            ),
        ]
    }

    /// Delta has no HSB colour parameters, so hide the Color tab.
    fn hidden_tabs(&self) -> Vec<GuiTab> {
        vec![GuiTab::Color]
    }

    fn build_uniforms(&self, s: &DeltaState, engine: &EngineState) -> DeltaUniforms {
        // Read modulated values from engine (base + LFO + audio routing)
        DeltaUniforms {
            delay_r: engine.get_param("delay_r").unwrap_or(s.delay_r),
            delay_g: engine.get_param("delay_g").unwrap_or(s.delay_g),
            delay_b: engine.get_param("delay_b").unwrap_or(s.delay_b),
            mix_amount: engine.get_param("mix_amount").unwrap_or(s.mix_amount),
        }
    }
}

/// Custom Motion tab with a polished layout.
/// The auto-generated tab is also available; this custom tab replaces it
/// for a nicer presentation.
struct MotionTab;

impl AnyGuiTab for MotionTab {
    fn name(&self) -> &str {
        "Motion"
    }

    fn replaces(&self) -> Option<GuiTab> {
        Some(GuiTab::Motion)
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<DeltaState>()
            .expect("MotionTab expects DeltaState");

        ui.text("RGB Spatial Delay");
        ui.separator();

        if ui.slider_config("Red Offset", -10.0, 10.0)
            .build(&mut state.delay_r)
        {
            engine.set_param_base("delay_r", state.delay_r);
        }
        if ui.slider_config("Green Offset", -10.0, 10.0)
            .build(&mut state.delay_g)
        {
            engine.set_param_base("delay_g", state.delay_g);
        }
        if ui.slider_config("Blue Offset", -10.0, 10.0)
            .build(&mut state.delay_b)
        {
            engine.set_param_base("delay_b", state.delay_b);
        }

        ui.separator();
        if ui.slider_config("Mix Amount", 0.0, 1.0)
            .build(&mut state.mix_amount)
        {
            engine.set_param_base("mix_amount", state.mix_amount);
        }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Delta v{}", env!("CARGO_PKG_VERSION"));

    rustjay_engine::run_with_tabs(DeltaEffect, vec![Box::new(MotionTab)])
}
