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
    /// Audio-reactivity routes (FFT band → parameter). Lives on `EngineState`
    /// like `modulation`, so the mixer's own snapshot cannot see it — presets
    /// captured it all along, which is why routes survived a preset save but
    /// not a workspace save. `Default` (no routes) for older scenes.
    #[serde(default = "no_routes")]
    pub audio_routing: rustjay_core::AudioRoutingState,
}

/// An empty routing state, so a scene saved before routes were persisted means
/// "no routes recorded" rather than "restore the two built-in defaults" — which
/// would quietly overwrite whatever the user had set up.
fn no_routes() -> rustjay_core::AudioRoutingState {
    rustjay_core::AudioRoutingState {
        matrix: rustjay_core::RoutingMatrix::new(),
        ..Default::default()
    }
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
            audio_routing: no_routes(),
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

    /// Attach a snapshot of the audio routing matrix (chainable).
    pub fn with_audio_routing(mut self, routing: &rustjay_core::AudioRoutingState) -> Self {
        self.audio_routing = routing.clone();
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

/// One bus group: which layers it holds, and how the composite is treated.
///
/// Members are named rather than positioned: a span would have to be recomputed
/// against a stack that may have been restacked since, and the uuids survive
/// that.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GroupDesc {
    /// Stable identity, reproduced on replay so `grp_<uuid>_…` params match.
    pub uuid: String,
    pub name: String,
    /// Member layer uuids, bottom of the group first.
    pub members: Vec<String>,
    pub opacity: f32,
    pub blend_mode: rustjay_mixer::BlendMode,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub collapsed: bool,
    /// The chain the composited members pass through.
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
    /// Bus groups over the layers. Absent in scenes saved before groups
    /// existed, which then load as a flat stack.
    #[serde(default)]
    pub groups: Vec<GroupDesc>,
}

/// One layer saved to the library, to be dropped into any scene later.
///
/// The uuids inside `layer` are the ones it had when saved; recall replaces
/// them so the same saved layer can be added twice without two channels
/// claiming one parameter prefix. `params` is keyed by the *saved* prefixes and
/// is rekeyed to match.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedLayer {
    #[serde(default)]
    pub version: u32,
    /// What the library shows.
    pub name: String,
    /// Source, mix settings, and FX chain.
    pub layer: LayerDesc,
    /// Base values for everything under the layer's prefix.
    #[serde(default)]
    pub params: std::collections::HashMap<String, f32>,
}

#[cfg(feature = "mixer")]
impl SavedLayer {
    /// Capture one live layer, with the params belonging to it.
    ///
    /// `params` is the whole scene's snapshot; only keys under this layer's
    /// `ch_<uuid>_` prefix are kept, so a saved layer carries its own settings
    /// and nothing else.
    pub fn capture(
        name: String,
        layer: LayerDesc,
        params: &std::collections::HashMap<String, f32>,
    ) -> Self {
        let prefix = format!("ch_{}_", layer.uuid);
        Self {
            version: SAVED_LAYER_VERSION,
            name,
            params: params
                .iter()
                .filter(|(k, _)| k.starts_with(&prefix))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            layer,
        }
    }

    /// Rewrite this layer's identity for a fresh instance, returning the params
    /// already rekeyed to match. Called on recall.
    pub fn instantiate(
        &self,
        uuid: &str,
    ) -> (LayerDesc, std::collections::HashMap<String, f32>) {
        let mut layer = self.layer.clone();
        let old_prefix = format!("ch_{}_", layer.uuid);
        let new_prefix = format!("ch_{uuid}_");
        layer.uuid = uuid.to_string();

        // Each FX slot needs a fresh uuid too, for the same reason the layer
        // does — its params live under `…fx<uuid>_`.
        let mut fx_renames = Vec::new();
        for fx in &mut layer.fx {
            let fresh = new_uuid();
            fx_renames.push((format!("fx{}_", fx.uuid), format!("fx{fresh}_")));
            fx.uuid = fresh;
        }

        let params = self
            .params
            .iter()
            .filter_map(|(key, value)| {
                let tail = key.strip_prefix(&old_prefix)?;
                let tail = fx_renames
                    .iter()
                    .find_map(|(from, to)| tail.strip_prefix(from.as_str()).map(|r| format!("{to}{r}")))
                    .unwrap_or_else(|| tail.to_string());
                Some((format!("{new_prefix}{tail}"), *value))
            })
            .collect();

        (layer, params)
    }
}

/// The master FX chain saved to the library, to be recalled into any scene.
///
/// The master chain is the one every layer passes through on the way out, so
/// it is worth keeping independently of the layers that feed it.
#[cfg(feature = "mixer")]
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedChain {
    #[serde(default)]
    pub version: u32,
    pub name: String,
    /// Ordered FX, as saved.
    pub fx: Vec<FxDesc>,
    /// Base values for everything under `master_fx<uuid>_`.
    #[serde(default)]
    pub params: std::collections::HashMap<String, f32>,
}

