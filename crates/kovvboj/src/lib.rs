//! KOVVBOJ — assembled VJ app.
//!
//! Assembles rustjay-mixer + rustjay-isf + rustjay-api + rustjay-modulation
//! into a single engine. Two ISF shader channels are composited via the mixer
//! with crossfader, blend modes, and transitions.

#[cfg(feature = "mixer")]
pub mod thumbs;
#[cfg(feature = "api")]
pub mod api_state;
pub mod control;
pub mod keymap;
pub mod persistence;
pub mod scene;
pub mod sources;
pub mod stage;
#[cfg(feature = "projection")]
use stage::KovvbojStage;
pub mod shell;
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
    KovvbojChannel, KovvbojEffect, KovvbojLibrary, KovvbojSourceEntry, KovvbojStateSnapshot,
};

#[cfg(feature = "mixer")]
use crate::control::param_router::ParamRouter;

#[cfg(feature = "mixer")]
use crate::scene::Scene;
#[cfg(feature = "mixer")]
use crate::sources::{Registry, ShaderWatcher};

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

/// What the inspector is showing.
///
/// Nodes are addressed by UUID rather than index so a selection survives a
/// reorder, a drag between chains, or a rebuild from a saved topology — the
/// same reason parameter prefixes are UUID-keyed.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Selection {
    /// Nothing picked — the inspector shows the master summary.
    #[default]
    None,
    /// A layer row: its source, mix and chain.
    Layer {
        layer: String,
    },
    /// A layer's source node.
    Source {
        layer: String,
    },
    /// An FX slot in a layer's chain.
    LayerFx {
        layer: String,
        fx: String,
    },
    /// An FX slot in the master chain.
    MasterFx {
        fx: String,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct KovvbojAppState {
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub mixer: Arc<Mutex<Mixer>>,
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub thumbs: crate::thumbs::Thumbnails,
    pub ready: bool,
    /// What the inspector panel is showing. Transient — not persisted.
    #[serde(skip)]
    pub selection: Selection,
    #[serde(skip)]
    pub registry: Registry,
    #[serde(skip)]
    pub shader_watcher: Option<ShaderWatcher>,
    #[cfg(feature = "projection")]
    pub stage: KovvbojStage,
    /// Pending scene to apply on next `prepare()` (runtime preset/workspace load).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_scene: Option<Scene>,
    /// Topology to replay on the next `prepare()` — how undo and redo land.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_topology: Option<crate::scene::Topology>,
    /// Graph states to step back through. Structural edits only; see [`push_undo`].
    ///
    /// [`push_undo`]: KovvbojAppState::push_undo
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub undo_stack: Vec<crate::scene::Topology>,
    /// States undone and not yet redone. Cleared by any fresh edit.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub redo_stack: Vec<crate::scene::Topology>,
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
    /// Active DMX senders keyed by sampler id. Not serialized; (re)built by the
    /// reconcile loop. Using the sampler id as the key keeps the sender attached
    /// to its output even when outputs are reordered or removed.
    #[serde(skip)]
    #[cfg(feature = "projection")]
    pub lighting_senders: std::collections::HashMap<rustjay_projection::SamplerId, rustjay_lighting::DmxSender>,
    /// Latest submitted DMX frame per lighting output, mirrored for the UI activity meters.
    #[serde(skip)]
    #[cfg(feature = "projection")]
    pub lighting_last_frames: std::collections::HashMap<rustjay_projection::SamplerId, rustjay_lighting::DmxFrame>,
    /// Latest DMX patch overlap warnings, computed each frame for the UI.
    #[serde(skip)]
    #[cfg(feature = "projection")]
    pub lighting_overlap_warnings: Vec<rustjay_lighting::Overlap>,
    /// Runtime deck creation queue (processed in `prepare()` where GPU resources are available).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_layers: Vec<PendingLayer>,
    /// Runtime deck removal queue (processed in `prepare()`).
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_removals: Vec<PendingRemoval>,
    /// FX slots queued for removal; drained in `prepare()`.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_fx_removals: Vec<PendingFxRemoval>,
    /// Layer sources queued for re-pointing; drained in `prepare()`.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub pending_source_swaps: Vec<PendingSourceSwap>,
    /// Library entry each layer was built from, keyed by its channel uuid.
    ///
    /// `rustjay_mixer::Channel` has nowhere to record what a layer's source
    /// came from, and the entry is what lets a scene be rebuilt — a camera
    /// device index or a stream URL cannot be recovered from the effect alone.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub layer_sources: std::collections::HashMap<String, crate::sources::SourceEntry>,
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
    /// Snapshot of all custom param base values (id → value), refreshed in
    /// `prepare()` only when they change, so the save paths can capture them into
    /// the scene without `&EngineState`. `param_bases_cache` backs change detection.
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub param_snapshot: std::collections::HashMap<String, f32>,
    #[serde(skip)]
    #[cfg(feature = "mixer")]
    pub param_bases_cache: Vec<f32>,
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
pub struct PendingLayer {
    /// Source entry from the library registry.
    pub source: crate::sources::SourceEntry,
}

/// An FX slot queued for removal.
///
/// Deferred rather than removed in place so `prepare()` — which already
/// snapshots the graph before draining pending edits — covers it for undo.
/// Removing inside the strip's own draw is impossible to snapshot: the mixer is
/// already mutably borrowed by the channel iteration.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug)]
pub struct PendingFxRemoval {
    /// Which chain the slot lives in.
    pub chain: ChainRef,
    /// UUID of the slot to remove.
    pub slot: String,
}

/// A layer whose source should be re-pointed at a different device or server.
///
/// Swapping `Channel::effect` keeps the layer itself — its chain, opacity,
/// blend, and every MIDI/modulation binding keyed to `ch_<uuid>_`. That is why
/// re-pointing needs no per-source rebind API: the layer outlives its source.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug)]
pub struct PendingSourceSwap {
    /// Layer (channel) uuid.
    pub layer_uuid: String,
    /// The library entry to bind instead.
    pub source: crate::sources::SourceEntry,
}

/// One layer queued for removal by the UI and processed in `prepare()`.
#[derive(Debug, Clone)]
pub struct PendingRemoval {
    /// Layer (channel) UUID to remove.
    pub layer_uuid: String,
}

/// Target location for a runtime effect addition.
#[derive(Debug, Clone)]
#[cfg(feature = "mixer")]
pub enum EffectTarget {
    /// Add to a layer's FX chain.
    Layer { layer_uuid: String },
    /// Add to the master FX chain.
    Master,
}

/// One ISF shader effect queued for creation by the UI and materialized in `prepare()`.
#[derive(Debug, Clone)]
#[cfg(feature = "mixer")]
pub struct PendingEffect {
    /// Path to the `.fs` ISF shader file.
    pub path: std::path::PathBuf,
    /// Where to add the effect.
    pub target: EffectTarget,
    /// Insertion position within the target chain (`None` = append). Set by
    /// library drag-and-drop, which lands on a specific gap in a strip.
    pub index: Option<usize>,
}

/// Which FX chain in the mixer graph a slot belongs to. Deck chains are keyed
/// by channel + deck UUID, matching how [`Selection`] addresses nodes.
#[cfg(feature = "mixer")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainRef {
    /// A layer's FX chain.
    Layer { layer: String },
    /// The master chain.
    Master,
}

#[cfg(feature = "mixer")]
impl ChainRef {
    /// The equivalent [`EffectTarget`] for queueing a new effect.
    pub fn effect_target(&self) -> EffectTarget {
        match self {
            ChainRef::Layer { layer } => EffectTarget::Layer {
                layer_uuid: layer.clone(),
            },
            ChainRef::Master => EffectTarget::Master,
        }
    }

    /// The [`Selection`] that addresses `fx` (a slot UUID) in this chain.
    pub fn selection_for(&self, fx: &str) -> Selection {
        match self {
            ChainRef::Layer { layer } => Selection::LayerFx {
                layer: layer.clone(),
                fx: fx.to_string(),
            },
            ChainRef::Master => Selection::MasterFx { fx: fx.to_string() },
        }
    }
}

/// The chain a [`ChainRef`] names, plus the parameter prefix its slots sit
/// under (`ch_<uuid>_` or `master_`).
#[cfg(feature = "mixer")]
fn chain_parts<'m>(
    mixer: &'m mut Mixer,
    chain: &ChainRef,
) -> Option<(&'m mut Vec<rustjay_mixer::EffectSlot>, String)> {
    match chain {
        ChainRef::Master => Some((&mut mixer.master, "master_".to_string())),
        ChainRef::Layer { layer } => {
            let ch = mixer.channels.iter_mut().find(|c| c.uuid == *layer)?;
            let base = format!("ch_{}_", ch.uuid);
            Some((&mut ch.chain, base))
        }
    }
}

