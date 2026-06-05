//! Varda — assembled VJ app.
//!
//! Assembles rustjay-mixer + rustjay-isf + rustjay-api + rustjay-modulation
//! into a single engine. Two ISF shader channels are composited via the mixer
//! with crossfader, blend modes, and transitions.

#[cfg(feature = "api")]
pub mod api_state;
pub mod control;
pub mod graph;
pub mod persistence;
pub mod scene;
pub mod sources;
pub mod stage;
#[cfg(feature = "projection")]
use stage::VardaStage;
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

#[cfg(all(feature = "mixer", feature = "api"))]
use crate::api_state::{
    VardaChannel, VardaDeck, VardaEffect, VardaLibrary, VardaSourceEntry, VardaStateSnapshot,
};

#[cfg(feature = "mixer")]
use crate::control::param_router::ParamRouter;

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
    #[cfg(feature = "projection")]
    pub stage: VardaStage,
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
            #[cfg(feature = "projection")]
            stage: VardaStage::with_default_surface(),
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
    /// Canonical live warp state, shared with the app state (GUI writer) and the
    /// projector's `VardaWarpStage` (reader). See `stage::WarpSync`.
    #[cfg(feature = "projection")]
    warp_sync: std::sync::Arc<std::sync::Mutex<stage::WarpSync>>,
}

