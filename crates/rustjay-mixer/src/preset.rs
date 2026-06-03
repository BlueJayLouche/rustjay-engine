//! Mixer preset (de)serialization — T18 / REQ-10.
//!
//! [`MixerState`] is the wire format for a mixer's *mix topology*: which
//! channels exist (by stable UUID), their opacity, blend mode, solo/mute, and
//! the crossfader position. It is intentionally decoupled from the runtime
//! [`Mixer`](crate::Mixer)/[`Channel`](crate::Channel) types so the serialized
//! shape can stay stable as the runtime structs evolve.
//!
//! ## What is and isn't captured
//!
//! - **Captured here:** crossfader + per-channel mix settings, matched back to
//!   live channels by UUID on load (REQ-10.1).
//! - **Captured by the engine's main preset:** every nested channel/master
//!   effect *parameter value*. T08 aggregates all of them as engine parameters
//!   (`ch_{uuid}_…`, `master_fx{k}_…`), so `Preset::from_state` already snapshots
//!   them — restoring per-effect parameters (REQ-10.2) without this module
//!   needing an `EffectInstance` serialization hook.
//! - **Not captured (by design):** channel *topology construction* (which effect
//!   type each channel holds). Reconstructing arbitrary `Box<dyn EffectInstance>`
//!   from a string needs an effect registry the engine doesn't have. Presets
//!   therefore restore mix state onto the **already-built** channel set, matched
//!   by UUID — the standard "snapshot the knobs" preset model. Channels in the
//!   preset with no matching live UUID are skipped; live channels absent from the
//!   preset keep their current state.
//!
//! ## Bounded deserialization (REQ-10.3 / AUDIT_ROADMAP 2.1)
//!
//! [`MixerState::from_json`] rejects payloads declaring more than
//! [`MAX_CHANNELS`] channels *before* any work, and [`Mixer::apply_state`]
//! clamps every restored value into range, so a malformed preset can neither
//! over-allocate nor push parameters out of bounds.

use serde::{Deserialize, Serialize};

use crate::{BlendMode, Mixer};
use rustjay_core::modulation::ModulationEngine;

/// Hard cap on channels a preset may declare. Mirrors the runtime limit enforced
/// by [`Mixer::add_channel`](crate::Mixer::add_channel).
pub const MAX_CHANNELS: usize = 8;

/// Current [`MixerState`] schema version. Bump on breaking format changes.
pub const MIXER_STATE_VERSION: u32 = 1;

/// Serialized mix settings for one channel, keyed by stable UUID.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChannelState {
    /// Stable channel identity (matches [`Channel::uuid`](crate::Channel::uuid)).
    pub uuid: String,
    /// Mix opacity, 0.0–1.0.
    pub opacity: f32,
    /// Blend mode (serialized by variant name).
    pub blend_mode: BlendMode,
    /// Solo flag.
    pub solo: bool,
    /// Mute flag.
    pub mute: bool,
}

/// Serializable snapshot of a [`Mixer`]'s mix state (REQ-10.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MixerState {
    /// Schema version for forward/backward compatibility.
    pub version: u32,
    /// Crossfader position, 0.0–1.0.
    pub crossfader: f32,
    /// Per-channel mix settings.
    pub channels: Vec<ChannelState>,
    /// UUID-stable modulation sources and assignments (T13).
    #[serde(default)]
    pub modulation: ModulationEngine,
}

impl MixerState {
    /// Parse a JSON preset payload with bounded validation (REQ-10.3).
    ///
    /// Rejects malformed JSON and any payload declaring more than
    /// [`MAX_CHANNELS`] channels, so no oversized allocation or GPU work can be
    /// driven by an untrusted file.
    pub fn from_json(data: &str) -> Result<Self, String> {
        let state: MixerState =
            serde_json::from_str(data).map_err(|e| format!("invalid mixer preset JSON: {e}"))?;
        if state.channels.len() > MAX_CHANNELS {
            return Err(format!(
                "mixer preset declares {} channels (max {MAX_CHANNELS})",
                state.channels.len()
            ));
        }
        Ok(state)
    }

    /// Serialize to a compact JSON string.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| format!("failed to serialize mixer preset: {e}"))
    }
}

impl Mixer {
    /// Snapshot the current mix state for preset storage (REQ-10.1).
    pub fn serialize_state(&self) -> MixerState {
        MixerState {
            version: MIXER_STATE_VERSION,
            crossfader: self.crossfader,
            channels: self
                .channels
                .iter()
                .map(|ch| ChannelState {
                    uuid: ch.uuid.clone(),
                    opacity: ch.opacity,
                    blend_mode: ch.blend_mode,
                    solo: ch.solo,
                    mute: ch.mute,
                })
                .collect(),
            modulation: self.modulation.clone(),
        }
    }