/// Move an FX slot within a chain, or from one chain to another.
///
/// Returns `false` and leaves everything untouched when the move is a no-op or
/// either end cannot be resolved — a slot must never be dropped on the floor.
///
/// A cross-chain move re-prefixes the slot and re-keys the engine's modulation
/// assignments and MIDI mappings, so a mapped effect keeps its knob.
#[cfg(feature = "mixer")]
pub fn move_effect(
    mixer: &mut Mixer,
    engine: &mut EngineState,
    from: &ChainRef,
    slot_uuid: &str,
    to: &ChainRef,
    index: usize,
) -> bool {
    let from_idx = {
        let Some((chain, _)) = chain_parts(mixer, from) else {
            return false;
        };
        match chain.iter().position(|s| s.uuid == slot_uuid) {
            Some(pos) => pos,
            None => return false,
        }
    };

    if from == to {
        // `index` is a gap position; removing the slot first shifts every later
        // gap down by one.
        let to_idx = if index > from_idx { index - 1 } else { index };
        if to_idx == from_idx {
            return false;
        }
        let Some((chain, _)) = chain_parts(mixer, from) else {
            return false;
        };
        let slot = chain.remove(from_idx);
        chain.insert(to_idx.min(chain.len()), slot);
        return true;
    }

    // Cross-chain: confirm the destination exists before detaching anything.
    let Some((_, to_base)) = chain_parts(mixer, to) else {
        return false;
    };
    let to_base = to_base.clone();
    let Some((from_chain, from_base)) = chain_parts(mixer, from) else {
        return false;
    };
    let old_prefix = format!("{from_base}fx{slot_uuid}_");
    let mut slot = from_chain.remove(from_idx);

    let new_prefix = format!("{to_base}fx{}_", slot.uuid);
    slot.effect.set_param_prefix(&new_prefix);

    let Some((to_chain, _)) = chain_parts(mixer, to) else {
        // Destination vanished between the check and here — put the slot back
        // rather than drop it.
        if let Some((src_chain, _)) = chain_parts(mixer, from) {
            src_chain.insert(from_idx.min(src_chain.len()), slot);
        }
        return false;
    };
    to_chain.insert(index.min(to_chain.len()), slot);

    // Re-registration keys base values by param id, so a prefix change would
    // otherwise reset the slot's knobs to their defaults. Queue the current
    // values under the new ids for the engine to apply right after.
    if let Ok(mut restore) = engine.param_restore.lock() {
        let descriptors = engine.param_descriptors.clone();
        for d in descriptors.iter() {
            if let Some(rest) = d.id.strip_prefix(old_prefix.as_str()) {
                let value = engine.get_param_base(&d.id).unwrap_or(d.default);
                restore.push((format!("{new_prefix}{rest}"), value));
            }
        }
    }

    rustjay_core::state::rekey_prefix(engine, &old_prefix, &new_prefix);
    true
}

impl KovvbojAppState {
    /// A complete scene snapshot: mixer knobs + topology + the unified modulation
    /// engine (captured via the `engine_modulation` handle, if available).
    #[cfg(feature = "mixer")]
    /// A scene snapshot, or `None` while the graph is not yet describable.
    ///
    /// Before the first `prepare()` the layer source map is empty, so every
    /// layer would serialise as a placeholder solid colour. Saving then
    /// clobbers a good scene with junk — which is what the startup auto-save
    /// did, since its elapsed timer starts at "forever ago".
    pub fn scene_snapshot_if_ready(&self, mixer: &Mixer) -> Option<Scene> {
        if !mixer.channels.is_empty() && self.layer_sources.is_empty() {
            log::debug!("[Workspace] scene save skipped: layer sources not resolved yet");
            return None;
        }
        Some(self.scene_snapshot(mixer))
    }

