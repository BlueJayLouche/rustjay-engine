//! HSB color template — reference example for rustjay-engine.
//!
//! Demonstrates single video input with HSB color manipulation,
//! audio reactivity, LFO, MIDI, OSC, and web server.

use rustjay_engine::prelude::*;

struct HsbEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct HsbUniforms {
    values: [f32; 4], // hue_shift, saturation, brightness, _pad
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct HsbState {
    hue_shift: f32,
    saturation: f32,
    brightness: f32,
    enabled: bool,
}

impl EffectPlugin for HsbEffect {
    type State = HsbState;
    type Uniforms = HsbUniforms;

    fn app_name(&self) -> &str {
        "template"
    }

    fn default_state(&self) -> HsbState {
        HsbState {
            saturation: 1.0,
            brightness: 1.0,
            enabled: true,
            ..Default::default()
        }
    }

    /// Declare HSB parameters for dynamic UI, LFO targets, and control mapping.
    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float(
                "hue_shift",
                "Hue Shift",
                ParamCategory::Color,
                -180.0,
                180.0,
                0.0,
                1.0,
            ),
            ParameterDescriptor::float(
                "saturation",
                "Saturation",
                ParamCategory::Color,
                0.0,
                2.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "brightness",
                "Brightness",
                ParamCategory::Color,
                0.0,
                2.0,
                1.0,
                0.01,
            ),
        ]
    }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/hsb.wgsl")
    }

    fn build_uniforms(&self, s: &HsbState, engine: &EngineState) -> HsbUniforms {
        if !s.enabled {
            return HsbUniforms {
                values: [0.0, 1.0, 1.0, 0.0],
            };
        }
        // Audio routing + LFO modulations are applied by the engine each frame
        // and available via get_param (base + modulation, clamped to descriptor range).
        let hue = engine.get_param("hue_shift").unwrap_or(s.hue_shift);
        let sat = engine.get_param("saturation").unwrap_or(s.saturation);
        let bright = engine.get_param("brightness").unwrap_or(s.brightness);
        HsbUniforms {
            values: [hue, sat, bright, 0.0],
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

    log::info!("Starting RustJay Template v{}", env!("CARGO_PKG_VERSION"));

    rustjay_engine::run(HsbEffect)
}