    /// Restore mix settings from a [`MixerState`] onto the live channel set.
    ///
    /// Channels are matched by UUID; values are clamped into range. Returns the
    /// number of channels matched (for logging). Does not allocate or touch GPU
    /// resources — only the live channels' mix settings and the crossfader.
    pub fn apply_state(&mut self, state: &MixerState) -> usize {
        self.crossfader = state.crossfader.clamp(0.0, 1.0);

        let mut matched = 0;
        for cs in &state.channels {
            if let Some(ch) = self.channels.iter_mut().find(|c| c.uuid == cs.uuid) {
                ch.opacity = cs.opacity.clamp(0.0, 1.0);
                ch.blend_mode = cs.blend_mode;
                ch.solo = cs.solo;
                ch.mute = cs.mute;
                matched += 1;
            }
        }

        // Restore modulation state (T13).
        self.modulation = state.modulation.clone();
        self.modulation.ensure_index();

        matched
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Channel;
    use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};

    struct Stub;
    impl EffectInstance for Stub {
        fn render_to(
            &mut self,
            _ctx: &mut RenderCtx<'_>,
            _inputs: &[EffectInput<'_>],
            _target: RenderTarget<'_>,
            _engine: &EngineState,
        ) {
        }
    }

    fn mixer_ab() -> Mixer {
        let mut m = Mixer::new();
        m.add_channel(Channel::new("a", "A", Box::new(Stub))).unwrap();
        m.add_channel(Channel::new("b", "B", Box::new(Stub))).unwrap();
        m
    }

    #[test]
    fn round_trips_mix_state() {
        let mut m = mixer_ab();
        m.crossfader = 0.3;
        m.channels[0].opacity = 0.4;
        m.channels[0].blend_mode = BlendMode::Add;
        m.channels[1].mute = true;

        let json = m.serialize_state().to_json().unwrap();

        // Apply onto a fresh mixer with the same topology.
        let mut restored = mixer_ab();
        let state = MixerState::from_json(&json).unwrap();
        assert_eq!(restored.apply_state(&state), 2);

        assert!((restored.crossfader - 0.3).abs() < 1e-6);
        assert!((restored.channels[0].opacity - 0.4).abs() < 1e-6);
        assert_eq!(restored.channels[0].blend_mode, BlendMode::Add);
        assert!(restored.channels[1].mute);
    }

    #[test]
    fn rejects_oversized_preset() {
        let chans: Vec<String> = (0..MAX_CHANNELS + 1)
            .map(|i| {
                format!(
                    r#"{{"uuid":"{i}","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false}}"#
                )
            })
            .collect();
        let json = format!(
            r#"{{"version":1,"crossfader":0.5,"channels":[{}]}}"#,
            chans.join(",")
        );
        assert!(MixerState::from_json(&json).is_err());
    }

    #[test]
    fn clamps_out_of_range_values() {
        let json = r#"{"version":1,"crossfader":5.0,"channels":[
            {"uuid":"a","opacity":9.0,"blend_mode":"Normal","solo":false,"mute":false}
        ]}"#;
        let mut m = mixer_ab();
        let state = MixerState::from_json(json).unwrap();
        m.apply_state(&state);
        assert_eq!(m.crossfader, 1.0);
        assert_eq!(m.channels[0].opacity, 1.0);
    }

    #[test]
    fn unmatched_channels_are_skipped() {
        let json = r#"{"version":1,"crossfader":0.5,"channels":[
            {"uuid":"ghost","opacity":0.1,"blend_mode":"Multiply","solo":false,"mute":false}
        ]}"#;
        let mut m = mixer_ab();
        let before = m.channels[0].opacity;
        let state = MixerState::from_json(json).unwrap();
        assert_eq!(m.apply_state(&state), 0);
        assert_eq!(m.channels[0].opacity, before);
    }

    #[test]
    fn round_trip_modulation_state() {
        let mut m = mixer_ab();
        let lfo = m.modulation.add_source(rustjay_core::ModulationSource::sine_lfo(1.0));
        m.modulation.assign("crossfader", &lfo, 0.5, None);
        m.modulation.assign("ch_a_opacity", &lfo, 0.25, None);
        m.modulation.assign("ch_b_opacity", &lfo, 0.25, None);

        let json = m.serialize_state().to_json().unwrap();

        // Restore onto a fresh mixer.
        let mut restored = mixer_ab();
        let state = MixerState::from_json(&json).unwrap();
        restored.apply_state(&state);

        assert_eq!(restored.modulation.source_count(), 1);
        assert!(restored.modulation.has_modulation("crossfader"));
        assert!(restored.modulation.has_modulation("ch_a_opacity"));
        assert!(restored.modulation.has_modulation("ch_b_opacity"));
    }

    #[test]
    fn backward_compat_missing_modulation_field() {
        // Old presets without the modulation field should deserialize cleanly.
        let json = r#"{"version":1,"crossfader":0.5,"channels":[
            {"uuid":"a","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false}
        ]}"#;
        let state = MixerState::from_json(json).unwrap();
        assert_eq!(state.crossfader, 0.5);
        assert_eq!(state.modulation.source_count(), 0);
    }
}