impl VardaRootPlugin {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            params_dirty: false,
            #[cfg(feature = "projection")]
            warp_sync: std::sync::Arc::new(std::sync::Mutex::new(stage::WarpSync::default())),
        }
    }

    /// Shared warp state for the projector stage (clone into the
    /// `run_with_projection_egui_tabs` setup closure).
    #[cfg(feature = "projection")]
    pub fn warp_sync(&self) -> std::sync::Arc<std::sync::Mutex<stage::WarpSync>> {
        self.warp_sync.clone()
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
        #[cfg(feature = "projection")]
        {
            s.stage.warp_sync = Some(self.warp_sync.clone());
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

        // Publish a fresh app-state snapshot (structure + live modulated values)
        // every frame into the engine's opaque `app_state` slot. The generic
        // `/api/app/state` route serves it, and the WS delta stream diffs it —
        // so runtime structure changes (add/remove/reorder/hot-reload) and live
        // param moves both surface. Only built when the `api` feature is on.
        #[cfg(all(feature = "mixer", feature = "api"))]
        {
            if let Ok(mixer) = self.mixer.lock() {
                let snapshot = build_varda_snapshot(&mixer, &state.registry, engine);
                if let Ok(mut guard) = engine.app_state.lock() {
                    *guard = Some(serde_json::to_value(&snapshot).unwrap_or_default());
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

    // Varda's egui tabs are non-replacing (each gets its own sidebar button via
    // the engine host), so the built-in tabs — including the working LFO and MIDI
    // panels the Varda tabs only summarize — stay available. Nothing is hidden.

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn on_engine_ready(&mut self, engine: &mut EngineState) {
        #[cfg(feature = "mixer")]
        {
            let mut router = ParamRouter::new();
            if let Ok(mut mixer) = self.mixer.lock() {
                for ch in mixer.channels.iter_mut() {
                    router.register_channel(&ch.uuid, &ch.name);
                    if let Some(compositor) = ch.effect.as_any_mut() {
                        if let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() {
                            for deck in &compositor.decks {
                                router.register_deck(&ch.uuid, &deck.uuid, &deck.name);
                            }
                        }
                    }
                }
                // `crossfader` and other bare ids resolve via pass-through — no
                // explicit registration needed.
                log::info!("[ParamRouter] populated with {} mappings", router.len());
            }
            engine.param_resolver = Some(rustjay_core::ParamResolver(std::sync::Arc::new(move |path| router.resolve(path))));
            // The app-state snapshot is published every frame in `prepare`
            // (with live values) — no one-time publish needed here.
        }
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
            let mod_arc = mixer.modulation.clone();
            {
                let mut mod_eng = mod_arc.lock().unwrap();
                let lfo = mod_eng.add_source(rustjay_core::modulation::ModulationSource::LFO {
                    waveform: rustjay_core::modulation::LFOWaveform::Sine,
                    frequency: 0.25,
                    phase: 0.0,
                    amplitude: 0.5,
                    bipolar: true,
                });
                mod_eng.assign("crossfader", &lfo, 1.0, None);

                let audio = mod_eng.add_source(rustjay_core::modulation::ModulationSource::AudioBand {
                    source_id: None,
                    freq_low: 20.0,
                    freq_high: 250.0,
                    gain: 2.0,
                    smoothing: 0.6,
                    mode: rustjay_core::modulation::AudioReactMode::Direct,
                    noise_gate: 0.1,
                });
                mod_eng.assign("crossfader", &audio, 1.0, None);
            }

            // Collect deck opacity keys and inject the shared modulation engine
            // into every DeckCompositor so deck-level params can be modulated.
            let mut deck_keys: Vec<String> = Vec::new();
            for ch in &mut mixer.channels {
                if let Some(compositor) = ch.effect.as_any_mut() {
                    if let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() {
                        for deck in &compositor.decks {
                            deck_keys.push(deck.opacity_key.clone());
                        }
                        compositor.set_modulation_engine(mod_arc.clone());
                    }
                }
            }

            // Demo: modulate every deck opacity with a slow triangle LFO.
            {
                let mut mod_eng = mod_arc.lock().unwrap();
                let deck_lfo = mod_eng.add_source(rustjay_core::modulation::ModulationSource::LFO {
                    waveform: rustjay_core::modulation::LFOWaveform::Triangle,
                    frequency: 0.2,
                    phase: 0.0,
                    amplitude: 1.0,
                    bipolar: true,
                });
                for key in &deck_keys {
                    mod_eng.assign(key, &deck_lfo, 0.4, None);
                }
            }

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

// ── API snapshot builders (behind `api` feature) ───────────────────────────

#[cfg(all(feature = "mixer", feature = "api"))]
fn build_varda_snapshot(
    mixer: &Mixer,
    registry: &Registry,
    engine: &EngineState,
) -> VardaStateSnapshot {
    use rustjay_mixer::{BlendMode, InputSelect};

    // Live (base + modulation) value of a param key, falling back to `base`.
    let live = |key: &str, base: f32| engine.get_param(key).unwrap_or(base);
    // Live blend-mode name for an enum param key.
    let live_blend = |key: &str, base: BlendMode| -> String {
        let bm = engine
            .get_param(key)
            .and_then(|v| BlendMode::from_index(v as u32))
            .unwrap_or(base);
        format!("{bm:?}")
    };

    let mut channels = Vec::new();
    let mut master_effects = Vec::new();

    for ch in &mixer.channels {
        let mut decks = Vec::new();
        let mut ch_effects = Vec::new();

        if let Some(compositor) = ch.effect.as_any() {
            if let Some(compositor) = compositor.downcast_ref::<DeckCompositor>() {
                for deck in &compositor.decks {
                    let mut deck_effects = Vec::new();
                    for slot in &deck.chain {
                        deck_effects.push(VardaEffect {
                            uuid: slot.uuid.clone(),
                            name: slot.effect.label().to_string(),
                            enabled: slot.enabled,
                            param_prefix: format!("{}fx{}_", deck.full_prefix, slot.uuid),
                        });
                    }
                    decks.push(VardaDeck {
                        uuid: deck.uuid.clone(),
                        name: deck.name.clone(),
                        channel_uuid: ch.uuid.clone(),
                        opacity_key: deck.opacity_key.clone(),
                        blend_key: deck.blend_key.clone(),
                        opacity: live(&deck.opacity_key, deck.opacity),
                        blend: live_blend(&deck.blend_key, deck.blend_mode),
                        effects: deck_effects,
                    });
                }
            }
        }

        for slot in &ch.chain {
            ch_effects.push(VardaEffect {
                uuid: slot.uuid.clone(),
                name: slot.effect.label().to_string(),
                enabled: slot.enabled,
                param_prefix: format!("ch_{}_fx{}_", ch.uuid, slot.uuid),
            });
        }

        channels.push(VardaChannel {
            uuid: ch.uuid.clone(),
            name: ch.name.clone(),
            opacity_key: format!("ch_{}_opacity", ch.uuid),
            blend_key: format!("ch_{}_blend", ch.uuid),
            input_select_key: format!("ch_{}_input_select", ch.uuid),
            opacity: live(&format!("ch_{}_opacity", ch.uuid), ch.opacity),
            blend: live_blend(&format!("ch_{}_blend", ch.uuid), ch.blend_mode),
            input_select: match ch.input_select {
                InputSelect::Slot1 => "Slot 1".to_string(),
                InputSelect::Slot2 => "Slot 2".to_string(),
                InputSelect::Both => "Both".to_string(),
            },
            decks,
            effects: ch_effects,
        });
    }

    for slot in &mixer.master {
        master_effects.push(VardaEffect {
            uuid: slot.uuid.clone(),
            name: slot.effect.label().to_string(),
            enabled: slot.enabled,
            param_prefix: format!("master_fx{}_", slot.uuid),
        });
    }

    VardaStateSnapshot {
        crossfader: live("crossfader", mixer.crossfader),
        channels,
        master_effects,
        library: registry_to_library(registry),
    }
}

#[cfg(all(feature = "mixer", feature = "api"))]
fn registry_to_library(registry: &Registry) -> VardaLibrary {
    VardaLibrary {
        shaders: registry.shaders.iter().map(source_entry_to_api).collect(),
        images: registry.images.iter().map(source_entry_to_api).collect(),
        videos: registry.videos.iter().map(source_entry_to_api).collect(),
        builtins: registry.builtins.iter().map(source_entry_to_api).collect(),
    }
}

#[cfg(all(feature = "mixer", feature = "api"))]
fn source_entry_to_api(e: &crate::sources::SourceEntry) -> VardaSourceEntry {
    use crate::sources::SourceKind;
    VardaSourceEntry {
        id: e.id.clone(),
        name: e.name.clone(),
        kind: match e.kind {
            SourceKind::Isf => "isf",
            SourceKind::Image => "image",
            SourceKind::Video => "video",
            SourceKind::SolidColor => "solid_color",
            SourceKind::Camera => "camera",
            SourceKind::Ndi => "ndi",
            SourceKind::Srt => "srt",
            SourceKind::Hls => "hls",
            SourceKind::Dash => "dash",
            SourceKind::Rtmp => "rtmp",
        }.to_string(),
        path: e.path.as_ref().map(|p| p.to_string_lossy().to_string()),
    }
}
