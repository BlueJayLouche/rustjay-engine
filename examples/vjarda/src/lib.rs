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
use sources::SourceKind;
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
#[cfg(feature = "hap")]
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
    /// Runtime deck removal queue (processed in `prepare()`).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_removals: Vec<PendingRemoval>,
    /// Runtime effect addition queue (processed in `prepare()` where GPU resources are available).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_effects: Vec<PendingEffect>,
    /// Set by the UI when it structurally edits an FX chain in place (e.g. removes
    /// a slot) so `prepare()` re-registers parameters and drops orphaned descriptors.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub params_dirty_request: bool,
    /// Handle to the engine's unified modulation engine, captured on the first
    /// `prepare()`. Lets the save paths (Cmd+S, preset export) snapshot modulation
    /// into the scene even though they don't otherwise have `&EngineState`.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub engine_modulation:
        Option<std::sync::Arc<std::sync::Mutex<rustjay_core::modulation::ModulationEngine>>>,
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

/// One deck queued for removal by the UI and processed in `prepare()`.
#[derive(Debug, Clone)]
#[cfg(feature = "mixer")]
pub struct PendingRemoval {
    /// Target channel UUID.
    pub channel_uuid: String,
    /// Deck UUID to remove.
    pub deck_uuid: String,
}

/// Target location for a runtime effect addition.
#[derive(Debug, Clone)]
#[cfg(feature = "mixer")]
pub enum EffectTarget {
    /// Add to a specific deck's FX chain.
    Deck {
        channel_uuid: String,
        deck_uuid: String,
    },
    /// Add to a channel's post-compositor FX chain.
    Channel {
        channel_uuid: String,
    },
    /// Add to the master FX chain.
    Master,
}

/// One ISF shader effect queued for creation by the UI and materialized in `prepare()`.
#[derive(Debug, Clone)]
#[cfg(feature = "mixer")]
pub struct PendingEffect {
    /// Path to the `.fs` ISF shader file.
    pub path: std::path::PathBuf,
    /// Where to append the effect.
    pub target: EffectTarget,
}

impl VardaAppState {
    /// A complete scene snapshot: mixer knobs + topology + the unified modulation
    /// engine (captured via the `engine_modulation` handle, if available).
    #[cfg(feature = "mixer")]
    pub fn scene_snapshot(&self, mixer: &Mixer) -> Scene {
        let scene = Scene::from_mixer(mixer);
        match &self.engine_modulation {
            Some(handle) => {
                let m = handle.lock().unwrap_or_else(|e| e.into_inner());
                scene.with_modulation(&m)
            }
            None => scene,
        }
    }

    /// Manually save the current workspace (scene + stage).
    #[cfg(feature = "mixer")]
    pub fn save_workspace(&self) {
        if let Ok(mixer) = self.mixer.lock() {
            let scene = self.scene_snapshot(&mixer);
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
            #[cfg(feature = "mixer")]
            engine_modulation: None,
            workspace: crate::persistence::default_workspace(),
            auto_save_last: None,
            keymap: crate::keymap::Keymap::default_bindings(),
            #[cfg(feature = "projection")]
            projection_handle: None,
            #[cfg(feature = "mixer")]
            pending_decks: Vec::new(),
            #[cfg(feature = "mixer")]
            pending_removals: Vec::new(),
            #[cfg(feature = "mixer")]
            pending_effects: Vec::new(),
            #[cfg(feature = "mixer")]
            params_dirty_request: false,
            #[cfg(feature = "sysmon")]
            sys: sysinfo::System::new_all(),
            #[cfg(feature = "sysmon")]
            sysmon_frame: 0,
        }
    }
}

