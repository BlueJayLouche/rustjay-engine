//! Scene — the full runtime state of the show.
//!
//! Channels, decks, effects, modulation, crossfader, and sequences.
//! Persisted as `.varda/scene.json`.

use serde::{Deserialize, Serialize};

/// Scene snapshot: mix settings + sequencer, decoupled from GPU topology.
///
/// Deck/channel topology is rebuilt from the app's default assembly on load;
/// this struct restores the *knobs* (crossfader, opacities, blends, modulation,
/// sequencer) onto that topology.
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
}

#[cfg(feature = "mixer")]
impl Scene {
    /// Snapshot from the live mixer.
    pub fn from_mixer(mixer: &rustjay_mixer::Mixer) -> Self {
        Self {
            version: 1,
            mixer_state: mixer.serialize_state(),
            sequencer: mixer.sequencer.clone(),
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
