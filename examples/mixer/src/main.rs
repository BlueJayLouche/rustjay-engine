//! Mixer — 2-channel compositing mixer example.
//!
//! Demonstrates `rustjay-mixer` as the engine root: two simple effects
//! (solid color + tinted passthrough) are composited with a crossfader,
//! per-channel opacity/blend, and transition state machines.

use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, ParameterDescriptor, ParamCategory,
    RenderCtx, RenderTarget,
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
        @vertex
        fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> @builtin(position) vec4<f32> {
            return vec4<f32>(position, 0.0, 1.0);
        }
        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(0.5, 0.2, 0.8, 1.0);
        }
        "#
    }

    fn build_uniforms(&self, _state: &(), _engine: &EngineState) -> SolidUniforms {
        SolidUniforms { red: 0.5, green: 0.2, blue: 0.8, _pad: 0.0 }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float("red",   "Red",   ParamCategory::Custom("Solid".into()), 0.0, 1.0, 0.5, 0.01),
            ParameterDescriptor::float("green", "Green", ParamCategory::Custom("Solid".into()), 0.0, 1.0, 0.2, 0.01),
            ParameterDescriptor::float("blue",  "Blue",  ParamCategory::Custom("Solid".into()), 0.0, 1.0, 0.8, 0.01),
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

    fn build_uniforms(&self, _state: &(), _engine: &EngineState) -> TintUniforms {
        TintUniforms { tint_r: 1.0, tint_g: 1.0, tint_b: 1.0, _pad: 0.0 }
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float("tint_r", "Tint R", ParamCategory::Custom("Tint".into()), 0.0, 1.0, 1.0, 0.01),
            ParameterDescriptor::float("tint_g", "Tint G", ParamCategory::Custom("Tint".into()), 0.0, 1.0, 1.0, 0.01),
            ParameterDescriptor::float("tint_b", "Tint B", ParamCategory::Custom("Tint".into()), 0.0, 1.0, 1.0, 0.01),
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
    pub fn new() -> Self {
        Self {
            mixer: Arc::new(Mutex::new(Mixer::new())),
            params_dirty: false,
        }
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
        self.mixer.lock().unwrap_or_else(|e| e.into_inner()).parameters()
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

        let solid = EffectNode::new(SolidEffect::default(), "Solid", device, queue, &dummy_engine);
        mixer.add_channel(Channel::new("a", "Channel A", Box::new(solid))).unwrap();

        let tint = EffectNode::new(TintEffect::default(), "Tint", device, queue, &dummy_engine);
        mixer.add_channel(Channel::new("b", "Channel B", Box::new(tint))).unwrap();

        drop(mixer);
        self.params_dirty = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_view: Option<&wgpu::TextureView>,
        input_sampler: Option<&wgpu::Sampler>,
        render_target_view: &wgpu::TextureView,
        _app_state: &mut MixerAppState,
        engine_state: &EngineState,
        vertex_buffer: &wgpu::Buffer,
        input_texture: Option<&wgpu::Texture>,
    ) -> bool {
        let mut ctx = RenderCtx {
            device,
            queue,
            encoder,
            vertex_buffer,
        };

        let size = [
            engine_state.resolution.internal_width,
            engine_state.resolution.internal_height,
        ];

        let primary = match (input_view, input_sampler) {
            (Some(view), Some(sampler)) => Some(EffectInput {
                view,
                sampler,
                generation: 0,
                texture: input_texture,
            }),
            _ => None,
        };
        let one;
        let inputs: &[EffectInput] = match primary {
            Some(p) => {
                one = [p];
                &one
            }
            None => &[],
        };

        let target = RenderTarget {
            view: render_target_view,
            size,
        };

        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        mixer.render_to(&mut ctx, inputs, target, engine_state);
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
