//! Scene — the full runtime state of the show.
//!
//! Channels, decks, effects, modulation, crossfader, and sequences.
//! Persisted as `.kovvboj/scene.json`.
//!
//! Two layers are persisted:
//! - **Knobs** ([`rustjay_mixer::MixerState`]) — crossfader, per-channel
//!   opacity/blend/solo/mute, modulation, sequencer.
//! - **Topology** ([`Topology`]) — which channels, decks, sources, and FX exist.
//!   Without this the graph would be rebuilt from the hard-coded default
//!   assembly and any runtime additions (decks, FX) would be lost on reload —
//!   along with the modulation that targets their now-missing param keys.

use serde::{Deserialize, Serialize};

#[cfg(feature = "mixer")]
use std::path::{Path, PathBuf};

/// Scene snapshot: mix settings + sequencer + routing topology.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scene {
    pub version: u32,
    /// Mixer-level mix settings (crossfader, channel opacities/blends, modulation).
    #[cfg(feature = "mixer")]
    pub mixer_state: rustjay_mixer::MixerState,
    /// Sequencer steps and playback state.
    #[cfg(feature = "mixer")]
    #[serde(default)]
    pub sequencer: rustjay_mixer::SequencerState,
    /// Routing graph: channels, decks, sources, and FX chains. `None` for scenes
    /// saved before topology persistence existed — those fall back to the default
    /// assembly on load.
    #[cfg(feature = "mixer")]
    #[serde(default)]
    pub topology: Option<Topology>,
    /// Snapshot of the unified `EngineState.modulation` (LFO/ADSR/audio-band/
    /// step-seq sources + their param assignments). Captured separately because
    /// the mixer no longer owns modulation (it lives on `EngineState`), so
    /// `mixer.serialize_state()` cannot see it. `Default` (empty) for old scenes.
    #[cfg(feature = "mixer")]
    #[serde(default)]
    pub modulation: rustjay_core::modulation::ModulationEngine,
    /// Base values of every custom parameter (fully-qualified id → value), e.g.
    /// each deck/channel/FX param. Captured so a graph rebuilt on load restores
    /// its FX-internal values, not just structure. `Default` (empty) for old scenes.
    #[cfg(feature = "mixer")]
    #[serde(default)]
    pub params: std::collections::HashMap<String, f32>,
}

#[cfg(feature = "mixer")]
impl Scene {
    /// Snapshot from the live mixer (knobs + topology). The unified modulation
    /// must be filled in by the caller via [`with_modulation`](Self::with_modulation),
    /// since it lives on `EngineState`, not the mixer.
    pub fn from_mixer(
        mixer: &rustjay_mixer::Mixer,
        sources: &std::collections::HashMap<String, crate::sources::SourceEntry>,
    ) -> Self {
        Self {
            version: 2,
            mixer_state: mixer.serialize_state(),
            sequencer: mixer.sequencer.clone(),
            topology: Some(Topology::from_mixer(mixer, sources)),
            modulation: rustjay_core::modulation::ModulationEngine::default(),
            params: std::collections::HashMap::new(),
        }
    }

    /// Attach a snapshot of the unified modulation engine (chainable).
    pub fn with_modulation(
        mut self,
        modulation: &rustjay_core::modulation::ModulationEngine,
    ) -> Self {
        self.modulation = modulation.clone();
        self
    }

    /// Attach a snapshot of custom param base values (chainable).
    pub fn with_params(mut self, params: std::collections::HashMap<String, f32>) -> Self {
        self.params = params;
        self
    }

    /// Apply knob settings onto an already-built mixer.
    ///
    /// Returns `Some(engine)` when the scene was saved with a v1 preset that
    /// carried modulation data. Callers should merge the returned engine into
    /// `EngineState.modulation` (see `UNIFIED_MODULATION_ROADMAP.md` M4.5).
    pub fn apply_to_mixer(
        &self,
        mixer: &mut rustjay_mixer::Mixer,
    ) -> Option<rustjay_core::modulation::ModulationEngine> {
        let (_, legacy) = mixer.apply_state(&self.mixer_state);
        mixer.sequencer = self.sequencer.clone();
        legacy
    }
}

// ---------------------------------------------------------------------------
// Topology descriptors — a serializable mirror of the live routing graph.
// ---------------------------------------------------------------------------