#[cfg(feature = "mixer")]
impl SavedChain {
    /// Capture the live master chain and the params belonging to it.
    pub fn capture(
        name: String,
        fx: Vec<FxDesc>,
        params: &std::collections::HashMap<String, f32>,
    ) -> Self {
        let keys: Vec<String> = fx.iter().map(|f| format!("master_fx{}_", f.uuid)).collect();
        Self {
            version: SAVED_LAYER_VERSION,
            name,
            params: params
                .iter()
                .filter(|(k, _)| keys.iter().any(|p| k.starts_with(p)))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            fx,
        }
    }

    /// Fresh uuids for a new instance, with the params rekeyed to match, so the
    /// same saved chain can be recalled twice without two slots claiming one
    /// parameter prefix.
    pub fn instantiate(&self) -> (Vec<FxDesc>, std::collections::HashMap<String, f32>) {
        let mut fx = self.fx.clone();
        let mut renames = Vec::new();
        for slot in &mut fx {
            let fresh = new_uuid();
            renames.push((
                format!("master_fx{}_", slot.uuid),
                format!("master_fx{fresh}_"),
            ));
            slot.uuid = fresh;
        }
        let params = self
            .params
            .iter()
            .filter_map(|(key, value)| {
                renames.iter().find_map(|(from, to)| {
                    key.strip_prefix(from.as_str())
                        .map(|tail| (format!("{to}{tail}"), *value))
                })
            })
            .collect();
        (fx, params)
    }
}

/// Short identity, matching the form used for layers and FX slots elsewhere.
#[cfg(feature = "mixer")]
pub fn new_uuid() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

/// Current saved-layer format.
#[cfg(feature = "mixer")]
pub const SAVED_LAYER_VERSION: u32 = 1;

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
            groups: mixer
                .groups
                .iter()
                .map(|g| GroupDesc {
                    uuid: g.uuid.clone(),
                    name: g.name.clone(),
                    members: mixer
                        .group_members(&g.uuid)
                        .into_iter()
                        .map(|i| mixer.channels[i].uuid.clone())
                        .collect(),
                    opacity: g.opacity,
                    blend_mode: g.blend_mode,
                    solo: g.solo,
                    mute: g.mute,
                    collapsed: g.collapsed,
                    fx: capture_fx(&g.chain),
                })
                .collect(),
        }
    }
}

#[cfg(all(test, feature = "mixer"))]
mod tests {
    use super::*;
    use rustjay_core::routing::{FftBand, ModulationTarget};

    fn scene_with_routes(n: usize) -> Scene {
        let mut routing = no_routes();
        for _ in 0..n {
            routing
                .matrix
                .add_route(FftBand::Mid, ModulationTarget::Brightness);
        }
        Scene::from_mixer(&rustjay_mixer::Mixer::new(), &Default::default())
            .with_audio_routing(&routing)
    }

