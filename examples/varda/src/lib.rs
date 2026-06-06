//! Varda — assembled VJ app.
//!
//! Assembles rustjay-mixer + rustjay-isf + rustjay-api + rustjay-modulation
//! into a single engine. Two ISF shader channels are composited via the mixer
//! with crossfader, blend modes, and transitions.

#[cfg(feature = "api")]
pub mod api_state;
pub mod control;
pub mod graph;
pub mod keymap;
pub mod persistence;
pub mod scene;
pub mod sources;
pub mod stage;
#[cfg(feature = "projection")]
use stage::VardaStage;
pub mod ui;

#[cfg(feature = "mixer")]
use rustjay_core::{EffectInput, EffectInstance, RenderCtx, RenderTarget};
use rustjay_core::{EffectPlugin, EngineState, RenderHookCtx};
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
#[cfg(feature = "mixer")]
use crate::scene::Scene;
#[cfg(all(feature = "mixer", feature = "ffmpeg"))]
use crate::sources::FfmpegSource;
#[cfg(all(feature = "mixer", feature = "hap"))]
use crate::sources::HapSource;
#[cfg(feature = "mixer")]
use crate::sources::{CameraSource, SolidColorSource};
use crate::sources::{Registry, ShaderWatcher};

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
    /// Pending scene to apply on next `prepare()` (runtime preset/workspace load).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_scene: Option<Scene>,
    /// Workspace handle for save/load.
    #[serde(skip)]
    pub workspace: crate::persistence::Workspace,
    /// Wall-clock timestamp of the last auto-save. None until the first save fires.
    #[serde(skip)]
    pub auto_save_last: Option<std::time::Instant>,
    /// Keymap bindings.
    #[serde(skip)]
    pub keymap: crate::keymap::Keymap,
    /// Cached projection subsystem handle for runtime headless output management.
    #[serde(skip)]
    #[cfg(feature = "projection")]
    pub projection_handle: Option<std::sync::Arc<std::sync::Mutex<dyn std::any::Any + Send>>>,
    /// Runtime deck creation queue (processed in `prepare()` where GPU resources are available).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_decks: Vec<PendingDeck>,
    /// Sysinfo state for CPU/memory readout (sysmon feature only).
    #[serde(skip)]
    #[cfg(feature = "sysmon")]
    pub sys: sysinfo::System,
    /// Frame counter for throttling sysinfo refresh.
    #[serde(skip)]
    #[cfg(feature = "sysmon")]
    pub sysmon_frame: u64,
    // (removed: headless_pushed_count replaced by per-config pushed flag)
}

/// One deck queued for creation by the UI and materialized in `prepare()`.
#[derive(Debug, Clone)]
#[cfg(feature = "mixer")]
pub struct PendingDeck {
    /// Target channel UUID.
    pub channel_uuid: String,
    /// Source entry from the library registry.
    pub source: crate::sources::SourceEntry,
}

impl VardaAppState {
    /// Manually save the current workspace (scene + stage).
    #[cfg(feature = "mixer")]
    pub fn save_workspace(&self) {
        if let Ok(mixer) = self.mixer.lock() {
            let scene = Scene::from_mixer(&mixer);
            match self.workspace.save_scene(&scene) {
                Ok(_) => log::info!("[Workspace] scene saved"),
                Err(e) => log::warn!("[Workspace] scene save failed: {}", e),
            }
        }
        #[cfg(feature = "projection")]
        {
            match self.workspace.save_stage(&self.stage) {
                Ok(_) => log::info!("[Workspace] stage saved"),
                Err(e) => log::warn!("[Workspace] stage save failed: {}", e),
            }
        }
        match self.workspace.save_keymap(&self.keymap) {
            Ok(_) => log::info!("[Workspace] keymap saved"),
            Err(e) => log::warn!("[Workspace] keymap save failed: {}", e),
        }
    }
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
                streams: Vec::new(),
                builtins: Vec::new(),
            },
            shader_watcher: None,
            #[cfg(feature = "projection")]
            stage: VardaStage::with_default_surface(),
            #[cfg(feature = "mixer")]
            pending_scene: None,
            workspace: crate::persistence::default_workspace(),
            auto_save_last: None,
            keymap: crate::keymap::Keymap::default_bindings(),
            #[cfg(feature = "projection")]
            projection_handle: None,
            #[cfg(feature = "mixer")]
            pending_decks: Vec::new(),
            #[cfg(feature = "sysmon")]
            sys: sysinfo::System::new_all(),
            #[cfg(feature = "sysmon")]
            sysmon_frame: 0,
        }
    }
}