/// One ISF effect slot in a chain (deck, channel, or master).
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FxDesc {
    /// Stable slot identity — reproduced on replay so the FX's param prefix
    /// (`…fx<uuid>_` / `master_fx<uuid>_`) matches its saved modulation.
    pub uuid: String,
    /// Path to the `.fs` ISF shader, stored relative to the crate root when
    /// possible for portability.
    pub path: PathBuf,
    /// Whether the slot is active.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// One layer: a source, its FX chain, and how it composites.
///
/// Replaces the old `DeckDesc`/`ChannelDesc` pair. A layer is a
/// `rustjay_mixer::Channel`, so its uuid is the channel uuid and its parameter
/// prefix is `ch_<uuid>_` — reproduced on replay so saved modulation matches.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerDesc {
    /// Stable identity, reproduced on replay.
    pub uuid: String,
    /// Display name.
    pub name: String,
    /// The library descriptor the source was built from (kind, path, device
    /// index, …). Paths are stored relative to the crate root when possible.
    pub source: crate::sources::SourceEntry,
    /// Base mix opacity.
    pub opacity: f32,
    /// How this layer composites over the ones beneath it.
    pub blend_mode: rustjay_mixer::BlendMode,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub mute: bool,
    /// Ordered post-source FX.
    #[serde(default)]
    pub fx: Vec<FxDesc>,
}

/// The full routing graph, serializable and replayable.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Topology {
    /// Format version. Absent (0) means the pre-layer nesting of decks inside
    /// channels, which cannot be flattened honestly — see `load`.
    #[serde(default)]
    pub version: u32,
    /// Layers, bottom of the stack first.
    #[serde(default)]
    pub layers: Vec<LayerDesc>,
    /// Master FX applied after compositing.
    #[serde(default)]
    pub master_fx: Vec<FxDesc>,
}

/// Current topology format. Bumped when the graph shape changes in a way older
/// files cannot express.
#[cfg(feature = "mixer")]
pub const TOPOLOGY_VERSION: u32 = 1;

#[cfg(feature = "mixer")]
fn default_true() -> bool {
    true
}

/// Crate root, used to relativize/resolve asset paths for portability.
#[cfg(feature = "mixer")]
pub(crate) fn topology_base() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Store `path` relative to `base` when it lives under it; otherwise keep it
/// absolute (e.g. a shader picked from an arbitrary location).
#[cfg(feature = "mixer")]
fn relativize(path: &Path, base: &Path) -> PathBuf {
    path.strip_prefix(base)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

/// Inverse of [`relativize`]: resolve a stored path back against `base`.
#[cfg(feature = "mixer")]
pub(crate) fn resolve(path: &Path, base: &Path) -> PathBuf {
    if path.is_relative() {
        base.join(path)
    } else {
        path.to_path_buf()
    }
}

#[cfg(feature = "mixer")]
impl Topology {
    /// Capture the live routing graph into serializable descriptors.
    ///
    /// FX slots with no recorded `source_path` are skipped — they cannot be
    /// rebuilt from disk — but this should not happen for ISF effects added
    /// through the normal paths.
    /// Capture the live graph.
    ///
    /// `sources` maps a layer's channel uuid to the library entry it was built
    /// from; `rustjay_mixer::Channel` has nowhere to keep it, so the host holds
    /// it alongside and hands it in here.
    pub fn from_mixer(
        mixer: &rustjay_mixer::Mixer,
        sources: &std::collections::HashMap<String, crate::sources::SourceEntry>,
    ) -> Self {
        let base = topology_base();

        let capture_fx = |chain: &[rustjay_mixer::EffectSlot]| -> Vec<FxDesc> {
            chain
                .iter()
                .filter_map(|slot| {
                    let path = slot.source_path.as_ref()?;
                    Some(FxDesc {
                        uuid: slot.uuid.clone(),
                        path: relativize(path, &base),
                        enabled: slot.enabled,
                    })
                })
                .collect()
        };

        let layers = mixer
            .channels
            .iter()
            .map(|ch| {
                let mut source = sources.get(&ch.uuid).cloned().unwrap_or_else(|| {
                    // A layer built outside the library still round-trips as a
                    // solid colour rather than vanishing from the saved scene.
                    crate::sources::SourceEntry {
                        id: ch.uuid.clone(),
                        name: ch.name.clone(),
                        kind: crate::sources::SourceKind::SolidColor,
                        path: None,
                        device_index: 0,
                    }
                });
                if let Some(path) = source.path.take() {
                    source.path = Some(relativize(&path, &base));
                }
                LayerDesc {
                    uuid: ch.uuid.clone(),
                    name: ch.name.clone(),
                    source,
                    opacity: ch.opacity,
                    blend_mode: ch.blend_mode,
                    solo: ch.solo,
                    mute: ch.mute,
                    fx: capture_fx(&ch.chain),
                }
            })
            .collect();

        Self {
            version: TOPOLOGY_VERSION,
            layers,
            master_fx: capture_fx(&mixer.master),
        }
    }
}
