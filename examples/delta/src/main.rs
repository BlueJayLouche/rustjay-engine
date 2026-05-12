//! Delta — RGB spatial delay / motion extraction.
//!
//! Demonstrates a second effect running through the same engine
//! with a different shader, different uniforms, and a custom GUI tab.

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

    fn build_uniforms(&self, s: &DeltaState, _engine: &EngineState) -> DeltaUniforms {
        DeltaUniforms {
            delay_r: s.delay_r,
            delay_g: s.delay_g,
            delay_b: s.delay_b,
            mix_amount: s.mix_amount,
        }
    }
}

struct MotionTab;

impl AnyGuiTab for MotionTab {
    fn name(&self) -> &str {
        "Motion"
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<DeltaState>()
            .expect("MotionTab expects DeltaState");

        ui.text("RGB Spatial Delay");
        ui.separator();

        ui.slider_config("Red Offset", -10.0, 10.0)
            .build(&mut state.delay_r);
        ui.slider_config("Green Offset", -10.0, 10.0)
            .build(&mut state.delay_g);
        ui.slider_config("Blue Offset", -10.0, 10.0)
            .build(&mut state.delay_b);

        ui.separator();
        ui.slider_config("Mix Amount", 0.0, 1.0)
            .build(&mut state.mix_amount);
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
