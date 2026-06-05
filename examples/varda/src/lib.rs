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

use rustjay_core::{EffectPlugin, EngineState, RenderHookCtx};
#[cfg(feature = "mixer")]
use rustjay_core::{EffectInput, EffectInstance, RenderCtx, RenderTarget};
#[cfg(feature = "mixer")]
use rustjay_mixer::{Channel, Mixer};
#[cfg(feature = "mixer")]
use rustjay_render::EffectNode;
use std::path::PathBuf;
#[cfg(feature = "mixer")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "mixer")]
use crate::graph::{Deck, DeckCompositor};
use crate::sources::{Registry, ShaderWatcher};
#[cfg(feature = "mixer")]
use crate::sources::{CameraSource, SolidColorSource};

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
pub struct VardaAppState {
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub mixer: Arc<Mutex<Mixer>>,
    pub ready: bool,
    #[serde(skip)]
    pub registry: Registry,
    #[serde(skip)]
    pub shader_watcher: Option<ShaderWatcher>,
}

impl Default for VardaAppState {
    fn default() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            ready: false,
            registry: Registry {
                shaders: Vec::new(),
                images: Vec::new(),
                videos: Vec::new(),
                builtins: Vec::new(),
            },
            shader_watcher: None,
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
        #[cfg_attr(not(feature = "mixer"), allow(unused_mut))]
        let mut s = VardaAppState::default();
        #[cfg(feature = "mixer")]
        {
            s.mixer = self.mixer.clone();
        }
        s
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn prepare(&mut self, state: &mut VardaAppState, engine: &EngineState, device: &wgpu::Device, queue: &wgpu::Queue) {
        if !state.ready {
            state.ready = true;
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let shaders_dir = manifest_dir.join("shaders");
            state.registry = Registry::scan(&shaders_dir, &manifest_dir.join("assets"));
            log::info!(
                "[Registry] scanned {} shaders, {} images, {} videos",
                state.registry.shaders.len(),
                state.registry.images.len(),
                state.registry.videos.len(),
            );
            state.shader_watcher = ShaderWatcher::new(&shaders_dir).ok();
            if state.shader_watcher.is_some() {
                log::info!("[ShaderWatcher] started");
            }
        }

        #[cfg(feature = "mixer")]
        if let Some(ref watcher) = state.shader_watcher {
            let events = watcher.poll();
            for event in events {
                for path in &event.paths {
                    log::info!("[ShaderWatcher] changed: {}", path.display());
                    if let Ok(mut mixer) = state.mixer.lock() {
                        for ch in mixer.channels.iter_mut() {
                            if let Some(compositor) = ch.effect.as_any_mut() {
                                if let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() {
                                    for deck in compositor.decks.iter_mut() {
                                        if deck.source_path.as_ref() == Some(path) {
                                            let name = path.file_stem()
                                                .and_then(|s| s.to_str())
                                                .unwrap_or("ISF Shader")
                                                .to_string();
                                            match rustjay_isf::IsfEffect::from_path(path) {
                                                Ok(isf) => {
                                                    let node = EffectNode::new(isf, &name, device, queue, engine);
                                                    deck.source = Box::new(node);
                                                    deck.source.set_param_prefix(&deck.full_prefix);
                                                    self.params_dirty = true;
                                                    log::info!("[HotReload] Reloaded {} for deck {}", path.display(), deck.uuid);
                                                }
                                                Err(e) => {
                                                    log::warn!("[HotReload] Failed to reload {}: {}", path.display(), e);
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
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

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        #[cfg(feature = "mixer")]
        {
            let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
            let dummy_engine = EngineState::new();
            let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let shaders_dir = manifest_dir.join("shaders");

            // Channel A: ColorCycle (ISF) + Solid Color (red)
            let mut comp_a = DeckCompositor::new();
            let path_a1 = shaders_dir.join("ColorCycle.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_a1) {
                let node = EffectNode::new(isf, "ColorCycle", device, queue, &dummy_engine);
                let mut deck = Deck::new("a1", "ColorCycle", Box::new(node));
                deck.source_path = Some(path_a1);
                comp_a.decks.push(deck);
            } else {
                log::warn!("Failed to load ColorCycle.fs");
            }
            let solid = SolidColorSource::new(device, wgpu::TextureFormat::Bgra8Unorm, [1.0, 0.0, 0.0, 1.0]);
            comp_a.decks.push(Deck::new("a2", "Solid Red", Box::new(solid)));
            if let Err(e) = mixer.add_channel(Channel::new("a", "Channel A", Box::new(comp_a))) {
                log::warn!("Failed to add channel A: {}", e);
            }

            // Channel B: AuroraWaves (ISF) + Camera
            let mut comp_b = DeckCompositor::new();
            let path_b1 = shaders_dir.join("AuroraWaves.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_b1) {
                let node = EffectNode::new(isf, "AuroraWaves", device, queue, &dummy_engine);
                let mut deck = Deck::new("b1", "AuroraWaves", Box::new(node));
                deck.source_path = Some(path_b1);
                comp_b.decks.push(deck);
            } else {
                log::warn!("Failed to load AuroraWaves.fs");
            }
            let camera = CameraSource::new(device, 0);
            comp_b.decks.push(Deck::new("b2", "Camera", Box::new(camera)));
            if let Err(e) = mixer.add_channel(Channel::new("b", "Channel B", Box::new(comp_b))) {
                log::warn!("Failed to add channel B: {}", e);
            }

            // Exercise a deck FX in the demo assembly (T03.1b)
            let fx_path = shaders_dir.join("ConcentricRings.fs");
            if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&fx_path) {
                let node = EffectNode::new(isf, "ConcentricRings", device, queue, &dummy_engine);
                if let Some(ch) = mixer.channels.first_mut() {
                    if let Some(compositor) = ch.effect.as_any_mut() {
                        if let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() {
                            if let Some(deck) = compositor.decks.first_mut() {
                                deck.add_effect(Box::new(node));
                                log::info!("Added deck FX ConcentricRings to deck {}", deck.uuid);
                            }
                        }
                    }
                }
            }

            // Phase 4 modulation demo: LFO + audio-band sources on crossfader
            let lfo = mixer.modulation.add_source(rustjay_core::modulation::ModulationSource::LFO {
                waveform: rustjay_core::modulation::LFOWaveform::Sine,
                frequency: 0.25,
                phase: 0.0,
                amplitude: 0.5,
                bipolar: true,
            });
            mixer.modulation.assign("crossfader", &lfo, 1.0, None);

            let audio = mixer.modulation.add_source(rustjay_core::modulation::ModulationSource::AudioBand {
                source_id: None,
                freq_low: 20.0,
                freq_high: 250.0,
                gain: 2.0,
                smoothing: 0.6,
                mode: rustjay_core::modulation::AudioReactMode::Direct,
                noise_gate: 0.1,
            });
            mixer.modulation.assign("crossfader", &audio, 1.0, None);

            drop(mixer);
            self.params_dirty = true;
        }
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
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