/// Build an `EffectInstance` + `Deck` from a library `SourceEntry`.
#[cfg(feature = "mixer")]
fn instantiate_source(
    entry: &crate::sources::SourceEntry,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    engine: &EngineState,
    channel_id: &str,
) -> anyhow::Result<crate::graph::Deck> {
    use crate::sources::{CameraSource, ImageSource, SolidColorSource, SourceKind};
    let format = wgpu::TextureFormat::Bgra8Unorm;

    let mut source: Box<dyn EffectInstance> = match entry.kind {
        SourceKind::Isf => {
            let path = entry
                .path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("ISF entry missing path"))?;
            let isf = rustjay_isf::IsfEffect::from_path(path)?;
            let node = EffectNode::new(isf, &entry.name, device, queue, engine);
            Box::new(node)
        }
        SourceKind::Image => {
            let path = entry
                .path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Image entry missing path"))?;
            Box::new(ImageSource::new(device, queue, format, path)?)
        }
        SourceKind::SolidColor => {
            Box::new(SolidColorSource::new(device, format, [1.0, 0.0, 1.0, 1.0]))
        }
        SourceKind::Camera => Box::new(CameraSource::new(device, 0)),
        SourceKind::Video => {
            let path = entry
                .path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Video entry missing path"))?;
            #[cfg(all(feature = "hap", not(feature = "ffmpeg")))]
            {
                return Ok(crate::graph::Deck::new(
                    format!("deck_{}_{}", channel_id, entry.id),
                    &entry.name,
                    Box::new(crate::sources::HapSource::new(device, queue, path)?),
                ));
            }
            #[cfg(all(feature = "ffmpeg", not(feature = "hap")))]
            {
                return Ok(crate::graph::Deck::new(
                    format!("deck_{}_{}", channel_id, entry.id),
                    &entry.name,
                    Box::new(crate::sources::FfmpegSource::new(device, queue, path)?),
                ));
            }
            #[cfg(all(feature = "hap", feature = "ffmpeg"))]
            {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                if ext == "mov" || ext == "hap" {
                    return Ok(crate::graph::Deck::new(
                        format!("deck_{}_{}", channel_id, entry.id),
                        &entry.name,
                        Box::new(crate::sources::HapSource::new(device, queue, path)?),
                    ));
                } else {
                    return Ok(crate::graph::Deck::new(
                        format!("deck_{}_{}", channel_id, entry.id),
                        &entry.name,
                        Box::new(crate::sources::FfmpegSource::new(device, queue, path)?),
                    ));
                }
            }
            #[cfg(not(any(feature = "hap", feature = "ffmpeg")))]
            {
                let _ = path;
                return Err(anyhow::anyhow!(
                    "Video support not enabled (hap or ffmpeg feature required)"
                ));
            }
        }
        #[cfg(feature = "ffmpeg")]
        SourceKind::Srt | SourceKind::Hls | SourceKind::Dash | SourceKind::Rtmp => {
            let url = entry
                .path
                .as_ref()
                .and_then(|p| p.to_str())
                .ok_or_else(|| anyhow::anyhow!("Stream entry missing URL"))?;
            return Ok(crate::graph::Deck::new(
                format!("deck_{}_{}", channel_id, entry.id),
                &entry.name,
                Box::new(crate::sources::StreamSource::new(device, queue, url)?),
            ));
        }
        #[cfg(not(feature = "ffmpeg"))]
        SourceKind::Srt | SourceKind::Hls | SourceKind::Dash | SourceKind::Rtmp => {
            return Err(anyhow::anyhow!(
                "Stream support requires the ffmpeg feature"
            ));
        }
        _ => {
            return Err(anyhow::anyhow!(
                "Source kind {:?} not yet supported for runtime creation",
                entry.kind
            ));
        }
    };

    let prefix = format!("ch_{}_deck_{}_{}_", channel_id, entry.id, entry.id);
    source.set_param_prefix(&prefix);
    let mut deck = crate::graph::Deck::new(
        format!("deck_{}_{}", channel_id, entry.id),
        &entry.name,
        source,
    );
    if entry.kind == SourceKind::Isf {
        deck.source_path = entry.path.clone();
    }
    Ok(deck)
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
    /// Canonical live dome state, shared with the app state and projector.
    #[cfg(feature = "projection")]
    dome_sync: std::sync::Arc<std::sync::Mutex<stage::DomeSync>>,
    /// Canonical live edge-blend state, shared with the app state and projector.
    #[cfg(feature = "projection")]
    edge_blend_sync: std::sync::Arc<std::sync::Mutex<stage::EdgeBlendSync>>,
}

