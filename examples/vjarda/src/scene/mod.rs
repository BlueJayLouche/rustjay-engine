//! Scene — the full runtime state of the show.
//!
//! Channels, decks, effects, modulation, crossfader, and sequences.
//! Persisted as `.varda/scene.json`.
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
}

#[cfg(feature = "mixer")]
impl Scene {
    /// Snapshot from the live mixer (knobs + topology).
    pub fn from_mixer(mixer: &rustjay_mixer::Mixer) -> Self {
        Self {
            version: 2,
            mixer_state: mixer.serialize_state(),
            sequencer: mixer.sequencer.clone(),
            topology: Some(Topology::from_mixer(mixer)),
        }
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

/// One deck: a source plus its post-source FX chain and mix settings.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeckDesc {
    /// Stable deck identity — reproduced on replay so the deck's param prefix
    /// (`ch_<ch>_deck_<uuid>_`) matches its saved modulation.
    pub uuid: String,
    /// Display name.
    pub name: String,
    /// The library descriptor the source was built from (kind, path, device
    /// index, …). Paths are stored relative to the crate root when possible.
    pub source: crate::sources::SourceEntry,
    /// Base mix opacity.
    pub opacity: f32,
    /// Blend mode onto the channel composite.
    pub blend_mode: rustjay_mixer::BlendMode,
    /// Ordered post-source FX.
    #[serde(default)]
    pub fx: Vec<FxDesc>,
}

/// One channel: an ordered deck list plus a post-compositor FX chain.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelDesc {
    /// Stable channel identity — reproduced on replay so param prefixes
    /// (`ch_<uuid>_…`) match saved modulation.
    pub uuid: String,
    /// Display name.
    pub name: String,
    /// Decks, composited in order.
    #[serde(default)]
    pub decks: Vec<DeckDesc>,
    /// Channel post-compositor FX.
    #[serde(default)]
    pub fx: Vec<FxDesc>,
}

/// The full routing graph, serializable and replayable.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Topology {
    /// Channels in mix order.
    #[serde(default)]
    pub channels: Vec<ChannelDesc>,
    /// Master FX applied after compositing.
    #[serde(default)]
    pub master_fx: Vec<FxDesc>,
}

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
    pub fn from_mixer(mixer: &rustjay_mixer::Mixer) -> Self {
        let base = topology_base();

        // Build an FxDesc list from a mixer effect chain.
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

        let mut channels = Vec::new();
        for ch in &mixer.channels {
            let mut decks = Vec::new();
            if let Some(compositor) = ch
                .effect
                .as_any()
                .and_then(|a| a.downcast_ref::<crate::graph::DeckCompositor>())
            {
                for deck in &compositor.decks {
                    // Prefer the recorded library entry; fall back to a minimal
                    // synthesized one for decks built without a registry entry.
                    let mut source = deck.source_entry.clone().unwrap_or_else(|| {
                        crate::sources::SourceEntry {
                            id: deck.uuid.clone(),
                            name: deck.name.clone(),
                            kind: deck.source_kind,
                            path: deck.source_path.clone(),
                            device_index: 0,
                        }
                    });
                    if let Some(p) = source.path.take() {
                        source.path = Some(relativize(&p, &base));
                    }
                    decks.push(DeckDesc {
                        uuid: deck.uuid.clone(),
                        name: deck.name.clone(),
                        source,
                        opacity: deck.opacity,
                        blend_mode: deck.blend_mode,
                        fx: capture_fx(&deck.chain),
                    });
                }
            }
            channels.push(ChannelDesc {
                uuid: ch.uuid.clone(),
                name: ch.name.clone(),
                decks,
                fx: capture_fx(&ch.chain),
            });
        }

        Self {
            channels,
            master_fx: capture_fx(&mixer.master),
        }
    }
}
