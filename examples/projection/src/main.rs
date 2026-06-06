//! Projection — example demonstrating `rustjay-projection` multi-window output.
//!
//! Creates the main output window plus two extra projector windows:
//! - Projector 1: identity passthrough
//! - Projector 2: edge-blend + warp (demonstrates chained stages)

use rustjay_core::{EffectPlugin, EngineState, ParamCategory, ParameterDescriptor};

// ---------------------------------------------------------------------------
// Simple solid-color effect
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SolidUniforms {
    red: f32,
    green: f32,
    blue: f32,
    _pad: f32,
}

#[derive(Default)]
struct SolidEffect;

impl EffectPlugin for SolidEffect {
    type State = ();
    type Uniforms = SolidUniforms;

    fn shader_source(&self) -> &'static str {
        r#"
        struct Uniforms {
            red: f32,
            green: f32,
            blue: f32,
            _pad: f32,
        }
        @group(1) @binding(0) var<uniform> u: Uniforms;

        @vertex
        fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(position, 0.0, 1.0);
        }
        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(u.red, u.green, u.blue, 1.0);
        }
        "#
    }

    fn build_uniforms(&self, _state: &(), engine: &EngineState) -> SolidUniforms {
        SolidUniforms {
            red: engine.get_param("red").unwrap_or(0.5),
            green: engine.get_param("green").unwrap_or(0.2),
            blue: engine.get_param("blue").unwrap_or(0.8),
            _pad: 0.0,
        }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float(
                "red",
                "Red",
                ParamCategory::Custom("Solid".into()),
                0.0,
                1.0,
                0.5,
                0.01,
            ),
            ParameterDescriptor::float(
                "green",
                "Green",
                ParamCategory::Custom("Solid".into()),
                0.0,
                1.0,
                0.2,
                0.01,
            ),
            ParameterDescriptor::float(
                "blue",
                "Blue",
                ParamCategory::Custom("Solid".into()),
                0.0,
                1.0,
                0.8,
                0.01,
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// Main — multi-window projection setup
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .init();

    log::info!(
        "Starting RustJay Projection example v{}",
        env!("CARGO_PKG_VERSION")
    );

    use rustjay_engine::run_with_projection;
    use rustjay_projection::{EdgeBlendStage, IdentityStage};
    use winit::window::WindowAttributes;

    run_with_projection(SolidEffect, vec![], |sub| {
        // Projector 1 — identity passthrough at 640×480
        sub.add_projector(
            WindowAttributes::default()
                .with_title("Projector 1 — Identity")
                .with_inner_size(winit::dpi::LogicalSize::new(640u32, 480u32)),
            |device, format| vec![Box::new(IdentityStage::new(device, format))],
        );

        // Projector 2 — edge-blend at 800×600
        sub.add_projector(
            WindowAttributes::default()
                .with_title("Projector 2 — Edge Blend")
                .with_inner_size(winit::dpi::LogicalSize::new(800u32, 600u32)),
            |device, format| {
                let mut blend = EdgeBlendStage::new(device, format);
                blend.config.left.enabled = true;
                blend.config.left.width = 0.15;
                blend.config.right.enabled = true;
                blend.config.right.width = 0.15;
                vec![Box::new(blend)]
            },
        );

        log::info!("Queued {} projector window(s)", sub.pending_len());
    })
}
