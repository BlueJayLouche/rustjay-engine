//! Varda — assembled VJ app.
//!
//! Assembles rustjay-mixer + rustjay-isf + rustjay-api + rustjay-modulation
//! into a single engine. Two ISF shader channels are composited via the mixer
//! with crossfader, blend modes, and transitions.

use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, RenderCtx, RenderHookCtx, RenderTarget,
};
use rustjay_mixer::{Channel, Mixer};
use rustjay_render::EffectNode;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
pub struct VardaAppState {
    #[serde(skip)]
    pub mixer: Arc<Mutex<Mixer>>,
    pub ready: bool,
}

impl Default for VardaAppState {
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

pub struct VardaRootPlugin {
    mixer: Arc<Mutex<Mixer>>,
    params_dirty: bool,
}

impl VardaRootPlugin {
    pub fn new() -> Self {
        Self {
            mixer: Arc::new(Mutex::new(Mixer::new())),
            params_dirty: false,
        }
    }
}

impl Default for VardaRootPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DummyUniforms {
    _pad: [f32; 4],
}

impl EffectPlugin for VardaRootPlugin {
    type State = VardaAppState;
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

    fn default_state(&self) -> VardaAppState {
        VardaAppState {
            mixer: self.mixer.clone(),
            ready: false,
        }
    }

    fn build_uniforms(&self, _state: &VardaAppState, _engine: &EngineState) -> DummyUniforms {
        DummyUniforms { _pad: [0.0; 4] }
    }

    fn parameters(&self) -> Vec<rustjay_core::ParameterDescriptor> {
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
        let shaders_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");

        // Channel A: ColorCycle ISF
        let path_a = shaders_dir.join("ColorCycle.fs");
        match rustjay_isf::IsfEffect::from_path(&path_a) {
            Ok(isf) => {
                let node = EffectNode::new(isf, "ColorCycle", device, queue, &dummy_engine);
                if let Err(e) = mixer.add_channel(Channel::new("a", "Channel A", Box::new(node))) {
                    log::warn!("Failed to add channel A: {}", e);
                }
            }
            Err(e) => log::warn!("Failed to load ColorCycle.fs: {}", e),
        }

        // Channel B: AuroraWaves ISF
        let path_b = shaders_dir.join("AuroraWaves.fs");
        match rustjay_isf::IsfEffect::from_path(&path_b) {
            Ok(isf) => {
                let node = EffectNode::new(isf, "AuroraWaves", device, queue, &dummy_engine);
                if let Err(e) = mixer.add_channel(Channel::new("b", "Channel B", Box::new(node))) {
                    log::warn!("Failed to add channel B: {}", e);
                }
            }
            Err(e) => log::warn!("Failed to load AuroraWaves.fs: {}", e),
        }

        drop(mixer);
        self.params_dirty = true;
    }

    fn render(
        &mut self,
        ctx: &mut RenderHookCtx<'_>,
        _app_state: &mut VardaAppState,
    ) -> bool {
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
            Some(rustjay_core::EffectInput { view, sampler, generation, texture }) => Some(EffectInput {
                view,
                sampler,
                generation,
                texture,
            }),
            _ => None,
        };
        let second = match (ctx.engine_state.second_input_view.as_ref(), ctx.engine_state.second_input_sampler.as_ref()) {
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

    log::info!("Starting Varda v{}", env!("CARGO_PKG_VERSION"));

    rustjay_engine::run(VardaRootPlugin::new())
}