    pub fn scene_snapshot(&self, mixer: &Mixer) -> Scene {
        let scene = Scene::from_mixer(mixer, &self.layer_sources).with_params(self.param_snapshot.clone());
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
/// How many structural edits you can step back through.
    ///
    /// Deep enough to cover a run of mistakes, shallow enough that the stack is
    /// never worth thinking about — each entry is a whole graph description.
    pub const UNDO_DEPTH: usize = 32;

    /// Record the current graph so the next structural edit can be undone.
    ///
    /// Call this *before* mutating, with the mixer already locked. Param edits
    /// deliberately do not push: they are continuous and driven by MIDI, LFO and
    /// OSC, so an undo stack of them would be noise rather than history.
    #[cfg(feature = "mixer")]
    pub fn push_undo_from(&mut self, mixer: &Mixer) {
        self.undo_stack
            .push(crate::scene::Topology::from_mixer(mixer, &self.layer_sources));
        if self.undo_stack.len() > Self::UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
        // A fresh edit invalidates anything that was undone.
        self.redo_stack.clear();
    }

    /// [`push_undo_from`](Self::push_undo_from) for callers that do not already
    /// hold the mixer lock.
    #[cfg(feature = "mixer")]
    pub fn push_undo(&mut self) {
        let snapshot = {
            let Ok(mixer) = self.mixer.lock() else {
                return;
            };
            crate::scene::Topology::from_mixer(&mixer, &self.layer_sources)
        };
        self.undo_stack.push(snapshot);
        if self.undo_stack.len() > Self::UNDO_DEPTH {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// Step back one structural edit. Takes effect on the next `prepare()`.
    ///
    /// ponytail: replaying a whole topology rebuilds every source, so an undo
    /// costs a hitch and restarts video playback. Acceptable for structural
    /// edits; the upgrade path is a diff-based apply that only touches the nodes
    /// that actually changed.
    #[cfg(feature = "mixer")]
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        if let Ok(mixer) = self.mixer.lock() {
            self.redo_stack
                .push(crate::scene::Topology::from_mixer(&mixer, &self.layer_sources));
        }
        self.pending_topology = Some(previous);
        true
    }

    /// Step forward again after an undo.
    #[cfg(feature = "mixer")]
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        if let Ok(mixer) = self.mixer.lock() {
            self.undo_stack
                .push(crate::scene::Topology::from_mixer(&mixer, &self.layer_sources));
        }
        self.pending_topology = Some(next);
        true
    }

    pub fn save_workspace(&self) {
        if let Ok(mixer) = self.mixer.lock() {
            // Before the first `prepare()` the layer source map is still empty,
            // so every layer would serialise as a placeholder solid colour.
            // Saving then would clobber a good scene with junk — as it does for
            // anyone who launches and quits inside the auto-save interval.
            if let Some(scene) = self.scene_snapshot_if_ready(&mixer) {
                match self.workspace.save_scene(&scene) {
                    Ok(_) => log::info!("[Workspace] scene saved"),
                    Err(e) => log::warn!("[Workspace] scene save failed: {}", e),
                }
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

impl Default for KovvbojAppState {
    fn default() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            #[cfg(feature = "mixer")]
            thumbs: crate::thumbs::Thumbnails::default(),
            ready: false,
            selection: Selection::None,
            registry: Registry {
                shaders: Vec::new(),
                images: Vec::new(),
                videos: Vec::new(),
                streams: Vec::new(),
                builtins: Vec::new(),
            },
            shader_watcher: None,
            #[cfg(feature = "projection")]
            stage: KovvbojStage::with_default_surface(),
            #[cfg(feature = "mixer")]
            pending_scene: None,
            #[cfg(feature = "mixer")]
            pending_topology: None,
            #[cfg(feature = "mixer")]
            undo_stack: Vec::new(),
            #[cfg(feature = "mixer")]
            redo_stack: Vec::new(),
            #[cfg(feature = "mixer")]
            engine_modulation: None,
            #[cfg(feature = "mixer")]
            param_snapshot: std::collections::HashMap::new(),
            #[cfg(feature = "mixer")]
            param_bases_cache: Vec::new(),
            workspace: crate::persistence::default_workspace(),
            auto_save_last: None,
            keymap: crate::keymap::Keymap::default_bindings(),
            #[cfg(feature = "projection")]
            projection_handle: None,
            #[cfg(feature = "projection")]
            lighting_senders: std::collections::HashMap::new(),
            #[cfg(feature = "projection")]
            lighting_last_frames: std::collections::HashMap::new(),
            #[cfg(feature = "projection")]
            lighting_overlap_warnings: Vec::new(),
            #[cfg(feature = "mixer")]
            pending_layers: Vec::new(),
            #[cfg(feature = "mixer")]
            pending_removals: Vec::new(),
            #[cfg(feature = "mixer")]
            pending_fx_removals: Vec::new(),
            #[cfg(feature = "mixer")]
            pending_source_swaps: Vec::new(),
            #[cfg(feature = "mixer")]
            layer_sources: std::collections::HashMap::new(),
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

/// Spawn a DMX transmitter for a lighting output's protocol + transport config.
#[cfg(feature = "projection")]
fn build_dmx_sender(
    output_type: &crate::stage::OutputType,
    transport: &crate::stage::LightingTransport,
) -> std::io::Result<rustjay_lighting::DmxSender> {
    use crate::stage::OutputType;
    use rustjay_lighting::{ArtNetTransport, Dest, DmxTransport, SacnTransport};

    let default_dest = |t: &OutputType| match t {
        OutputType::ArtNet => Dest::Broadcast,
        _ => Dest::Multicast,
    };
    let dest = if transport.dest_ip.trim().is_empty() {
        default_dest(output_type)
    } else {
        match transport.dest_ip.trim().parse::<std::net::Ipv4Addr>() {
            Ok(ip) => Dest::Unicast(ip),
            Err(_) => default_dest(output_type),
        }
    };

    let tx: Box<dyn DmxTransport> = match output_type {
        OutputType::ArtNet => Box::new(ArtNetTransport::new(dest)?),
        _ => Box::new(SacnTransport::new(dest, transport.priority, "KOVVBOJ")?),
    };
    Ok(rustjay_lighting::DmxSender::spawn(tx, transport.fps))
}

/// Build the atlas layout that packs all of an output's segments.
#[cfg(feature = "projection")]
/// Resolve the normalized region a segment samples: the referenced surface's
/// `uv_crop_rect` when `source_surface` is set (and found), otherwise the
/// segment's own `region`.
#[cfg(feature = "projection")]
fn segment_region(
    seg: &crate::stage::LightingSegment,
    surfaces: &[crate::stage::KovvbojSurface],
) -> [f32; 4] {
    match &seg.source_surface {
        Some(uuid) => surfaces
            .iter()
            .find(|s| &s.uuid == uuid)
            .map(|s| s.uv_crop_rect)
            .unwrap_or(seg.region),
        None => seg.region,
    }
}

/// Resolve a segment's source texture override. When the segment references a
/// surface whose source is a mixer channel, returns that channel's texture view
/// (via `resolve_channel`) so the segment samples the channel directly instead
/// of the master composite. Master/Domemaster/Deck sources return `None`
/// (sample master).
#[cfg(feature = "projection")]
fn resolve_segment_source(
    seg: &crate::stage::LightingSegment,
    surfaces: &[crate::stage::KovvbojSurface],
    resolve_channel: impl Fn(&str) -> Option<std::sync::Arc<wgpu::TextureView>>,
) -> Option<std::sync::Arc<wgpu::TextureView>> {
    let uuid = seg.source_surface.as_ref()?;
    let surf = surfaces.iter().find(|s| &s.uuid == uuid)?;
    match &surf.source {
        crate::stage::SurfaceSource::Channel(ch) => resolve_channel(ch),
        // Deck routing is not yet implemented (mirrors projector behaviour);
        // Master/Domemaster sample the master composite at the surface's crop.
        _ => None,
    }
}

#[cfg(feature = "projection")]
fn output_atlas_layout(
    output: &crate::stage::LightingOutput,
    surfaces: &[crate::stage::KovvbojSurface],
) -> rustjay_projection::AtlasLayout {
    let segs: Vec<_> = output
        .segments
        .iter()
        .map(|s| {
            (
                [s.grid[0].max(1) as u32, s.grid[1].max(1) as u32],
                segment_region(s, surfaces),
            )
        })
        .collect();
    rustjay_projection::AtlasLayout::from_segments(segs)
}

/// Map a BGRA8 atlas readback into a [`rustjay_lighting::DmxFrame`] for a
/// lighting output. M3: multi-segment, profile-driven, scan-order aware.
#[cfg(feature = "projection")]
fn build_dmx_frame(
    output: &crate::stage::LightingOutput,
    profiles: &[crate::stage::FixtureProfile],
    bgra: &[u8],
    layout: &rustjay_projection::AtlasLayout,
) -> rustjay_lighting::DmxFrame {
    let mut frame = rustjay_lighting::DmxFrame::new();
    for (tile, seg) in layout.tiles.iter().zip(output.segments.iter()) {
        if !seg.enabled {
            continue;
        }
        let profile = profiles
            .iter()
            .find(|p| p.id == seg.profile)
            .unwrap_or_else(|| {
                log::warn!(
                    "[Lighting] profile '{}' not found; falling back to RGB",
                    seg.profile
                );
                profiles
                    .first()
                    .expect("at least one fixture profile must exist")
            });

        let pixels = rustjay_lighting::demux_tile(
            bgra,
            layout.size[0],
            [tile.offset[0], tile.offset[1]],
            [tile.size[0], tile.size[1]],
            seg.scan,
        );

        let footprint = profile.footprint();
        let mut fixtures = Vec::with_capacity(pixels.len() * footprint);
        for pixel in pixels {
            fixtures.extend_from_slice(&rustjay_lighting::color_pipeline(
                pixel,
                output.gamma,
                &seg.color,
                profile,
            ));
        }

        rustjay_lighting::pack_fixtures(
            &mut frame,
            footprint,
            &fixtures,
            seg.start_universe,
            seg.start_channel,
        );
    }
    frame
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
) -> anyhow::Result<Box<dyn EffectInstance>> {
    use crate::sources::{CameraSource, ImageSource, SolidColorSource, SourceKind};
    let format = rustjay_core::working_format();

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
        SourceKind::Srt
        | SourceKind::Hls
        | SourceKind::Dash
        | SourceKind::Rtmp
        | SourceKind::Http
        | SourceKind::Rtsp => {
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
        SourceKind::Spout => {
            #[cfg(target_os = "windows")]
            {
                Box::new(crate::sources::SpoutSource::new(device, entry.name.clone())?)
            }
            #[cfg(not(target_os = "windows"))]
            {
                return Err(anyhow::anyhow!("Spout is only available on Windows"));
            }
        }
    };

    Ok(source)
}

/// Move the just-appended slot (last) to `index` when a drop specified an
/// insertion position; returns the slot's final index.
#[cfg(feature = "mixer")]
fn position_new_slot(chain: &mut Vec<rustjay_mixer::EffectSlot>, index: Option<usize>) -> usize {
    match index {
        Some(i) if i + 1 < chain.len() => {
            let slot = chain.pop().expect("chain just grew");
            let pos = i.min(chain.len());
            chain.insert(pos, slot);
            pos
        }
        _ => chain.len() - 1,
    }
}

/// Rebuild every slot in `chain` whose source is `path`. Returns whether any
/// were replaced, so the caller can flag params dirty once.
///
/// A free function rather than a closure: the master chain and the channels
/// cannot both be borrowed from the mixer at the same time.
#[cfg(feature = "mixer")]
#[allow(clippy::too_many_arguments)]
fn reload_matching_slots(
    chain: &mut [rustjay_mixer::EffectSlot],
    base: &str,
    path: &std::path::Path,
    name: &str,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    engine: &EngineState,
) -> bool {
    let mut any = false;
    for slot in chain.iter_mut() {
        if slot.source_path.as_deref() != Some(path) {
            continue;
        }
        match rustjay_isf::IsfEffect::from_path(path) {
            Ok(isf) => {
                let node = EffectNode::new(isf, name, device, queue, engine);
                slot.effect = Box::new(node);
                slot.effect
                    .set_param_prefix(&format!("{base}fx{}_", slot.uuid));
                any = true;
                log::info!("[HotReload] reloaded FX {} in {base}", slot.uuid);
            }
            Err(e) => log::warn!("[HotReload] failed to reload {}: {e}", path.display()),
        }
    }
    any
}

/// Whether a saved topology can be replayed by this build.
///
/// A version-0 file nests decks inside channels. Flattening it is not possible
/// honestly: a channel's post-FX ran once over the composite of its decks, and
/// once those decks are sibling layers there is nowhere for that effect to go
/// that renders the same picture.
#[cfg(feature = "mixer")]
fn usable_topology(topo: &crate::scene::Topology) -> bool {
    topo.version >= crate::scene::TOPOLOGY_VERSION && !topo.layers.is_empty()
}

/// Tell the user why their saved graph did not load, and leave the file alone.
#[cfg(feature = "mixer")]
fn warn_stale_topology(topo: &crate::scene::Topology, engine: &EngineState) {
    if topo.version >= crate::scene::TOPOLOGY_VERSION {
        return;
    }
    log::warn!(
        "[Topology] scene is version {} (this build reads {}); starting with an empty stack",
        topo.version,
        crate::scene::TOPOLOGY_VERSION
    );
    engine.notify(
        "This scene predates layers and was not loaded. Your file is untouched."
            .to_string(),
        rustjay_core::NotificationLevel::Warning,
        std::time::Duration::from_secs(8),
    );
}

/// Build an [`EffectSlot`](rustjay_mixer::EffectSlot) from a saved [`FxDesc`],
/// reproducing the slot's stable uuid so its param prefix matches saved
/// modulation. The caller is responsible for assigning the param prefix. Returns `None` if the shader fails to
/// load, logging the cause.
///
/// Chains live on `rustjay_mixer::Channel` now, so every caller prefixes.
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

pub struct KovvbojRootPlugin {
    #[cfg(feature = "mixer")]
    mixer: Arc<Mutex<Mixer>>,
    /// Layer source entries built during `init()`, handed to the app state on
    /// the first `prepare()` — `init` runs before any state exists.
    #[cfg(feature = "mixer")]
    layer_sources_init: std::collections::HashMap<String, crate::sources::SourceEntry>,
    params_dirty: bool,
    /// Modulation snapshot loaded from the workspace scene in `init()` (which has
    /// no `&EngineState`), applied into `engine.modulation` on the first `prepare()`.
    #[cfg(feature = "mixer")]
    pending_modulation: Option<rustjay_core::modulation::ModulationEngine>,
    /// Custom param base values loaded from the workspace scene in `init()`, queued
    /// into `engine.param_restore` on the first `prepare()` so the renderer applies
    /// them after the rebuilt graph's params (re)register.
    #[cfg(feature = "mixer")]
    pending_params: Option<std::collections::HashMap<String, f32>>,
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

impl KovvbojRootPlugin {
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "mixer")]
            mixer: Arc::new(Mutex::new(Mixer::new())),
            layer_sources_init: std::collections::HashMap::new(),
            params_dirty: false,
            #[cfg(feature = "mixer")]
            pending_modulation: None,
            #[cfg(feature = "mixer")]
            pending_params: None,
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

impl Default for KovvbojRootPlugin {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "mixer")]
impl KovvbojRootPlugin {
    fn build_default_graph(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        // A free-standing layer stack: channel opacities are the layers' own,
        // never scaled by a two-channel crossfader.
        mixer.use_crossfader = false;
        let dummy_engine = EngineState::new();
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let shaders_dir = manifest_dir.join("shaders");
        let mut sources = std::collections::HashMap::new();

        // Two layers to open on: a generator underneath, a camera over it.
        let defaults: [(&str, crate::sources::SourceEntry); 2] = [
            (
                "ColorCycle",
                crate::sources::SourceEntry {
                    id: "colorcycle".to_string(),
                    name: "ColorCycle".to_string(),
                    kind: crate::sources::SourceKind::Isf,
                    path: Some(shaders_dir.join("ColorCycle.fs")),
                    device_index: 0,
                },
            ),
            (
                "Camera",
                crate::sources::SourceEntry {
                    id: "camera".to_string(),
                    name: "Camera".to_string(),
                    kind: crate::sources::SourceKind::Camera,
                    path: None,
                    device_index: 0,
                },
            ),
        ];

        for (uuid, entry) in defaults {
            match instantiate_source(&entry, device, queue, &dummy_engine) {
                Ok(mut source) => {
                    source.set_param_prefix(&format!("ch_{uuid}_"));
                    let channel = Channel::new(uuid, &entry.name, source);
                    if let Err(e) = mixer.add_channel(channel) {
                        log::warn!("[Graph] could not add layer '{}': {}", entry.name, e);
                        continue;
                    }
                    sources.insert(uuid.to_string(), entry);
                }
                Err(e) => log::warn!("[Graph] could not build layer '{}': {}", entry.name, e),
            }
        }

        self.layer_sources_init = sources;


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
        // previously added to mixer.modulation are now omitted; kovvboj will ship
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
        // Channels are a free-standing layer stack here, so the two-channel
        // crossfader special case must not apply — see `Mixer::use_crossfader`.
        mixer.use_crossfader = false;
        let mut sources = std::collections::HashMap::new();

        for desc in &topo.layers {
            // Resolve the source path back to absolute before instantiating.
            let mut entry = desc.source.clone();
            if let Some(p) = entry.path.take() {
                entry.path = Some(crate::scene::resolve(&p, &base));
            }
            let source = match instantiate_source(&entry, device, queue, &dummy_engine) {
                Ok(source) => source,
                Err(e) => {
                    log::warn!("[Topology] failed to rebuild layer '{}': {}", desc.name, e);
                    continue;
                }
            };

            let mut channel = Channel::new(desc.uuid.clone(), desc.name.clone(), source);
            channel.opacity = desc.opacity;
            channel.blend_mode = desc.blend_mode;
            channel.solo = desc.solo;
            channel.mute = desc.mute;

            let prefix = format!("ch_{}_", desc.uuid);
            channel.effect.set_param_prefix(&prefix);
            for fx in &desc.fx {
                if let Some(mut slot) = build_fx_slot(fx, &base, device, queue, &dummy_engine) {
                    slot.effect
                        .set_param_prefix(&format!("{prefix}fx{}_", slot.uuid));
                    channel.chain.push(slot);
                }
            }

            let uuid = desc.uuid.clone();
            if let Err(e) = mixer.add_channel(channel) {
                log::warn!("[Topology] failed to add layer '{}': {}", desc.name, e);
                continue;
            }
            sources.insert(uuid, desc.source.clone());
        }

        for fx in &topo.master_fx {
            if let Some(mut slot) = build_fx_slot(fx, &base, device, queue, &dummy_engine) {
                slot.effect
                    .set_param_prefix(&format!("master_fx{}_", slot.uuid));
                mixer.master.push(slot);
            }
        }

        log::info!(
            "[Topology] rebuilt {} layers, {} master FX",
            topo.layers.len(),
            topo.master_fx.len()
        );
        drop(mixer);
        self.layer_sources_init = sources;
        self.params_dirty = true;
    }
}

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DummyUniforms {
    _pad: [f32; 4],
}

impl EffectPlugin for KovvbojRootPlugin {
    type State = KovvbojAppState;
    type Uniforms = DummyUniforms;

    /// Distinct app identity: drives the control window title, the top-bar name,
    /// and isolates this example's config/presets (`~/.config/rustjay/Kovvboj.json`)
    /// so it doesn't collide with other examples.
    fn app_name(&self) -> &str {
        "KOVVBOJ"
    }

    fn hide_main_output_by_default(&self) -> bool {
        cfg!(feature = "projection")
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

    fn default_state(&self) -> KovvbojAppState {
        #[cfg_attr(not(feature = "mixer"), allow(unused_mut))]
        let mut s = KovvbojAppState::default();
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
        state: &mut KovvbojAppState,
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

                // Queue the workspace's saved param base values; the renderer
                // applies them after the rebuilt graph's params (re)register.
                if let Some(params) = self.pending_params.take()
                    && let Ok(mut restore) = engine.param_restore.lock() {
                        restore.extend(params);
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
                            state.stage.ensure_builtin_fixture_profiles();
                            log::info!(
                                "[Prepare] loaded stage: {} projectors, {} surfaces, {} fixture profiles",
                                state.stage.projectors.len(),
                                state.stage.surfaces.len(),
                                state.stage.fixture_profiles.len()
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
            // Undo / redo land here: replay the recorded graph, then let the
            // usual params_dirty pass re-register everything under the restored
            // uuids.
            if let Some(topo) = state.pending_topology.take() {
                self.apply_topology(&topo, device, queue);
                state.params_dirty_request = true;
            }

            // Apply pending scene from preset load or runtime restore.
            if let Some(scene) = state.pending_scene.take() {
                // Rebuild the routing graph when the scene carries topology, so
                // switching presets recreates the deck/FX graph (not just knobs).
                // apply_topology clears + replaces the live graph with the saved
                // UUIDs and flags params_dirty; do it before applying knobs (which
                // match channels by UUID) and modulation (keyed by param id).
                match scene.topology.as_ref() {
                    Some(topo) if usable_topology(topo) => {
                        self.apply_topology(topo, device, queue);
                    }
                    Some(topo) => warn_stale_topology(topo, engine),
                    None => {}
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
                // Queue the saved param base values; the renderer applies them
                // after the rebuilt graph's params (re)register (set_param_base
                // needs &mut engine, which we don't have here).
                if !scene.params.is_empty()
                    && let Ok(mut restore) = engine.param_restore.lock() {
                        restore.extend(scene.params.clone());
                    }
            }

            if let Some(ref watcher) = state.shader_watcher {
                for event in watcher.poll() {
                    for path in &event.paths {
                        log::info!("[ShaderWatcher] changed: {}", path.display());
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("ISF Shader")
                            .to_string();
                        let Ok(mut mixer) = state.mixer.lock() else {
                            continue;
                        };

                        // A layer whose *source* is this shader.
                        let layers: Vec<String> = state
                            .layer_sources
                            .iter()
                            .filter(|(_, e)| e.path.as_ref() == Some(path))
                            .map(|(uuid, _)| uuid.clone())
                            .collect();
                        for uuid in layers {
                            let Some(ch) = mixer.channels.iter_mut().find(|c| c.uuid == uuid)
                            else {
                                continue;
                            };
                            match rustjay_isf::IsfEffect::from_path(path) {
                                Ok(isf) => {
                                    let node = EffectNode::new(isf, &name, device, queue, engine);
                                    ch.effect = Box::new(node);
                                    ch.effect.set_param_prefix(&format!("ch_{uuid}_"));
                                    self.params_dirty = true;
                                    log::info!("[HotReload] reloaded source for layer {uuid}");
                                }
                                Err(e) => log::warn!(
                                    "[HotReload] failed to reload {}: {e}",
                                    path.display()
                                ),
                            }
                        }

                        // Every FX slot built from it, in any chain.
                        let mut any = reload_matching_slots(
                            &mut mixer.master,
                            "master_",
                            path,
                            &name,
                            device,
                            queue,
                            engine,
                        );
                        for ch in mixer.channels.iter_mut() {
                            let base = format!("ch_{}_", ch.uuid);
                            any |= reload_matching_slots(
                                &mut ch.chain, &base, path, &name, device, queue, engine,
                            );
                        }
                        if any {
                            self.params_dirty = true;
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
                let snapshot = build_kovvboj_snapshot(&mixer, &state.registry, engine);
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
                if let Ok(mixer) = state.mixer.lock()
                    && let Some(scene) = state.scene_snapshot_if_ready(&mixer)
                    && let Err(e) = state.workspace.save_scene(&scene)
                {
                    log::warn!("[AutoSave] scene failed: {}", e);
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

            // Refresh the custom-param snapshot only when base values actually
            // change (param edit, preset load) — cheap `Vec<f32>` compare per
            // frame, no allocation otherwise. The save paths read this.
            if state.param_bases_cache != engine.custom_param_bases {
                state.param_bases_cache = engine.custom_param_bases.clone();
                state.param_snapshot = engine
                    .param_descriptors
                    .iter()
                    .zip(engine.custom_param_bases.iter())
                    .map(|(d, &v)| (d.id.clone(), v))
                    .collect();
            }

            // One snapshot for whatever structural edits this frame carries —
            // deck removals, new decks, new FX. Taken before any of them are
            // applied, so undo steps back to the graph as it was.
            if !state.pending_removals.is_empty()
                || !state.pending_layers.is_empty()
                || !state.pending_effects.is_empty()
                || !state.pending_fx_removals.is_empty()
                || !state.pending_source_swaps.is_empty()
            {
                state.push_undo();
            }

            // Drain queued FX removals, purging each slot's modulation.
            let fx_removals: Vec<PendingFxRemoval> =
                std::mem::take(&mut state.pending_fx_removals);
            if !fx_removals.is_empty() {
                if let Ok(mut mixer) = state.mixer.lock() {
                    for req in fx_removals {
                        let Some((chain, base)) = chain_parts(&mut mixer, &req.chain) else {
                            continue;
                        };
                        chain.retain(|s| s.uuid != req.slot);
                        let prefix = format!("{base}fx{}_", req.slot);
                        if let Ok(mut m) = engine.modulation.lock() {
                            m.remove_assignments_with_prefix(&prefix);
                        }
                    }
                }
                state.params_dirty_request = true;
            }

            // Hand over the layer source entries built during `init()`, which
            // runs before any app state exists. Without this the map stays
            // empty and every layer saves as a placeholder solid colour.
            if !self.layer_sources_init.is_empty() {
                state
                    .layer_sources
                    .extend(std::mem::take(&mut self.layer_sources_init));
            }

            // The dimmer is a normal parameter, so MIDI/OSC/LFO can drive it;
            // the mixer just reads the resolved value each frame.
            if let Ok(mut mixer) = state.mixer.lock() {
                mixer.master_dim = engine
                    .get_param(crate::ui::MASTER_DIM)
                    .unwrap_or(1.0);

            }

            // Re-point a layer's source. The layer keeps its uuid, so its
            // chain and every binding under `ch_<uuid>_` survive untouched.
            let swaps: Vec<PendingSourceSwap> = std::mem::take(&mut state.pending_source_swaps);
            for req in swaps {
                match instantiate_source(&req.source, device, queue, engine) {
                    Ok(mut source) => {
                        source.set_param_prefix(&format!("ch_{}_", req.layer_uuid));
                        let Ok(mut mixer) = state.mixer.lock() else {
                            continue;
                        };
                        let Some(ch) =
                            mixer.channels.iter_mut().find(|c| c.uuid == req.layer_uuid)
                        else {
                            continue;
                        };
                        ch.effect = source;
                        ch.name = req.source.name.clone();
                        drop(mixer);
                        state
                            .layer_sources
                            .insert(req.layer_uuid.clone(), req.source.clone());
                        self.params_dirty = true;
                        engine.notify(
                            format!("Connected to '{}'", req.source.name),
                            rustjay_core::NotificationLevel::Info,
                            std::time::Duration::from_secs(3),
                        );
                    }
                    Err(e) => engine.notify(
                        format!("Could not connect to '{}': {e}", req.source.name),
                        rustjay_core::NotificationLevel::Error,
                        std::time::Duration::from_secs(5),
                    ),
                }
            }

            // Layer removals.
            let removals: Vec<PendingRemoval> = std::mem::take(&mut state.pending_removals);
            for req in removals {
                let Ok(mut mixer) = state.mixer.lock() else {
                    continue;
                };
                let Some(index) = mixer.channels.iter().position(|c| c.uuid == req.layer_uuid)
                else {
                    continue;
                };
                let Ok(removed) = mixer.remove_channel(index) else {
                    continue;
                };
                drop(mixer);
                self.params_dirty = true;
                state.layer_sources.remove(&req.layer_uuid);
                // One prefix sweep covers the layer's source params and every FX
                // it carried (`ch_<uuid>_…` and `ch_<uuid>_fx…`).
                if let Ok(mut m) = engine.modulation.lock() {
                    m.remove_assignments_with_prefix(&format!("ch_{}_", req.layer_uuid));
                }
                engine.notify(
                    format!("Removed layer '{}'", removed.name),
                    rustjay_core::NotificationLevel::Info,
                    std::time::Duration::from_secs(3),
                );
            }

            // New layers.
            let pending: Vec<PendingLayer> = std::mem::take(&mut state.pending_layers);
            for req in pending {
                let uuid = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
                match instantiate_source(&req.source, device, queue, engine) {
                    Ok(mut source) => {
                        source.set_param_prefix(&format!("ch_{uuid}_"));
                        let mut channel = Channel::new(uuid.clone(), &req.source.name, source);
                        channel.opacity = 1.0;
                        let name = req.source.name.clone();
                        let Ok(mut mixer) = state.mixer.lock() else {
                            continue;
                        };
                        // New layers go on top of the stack, which is where you
                        // expect a thing you just added to appear.
                        if mixer.add_channel(channel).is_err() {
                            drop(mixer);
                            engine.notify(
                                format!("Could not add layer '{name}'"),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            );
                            continue;
                        }
                        drop(mixer);
                        state.layer_sources.insert(uuid, req.source.clone());
                        self.params_dirty = true;
                        engine.notify(
                            format!("Added layer '{name}'"),
                            rustjay_core::NotificationLevel::Info,
                            std::time::Duration::from_secs(3),
                        );
                    }
                    Err(e) => engine.notify(
                        format!("Could not build '{}': {e}", req.source.name),
                        rustjay_core::NotificationLevel::Error,
                        std::time::Duration::from_secs(5),
                    ),
                }
            }


            let pending_effects: Vec<PendingEffect> = std::mem::take(&mut state.pending_effects);
            let mut mixer_guard = (!pending_effects.is_empty())
                .then(|| state.mixer.lock().unwrap_or_else(|e| e.into_inner()));
            for req in pending_effects {
                let Some(mixer) = mixer_guard.as_mut() else {
                    continue;
                };
                match req.target {
                    EffectTarget::Master => match rustjay_isf::IsfEffect::from_path(&req.path) {
                        Ok(isf) => {
                            let name = isf.shader_name.clone();
                            let node = EffectNode::new(isf, &name, device, queue, engine);
                            mixer.add_master_effect(Box::new(node));
                            let pos = position_new_slot(&mut mixer.master, req.index);
                            mixer.master[pos].source_path = Some(req.path.clone());
                            self.params_dirty = true;
                            engine.notify(
                                format!("Added master FX '{name}'"),
                                rustjay_core::NotificationLevel::Success,
                                std::time::Duration::from_secs(3),
                            );
                        }
                        Err(e) => engine.notify(
                            format!("Failed to load master FX: {e}"),
                            rustjay_core::NotificationLevel::Error,
                            std::time::Duration::from_secs(4),
                        ),
                    },
                    EffectTarget::Layer { ref layer_uuid } => {
                        let Some(channel) =
                            mixer.channels.iter_mut().find(|c| &c.uuid == layer_uuid)
                        else {
                            engine.notify(
                                "Layer no longer exists".to_string(),
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
                                let pos = position_new_slot(&mut channel.chain, req.index);
                                channel.chain[pos].source_path = Some(req.path.clone());
                                self.params_dirty = true;
                                engine.notify(
                                    format!("Added '{name}' to {}", channel.name),
                                    rustjay_core::NotificationLevel::Success,
                                    std::time::Duration::from_secs(3),
                                );
                            }
                            Err(e) => engine.notify(
                                format!("Failed to load FX: {e}"),
                                rustjay_core::NotificationLevel::Error,
                                std::time::Duration::from_secs(4),
                            ),
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
                        let sender_name = format!("kovvboj — {}", proj.name);

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
                                log::error!("[Kovvboj] Failed to start projector {i} recording: {e}");
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
                        let sender_name = format!("kovvboj — {}", hl.name);

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
                                log::error!("[Kovvboj] Failed to start headless {i} recording: {e}");
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

                    // ── Lighting outputs (sACN / Art-Net) ───────────────────
                    // One pixel sampler per lighting output (stable id), packing
                    // all segments into an atlas. Each frame: render atlas →
                    // readback → demux tiles in scan order → map RGB → patch per
                    // segment into a DmxFrame → submit to the TX thread.
                    {
                        use std::collections::HashSet;

                        let profiles = state.stage.fixture_profiles.clone();
                        let mut active_ids = HashSet::new();
                        let mut overlap_spans = Vec::new();

                        // Lock the mixer once to resolve channel-sourced segments
                        // to their channel textures (released at block end).
                        #[cfg(feature = "mixer")]
                        let mixer_guard = state.mixer.lock().ok();

                        for lo in state.stage.lighting_outputs.iter_mut() {
                            let layout = output_atlas_layout(lo, &state.stage.surfaces);
                            let sampler_id = match lo.sampler_id {
                                Some(id) => {
                                    sub.update_pixel_sampler(id, layout);
                                    id
                                }
                                None => {
                                    let Some(id) = sub.add_pixel_sampler(layout) else {
                                        continue;
                                    };
                                    lo.sampler_id = Some(id);
                                    id
                                }
                            };
                            active_ids.insert(sampler_id);

                            // Per-segment source override: a surface sourced from a
                            // mixer channel makes its segment sample that channel's
                            // texture instead of the master composite.
                            let tile_sources: Vec<Option<std::sync::Arc<wgpu::TextureView>>> = lo
                                .segments
                                .iter()
                                .map(|seg| {
                                    resolve_segment_source(seg, &state.stage.surfaces, |_ch| {
                                        #[cfg(feature = "mixer")]
                                        {
                                            mixer_guard
                                                .as_ref()
                                                .and_then(|m| m.channel_texture(_ch))
                                                .map(|t| {
                                                    std::sync::Arc::new(t.texture.create_view(
                                                        &wgpu::TextureViewDescriptor::default(),
                                                    ))
                                                })
                                        }
                                        #[cfg(not(feature = "mixer"))]
                                        {
                                            None
                                        }
                                    })
                                })
                                .collect();
                            sub.set_sampler_tile_sources(sampler_id, &tile_sources);

                            let want = lo.enabled
                                && matches!(
                                    lo.output_type,
                                    OutputType::Sacn | OutputType::ArtNet
                                );

                            let has_sender = state.lighting_senders.contains_key(&sampler_id);
                            if want && !has_sender {
                                match build_dmx_sender(&lo.output_type, &lo.transport) {
                                    Ok(sender) => {
                                        state.lighting_senders.insert(sampler_id, sender);
                                        engine.notify(
                                            format!(
                                                "{} output started: {}",
                                                lo.output_type.label(),
                                                lo.name
                                            ),
                                            rustjay_core::NotificationLevel::Success,
                                            std::time::Duration::from_secs(3),
                                        );
                                    }
                                    Err(e) => engine.notify(
                                        format!("{} output failed: {e}", lo.output_type.label()),
                                        rustjay_core::NotificationLevel::Error,
                                        std::time::Duration::from_secs(4),
                                    ),
                                }
                            } else if !want && has_sender {
                                if let Some(sender) = state.lighting_senders.remove(&sampler_id) {
                                    sender.shutdown();
                                }
                                state.lighting_last_frames.remove(&sampler_id);
                            }

                            if want {
                                if let Some((px, layout)) = sub.pixel_sampler_atlas(sampler_id) {
                                    let frame = build_dmx_frame(
                                        lo,
                                        &profiles,
                                        px,
                                        layout,
                                    );
                                    state.lighting_last_frames.insert(sampler_id, frame.clone());
                                    if let Some(sender) = state.lighting_senders.get(&sampler_id) {
                                        sender.submit(frame);
                                    }
                                }
                                sink_labels.push(lo.output_type.label().to_string());
                            }

                            // Collect patch spans for overlap detection.
                            for seg in lo.segments.iter().filter(|s| s.enabled) {
                                let profile = profiles.iter().find(|p| p.id == seg.profile);
                                let footprint = profile.map(|p| p.channels.len()).unwrap_or(3);
                                let count = (seg.grid[0] as usize) * (seg.grid[1] as usize);
                                overlap_spans.extend(rustjay_lighting::segment_spans(
                                    lo.name.clone(),
                                    seg.name.clone(),
                                    seg.start_universe,
                                    seg.start_channel,
                                    footprint,
                                    count,
                                ));
                            }
                        }

                        // Stop senders for outputs that no longer exist.
                        let stale_senders: Vec<_> = state
                            .lighting_senders
                            .keys()
                            .filter(|id| !active_ids.contains(id))
                            .copied()
                            .collect();
                        for id in stale_senders {
                            if let Some(sender) = state.lighting_senders.remove(&id) {
                                sender.shutdown();
                            }
                            state.lighting_last_frames.remove(&id);
                        }
                        // Remove samplers for outputs that no longer exist.
                        sub.remove_stale_pixel_samplers(&active_ids);

                        // Compute overlap warnings for the UI.
                        state.lighting_overlap_warnings = rustjay_lighting::find_overlaps(&overlap_spans);
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

    fn build_uniforms(&self, _state: &KovvbojAppState, _engine: &EngineState) -> DummyUniforms {
        DummyUniforms { _pad: [0.0; 4] }
    }

    fn parameters(&self) -> Vec<rustjay_core::ParameterDescriptor> {
        #[cfg(feature = "mixer")]
        {
            let mut params = self
                .mixer
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .parameters();
            // The master dimmer is the host's own control, not the mixer's, but
            // it is a real parameter so a blackout fader is reachable from MIDI,
            // OSC and modulation like everything else.
            params.push(rustjay_core::ParameterDescriptor {
                id: crate::ui::MASTER_DIM.to_string(),
                name: "Master Dim".to_string(),
                param_type: rustjay_core::ParamType::Float,
                min: 0.0,
                max: 1.0,
                default: 1.0,
                step: 0.01,
                category: rustjay_core::ParamCategory::Color,
            });
            params
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

    // Kovvboj's egui tabs are non-replacing (each gets its own sidebar button via
    // the engine host), so the built-in tabs — including the working LFO and MIDI
    // panels the Kovvboj tabs only summarize — stay available. Nothing is hidden.

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn on_engine_ready(&mut self, engine: &mut EngineState) {
        #[cfg(feature = "mixer")]
        {
            let mut router = ParamRouter::new();
            if let Ok(mixer) = self.mixer.lock() {
                for ch in mixer.channels.iter() {
                    router.register_channel(&ch.uuid, &ch.name);
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
                .filter(|t| usable_topology(t))
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
                // Modulation + param values can't be applied here (no engine);
                // stash them for the first prepare().
                if !scene.modulation.sources.is_empty() {
                    self.pending_modulation = Some(scene.modulation.clone());
                }
                if !scene.params.is_empty() {
                    self.pending_params = Some(scene.params.clone());
                }
                log::info!("[Workspace] restored scene");
            }
        }
    }

    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, app_state: &mut KovvbojAppState) -> bool {
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

            // Layer thumbnails, from the outputs that render_to just produced.
            app_state
                .thumbs
                .update(ctx.device, ctx.encoder, ctx.vertex_buffer, &mixer);

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

                    // Cropping is driven solely by the surface's `uv_crop_rect`
                    // (its position/size box over the master, kept in sync with
                    // the surface rectangle in the Stage tab). The cropped region
                    // fills the output quad, matching the Stage-tab canvas.
                    let uv_scale = [1.0, 1.0];
                    let uv_offset = [0.0, 0.0];

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
fn build_kovvboj_snapshot(
    mixer: &Mixer,
    registry: &Registry,
    engine: &EngineState,
) -> KovvbojStateSnapshot {
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
        // A layer has no nested decks any more; the list is kept in the API
        // shape so existing clients keep parsing, and stays empty.
        let decks = Vec::new();
        let mut ch_effects = Vec::new();

        for slot in &ch.chain {
            ch_effects.push(KovvbojEffect {
                uuid: slot.uuid.clone(),
                name: slot.effect.label().to_string(),
                enabled: slot.enabled,
                param_prefix: format!("ch_{}_fx{}_", ch.uuid, slot.uuid),
            });
        }

        channels.push(KovvbojChannel {
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
        master_effects.push(KovvbojEffect {
            uuid: slot.uuid.clone(),
            name: slot.effect.label().to_string(),
            enabled: slot.enabled,
            param_prefix: format!("master_fx{}_", slot.uuid),
        });
    }

    KovvbojStateSnapshot {
        crossfader: live("crossfader", mixer.crossfader),
        channels,
        master_effects,
        library: registry_to_library(registry),
    }
}

#[cfg(all(feature = "mixer", feature = "api"))]
fn registry_to_library(registry: &Registry) -> KovvbojLibrary {
    KovvbojLibrary {
        shaders: registry.shaders.iter().map(source_entry_to_api).collect(),
        images: registry.images.iter().map(source_entry_to_api).collect(),
        videos: registry.videos.iter().map(source_entry_to_api).collect(),
        builtins: registry.builtins.iter().map(source_entry_to_api).collect(),
    }
}

#[cfg(all(feature = "mixer", feature = "api"))]
fn source_entry_to_api(e: &crate::sources::SourceEntry) -> KovvbojSourceEntry {
    use crate::sources::SourceKind;
    KovvbojSourceEntry {
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
            SourceKind::Spout => "spout",
            SourceKind::Srt => "srt",
            SourceKind::Hls => "hls",
            SourceKind::Dash => "dash",
            SourceKind::Rtmp => "rtmp",
            SourceKind::Http => "http",
            SourceKind::Rtsp => "rtsp",
        }
        .to_string(),
        path: e.path.as_ref().map(|p| p.to_string_lossy().to_string()),
        device_index: e.device_index,
    }
}


#[cfg(all(test, feature = "mixer"))]
mod tests {
    use super::*;

    /// Minimal `EffectInstance` that records its param prefix.
    struct DummyFx {
        prefix: String,
    }

    impl DummyFx {
        fn new() -> Self {
            Self {
                prefix: String::new(),
            }
        }
    }

    impl EffectInstance for DummyFx {
        fn label(&self) -> &str {
            "dummy"
        }
        fn as_any(&self) -> Option<&dyn std::any::Any> {
            Some(self)
        }
        fn set_param_prefix(&mut self, prefix: &str) {
            self.prefix = prefix.to_string();
        }
        fn render_to(
            &mut self,
            _ctx: &mut RenderCtx<'_>,
            _inputs: &[EffectInput<'_>],
            _target: RenderTarget<'_>,
            _engine: &EngineState,
        ) {
        }
    }

    /// Two layers, each carrying two FX slots.
    fn test_mixer() -> Mixer {
        let mut mixer = Mixer::new();
        mixer.use_crossfader = false;
        for (uuid, name) in [("l1", "Layer 1"), ("l2", "Layer 2")] {
            let mut channel = Channel::new(uuid, name, Box::new(DummyFx::new()));
            channel.add_effect(Box::new(DummyFx::new()));
            channel.add_effect(Box::new(DummyFx::new()));
            mixer.add_channel(channel).unwrap();
        }
        mixer
    }

    fn layer_ref<'m>(mixer: &'m Mixer, layer: &str) -> &'m Channel {
        mixer.channels.iter().find(|c| c.uuid == layer).unwrap()
    }

    #[test]
    fn same_chain_drop_reorders_and_keeps_prefixes() {
        let mut mixer = test_mixer();
        let mut engine = EngineState::new();
        let chain = ChainRef::Layer { layer: "l1".into() };
        let first_uuid = layer_ref(&mixer, "l1").chain[0].uuid.clone();
        let second_uuid = layer_ref(&mixer, "l1").chain[1].uuid.clone();
        let first_prefix = format!("ch_l1_fx{}_", first_uuid);

        // Dropping on the slot's own gaps is a no-op.
        assert!(!move_effect(&mut mixer, &mut engine, &chain, &first_uuid, &chain, 0));
        assert!(!move_effect(&mut mixer, &mut engine, &chain, &first_uuid, &chain, 1));

        // Dropping on the trailing gap moves the slot to the end.
        assert!(move_effect(&mut mixer, &mut engine, &chain, &first_uuid, &chain, 2));
        let d = layer_ref(&mixer, "l1");
        assert_eq!(d.chain[0].uuid, second_uuid);
        assert_eq!(d.chain[1].uuid, first_uuid);
        // Prefixes are UUID-stable: a reorder re-keys nothing.
        let fx = d.chain[1]
            .effect
            .as_any()
            .and_then(|a| a.downcast_ref::<DummyFx>())
            .unwrap();
        assert_eq!(fx.prefix, first_prefix);
    }

    #[test]
    fn cross_chain_move_reprefixes_and_rekeys_engine_stores() {
        let mut mixer = test_mixer();
        let mut engine = EngineState::new();
        let from = ChainRef::Layer { layer: "l1".into() };
        let d = layer_ref(&mixer, "l1");
        let uuid = d.chain[0].uuid.clone();
        let old_prefix = format!("ch_{}_fx{}_", d.uuid, uuid);

        // Wire a modulation assignment, a MIDI mapping and a param value to
        // the slot's old prefix.
        engine.modulation.lock().unwrap().assignments.insert(
            format!("{old_prefix}angle"),
            vec![rustjay_core::modulation::ParamModulation {
                source_id: "lfo_0".into(),
                amount: 1.0,
                component: None,
            }],
        );
        engine.midi_mappings.push(rustjay_core::MidiMappingSnapshot {
            name: "angle".into(),
            param_path: format!("color/{old_prefix}angle"),
            kind: rustjay_core::MidiMsgKind::Cc,
            selector: 20,
            channel: 0,
            min_value: 0.0,
            max_value: 1.0,
        });
        engine.param_descriptors = std::sync::Arc::new(vec![
            rustjay_core::ParameterDescriptor::float(
                format!("{old_prefix}angle"),
                "angle",
                rustjay_core::ParamCategory::Color,
                0.0,
                1.0,
                0.0,
                0.01,
            ),
        ]);
        engine.custom_param_bases = vec![0.7];
        engine.custom_params = vec![0.7];

        assert!(move_effect(
            &mut mixer,
            &mut engine,
            &from,
            &uuid,
            &ChainRef::Master,
            0
        ));

        // The slot moved; the deck chain shrank.
        assert_eq!(mixer.master.len(), 1);
        assert_eq!(mixer.master[0].uuid, uuid);
        assert_eq!(layer_ref(&mixer, "l1").chain.len(), 1);

        // The slot was re-prefixed at the destination.
        let new_prefix = format!("master_fx{uuid}_");
        let fx = mixer.master[0]
            .effect
            .as_any()
            .and_then(|a| a.downcast_ref::<DummyFx>())
            .unwrap();
        assert_eq!(fx.prefix, new_prefix);

        // Modulation and MIDI follow the slot.
        assert!(
            engine
                .modulation
                .lock()
                .unwrap()
                .assignments
                .contains_key(&format!("{new_prefix}angle"))
        );
        assert_eq!(
            engine.midi_mappings[0].param_path,
            format!("color/{new_prefix}angle")
        );

        // The param value is queued for restore under the new id.
        assert!(
            engine
                .param_restore
                .lock()
                .unwrap()
                .contains(&(format!("{new_prefix}angle"), 0.7))
        );
    }

    #[test]
    fn cross_deck_move_inserts_at_drop_index() {
        let mut mixer = test_mixer();
        let mut engine = EngineState::new();
        let from = ChainRef::Layer { layer: "l1".into() };
        let to = ChainRef::Layer { layer: "l2".into() };
        let uuid = layer_ref(&mixer, "l1").chain[0].uuid.clone();
        let existing = layer_ref(&mixer, "l2").chain[0].uuid.clone();

        assert!(move_effect(&mut mixer, &mut engine, &from, &uuid, &to, 0));

        let d2 = layer_ref(&mixer, "l2");
        assert_eq!(d2.chain.len(), 3);
        assert_eq!(d2.chain[0].uuid, uuid, "inserted at gap 0");
        assert_eq!(d2.chain[1].uuid, existing);
        assert_eq!(layer_ref(&mixer, "l1").chain.len(), 1);
    }

    #[test]
    fn move_to_missing_chain_keeps_slot_in_source() {
        let mut mixer = test_mixer();
        let mut engine = EngineState::new();
        let from = ChainRef::Layer { layer: "l1".into() };
        let uuid = layer_ref(&mixer, "l1").chain[0].uuid.clone();
        // A layer uuid that is not in the mixer at all.
        let missing = ChainRef::Layer {
            layer: "deleted-layer".into(),
        };

        assert!(!move_effect(&mut mixer, &mut engine, &from, &uuid, &missing, 0));

        let d = layer_ref(&mixer, "l1");
        assert_eq!(d.chain.len(), 2, "slot restored, not lost");
        assert!(d.chain.iter().any(|s| s.uuid == uuid));
    }

    #[test]
    fn undo_stack_caps_depth_and_redo_clears_on_a_fresh_edit() {
        let mut state = KovvbojAppState::default();

        // More pushes than the cap: the oldest entries fall off the bottom.
        for _ in 0..(KovvbojAppState::UNDO_DEPTH + 5) {
            state.push_undo();
        }
        assert_eq!(
            state.undo_stack.len(),
            KovvbojAppState::UNDO_DEPTH,
            "the stack must stay capped"
        );

        assert!(state.undo(), "an undo should be available");
        assert_eq!(state.redo_stack.len(), 1, "undo feeds the redo stack");
        assert!(
            state.pending_topology.is_some(),
            "undo queues a topology for the next prepare()"
        );

        state.pending_topology = None;
        assert!(state.redo(), "a redo should be available");
        assert!(state.redo_stack.is_empty());
        assert!(state.pending_topology.is_some());

        // Any fresh edit invalidates the redo history.
        state.undo();
        assert_eq!(state.redo_stack.len(), 1);
        state.push_undo();
        assert!(
            state.redo_stack.is_empty(),
            "a new edit must drop what was undone"
        );
    }

    #[test]
    fn undo_and_redo_report_false_when_there_is_nothing_to_do() {
        let mut state = KovvbojAppState::default();
        assert!(!state.undo());
        assert!(!state.redo());
        assert!(state.pending_topology.is_none());
    }

    /// Restacking must move a layer to the dropped-on layer's position, and
    /// reordering must not disturb uuids — parameter prefixes and modulation
    /// are keyed to them.
    #[test]
    fn restacking_moves_a_layer_and_keeps_uuids() {
        let mut mixer = Mixer::new();
        mixer.use_crossfader = false;
        for uuid in ["bottom", "middle", "top"] {
            mixer
                .add_channel(Channel::new(uuid, uuid, Box::new(DummyFx::new())))
                .unwrap();
        }
        let ids = |m: &Mixer| m.channels.iter().map(|c| c.uuid.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&mixer), ["bottom", "middle", "top"]);

        // Drag "bottom" onto "top": it takes top's index.
        let from = mixer.channels.iter().position(|c| c.uuid == "bottom").unwrap();
        let to = mixer.channels.iter().position(|c| c.uuid == "top").unwrap();
        mixer.reorder_channel(from, to);
        assert_eq!(ids(&mixer), ["middle", "top", "bottom"]);

        // Dropping a layer on itself is a no-op.
        let same = mixer.channels.iter().position(|c| c.uuid == "top").unwrap();
        mixer.reorder_channel(same, same);
        assert_eq!(ids(&mixer), ["middle", "top", "bottom"]);
    }

    /// Re-pointing a device layer must keep the layer: its chain, its mix, and
    /// every binding keyed to `ch_<uuid>_`. That is the whole reason the swap
    /// replaces `Channel::effect` rather than rebuilding the layer.
    #[cfg(feature = "mixer")]
    #[test]
    fn swapping_a_layer_source_keeps_the_chain_and_bindings() {
        let mut mixer = Mixer::new();
        let mut channel = Channel::new("cam", "MacBook Camera", Box::new(DummyFx::new()));
        channel.add_effect(Box::new(DummyFx::new()));
        channel.opacity = 0.42;
        channel.blend_mode = rustjay_mixer::BlendMode::Add;
        mixer.add_channel(channel).unwrap();

        let fx_uuid = mixer.channels[0].chain[0].uuid.clone();
        let prefix = format!("ch_cam_fx{fx_uuid}_");

        let engine = EngineState::new();
        engine
            .modulation
            .lock()
            .unwrap()
            .assignments
            .insert(format!("{prefix}angle"), Vec::new());

        // The swap the UI queues: same layer uuid, different source.
        let ch = mixer.channels.iter_mut().find(|c| c.uuid == "cam").unwrap();
        ch.effect = Box::new(DummyFx::new());
        ch.name = "OBS Virtual Camera".to_string();

        let ch = &mixer.channels[0];
        assert_eq!(ch.uuid, "cam", "the layer keeps its identity");
        assert_eq!(ch.name, "OBS Virtual Camera", "and takes the new name");
        assert_eq!(ch.chain.len(), 1, "its chain survives");
        assert_eq!(ch.chain[0].uuid, fx_uuid, "with the same slot uuid");
        assert_eq!(ch.opacity, 0.42, "and its mix");
        assert!(
            engine
                .modulation
                .lock()
                .unwrap()
                .assignments
                .contains_key(&format!("{prefix}angle")),
            "bindings are keyed to the layer, so a source swap must not touch them"
        );
    }
}