impl VardaRootPlugin {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            params_dirty: false,
            #[cfg(feature = "projection")]
            warp_sync: std::sync::Arc::new(std::sync::Mutex::new(stage::WarpSync::default())),
            #[cfg(feature = "projection")]
            dome_sync: std::sync::Arc::new(std::sync::Mutex::new(stage::DomeSync::default())),
            #[cfg(feature = "projection")]
            edge_blend_sync: std::sync::Arc::new(std::sync::Mutex::new(
                stage::EdgeBlendSync::default(),
            )),
        }
    }

    /// Shared warp state for the projector stage.
    #[cfg(feature = "projection")]
    pub fn warp_sync(&self) -> std::sync::Arc<std::sync::Mutex<stage::WarpSync>> {
        self.warp_sync.clone()
    }

    /// Shared dome state for the projector stage.
    #[cfg(feature = "projection")]
    pub fn dome_sync(&self) -> std::sync::Arc<std::sync::Mutex<stage::DomeSync>> {
        self.dome_sync.clone()
    }

    /// Shared edge-blend state for the projector stage.
    #[cfg(feature = "projection")]
    pub fn edge_blend_sync(&self) -> std::sync::Arc<std::sync::Mutex<stage::EdgeBlendSync>> {
        self.edge_blend_sync.clone()
    }
}

