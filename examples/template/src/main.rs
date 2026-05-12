//! HSB color template — reference example for rustjay-engine.
//!
//! Demonstrates single video input with HSB color manipulation,
//! audio reactivity, LFO, MIDI, OSC, and web server.

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

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

    fn app_name(&self) -> &str { "template" }

    fn default_state(&self) -> HsbState {
        HsbState { saturation: 1.0, brightness: 1.0, ..Default::default() }
    }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/hsb.wgsl")
    }

    fn build_uniforms(&self, s: &HsbState, engine: &EngineState) -> HsbUniforms {
        if !s.enabled {
            return HsbUniforms { values: [0.0, 1.0, 1.0, 0.0] };
        }

        let (mut hue, mut sat, mut bright) = if engine.audio_routing.enabled {
            engine.audio_routing.matrix.apply_to_hsb(s.hue_shift, s.saturation, s.brightness)
        } else {
            (s.hue_shift, s.saturation, s.brightness)
        };

        let (hue_mod, sat_mod, bright_mod) = engine.lfo.bank.get_hsb_modulations();
        hue = (hue + hue_mod * 90.0).clamp(-180.0, 180.0);
        sat = (sat + sat_mod).clamp(0.0, 2.0);
        bright = (bright + bright_mod).clamp(0.0, 2.0);

        HsbUniforms { values: [hue, sat, bright, 0.0] }
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
