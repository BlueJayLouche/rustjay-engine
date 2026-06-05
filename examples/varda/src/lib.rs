//! Varda — assembled VJ app.
//!
//! Assembles rustjay-mixer + rustjay-isf + rustjay-api + rustjay-modulation
//! into a single engine. Two ISF shader channels are composited via the mixer
//! with crossfader, blend modes, and transitions.

pub mod control;
pub mod graph;
pub mod persistence;
pub mod scene;
pub mod sources;
pub mod stage;
pub mod ui;

use rustjay_core::{
    EffectInput, EffectInstance, EffectPlugin, EngineState, RenderCtx, RenderHookCtx, RenderTarget,
};
#[cfg(feature = "mixer")]
use rustjay_mixer::{Channel, Mixer};
use rustjay_render::EffectNode;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[cfg(feature = "mixer")]
use crate::graph::{Deck, DeckCompositor};

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
pub struct VardaAppState {
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub mixer: Arc<Mutex<Mixer>>,
    pub ready: bool,
}

impl Default for VardaAppState {
    fn default() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            ready: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Root plugin — wraps the mixer as the engine root
// ---------------------------------------------------------------------------

pub struct VardaRootPlugin {
    #[cfg(feature = "mixer")]
    mixer: Arc<Mutex<Mixer>>,
    params_dirty: bool,
}

impl VardaRootPlugin {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "mixer")]
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
        let mut s = VardaAppState::default();
        #[cfg(feature = "mixer")]
        {
            s.mixer = self.mixer.clone();
        }
        s
    }

    fn build_uniforms(&self, _state: &VardaAppState, _engine: &EngineState) -> DummyUniforms {
        DummyUniforms { _pad: [0.0; 4] }
    }

    fn parameters(&self) -> Vec<rustjay_core::ParameterDescriptor> {
        #[cfg(feature = "mixer")]
        {
            self.mixer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .parameters()
        }
        #[cfg(not(feature = "mixer"))]
        {
            vec![]
        }
    }

    fn parameters_dirty(&self) -> bool {
        self.params_dirty
    }

    fn clear_parameters_dirty(&mut self) {
        self.params_dirty = false;
    }

    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        #[cfg(feature = "mixer")]
        {
            let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
            let dummy_engine = EngineState::new();
            let shaders_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");

            // Channel A: ColorCycle + ConcentricRings
            let mut comp_a = DeckCompositor::new();
            let path_a1 = shaders_dir.join("ColorCycle.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_a1) {
                let node = EffectNode::new(isf, "ColorCycle", device, queue, &dummy_engine);
                comp_a.decks.push(Deck::new("a1", "ColorCycle", Box::new(node)));
            } else {
                log::warn!("Failed to load ColorCycle.fs");
            }
            let path_a2 = shaders_dir.join("ConcentricRings.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_a2) {
                let node = EffectNode::new(isf, "ConcentricRings", device, queue, &dummy_engine);
                comp_a.decks.push(Deck::new("a2", "ConcentricRings", Box::new(node)));
            } else {
                log::warn!("Failed to load ConcentricRings.fs");
            }
            if let Err(e) = mixer.add_channel(Channel::new("a", "Channel A", Box::new(comp_a))) {
                log::warn!("Failed to add channel A: {}", e);
            }

            // Channel B: AuroraWaves + ColorCycle
            let mut comp_b = DeckCompositor::new();
            let path_b1 = shaders_dir.join("AuroraWaves.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_b1) {
                let node = EffectNode::new(isf, "AuroraWaves", device, queue, &dummy_engine);
                comp_b.decks.push(Deck::new("b1", "AuroraWaves", Box::new(node)));
            } else {
                log::warn!("Failed to load AuroraWaves.fs");
            }
            let path_b2 = shaders_dir.join("ColorCycle.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_b2) {
                let node = EffectNode::new(isf, "ColorCycle", device, queue, &dummy_engine);
                comp_b.decks.push(Deck::new("b2", "ColorCycle", Box::new(node)));
            } else {
                log::warn!("Failed to load ColorCycle.fs");
            }
            if let Err(e) = mixer.add_channel(Channel::new("b", "Channel B", Box::new(comp_b))) {
                log::warn!("Failed to add channel B: {}", e);
            }

            drop(mixer);
            self.params_dirty = true;
        }
    }

    fn render(
        &mut self,
        ctx: &mut RenderHookCtx<'_>,
        _app_state: &mut VardaAppState,
    ) -> bool {
        #[cfg(feature = "mixer")]
        {
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
        #[cfg(not(feature = "mixer"))]
        {
            // Fallback when mixer is disabled: let the engine render the default shader pass.
            false
        }
    }
}