impl Default for VardaRootPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "mixer")]
impl VardaRootPlugin {
    fn build_default_graph(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
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
        let solid = SolidColorSource::new(
            device,
            wgpu::TextureFormat::Bgra8Unorm,
            [1.0, 0.0, 0.0, 1.0],
        );
        comp_a
            .decks
            .push(Deck::new("a2", "Solid Red", Box::new(solid)));

        #[cfg(feature = "hap")]
        {
            let assets_dir = manifest_dir.join("assets");
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if ext == "mov" || ext == "hap" {
                            match HapSource::new(device, queue, &path) {
                                Ok(hap) => {
                                    let name = path
                                        .file_stem()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("HAP Video")
                                        .to_string();
                                    comp_a.decks.push(Deck::new(
                                        format!("a_hap_{}", comp_a.decks.len()),
                                        &name,
                                        Box::new(hap),
                                    ));
                                    log::info!("Loaded HAP source: {}", path.display());
                                }
                                Err(e) => {
                                    log::warn!("Failed to open HAP file {}: {}", path.display(), e);
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

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
        comp_b
            .decks
            .push(Deck::new("b2", "Camera", Box::new(camera)));

        #[cfg(feature = "ffmpeg")]
        {
            let assets_dir = manifest_dir.join("assets");
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if ext == "mp4" || ext == "mkv" || ext == "avi" || ext == "webm" {
                            match FfmpegSource::new(device, queue, &path) {
                                Ok(src) => {
                                    let name = path
                                        .file_stem()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("Video")
                                        .to_string();
                                    comp_b.decks.push(Deck::new(
                                        format!("b_vid_{}", comp_b.decks.len()),
                                        &name,
                                        Box::new(src),
                                    ));
                                    log::info!("Loaded video source: {}", path.display());
                                }
                                Err(e) => {
                                    log::warn!(
                                        "Failed to open video file {}: {}",
                                        path.display(),
                                        e
                                    );
                                }
                            }
                            break;
                        }
                    }
                }
            }
        }

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

        // Phase 12 demo: pre-populate sequencer with a beat-synced sequence
        mixer.sequencer.steps = vec![
            rustjay_mixer::TransitionStep::crossfade(1.0, 4.0),
            rustjay_mixer::TransitionStep::hold(4.0),
            rustjay_mixer::TransitionStep::crossfade(0.0, 4.0),
            rustjay_mixer::TransitionStep::hold(4.0),
        ];
        mixer.sequencer.looping = true;
        log::info!(
            "Sequencer pre-loaded with {} beat-synced steps",
            mixer.sequencer.steps.len()
        );

        // FX demo exercise: add a channel FX to Channel B (end-to-end)
        let ch_fx_path = shaders_dir.join("brightness_contrast.fs");
        if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&ch_fx_path) {
            let node = EffectNode::new(isf, "BrightnessContrast", device, queue, &dummy_engine);
            if let Some(ch_b) = mixer.channels.get_mut(1) {
                ch_b.add_effect(Box::new(node));
                log::info!("Added channel FX BrightnessContrast to Channel B");
            }
        }

        // Phase 4 modulation demo: LFO + audio-band sources on crossfader
        let mod_arc = mixer.modulation.clone();
        {
            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            let lfo = mod_eng.add_source(rustjay_core::modulation::ModulationSource::LFO {
                waveform: rustjay_core::modulation::LFOWaveform::Sine,
                frequency: 0.25,
                phase: 0.0,
                amplitude: 0.5,
                bipolar: true,
                tempo_sync: false,
                division: 2,
                phase_offset_degrees: 0.0,
                last_beat_phase: 0.0,
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
            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            let deck_lfo = mod_eng.add_source(rustjay_core::modulation::ModulationSource::LFO {
                waveform: rustjay_core::modulation::LFOWaveform::Triangle,
                frequency: 0.2,
                phase: 0.0,
                amplitude: 1.0,
                bipolar: true,
                tempo_sync: false,
                division: 2,
                phase_offset_degrees: 0.0,
                last_beat_phase: 0.0,
            });
            for key in &deck_keys {
                mod_eng.assign(key, &deck_lfo, 0.4, None);
            }

            // T04.3 carry-over: ADSR envelope + step sequencer demo sources
            let adsr = mod_eng.add_source(rustjay_core::modulation::ModulationSource::ADSR {
                attack: 0.5,
                decay: 0.3,
                sustain: 0.6,
                release: 1.0,
                stage: rustjay_core::modulation::ADSRStage::Idle,
                stage_time: 0.0,
                gate: true,
                current_level: 0.0,
            });
            mod_eng.assign("crossfader", &adsr, 0.3, None);

            let step_seq =
                mod_eng.add_source(rustjay_core::modulation::ModulationSource::StepSequencer {
                    steps: vec![0.0, 0.25, 0.5, 0.75, 1.0, 0.75, 0.5, 0.25],
                    rate: 4.0,
                    interpolation: rustjay_core::modulation::StepInterpolation::Linear,
                    bipolar: false,
                });
            mod_eng.assign("crossfader", &step_seq, 0.2, None);
        }

        drop(mixer);
        self.params_dirty = true;
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
            s.stage.dome_sync = Some(self.dome_sync.clone());
            s.stage.edge_blend_sync = Some(self.edge_blend_sync.clone());
        }
        s
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn prepare(
        &mut self,
        state: &mut VardaAppState,
        engine: &EngineState,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        if !state.ready {
            state.ready = true;

            // Capture projection subsystem handle for runtime headless management.
            #[cfg(feature = "projection")]
            {
                state.projection_handle = engine.projection_handle.clone();
            }

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

            // First-frame workspace load: stage, keymap (scene is loaded in `init`).
            #[cfg(feature = "projection")]
            {
                let stage_path = state.workspace.stage_path();
                if stage_path.exists() {
                    match state.workspace.load_stage() {
                        Ok(loaded_stage) => {
                            state.stage = loaded_stage;
                            // Re-inject Arc handles wiped by serde(skip) on deserialize.
                            state.stage.warp_sync = Some(self.warp_sync.clone());
                            state.stage.dome_sync = Some(self.dome_sync.clone());
                            state.stage.edge_blend_sync = Some(self.edge_blend_sync.clone());
                            state.stage.publish_warp();
                            // Dome/edge-blend runtime state lives in the Sync structs and is
                            // not serialized, so publish defaults here — ephemeral by design.
                            state.stage.publish_dome(
                                false,
                                rustjay_projection::DomemasterConfig::default(),
                                [0.0; 3],
                            );
                            state
                                .stage
                                .publish_edge_blend(rustjay_projection::EdgeBlendConfig::default());
                            log::info!(
                                "[Workspace] loaded stage with {} surfaces",
                                state.stage.surfaces.len()
                            );
                        }
                        Err(e) => {
                            log::warn!("[Workspace] failed to load stage: {}", e);
                        }
                    }
                }
            }
            let keymap_path = state.workspace.keymap_path();
            if keymap_path.exists() {
                match state.workspace.load_keymap() {
                    Ok(km) => {
                        state.keymap = km;
                        log::info!(
                            "[Workspace] loaded keymap with {} bindings",
                            state.keymap.bindings.len()
                        );
                    }
                    Err(e) => {
                        log::warn!("[Workspace] failed to load keymap: {}", e);
                    }
                }
            }
        }

        #[cfg(feature = "mixer")]
        {
            // Apply pending scene from preset load or runtime restore.
            if let Some(scene) = state.pending_scene.take() {
                if let Ok(mut mixer) = state.mixer.lock() {
                    scene.apply_to_mixer(&mut mixer);
                    log::info!("[Scene] applied pending scene snapshot");
                }
            }

            if let Some(ref watcher) = state.shader_watcher {
                let events = watcher.poll();
                for event in events {
                    for path in &event.paths {
                        log::info!("[ShaderWatcher] changed: {}", path.display());
                        if let Ok(mut mixer) = state.mixer.lock() {
                            for ch in mixer.channels.iter_mut() {
                                if let Some(compositor) = ch.effect.as_any_mut() {
                                    if let Some(compositor) =
                                        compositor.downcast_mut::<DeckCompositor>()
                                    {
                                        for deck in compositor.decks.iter_mut() {
                                            if deck.source_path.as_ref() == Some(path) {
                                                let name = path
                                                    .file_stem()
                                                    .and_then(|s| s.to_str())
                                                    .unwrap_or("ISF Shader")
                                                    .to_string();
                                                match rustjay_isf::IsfEffect::from_path(path) {
                                                    Ok(isf) => {
                                                        let node = EffectNode::new(
                                                            isf, &name, device, queue, engine,
                                                        );
                                                        deck.source = Box::new(node);
                                                        deck.source
                                                            .set_param_prefix(&deck.full_prefix);
                                                        self.params_dirty = true;
                                                        log::info!(
                                                            "[HotReload] Reloaded {} for deck {}",
                                                            path.display(),
                                                            deck.uuid
                                                        );
                                                    }
                                                    Err(e) => {
                                                        log::warn!(
                                                            "[HotReload] Failed to reload {}: {}",
                                                            path.display(),
                                                            e
                                                        );
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
                    match serde_json::to_value(&snapshot) {
                        Ok(val) => *guard = Some(val),
                        Err(e) => log::warn!("[API] snapshot serialization failed: {}", e),
                    }
                }
            }
        }

        // Auto-save workspace every 30 seconds (wall-clock, not frame-count).
        let now = std::time::Instant::now();
        let auto_save_elapsed = state
            .auto_save_last
            .map_or(f32::MAX, |t| now.duration_since(t).as_secs_f32());
        if auto_save_elapsed >= 30.0 {
            state.auto_save_last = Some(now);
            #[cfg(feature = "mixer")]
            {
                if let Ok(mixer) = state.mixer.lock() {
                    let scene = Scene::from_mixer(&mixer);
                    if let Err(e) = state.workspace.save_scene(&scene) {
                        log::warn!("[AutoSave] scene failed: {}", e);
                    }
                }
            }
            #[cfg(feature = "projection")]
            {
                if let Err(e) = state.workspace.save_stage(&state.stage) {
                    log::warn!("[AutoSave] stage failed: {}", e);
                }
            }
            if let Err(e) = state.workspace.save_keymap(&state.keymap) {
                log::warn!("[AutoSave] keymap failed: {}", e);
            }
        }

        // Refresh CPU / memory readout every 60 frames (~1 s at 60 fps).
        #[cfg(feature = "sysmon")]
        {
            state.sysmon_frame = state.sysmon_frame.wrapping_add(1);
            if state.sysmon_frame % 60 == 0 {
                state.sys.refresh_memory();
                state.sys.refresh_cpu_usage();
                let cpu_avg = if state.sys.cpus().is_empty() {
                    0.0
                } else {
                    state.sys.cpus().iter().map(|c| c.cpu_usage()).sum::<f32>()
                        / state.sys.cpus().len() as f32
                };
                let mem_total = state.sys.total_memory();
                let mem_used = state.sys.used_memory();
                if let Ok(mut perf) = engine.performance.lock() {
                    perf.cpu_percent = cpu_avg.clamp(0.0, 100.0);
                    perf.mem_used_mb = mem_used / 1_048_576;
                    perf.mem_total_mb = mem_total / 1_048_576;
                }
            }
        }

        // Materialise runtime deck-creation requests queued by the UI.
        #[cfg(feature = "mixer")]
        {
            let pending: Vec<PendingDeck> = std::mem::take(&mut state.pending_decks);
            for req in pending {
                let Ok(mut mixer) = state.mixer.lock() else {
                    continue;
                };
                let channel = mixer
                    .channels
                    .iter_mut()
                    .find(|c| c.uuid == req.channel_uuid || c.name == req.channel_uuid);
                let Some(channel) = channel else {
                    engine.notify(
                        format!("Channel '{}' not found", req.channel_uuid),
                        rustjay_core::NotificationLevel::Error,
                        std::time::Duration::from_secs(4),
                    );
                    continue;
                };
                let result = instantiate_source(&req.source, device, queue, engine, &channel.uuid);
                match result {
                    Ok(deck) => {
                        let name = deck.name.clone();
                        if let Some(compositor) = channel.effect.as_any_mut() {
                            if let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() {
                                compositor.decks.push(deck);
                                self.params_dirty = true;
                                engine.notify(
                                    format!("Added deck '{}' to {}", name, channel.name),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                );
                            } else {
                                engine.notify(
                                    "Channel does not use DeckCompositor".to_string(),
                                    rustjay_core::NotificationLevel::Error,
                                    std::time::Duration::from_secs(4),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        engine.notify(
                            format!("Failed to create deck: {}", e),
                            rustjay_core::NotificationLevel::Error,
                            std::time::Duration::from_secs(4),
                        );
                    }
                }
            }
        }

        // Sync headless outputs: add any newly-enabled configs.
        #[cfg(feature = "projection")]
        {
            let needs_push = state
                .stage
                .headless_outputs
                .iter()
                .any(|h| h.enabled && !h.pushed);
            if needs_push {
                if let Some(handle) = &state.projection_handle {
                    let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(sub) =
                        any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>()
                    {
                        for cfg in state.stage.headless_outputs.iter_mut() {
                            if cfg.enabled && !cfg.pushed {
                                sub.add_headless_output(
                                    cfg.width,
                                    cfg.height,
                                    vec![Box::new(rustjay_projection::IdentityStage::new(
                                        device,
                                        wgpu::TextureFormat::Rgba8Unorm,
                                    ))],
                                );
                                cfg.pushed = true;
                                log::info!(
                                    "[Headless] added {}x{} output '{}'",
                                    cfg.width,
                                    cfg.height,
                                    cfg.name
                                );
                            }
                        }
                    } else {
                        log::warn!("[Headless] projection_handle downcast failed — headless outputs not created");
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
            engine.param_resolver = Some(rustjay_core::ParamResolver(std::sync::Arc::new(
                move |path| router.resolve(path),
            )));
            // The app-state snapshot is published every frame in `prepare`
            // (with live values) — no one-time publish needed here.
        }
    }

    fn serialize_preset_state(&self, _state: &Self::State) -> Option<String> {
        #[cfg(feature = "mixer")]
        {
            if let Ok(mixer) = self.mixer.lock() {
                let scene = Scene::from_mixer(&mixer);
                return serde_json::to_string(&scene).ok();
            }
        }
        None
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn deserialize_preset_state(&self, data: &str, state: &mut Self::State) {
        #[cfg(feature = "mixer")]
        {
            match serde_json::from_str::<Scene>(data) {
                Ok(scene) => {
                    state.pending_scene = Some(scene);
                    log::info!("[Preset] deserialized scene snapshot");
                }
                Err(e) => {
                    log::warn!("[Preset] failed to deserialize scene: {}", e);
                }
            }
        }
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn on_preset_applied(&self, _state: &mut Self::State, _engine: &mut EngineState) {
        // Scene is applied in `prepare()` where we have device/queue access.
        // Stage is not part of presets (scene/stage separation).
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        #[cfg(feature = "mixer")]
        {
            self.build_default_graph(device, queue);

            // Try to restore saved scene knobs onto the default graph.
            // FIXME: hardcodes default_workspace() because init() has no access to State.
            // Wire a workspace field onto the plugin when per-project paths are needed.
            let workspace = crate::persistence::default_workspace();
            if workspace.exists() {
                match workspace.load_scene() {
                    Ok(scene) => {
                        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
                        scene.apply_to_mixer(&mut mixer);
                        log::info!("[Workspace] restored scene onto default graph");
                    }
                    Err(e) => {
                        log::warn!("[Workspace] failed to load scene: {}", e);
                    }
                }
            }
        }
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, _app_state: &mut VardaAppState) -> bool {
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
        }
        .to_string(),
        path: e.path.as_ref().map(|p| p.to_string_lossy().to_string()),
    }
}