    #[test]
    fn audio_routes_survive_a_scene_round_trip() {
        let json = serde_json::to_string(&scene_with_routes(3)).expect("serialise");
        let back: Scene = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.audio_routing.matrix.len(), 3);
    }

    fn desc_with_fx(uuid: &str, fx: &[&str]) -> LayerDesc {
        LayerDesc {
            uuid: uuid.to_string(),
            name: "Cam + Blur".into(),
            source: crate::sources::SourceEntry {
                id: "cam".into(),
                name: "Camera".into(),
                kind: crate::sources::SourceKind::Camera,
                path: None,
                device_index: 0,
            },
            opacity: 0.5,
            blend_mode: rustjay_mixer::BlendMode::Add,
            solo: false,
            mute: true,
            fx: fx
                .iter()
                .map(|u| FxDesc {
                    uuid: (*u).to_string(),
                    path: "blur.fs".into(),
                    enabled: true,
                })
                .collect(),
        }
    }

    #[test]
    fn a_saved_layer_keeps_only_its_own_params() {
        let mut params = std::collections::HashMap::new();
        params.insert("ch_aaa_opacity".to_string(), 0.25);
        params.insert("ch_aaa_fx111_amount".to_string(), 0.75);
        params.insert("ch_bbb_opacity".to_string(), 1.0); // another layer
        let saved = SavedLayer::capture("Cam".into(), desc_with_fx("aaa", &["111"]), &params);
        assert_eq!(saved.params.len(), 2);
        assert!(saved.params.keys().all(|k| k.starts_with("ch_aaa_")));
    }

    /// Recall must not reuse the saved identity, or adding the same layer twice
    /// would give two channels one parameter prefix.
    #[test]
    fn recall_rekeys_the_layer_and_every_fx_slot() {
        let mut params = std::collections::HashMap::new();
        params.insert("ch_aaa_opacity".to_string(), 0.25);
        params.insert("ch_aaa_fx111_amount".to_string(), 0.75);
        params.insert("ch_aaa_fx222_amount".to_string(), 0.5);
        let saved =
            SavedLayer::capture("Cam".into(), desc_with_fx("aaa", &["111", "222"]), &params);

        let (desc, keyed) = saved.instantiate("zzz");
        assert_eq!(desc.uuid, "zzz");
        assert_eq!(desc.opacity, 0.5, "mix settings come back");
        assert!(desc.mute);
        assert_eq!(desc.fx.len(), 2);

        // Fresh slot ids, and the params follow them.
        assert!(desc.fx.iter().all(|f| f.uuid != "111" && f.uuid != "222"));
        assert_eq!(keyed.len(), 3);
        assert_eq!(keyed.get("ch_zzz_opacity"), Some(&0.25));
        assert_eq!(
            keyed.get(&format!("ch_zzz_fx{}_amount", desc.fx[0].uuid)),
            Some(&0.75)
        );
        assert_eq!(
            keyed.get(&format!("ch_zzz_fx{}_amount", desc.fx[1].uuid)),
            Some(&0.5)
        );

        // Two recalls must not collide.
        let (other, _) = saved.instantiate("yyy");
        assert_ne!(other.fx[0].uuid, desc.fx[0].uuid);
    }

    #[test]
    fn a_saved_layer_round_trips_through_json() {
        let saved = SavedLayer::capture("Cam".into(), desc_with_fx("aaa", &["111"]), &Default::default());
        let json = serde_json::to_string(&saved).expect("serialise");
        let back: SavedLayer = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back.name, "Cam");
        assert_eq!(back.layer.fx.len(), 1);
        assert_eq!(back.layer.blend_mode, rustjay_mixer::BlendMode::Add);
    }

    fn chain(fx: &[&str]) -> Vec<FxDesc> {
        fx.iter()
            .map(|u| FxDesc {
                uuid: (*u).to_string(),
                path: "glow.fs".into(),
                enabled: true,
            })
            .collect()
    }

    #[test]
    fn a_saved_chain_keeps_only_master_params() {
        let mut params = std::collections::HashMap::new();
        params.insert("master_fx11_amount".to_string(), 0.4);
        params.insert("ch_aaa_fx22_amount".to_string(), 0.9); // a layer's FX
        let saved = SavedChain::capture("Master 1".into(), chain(&["11"]), &params);
        assert_eq!(saved.params.len(), 1);
        assert!(saved.params.contains_key("master_fx11_amount"));
    }

    /// Recalling the same chain twice must not give two slots one prefix.
    #[test]
    fn recalling_a_chain_rekeys_every_slot() {
        let mut params = std::collections::HashMap::new();
        params.insert("master_fx11_amount".to_string(), 0.4);
        params.insert("master_fx22_amount".to_string(), 0.7);
        let saved = SavedChain::capture("Master 1".into(), chain(&["11", "22"]), &params);

        let (fx, keyed) = saved.instantiate();
        assert_eq!(fx.len(), 2);
        assert!(fx.iter().all(|f| f.uuid != "11" && f.uuid != "22"));
        assert_eq!(
            keyed.get(&format!("master_fx{}_amount", fx[0].uuid)),
            Some(&0.4)
        );
        assert_eq!(
            keyed.get(&format!("master_fx{}_amount", fx[1].uuid)),
            Some(&0.7)
        );

        let (again, _) = saved.instantiate();
        assert_ne!(again[0].uuid, fx[0].uuid, "two recalls do not collide");
    }

    #[test]
    fn a_group_survives_a_topology_round_trip() {
        let topo = Topology {
            version: TOPOLOGY_VERSION,
            layers: vec![desc_with_fx("a", &[]), desc_with_fx("b", &[])],
            master_fx: Vec::new(),
            groups: vec![GroupDesc {
                uuid: "g1".into(),
                name: "Backdrop".into(),
                members: vec!["a".into(), "b".into()],
                opacity: 0.4,
                blend_mode: rustjay_mixer::BlendMode::Add,
                solo: false,
                mute: true,
                collapsed: true,
                fx: chain(&["f1"]),
            }],
        };

        let json = serde_json::to_string(&topo).expect("serialise");
        let back: Topology = serde_json::from_str(&json).expect("deserialise");

        assert_eq!(back.groups.len(), 1);
        let g = &back.groups[0];
        assert_eq!(g.uuid, "g1", "the uuid comes back, so grp_ params still match");
        assert_eq!(g.members, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(g.opacity, 0.4);
        assert!(g.mute && g.collapsed);
        assert_eq!(g.fx.len(), 1, "the group's own chain came with it");
    }

    /// A scene from before groups existed loads as a flat stack rather than
    /// failing.
    #[test]
    fn a_topology_without_groups_still_loads() {
        let json = r#"{"version":1,"layers":[],"master_fx":[]}"#;
        let topo: Topology = serde_json::from_str(json).expect("older scene loads");
        assert!(topo.groups.is_empty());
    }

    /// A scene written before routes were persisted must not look like it
    /// carried the two built-in defaults, or loading it would overwrite the
    /// routes already set up.
    #[test]
    fn a_scene_without_routes_restores_none() {
        let json = serde_json::to_string(&scene_with_routes(2)).expect("serialise");
        let mut value: serde_json::Value = serde_json::from_str(&json).expect("as value");
        value
            .as_object_mut()
            .expect("object")
            .remove("audio_routing");
        let back: Scene = serde_json::from_value(value).expect("deserialise");
        assert!(back.audio_routing.matrix.is_empty());
    }
}