/// Build an `EffectInstance` + `Deck` from a library `SourceEntry`.
/// Instantiate a [`Deck`](crate::graph::Deck) from a library [`SourceEntry`].
///
/// `deck_uuid` forces the deck's stable identity (used by topology replay so
/// the rebuilt deck reproduces the exact param prefixes its saved modulation
/// targets); pass `None` to derive the default `deck_<channel>_<entry>` id.
///
/// Every source kind funnels into a single `Deck` construction so the captured
/// descriptor (`source_entry`, `source_path`, kind) is recorded uniformly — a
/// camera's device index or a stream URL is otherwise unrecoverable from
/// `source_path` alone.
#[cfg(feature = "mixer")]
fn instantiate_source(
    entry: &crate::sources::SourceEntry,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    engine: &EngineState,
    channel_id: &str,
    deck_uuid: Option<&str>,
) -> anyhow::Result<crate::graph::Deck> {
    use crate::sources::{CameraSource, ImageSource, SolidColorSource, SourceKind};
    let format = wgpu::TextureFormat::Bgra8Unorm;

    let source: Box<dyn EffectInstance> = match entry.kind {
        SourceKind::Isf => {
            let path = entry
                .path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("ISF entry missing path"))?;
            let isf = rustjay_isf::IsfEffect::from_path(path)?;
            Box::new(EffectNode::new(isf, &entry.name, device, queue, engine))
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
        SourceKind::Camera => Box::new(CameraSource::new(device, entry.device_index)),
        SourceKind::Video => {
            let path = entry
                .path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Video entry missing path"))?;
            #[cfg(all(feature = "hap", not(feature = "ffmpeg")))]
            {
                Box::new(crate::sources::HapSource::new(device, queue, path)?)
            }
            #[cfg(all(feature = "ffmpeg", not(feature = "hap")))]
            {
                Box::new(crate::sources::FfmpegSource::new(device, queue, path)?)
            }
            #[cfg(all(feature = "hap", feature = "ffmpeg"))]
            {
                if rustjay_io::detect_hap_codec(path).unwrap_or(false) {
                    Box::new(crate::sources::HapSource::new(device, queue, path)?)
                        as Box<dyn EffectInstance>
                } else {
                    Box::new(crate::sources::FfmpegSource::new(device, queue, path)?)
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
        SourceKind::Srt | SourceKind::Hls | SourceKind::Dash | SourceKind::Rtmp => {
            #[cfg(feature = "ffmpeg")]
            {
                let url = entry
                    .path
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .ok_or_else(|| anyhow::anyhow!("Stream entry missing URL"))?;
                Box::new(crate::sources::StreamSource::new(device, queue, url)?)
            }
            #[cfg(not(feature = "ffmpeg"))]
            {
                return Err(anyhow::anyhow!("Stream support requires the ffmpeg feature"));
            }
        }
        SourceKind::Ndi => {
            #[cfg(feature = "ndi")]
            {
                Box::new(crate::sources::NdiSource::new(device, entry.name.clone()))
            }
            #[cfg(not(feature = "ndi"))]
            {
                return Err(anyhow::anyhow!("NDI support requires the ndi feature"));
            }
        }
        SourceKind::Syphon => {
            #[cfg(target_os = "macos")]
            {
                let server_uuid = entry
                    .path
                    .as_ref()
                    .and_then(|p| p.to_str())
                    .unwrap_or("")
                    .to_string();
                Box::new(crate::sources::SyphonSource::new(
                    device,
                    queue,
                    entry.name.clone(),
                    server_uuid,
                ))
            }
            #[cfg(not(target_os = "macos"))]
            {
                return Err(anyhow::anyhow!("Syphon is only available on macOS"));
            }
        }
    };

    let deck_uuid = deck_uuid
        .map(String::from)
        .unwrap_or_else(|| format!("deck_{}_{}", channel_id, entry.id));
    let mut deck = crate::graph::Deck::new(deck_uuid, &entry.name, source, entry.kind);
    deck.source_path = entry.path.clone();
    deck.source_entry = Some(entry.clone());
    Ok(deck)
}

/// Build an [`EffectSlot`](rustjay_mixer::EffectSlot) from a saved [`FxDesc`],
/// reproducing the slot's stable uuid so its param prefix matches saved
/// modulation. The caller is responsible for assigning the param prefix
/// (deck chains are re-prefixed by `Deck::set_full_prefix`; channel/master
/// chains must be prefixed explicitly). Returns `None` if the shader fails to
/// load, logging the cause.
#[cfg(feature = "mixer")]
fn build_fx_slot(
    fx: &crate::scene::FxDesc,
    base: &std::path::Path,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    engine: &EngineState,
) -> Option<rustjay_mixer::EffectSlot> {
    let path = crate::scene::resolve(&fx.path, base);
    match rustjay_isf::IsfEffect::from_path(&path) {
        Ok(isf) => {
            let name = isf.shader_name.clone();
            let node = EffectNode::new(isf, &name, device, queue, engine);
            Some(rustjay_mixer::EffectSlot {
                effect: Box::new(node),
                enabled: fx.enabled,
                uuid: fx.uuid.clone(),
                source_path: Some(path),
            })
        }
        Err(e) => {
            log::warn!("[Topology] failed to load FX {}: {}", path.display(), e);
            None
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
    /// Modulation snapshot loaded from the workspace scene in `init()` (which has
    /// no `&EngineState`), applied into `engine.modulation` on the first `prepare()`.
    #[cfg(feature = "mixer")]
    pending_modulation: Option<rustjay_core::modulation::ModulationEngine>,
    /// Per-projector warp state. Each projector gets its own sync so surface-
    /// specific warp edits don't leak across outputs.
    #[cfg(feature = "projection")]
    warp_syncs: std::sync::Mutex<Vec<std::sync::Arc<std::sync::Mutex<stage::WarpSync>>>>,
    /// Canonical live dome state, shared with the app state and projector.
    #[cfg(feature = "projection")]
    dome_sync: std::sync::Arc<std::sync::Mutex<stage::DomeSync>>,
    /// Canonical live edge-blend state, shared with the app state and projector.
    #[cfg(feature = "projection")]
    edge_blend_sync: std::sync::Arc<std::sync::Mutex<stage::EdgeBlendSync>>,
    /// Per-projector source texture overrides. Shared between the stage factory
    /// (created in main.rs) and the app state (updated in prepare()).
    #[cfg(feature = "projection")]
    source_syncs: std::sync::Mutex<Vec<std::sync::Arc<std::sync::Mutex<stage::SourceSync>>>>,
    /// Per-projector output rotation. Shared between the stage factory and app state.
    #[cfg(feature = "projection")]
    rotation_syncs: std::sync::Mutex<Vec<std::sync::Arc<std::sync::Mutex<rustjay_projection::RotationSync>>>>,
}

impl VardaRootPlugin {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            params_dirty: false,
            #[cfg(feature = "mixer")]
            pending_modulation: None,
            #[cfg(feature = "projection")]
            warp_syncs: std::sync::Mutex::new(Vec::new()),
            #[cfg(feature = "projection")]
            dome_sync: std::sync::Arc::new(std::sync::Mutex::new(stage::DomeSync::default())),
            #[cfg(feature = "projection")]
            edge_blend_sync: std::sync::Arc::new(std::sync::Mutex::new(
                stage::EdgeBlendSync::default(),
            )),
            #[cfg(feature = "projection")]
            source_syncs: std::sync::Mutex::new(Vec::new()),
            #[cfg(feature = "projection")]
            rotation_syncs: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Ensure source_syncs has at least `count` entries.
    #[cfg(feature = "projection")]
    pub fn ensure_source_syncs(&self, count: usize) {
        let mut syncs = self.source_syncs.lock().unwrap();
        while syncs.len() < count {
            syncs.push(std::sync::Arc::new(std::sync::Mutex::new(
                stage::SourceSync::default(),
            )));
        }
        syncs.truncate(count);
    }

    /// Ensure rotation_syncs has at least `count` entries.
    #[cfg(feature = "projection")]
    pub fn ensure_rotation_syncs(&self, count: usize) {
        let mut syncs = self.rotation_syncs.lock().unwrap();
        while syncs.len() < count {
            syncs.push(std::sync::Arc::new(std::sync::Mutex::new(
                rustjay_projection::RotationSync::default(),
            )));
        }
        syncs.truncate(count);
    }

    /// Shared per-projector source syncs.
    #[cfg(feature = "projection")]
    pub fn source_syncs(&self) -> Vec<std::sync::Arc<std::sync::Mutex<stage::SourceSync>>> {
        self.source_syncs.lock().unwrap().clone()
    }

    /// Shared per-projector rotation syncs.
    #[cfg(feature = "projection")]
    pub fn rotation_syncs(&self) -> Vec<std::sync::Arc<std::sync::Mutex<rustjay_projection::RotationSync>>> {
        self.rotation_syncs.lock().unwrap().clone()
    }

    /// Ensure warp_syncs has at least `count` entries.
    #[cfg(feature = "projection")]
    pub fn ensure_warp_syncs(&self, count: usize) {
        let mut syncs = self.warp_syncs.lock().unwrap();
        while syncs.len() < count {
            syncs.push(std::sync::Arc::new(std::sync::Mutex::new(
                stage::WarpSync::default(),
            )));
        }
        syncs.truncate(count);
    }

    /// Shared per-projector warp syncs.
    #[cfg(feature = "projection")]
    pub fn warp_syncs(&self) -> Vec<std::sync::Arc<std::sync::Mutex<stage::WarpSync>>> {
        self.warp_syncs.lock().unwrap().clone()
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
            let mut deck = Deck::new("a1", "ColorCycle", Box::new(node), SourceKind::Isf);
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
            .push(Deck::new("a2", "Solid Red", Box::new(solid), SourceKind::SolidColor));

        #[cfg(feature = "hap")]
        {
            let assets_dir = manifest_dir.join("assets");
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    #[cfg(feature = "ffmpeg")]
                    let is_hap = rustjay_io::detect_hap_codec(&path).unwrap_or(false);
                    #[cfg(not(feature = "ffmpeg"))]
                    let is_hap = path
                        .extension()
                        .map(|e| e == "mov" || e == "hap")
                        .unwrap_or(false);
                    if is_hap {
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
                                    SourceKind::Video,
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

        if let Err(e) = mixer.add_channel(Channel::new("a", "Channel A", Box::new(comp_a))) {
            log::warn!("Failed to add channel A: {}", e);
        }

        // Channel B: AuroraWaves (ISF) + Camera
        let mut comp_b = DeckCompositor::new();
        let path_b1 = shaders_dir.join("AuroraWaves.fs");
        if let Ok(isf) = rustjay_isf::IsfEffect::from_path(&path_b1) {
            let node = EffectNode::new(isf, "AuroraWaves", device, queue, &dummy_engine);
            let mut deck = Deck::new("b1", "AuroraWaves", Box::new(node), SourceKind::Isf);
            deck.source_path = Some(path_b1);
            comp_b.decks.push(deck);
        } else {
            log::warn!("Failed to load AuroraWaves.fs");
        }
        let camera = CameraSource::new(device, 0);
        comp_b
            .decks
            .push(Deck::new("b2", "Camera", Box::new(camera), SourceKind::Camera));

        #[cfg(feature = "ffmpeg")]
        {
            let assets_dir = manifest_dir.join("assets");
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Some(ext) = path.extension() {
                        if ext == "mp4" || ext == "mkv" || ext == "avi" || ext == "webm" || ext == "mov" {
                            #[cfg(all(feature = "ffmpeg", feature = "hap"))]
                            let result = {
                                let is_hap = rustjay_io::detect_hap_codec(&path).unwrap_or(false);
                                if is_hap {
                                    HapSource::new(device, queue, &path).map(|s| Box::new(s) as Box<dyn rustjay_core::EffectInstance>)
                                } else {
                                    FfmpegSource::new(device, queue, &path).map(|s| Box::new(s) as Box<dyn rustjay_core::EffectInstance>)
                                }
                            };
                            #[cfg(all(feature = "ffmpeg", not(feature = "hap")))]
                            let result = FfmpegSource::new(device, queue, &path).map(|s| Box::new(s) as Box<dyn rustjay_core::EffectInstance>);
                            match result {
                                Ok(src) => {
                                    let name = path
                                        .file_stem()
                                        .and_then(|s| s.to_str())
                                        .unwrap_or("Video")
                                        .to_string();
                                    comp_b.decks.push(Deck::new(
                                        format!("b_vid_{}", comp_b.decks.len()),
                                        &name,
                                        src,
                                        SourceKind::Video,
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
                            if let Some(slot) = deck.chain.last_mut() {
                                slot.source_path = Some(fx_path.clone());
                            }
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
                if let Some(slot) = ch_b.chain.last_mut() {
                    slot.source_path = Some(ch_fx_path.clone());
                }
                log::info!("Added channel FX BrightnessContrast to Channel B");
            }
        }

        // NOTE: Phase 4 removed mixer-owned modulation. Demo sources that were
        // previously added to mixer.modulation are now omitted; varda will ship
        // a default preset that loads into the unified EngineState.modulation
        // instead (M6.3). DeckCompositor no longer needs set_modulation_engine().

        drop(mixer);
        self.params_dirty = true;
    }

    /// Rebuild the routing graph from a saved [`Topology`](crate::scene::Topology).
    ///
    /// Replaces [`build_default_graph`](Self::build_default_graph) when a scene
    /// carries topology. Channel/deck/slot uuids are reproduced exactly so the
    /// rebuilt param prefixes match the modulation restored by
    /// [`Scene::apply_to_mixer`](crate::scene::Scene::apply_to_mixer). Runs in
    /// `init()` with a throwaway engine, mirroring `build_default_graph`; the
    /// real engine wires params on the next `params_dirty` registration.
    fn apply_topology(
        &mut self,
        topo: &crate::scene::Topology,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        let dummy_engine = EngineState::new();
        let base = crate::scene::topology_base();

        // Idempotent: replace whatever graph is live (empty at `init`, the old
        // graph when switching presets at runtime). Dropping the old channels
        // releases their sources (cameras, decoders, GPU textures).
        mixer.channels.clear();
        mixer.master.clear();

        for ch_desc in &topo.channels {
            let mut comp = DeckCompositor::new();
            for deck_desc in &ch_desc.decks {
                // Resolve the source path back to absolute before instantiating.
                let mut entry = deck_desc.source.clone();
                if let Some(p) = entry.path.take() {
                    entry.path = Some(crate::scene::resolve(&p, &base));
                }
                match instantiate_source(
                    &entry,
                    device,
                    queue,
                    &dummy_engine,
                    &ch_desc.uuid,
                    Some(deck_desc.uuid.as_str()),
                ) {
                    Ok(mut deck) => {
                        deck.opacity = deck_desc.opacity;
                        deck.blend_mode = deck_desc.blend_mode;
                        // Deck FX: pushed with the saved uuid; the channel's
                        // set_param_prefix → Deck::set_full_prefix re-prefixes
                        // these slots once the channel is created below.
                        for fx in &deck_desc.fx {
                            if let Some(slot) =
                                build_fx_slot(fx, &base, device, queue, &dummy_engine)
                            {
                                deck.chain.push(slot);
                            }
                        }
                        comp.decks.push(deck);
                    }
                    Err(e) => {
                        log::warn!(
                            "[Topology] failed to rebuild deck '{}' on channel '{}': {}",
                            deck_desc.name,
                            ch_desc.name,
                            e
                        );
                    }
                }
            }

            let mut channel = Channel::new(ch_desc.uuid.clone(), ch_desc.name.clone(), Box::new(comp));
            // Channel post-FX: prefix explicitly (channel chains aren't
            // auto-prefixed the way deck chains are).
            for fx in &ch_desc.fx {
                if let Some(mut slot) = build_fx_slot(fx, &base, device, queue, &dummy_engine) {
                    slot.effect
                        .set_param_prefix(&format!("ch_{}_fx{}_", ch_desc.uuid, slot.uuid));
                    channel.chain.push(slot);
                }
            }
            if let Err(e) = mixer.add_channel(channel) {
                log::warn!("[Topology] failed to add channel '{}': {}", ch_desc.name, e);
            }
        }

        // Master FX: prefix explicitly (`master_fx<uuid>_`).
        for fx in &topo.master_fx {
            if let Some(mut slot) = build_fx_slot(fx, &base, device, queue, &dummy_engine) {
                slot.effect
                    .set_param_prefix(&format!("master_fx{}_", slot.uuid));
                mixer.master.push(slot);
            }
        }

        log::info!(
            "[Topology] rebuilt {} channels, {} master FX",
            topo.channels.len(),
            topo.master_fx.len()
        );
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

    /// Distinct app identity: drives the control window title, the top-bar name,
    /// and isolates this example's config/presets (`~/.config/rustjay/Varda.json`)
    /// so it doesn't collide with other examples.
    fn app_name(&self) -> &str {
        "Varda"
    }

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
            // Create local default syncs for the initial app state.
            // Do NOT touch the plugin's internal sync vectors here —
            // main.rs and prepare() own the canonical counts.
            s.stage.warp_syncs = vec![std::sync::Arc::new(std::sync::Mutex::new(
                stage::WarpSync::default(),
            ))];
            s.stage.dome_sync = Some(self.dome_sync.clone());
            s.stage.edge_blend_sync = Some(self.edge_blend_sync.clone());
            s.stage.source_syncs = vec![std::sync::Arc::new(std::sync::Mutex::new(
                stage::SourceSync::default(),
            ))];
            s.stage.rotation_syncs = vec![std::sync::Arc::new(std::sync::Mutex::new(
                rustjay_projection::RotationSync::default(),
            ))];
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

            // Capture a handle to the unified modulation engine so the save paths
            // (Cmd+S, preset export) can snapshot it without `&EngineState`.
            #[cfg(feature = "mixer")]
            {
                state.engine_modulation = Some(engine.modulation.clone());

                // Restore the modulation snapshot loaded from the workspace scene
                // in `init()` (topology already rebuilt there, so the param keys
                // its assignments target now exist).
                if let Some(modulation) = self.pending_modulation.take() {
                    let n = modulation.sources.len();
                    *engine.modulation.lock().unwrap_or_else(|e| e.into_inner()) = modulation;
                    log::info!("[Workspace] restored {n} modulation source(s)");
                }
            }

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
                            // Preserve runtime sync handles so projector stages stay connected.
                            let warp_syncs = std::mem::take(&mut state.stage.warp_syncs);
                            let source_syncs = std::mem::take(&mut state.stage.source_syncs);
                            let rotation_syncs = std::mem::take(&mut state.stage.rotation_syncs);
                            log::info!(
                                "[Prepare] before load: old warp={}, source={}, rotation={}",
                                warp_syncs.len(),
                                source_syncs.len(),
                                rotation_syncs.len()
                            );

                            state.stage = loaded_stage;
                            log::info!(
                                "[Prepare] loaded stage: {} projectors, {} surfaces",
                                state.stage.projectors.len(),
                                state.stage.surfaces.len()
                            );

                            // Restore runtime syncs.
                            state.stage.warp_syncs = warp_syncs;
                            state.stage.source_syncs = source_syncs;
                            state.stage.rotation_syncs = rotation_syncs;
                            self.ensure_warp_syncs(state.stage.projectors.len());
                            state.stage.warp_syncs = self.warp_syncs.lock().unwrap().clone();
                            self.ensure_source_syncs(state.stage.projectors.len());
                            state.stage.source_syncs = self.source_syncs.lock().unwrap().clone();
                            self.ensure_rotation_syncs(state.stage.projectors.len());
                            state.stage.rotation_syncs = self.rotation_syncs.lock().unwrap().clone();
                            log::info!(
                                "[Prepare] after sync injection: warp={}, source={}, rotation={}",
                                state.stage.warp_syncs.len(),
                                state.stage.source_syncs.len(),
                                state.stage.rotation_syncs.len()
                            );
                            for (i, sync) in state.stage.warp_syncs.iter().enumerate() {
                                log::info!("[Prepare] warp_sync[{}] ptr={:p}", i, std::sync::Arc::as_ptr(sync));
                            }
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
                            log::info!(
                                "[Prepare] fallback stage: {} projectors, {} warp_syncs",
                                state.stage.projectors.len(),
                                state.stage.warp_syncs.len()
                            );
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
                // Rebuild the routing graph when the scene carries topology, so
                // switching presets recreates the deck/FX graph (not just knobs).
                // apply_topology clears + replaces the live graph with the saved
                // UUIDs and flags params_dirty; do it before applying knobs (which
                // match channels by UUID) and modulation (keyed by param id).
                if let Some(topo) = scene
                    .topology
                    .as_ref()
                    .filter(|t| !t.channels.is_empty())
                {
                    self.apply_topology(topo, device, queue);
                }
                if let Ok(mut mixer) = state.mixer.lock() {
                    if let Some(legacy_mod) = scene.apply_to_mixer(&mut mixer) {
                        // v1 scene carried modulation in the mixer; merge into unified engine.
                        let mut mod_eng = engine.modulation.lock().unwrap_or_else(|e| e.into_inner());
                        for entry in legacy_mod.sources {
                            // S3: guard against duplicate UUIDs if the workflow ever allows queued scenes.
                            if !mod_eng.has_source(&entry.uuid) {
                                mod_eng.add_source_with_uuid(entry.uuid, entry.source);
                            }
                        }
                        for (param, assignments) in legacy_mod.assignments {
                            for a in assignments {
                                mod_eng.assign(&param, &a.source_id, a.amount, a.component);
                            }
                        }
                        log::info!("[Scene] merged legacy modulation from v1 preset");
                    }
                    log::info!("[Scene] applied pending scene snapshot");
                }
                // Restore the unified modulation snapshot (v2 scenes). For preset
                // loads the engine PresetBank has already applied an identical
                // snapshot, so this is idempotent; it also covers any non-preset
                // runtime scene load.
                if !scene.modulation.sources.is_empty() {
                    *engine.modulation.lock().unwrap_or_else(|e| e.into_inner()) =
                        scene.modulation.clone();
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
                    let scene = state.scene_snapshot(&mixer);
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
            // The UI structurally edited an FX chain in place (e.g. removed a
            // slot); re-register parameters so orphaned descriptors are dropped.
            if state.params_dirty_request {
                state.params_dirty_request = false;
                self.params_dirty = true;
            }

            // Process pending deck removals first.
            let removals: Vec<PendingRemoval> = std::mem::take(&mut state.pending_removals);
            for req in removals {
                let Ok(mut mixer) = state.mixer.lock() else {
                    continue;
                };
                let channel = mixer
                    .channels
                    .iter_mut()
                    .find(|c| c.uuid == req.channel_uuid || c.name == req.channel_uuid);
                let Some(channel) = channel else {
                    continue;
                };
                if let Some(compositor) = channel.effect.as_any_mut() {
                    if let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() {
                        if let Some(deck) = compositor.remove_deck(&req.deck_uuid) {
                            self.params_dirty = true;
                            engine.notify(
                                format!("Removed deck '{}' from {}", deck.name, channel.name),
                                rustjay_core::NotificationLevel::Info,
                                std::time::Duration::from_secs(3),
                            );
                        }
                    }
                }
            }

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
                let result =
                    instantiate_source(&req.source, device, queue, engine, &channel.uuid, None);
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

            // ── Process pending effect additions ─────────────────────────────
            // Lock the mixer once for the whole batch (only when there's work),
            // rather than re-locking per effect.
            let pending_effects: Vec<PendingEffect> = std::mem::take(&mut state.pending_effects);
            let mut mixer_guard = (!pending_effects.is_empty())
                .then(|| state.mixer.lock().unwrap_or_else(|e| e.into_inner()));
            for req in pending_effects {
                let Some(mixer) = mixer_guard.as_mut() else {
                    continue;
                };
                match req.target {
                    EffectTarget::Master => {
                        match rustjay_isf::IsfEffect::from_path(&req.path) {
                            Ok(isf) => {
                                let name = isf.shader_name.clone();
                                let node = EffectNode::new(isf, &name, device, queue, engine);
                                mixer.add_master_effect(Box::new(node));
                                if let Some(slot) = mixer.master.last_mut() {
                                    slot.source_path = Some(req.path.clone());
                                }
                                self.params_dirty = true;
                                engine.notify(
                                    format!("Added master FX '{}'", name),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                );
                            }
                            Err(e) => {
                                engine.notify(
                                    format!("Failed to load master FX: {}", e),
                                    rustjay_core::NotificationLevel::Error,
                                    std::time::Duration::from_secs(4),
                                );
                            }
                        }
                    }
                    EffectTarget::Channel { channel_uuid } => {
                        let channel = mixer
                            .channels
                            .iter_mut()
                            .find(|c| c.uuid == channel_uuid || c.name == channel_uuid);
                        let Some(channel) = channel else {
                            engine.notify(
                                format!("Channel '{}' not found", channel_uuid),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            );
                            continue;
                        };
                        match rustjay_isf::IsfEffect::from_path(&req.path) {
                            Ok(isf) => {
                                let name = isf.shader_name.clone();
                                let node = EffectNode::new(isf, &name, device, queue, engine);
                                channel.add_effect(Box::new(node));
                                if let Some(slot) = channel.chain.last_mut() {
                                    slot.source_path = Some(req.path.clone());
                                }
                                self.params_dirty = true;
                                engine.notify(
                                    format!("Added FX '{}' to channel '{}'", name, channel.name),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                );
                            }
                            Err(e) => {
                                engine.notify(
                                    format!("Failed to load channel FX: {}", e),
                                    rustjay_core::NotificationLevel::Error,
                                    std::time::Duration::from_secs(4),
                                );
                            }
                        }
                    }
                    EffectTarget::Deck { channel_uuid, deck_uuid } => {
                        let channel = mixer
                            .channels
                            .iter_mut()
                            .find(|c| c.uuid == channel_uuid || c.name == channel_uuid);
                        let Some(channel) = channel else {
                            engine.notify(
                                format!("Channel '{}' not found", channel_uuid),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            );
                            continue;
                        };
                        let Some(compositor) = channel.effect.as_any_mut() else {
                            engine.notify(
                                "Channel does not support deck FX".to_string(),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            );
                            continue;
                        };
                        let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() else {
                            engine.notify(
                                "Channel does not use DeckCompositor".to_string(),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            );
                            continue;
                        };
                        let deck = compositor
                            .decks
                            .iter_mut()
                            .find(|d| d.uuid == deck_uuid);
                        let Some(deck) = deck else {
                            engine.notify(
                                format!("Deck '{}' not found", deck_uuid),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            );
                            continue;
                        };
                        match rustjay_isf::IsfEffect::from_path(&req.path) {
                            Ok(isf) => {
                                let name = isf.shader_name.clone();
                                let node = EffectNode::new(isf, &name, device, queue, engine);
                                deck.add_effect(Box::new(node));
                                if let Some(slot) = deck.chain.last_mut() {
                                    slot.source_path = Some(req.path.clone());
                                }
                                self.params_dirty = true;
                                engine.notify(
                                    format!("Added FX '{}' to deck '{}'", name, deck.name),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                );
                            }
                            Err(e) => {
                                engine.notify(
                                    format!("Failed to load deck FX: {}", e),
                                    rustjay_core::NotificationLevel::Error,
                                    std::time::Duration::from_secs(4),
                                );
                            }
                        }
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
                                        // Must match HeadlessOutput's BGRA offscreen.
                                        wgpu::TextureFormat::Bgra8Unorm,
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

            // Sync per-output recording state (auto-starts if output_type == Recording).
            if let Some(handle) = &state.projection_handle {
                let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(sub) = any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() {
                    let fps = engine.target_fps as f32;
                    let codec = rustjay_io::RecorderCodec::H264;

                    // Level-triggered: each frame, reconcile every enabled
                    // projector's active sinks against its selected output_type
                    // (mutually exclusive). Collect labels of what is live for the
                    // top-bar services strip.
                    use crate::stage::OutputType;
                    let mut sink_labels: Vec<String> = Vec::new();
                    let mut enabled_idx = 0;
                    for (i, proj) in state.stage.projectors.iter().enumerate() {
                        if !proj.enabled {
                            continue;
                        }
                        let idx = enabled_idx;
                        enabled_idx += 1;
                        let sender_name = format!("vjarda — {}", proj.name);

                        // ── Disk recording ──────────────────────────────────
                        let want_rec = matches!(proj.output_type, OutputType::Recording);
                        if want_rec && !sub.is_projector_recording(idx) {
                            let ts = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let dir = std::path::PathBuf::from("recordings");
                            std::fs::create_dir_all(&dir).ok();
                            let path = dir.join(format!("projector_{}_{}_{}.mp4", i, proj.name, ts));
                            if let Err(e) = sub.start_projector_recording(idx, &path, fps, codec) {
                                log::error!("[Varda] Failed to start projector {i} recording: {e}");
                            }
                        } else if !want_rec && sub.is_projector_recording(idx) {
                            sub.stop_projector_recording(idx);
                        }

                        // ── NDI sender ──────────────────────────────────────
                        let want_ndi = matches!(proj.output_type, OutputType::Ndi);
                        if want_ndi && !sub.is_projector_ndi(idx) {
                            match sub.start_projector_ndi(idx, &sender_name) {
                                Ok(_) => engine.notify(
                                    format!("NDI output started: {sender_name}"),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                ),
                                Err(e) => engine.notify(
                                    format!("NDI output failed: {e}"),
                                    rustjay_core::NotificationLevel::Error,
                                    std::time::Duration::from_secs(4),
                                ),
                            }
                        } else if !want_ndi && sub.is_projector_ndi(idx) {
                            sub.stop_projector_ndi(idx);
                        }

                        // ── Syphon sender (macOS) ───────────────────────────
                        #[cfg(target_os = "macos")]
                        {
                            let want_syphon = matches!(proj.output_type, OutputType::Syphon);
                            if want_syphon && !sub.is_projector_syphon(idx) {
                                match sub.start_projector_syphon(idx, &sender_name) {
                                    Ok(_) => engine.notify(
                                        format!("Syphon output started: {sender_name}"),
                                        rustjay_core::NotificationLevel::Success,
                                        std::time::Duration::from_secs(3),
                                    ),
                                    Err(e) => engine.notify(
                                        format!("Syphon output failed: {e}"),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    ),
                                }
                            } else if !want_syphon && sub.is_projector_syphon(idx) {
                                sub.stop_projector_syphon(idx);
                            }
                        }

                        // ── Spout sender (Windows) ──────────────────────────
                        #[cfg(target_os = "windows")]
                        {
                            let want_spout = matches!(proj.output_type, OutputType::Spout);
                            if want_spout && !sub.is_projector_spout(idx) {
                                match sub.start_projector_spout(idx, &sender_name) {
                                    Ok(_) => engine.notify(
                                        format!("Spout output started: {sender_name}"),
                                        rustjay_core::NotificationLevel::Success,
                                        std::time::Duration::from_secs(3),
                                    ),
                                    Err(e) => engine.notify(
                                        format!("Spout output failed: {e}"),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    ),
                                }
                            } else if !want_spout && sub.is_projector_spout(idx) {
                                sub.stop_projector_spout(idx);
                            }
                        }

                        // ── V4L2 loopback sender (Linux) ────────────────────
                        #[cfg(target_os = "linux")]
                        {
                            let want_v4l2 = matches!(proj.output_type, OutputType::V4l2);
                            // Loopback devices must be pre-created (v4l2loopback);
                            // default to /dev/video{10+idx} per projector.
                            let dev = format!("/dev/video{}", 10 + idx);
                            if want_v4l2 && !sub.is_projector_v4l2(idx) {
                                match sub.start_projector_v4l2(idx, &dev) {
                                    Ok(_) => engine.notify(
                                        format!("V4L2 output started on {dev}"),
                                        rustjay_core::NotificationLevel::Success,
                                        std::time::Duration::from_secs(3),
                                    ),
                                    Err(e) => engine.notify(
                                        format!("V4L2 output failed: {e}"),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    ),
                                }
                            } else if !want_v4l2 && sub.is_projector_v4l2(idx) {
                                sub.stop_projector_v4l2(idx);
                            }
                        }

                        // Report what is actually live this frame. Labels are kept
                        // short (kind only) so the top-bar pills stay compact.
                        if sub.is_projector_ndi(idx) {
                            sink_labels.push("NDI".to_string());
                        }
                        if sub.is_projector_syphon(idx) {
                            sink_labels.push("SYPHON".to_string());
                        }
                        if sub.is_projector_spout(idx) {
                            sink_labels.push("SPOUT".to_string());
                        }
                        if sub.is_projector_v4l2(idx) {
                            sink_labels.push("V4L2".to_string());
                        }
                        if sub.is_projector_recording(idx) {
                            sink_labels.push("REC".to_string());
                        }
                    }

                    // Headless outputs: same level-triggered reconcile as projectors.
                    let mut enabled_idx = 0;
                    for (i, hl) in state.stage.headless_outputs.iter().enumerate() {
                        if !(hl.enabled && hl.pushed) {
                            continue;
                        }
                        let idx = enabled_idx;
                        enabled_idx += 1;
                        let sender_name = format!("vjarda — {}", hl.name);

                        let want_rec = matches!(hl.output_type, OutputType::Recording);
                        if want_rec && !sub.is_headless_recording(idx) {
                            let ts = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs();
                            let dir = std::path::PathBuf::from("recordings");
                            std::fs::create_dir_all(&dir).ok();
                            let path = dir.join(format!("headless_{}_{}_{}.mp4", i, hl.name, ts));
                            if let Err(e) = sub.start_headless_recording(idx, &path, fps, codec) {
                                log::error!("[Varda] Failed to start headless {i} recording: {e}");
                            }
                        } else if !want_rec && sub.is_headless_recording(idx) {
                            sub.stop_headless_recording(idx);
                        }

                        let want_ndi = matches!(hl.output_type, OutputType::Ndi);
                        if want_ndi && !sub.is_headless_ndi(idx) {
                            match sub.start_headless_ndi(idx, &sender_name) {
                                Ok(_) => engine.notify(
                                    format!("NDI output started: {sender_name}"),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                ),
                                Err(e) => engine.notify(
                                    format!("NDI output failed: {e}"),
                                    rustjay_core::NotificationLevel::Error,
                                    std::time::Duration::from_secs(4),
                                ),
                            }
                        } else if !want_ndi && sub.is_headless_ndi(idx) {
                            sub.stop_headless_ndi(idx);
                        }

                        #[cfg(target_os = "macos")]
                        {
                            let want_syphon = matches!(hl.output_type, OutputType::Syphon);
                            if want_syphon && !sub.is_headless_syphon(idx) {
                                match sub.start_headless_syphon(idx, &sender_name) {
                                    Ok(_) => engine.notify(
                                        format!("Syphon output started: {sender_name}"),
                                        rustjay_core::NotificationLevel::Success,
                                        std::time::Duration::from_secs(3),
                                    ),
                                    Err(e) => engine.notify(
                                        format!("Syphon output failed: {e}"),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    ),
                                }
                            } else if !want_syphon && sub.is_headless_syphon(idx) {
                                sub.stop_headless_syphon(idx);
                            }
                        }

                        #[cfg(target_os = "windows")]
                        {
                            let want_spout = matches!(hl.output_type, OutputType::Spout);
                            if want_spout && !sub.is_headless_spout(idx) {
                                if let Err(e) = sub.start_headless_spout(idx, &sender_name) {
                                    engine.notify(
                                        format!("Spout output failed: {e}"),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    );
                                }
                            } else if !want_spout && sub.is_headless_spout(idx) {
                                sub.stop_headless_spout(idx);
                            }
                        }

                        #[cfg(target_os = "linux")]
                        {
                            let want_v4l2 = matches!(hl.output_type, OutputType::V4l2);
                            let dev = format!("/dev/video{}", 20 + idx);
                            if want_v4l2 && !sub.is_headless_v4l2(idx) {
                                if let Err(e) = sub.start_headless_v4l2(idx, &dev) {
                                    engine.notify(
                                        format!("V4L2 output failed: {e}"),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    );
                                }
                            } else if !want_v4l2 && sub.is_headless_v4l2(idx) {
                                sub.stop_headless_v4l2(idx);
                            }
                        }

                        if sub.is_headless_ndi(idx) {
                            sink_labels.push("NDI".to_string());
                        }
                        if sub.is_headless_syphon(idx) {
                            sink_labels.push("SYPHON".to_string());
                        }
                        if sub.is_headless_spout(idx) {
                            sink_labels.push("SPOUT".to_string());
                        }
                        if sub.is_headless_v4l2(idx) {
                            sink_labels.push("V4L2".to_string());
                        }
                        if sub.is_headless_recording(idx) {
                            sink_labels.push("REC".to_string());
                        }
                    }

                    // Publish active output sinks (projectors + headless) for the
                    // top-bar status strip.
                    if let Ok(mut sinks) = engine.output_sinks.lock() {
                        if *sinks != sink_labels {
                            *sinks = sink_labels;
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

    // Varda's egui tabs are non-replacing (each gets its own sidebar button via
    // the engine host), so the built-in tabs — including the working LFO and MIDI
    // panels the Varda tabs only summarize — stay available. Nothing is hidden.

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn on_engine_ready(&mut self, engine: &mut EngineState) {
        // When projection is enabled we don't need the primary output window —
        // projectors are the sole visible outputs.
        #[cfg(feature = "projection")]
        {
            engine.no_primary_output = true;
        }

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

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn serialize_preset_state(&self, state: &Self::State) -> Option<String> {
        #[cfg(feature = "mixer")]
        {
            if let Ok(mixer) = self.mixer.lock() {
                let scene = state.scene_snapshot(&mixer);
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
            // FIXME: hardcodes default_workspace() because init() has no access to State.
            // Wire a workspace field onto the plugin when per-project paths are needed.
            let workspace = crate::persistence::default_workspace();
            let scene = if workspace.exists() {
                match workspace.load_scene() {
                    Ok(scene) => Some(scene),
                    Err(e) => {
                        log::warn!("[Workspace] failed to load scene: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Rebuild the saved routing graph when present; otherwise fall back
            // to the hard-coded default assembly. Topology must exist before the
            // knobs/modulation are applied so the param keys they target resolve.
            match scene
                .as_ref()
                .and_then(|s| s.topology.as_ref())
                .filter(|t| !t.channels.is_empty())
            {
                Some(topo) => self.apply_topology(topo, device, queue),
                None => self.build_default_graph(device, queue),
            }

            // Restore knob settings (crossfader, opacities, blends, modulation).
            if let Some(scene) = &scene {
                let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
                let legacy_mod = scene.apply_to_mixer(&mut mixer);
                if legacy_mod.is_some() {
                    // EngineState is not available in init(); legacy v1 modulation
                    // cannot be merged here. Re-load the preset at runtime via the
                    // web/MIDI interface to trigger the prepare() migration path.
                    log::warn!("[Workspace] v1 scene modulation skipped at init (no engine access); reload preset at runtime to migrate");
                }
                // The unified modulation snapshot can't be applied here (no engine);
                // stash it for the first prepare() to write into EngineState.modulation.
                if !scene.modulation.sources.is_empty() {
                    self.pending_modulation = Some(scene.modulation.clone());
                }
                log::info!("[Workspace] restored scene");
            }
        }
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, app_state: &mut VardaAppState) -> bool {
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

            #[cfg(not(feature = "projection"))]
            {
                let _ = app_state.ready;
            }

            #[cfg(feature = "projection")]
            {
                use crate::stage::{SourceSync, SurfaceSource};
                let stage = &mut app_state.stage;
                // Grow/shrink source_syncs and rotation_syncs to match projector count.
                while stage.source_syncs.len() < stage.projectors.len() {
                    stage.source_syncs.push(std::sync::Arc::new(
                        std::sync::Mutex::new(SourceSync::default()),
                    ));
                }
                stage.source_syncs.truncate(stage.projectors.len());
                while stage.rotation_syncs.len() < stage.projectors.len() {
                    stage.rotation_syncs.push(std::sync::Arc::new(
                        std::sync::Mutex::new(rustjay_projection::RotationSync::default()),
                    ));
                }
                stage.rotation_syncs.truncate(stage.projectors.len());

                // Update rotation syncs from projector configs.
                for (i, proj) in stage.projectors.iter().enumerate() {
                    if let Some(sync) = stage.rotation_syncs.get(i) {
                        if let Ok(mut g) = sync.lock() {
                            g.set_rotation(proj.rotation.index());
                        }
                    }
                }

                for (i, proj) in stage.projectors.iter().enumerate() {
                    if !proj.enabled {
                        continue;
                    }
                    let sync = &stage.source_syncs[i];
                    let surface = proj
                        .surface_index
                        .and_then(|idx| stage.surfaces.get(idx))
                        .or_else(|| stage.surfaces.first());

                    let source_key = surface.map(|s| s.source.label());

                    // Compute UV transform for Mapped surfaces with Master source.
                    let (uv_scale, uv_offset) = surface
                        .filter(|s| s.source == SurfaceSource::Master && s.content_mapping == crate::stage::ContentMapping::Mapped)
                        .map(|s| {
                            let bb = s.bounding_box();
                            let scale = [bb[2] - bb[0], bb[3] - bb[1]];
                            let offset = [bb[0], bb[1]];
                            (scale, offset)
                        })
                        .unwrap_or(([1.0, 1.0], [0.0, 0.0]));

                    let uv_crop = surface.map(|s| s.uv_crop_rect).unwrap_or([0.0, 0.0, 1.0, 1.0]);

                    // Current generation of the routed source texture. A channel's
                    // output ping-pongs between two physical buffers as its FX-chain
                    // parity changes, so the cached view must be rebuilt when this
                    // moves — otherwise the surface samples a stale buffer and the
                    // FX appear to toggle at random.
                    let current_gen = surface.and_then(|surf| match &surf.source {
                        SurfaceSource::Channel(uuid) => {
                            mixer.channel_texture(uuid).map(|t| t.generation)
                        }
                        _ => None,
                    });

                    let (needs_update, override_view) = if let Ok(g) = sync.lock() {
                        let source_changed = g.source_key.as_ref() != source_key.as_ref();
                        let uv_changed = g.uv_scale != uv_scale || g.uv_offset != uv_offset || g.uv_crop != uv_crop;
                        let gen_changed = g.output_generation != current_gen;
                        if !source_changed && !uv_changed && !gen_changed {
                            // Nothing changed — keep current state.
                            (false, g.override_view.clone())
                        } else {
                            // Source or UV changed — compute new view.
                            let view = match surface {
                                Some(surf) => match &surf.source {
                                    SurfaceSource::Master => None,
                                    SurfaceSource::Channel(uuid) => {
                                        mixer.channel_texture(uuid).map(|tex| {
                                            std::sync::Arc::new(tex.texture.create_view(
                                                &wgpu::TextureViewDescriptor::default(),
                                            ))
                                        })
                                    }
                                    SurfaceSource::Deck { .. } => {
                                        log::warn!(
                                            "Deck source routing not yet implemented, falling back to Master"
                                        );
                                        None
                                    }
                                    SurfaceSource::Domemaster => None,
                                },
                                None => None,
                            };
                            (true, view)
                        }
                    } else {
                        (false, None)
                    };

                    if needs_update {
                        if let Ok(mut g) = sync.lock() {
                            g.source_key = source_key;
                            g.override_view = override_view;
                            g.output_generation = current_gen;
                            g.uv_scale = uv_scale;
                            g.uv_offset = uv_offset;
                            g.uv_crop = uv_crop;
                            g.version = g.version.wrapping_add(1);
                        }
                    }
                }

                // TODO(S2): headless_outputs.surface_index is stored and UI-editable
                // but not yet wired into the render hook. Headless outputs currently
                // use a passthrough IdentityStage. Add per-headless source routing
                // when the headless stage chain is made dynamic.
            }

            true
        }
        #[cfg(not(feature = "mixer"))]
        {
            // Fallback when mixer is disabled: let the engine render the default shader pass.
            let _ = app_state.ready;
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
            SourceKind::Syphon => "syphon",
            SourceKind::Srt => "srt",
            SourceKind::Hls => "hls",
            SourceKind::Dash => "dash",
            SourceKind::Rtmp => "rtmp",
        }
        .to_string(),
        path: e.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        device_index: e.device_index,
    }
}
