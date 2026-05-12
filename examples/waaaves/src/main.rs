//! Waaaves — Multi-pass feedback pipeline.
//!
//! Three shader blocks run in sequence with cross-block feedback:
//!   1. Block A: feedback mix + warp distortion
//!   2. Block B: blur + trail decay
//!   3. Block C: HSB color grading
//!
//! Demonstrates `RenderGraph`, `PreviousFrameTexture`, and per-pass uniforms.

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

use rustjay_engine::prelude::*;

struct WaaavesEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct WaaavesUniforms {
    feedback_amount: f32,
    warp_amount: f32,
    blur_amount: f32,
    hue_shift: f32,
    saturation: f32,
    brightness: f32,
    trail_decay: f32,
    mix_original: f32,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct WaaavesState {
    feedback_amount: f32,
    warp_amount: f32,
    blur_amount: f32,
    hue_shift: f32,
    saturation: f32,
    brightness: f32,
    trail_decay: f32,
    mix_original: f32,
}

impl EffectPlugin for WaaavesEffect {
    type State = WaaavesState;
    type Uniforms = WaaavesUniforms;

    fn app_name(&self) -> &str { "waaaves" }

    fn default_state(&self) -> WaaavesState {
        WaaavesState {
            feedback_amount: 0.5,
            saturation: 1.0,
            brightness: 1.0,
            trail_decay: 0.9,
            ..Default::default()
        }
    }

    fn shader_source(&self) -> &'static str {
        // Pass 0 shader (block_a) — also returned as the default single-pass shader.
        include_str!("shaders/block_a.wgsl")
    }

    fn render_graph(&self) -> Option<RenderGraph> {
        Some(
            RenderGraph::new()
                .with_pass(Pass {
                    label: "Block A",
                    shader: include_str!("shaders/block_a.wgsl"),
                    input: PassInput::EngineInput,
                })
                .with_pass(Pass {
                    label: "Block B",
                    shader: include_str!("shaders/block_b.wgsl"),
                    input: PassInput::PreviousPass,
                })
                .with_pass(Pass {
                    label: "Block C",
                    shader: include_str!("shaders/block_c.wgsl"),
                    input: PassInput::PreviousPass,
                })
                .with_feedback(),
        )
    }

    fn build_uniforms(&self, s: &WaaavesState, _engine: &EngineState) -> WaaavesUniforms {
        WaaavesUniforms {
            feedback_amount: s.feedback_amount,
            warp_amount: s.warp_amount,
            blur_amount: s.blur_amount,
            hue_shift: s.hue_shift,
            saturation: s.saturation,
            brightness: s.brightness,
            trail_decay: s.trail_decay,
            mix_original: s.mix_original,
        }
    }
}

struct WaaavesTab;

impl AnyGuiTab for WaaavesTab {
    fn name(&self) -> &str { "Waaaves" }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<WaaavesState>()
            .expect("WaaavesTab expects WaaavesState");

        ui.text("Feedback Pipeline");
        ui.separator();

        ui.slider_config("Feedback", 0.0, 1.0)
            .build(&mut state.feedback_amount);
        ui.slider_config("Warp", 0.0, 1.0)
            .build(&mut state.warp_amount);
        ui.slider_config("Blur", 0.0, 1.0)
            .build(&mut state.blur_amount);
        ui.slider_config("Trail Decay", 0.0, 1.0)
            .build(&mut state.trail_decay);

        ui.separator();
        ui.slider_config("Hue Shift", -180.0, 180.0)
            .build(&mut state.hue_shift);
        ui.slider_config("Saturation", 0.0, 2.0)
            .build(&mut state.saturation);
        ui.slider_config("Brightness", 0.0, 2.0)
            .build(&mut state.brightness);

        ui.separator();
        ui.slider_config("Mix Original", 0.0, 1.0)
            .build(&mut state.mix_original);
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

    log::info!("Starting RustJay Waaaves v{}", env!("CARGO_PKG_VERSION"));

    rustjay_engine::run_with_tabs(WaaavesEffect, vec![Box::new(WaaavesTab)])
}
