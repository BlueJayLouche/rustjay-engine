//! Mixer — 2-channel compositing mixer example.
//!
//! Demonstrates `rustjay-mixer` as the engine root: two simple effects
//! (solid color + tinted passthrough) are composited with a crossfader,
//! per-channel opacity/blend, and transition state machines.

use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, ParamCategory, ParameterDescriptor,
    RenderCtx, RenderHookCtx, RenderTarget,
};
use rustjay_mixer::{Channel, Mixer};
use rustjay_render::EffectNode;
use std::sync::{Arc, Mutex};

mod tabs;

// ---------------------------------------------------------------------------
// Channel A: solid color effect
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
// Channel B: tinted passthrough effect
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct TintUniforms {
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
    _pad: f32,
}

#[derive(Default)]
struct TintEffect;

impl EffectPlugin for TintEffect {
    type State = ();
    type Uniforms = TintUniforms;

    fn shader_source(&self) -> &'static str {
        r#"
        struct VertexOutput {
            @builtin(position) position: vec4<f32>,
            @location(0) texcoord: vec2<f32>,
        };

        @group(0) @binding(0) var input_tex: texture_2d<f32>;
        @group(0) @binding(1) var input_sampler: sampler;

        struct Uniforms {
            tint_r: f32,
            tint_g: f32,
            tint_b: f32,
            _pad: f32,
        }
        @group(1) @binding(0) var<uniform> u: Uniforms;

        @vertex
        fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
            var out: VertexOutput;
            out.position = vec4<f32>(position, 0.0, 1.0);
            out.texcoord = texcoord;
            return out;
        }
        @fragment
        fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
            let color = textureSample(input_tex, input_sampler, in.texcoord);
            return vec4<f32>(color.rgb * vec3<f32>(u.tint_r, u.tint_g, u.tint_b), color.a);
        }
        "#
    }

    fn build_uniforms(&self, _state: &(), engine: &EngineState) -> TintUniforms {
        TintUniforms {
            tint_r: engine.get_param("tint_r").unwrap_or(1.0),
            tint_g: engine.get_param("tint_g").unwrap_or(1.0),
            tint_b: engine.get_param("tint_b").unwrap_or(1.0),
            _pad: 0.0,
        }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float(
                "tint_r",
                "Tint R",
                ParamCategory::Custom("Tint".into()),
                0.0,
                1.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "tint_g",
                "Tint G",
                ParamCategory::Custom("Tint".into()),
                0.0,
                1.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "tint_b",
                "Tint B",
                ParamCategory::Custom("Tint".into()),
                0.0,
                1.0,
                1.0,
                0.01,
            ),
        ]
    }
}

// ---------------------------------------------------------------------------
// App state shared between plugin and tabs
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
pub struct MixerAppState {
    /// Shared mixer instance — populated in `MixerRootPlugin::init`.
    #[serde(skip)]
    pub mixer: Arc<Mutex<Mixer>>,
    /// Flag set after init so tabs know channels are ready.
    pub ready: bool,
}

impl Default for MixerAppState {
    fn default() -> Self {
        Self {
            mixer: Arc::new(Mutex::new(Mixer::new())),
            ready: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Root plugin — wraps the mixer as the engine root
// ---------------------------------------------------------------------------

pub struct MixerRootPlugin {
    mixer: Arc<Mutex<Mixer>>,
    params_dirty: bool,
}

impl MixerRootPlugin {
    /// Create a new mixer root plugin.
    pub fn new() -> Self {
        Self {
            mixer: Arc::new(Mutex::new(Mixer::new())),
            params_dirty: false,
        }
    }
}

impl Default for MixerRootPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DummyUniforms {
    _pad: [f32; 4],
}

impl EffectPlugin for MixerRootPlugin {
    type State = MixerAppState;
    type Uniforms = DummyUniforms;

    fn shader_source(&self) -> &'static str {
        r#"
        @vertex
        fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(position, 0.0, 1.0);
        }
        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(0.0, 0.0, 0.0, 1.0);
        }
        "#
    }

    fn build_uniforms(&self, _state: &MixerAppState, _engine: &EngineState) -> DummyUniforms {
        DummyUniforms { _pad: [0.0; 4] }
    }

    fn default_state(&self) -> MixerAppState {
        MixerAppState {
            mixer: self.mixer.clone(),
            ready: false,
        }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        self.mixer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .parameters()
    }

    fn parameters_dirty(&self) -> bool {
        self.params_dirty
    }

    fn clear_parameters_dirty(&mut self) {
        self.params_dirty = false;
    }

    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        let dummy_engine = EngineState::new();

        let solid = EffectNode::new(SolidEffect, "Solid", device, queue, &dummy_engine);
        mixer
            .add_channel(Channel::new("a", "Channel A", Box::new(solid)))
            .unwrap();

        let tint = EffectNode::new(TintEffect, "Tint", device, queue, &dummy_engine);
        mixer
            .add_channel(Channel::new("b", "Channel B", Box::new(tint)))
            .unwrap();

        drop(mixer);
        self.params_dirty = true;
    }

    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, _app_state: &mut MixerAppState) -> bool {
        let mut render_ctx = RenderCtx {
            device: ctx.device,
            queue: ctx.queue,
            encoder: ctx.encoder,
            vertex_buffer: ctx.vertex_buffer,
        };

        let size = [
            ctx.engine_state.resolution.internal_width,
            ctx.engine_state.resolution.internal_height,
        ];

        let primary = match ctx.input {
            Some(rustjay_core::EffectInput {
                view,
                sampler,
                generation,
                texture,
            }) => Some(EffectInput {
                view,
                sampler,
                generation,
                texture,
            }),
            _ => None,
        };
        let second = match (
            ctx.engine_state.second_input_view.as_ref(),
            ctx.engine_state.second_input_sampler.as_ref(),
        ) {
            (Some(view), Some(sampler)) => Some(EffectInput {
                view,
                sampler,
                generation: ctx.engine_state.second_input_generation,
                texture: None,
            }),
            _ => None,
        };
        let one;
        let two;
        let inputs: &[EffectInput] = match (primary, second) {
            (Some(p), Some(s)) => {
                two = [p, s];
                &two
            }
            (Some(p), None) => {
                one = [p];
                &one
            }
            _ => &[],
        };

        let target = RenderTarget {
            view: ctx.target_view,
            size,
        };

        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        mixer.render_to(&mut render_ctx, inputs, target, ctx.engine_state);
        true
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Mixer v{}", env!("CARGO_PKG_VERSION"));

    rustjay_engine::run_with_tabs(
        MixerRootPlugin::new(),
        vec![
            Box::new(tabs::mixer_tab::MixerTab),
            Box::new(tabs::channel_tab::ChannelTab),
            Box::new(tabs::transition_tab::TransitionTab::default()),
        ],
    )
}
