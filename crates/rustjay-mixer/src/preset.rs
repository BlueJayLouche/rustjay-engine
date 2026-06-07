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
/// Hard cap on modulation sources a preset may declare (SEC-1).
pub const MAX_MOD_SOURCES: usize = 64;
/// Hard cap on total modulation assignment entries a preset may declare (SEC-1).
pub const MAX_MOD_ASSIGNMENTS: usize = 256;

/// Current [`MixerState`] schema version. Bump on breaking format changes.
pub const MIXER_STATE_VERSION: u32 = 2;

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
        if state.modulation.sources.len() > MAX_MOD_SOURCES {
            return Err(format!(
                "mixer preset declares {} modulation sources (max {MAX_MOD_SOURCES})",
                state.modulation.sources.len()
            ));
        }
        let total_assignments: usize = state.modulation.assignments.values().map(|v| v.len()).sum();
        if total_assignments > MAX_MOD_ASSIGNMENTS {
            return Err(format!(
                "mixer preset declares {total_assignments} modulation assignments (max {MAX_MOD_ASSIGNMENTS})"
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
            modulation: ModulationEngine::default(),
        }
    }

    /// Restore mix settings from a [`MixerState`] onto the live channel set.
    ///
    /// Channels are matched by UUID; values are clamped into range. Returns
    /// `(matched_count, legacy_modulation)` where `legacy_modulation` is
    /// `Some(engine)` only when the preset was written before version
    /// [`MIXER_STATE_VERSION`] and carried non-empty modulation data. Callers
    /// with access to `EngineState.modulation` should merge the returned engine
    /// via `add_source_with_uuid` / `assign` (REQ-10 / UNIFIED_MODULATION_ROADMAP M4.5).
    pub fn apply_state(&mut self, state: &MixerState) -> (usize, Option<ModulationEngine>) {
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

        // v1 presets carried modulation in the mixer; hand it back to the caller
        // so it can be merged into the unified EngineState.modulation.
        let legacy = if state.version < MIXER_STATE_VERSION && state.modulation.source_count() > 0 {
            let mut eng = state.modulation.clone();
            eng.ensure_index();
            Some(eng)
        } else {
            None
        };

        (matched, legacy)
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
        m.add_channel(Channel::new("a", "A", Box::new(Stub)))
            .unwrap();
        m.add_channel(Channel::new("b", "B", Box::new(Stub)))
            .unwrap();
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
        assert_eq!(restored.apply_state(&state).0, 2);

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
        assert_eq!(m.apply_state(&state).0, 0);
        assert_eq!(m.channels[0].opacity, before);
    }

    #[test]
    fn round_trip_modulation_state() {
        // Since the mixer no longer owns a ModulationEngine (Phase 4),
        // serialize_state writes an empty engine. The MixerState wire format
        // still carries a `modulation` field for backward compat, so old
        // presets deserialize without error, but the mixer ignores it on load.
        let m = mixer_ab();
        let json = m.serialize_state().to_json().unwrap();

        let state = MixerState::from_json(&json).unwrap();
        // Field is present but empty because mixer no longer stores modulation.
        assert_eq!(state.modulation.source_count(), 0);

        // Old presets with modulation still parse.
        let json_with_mod = r#"{"version":1,"crossfader":0.5,"channels":[
            {"uuid":"a","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false},
            {"uuid":"b","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false}
        ],"modulation":{"sources":[{"uuid":"lfo_1","source":{"LFO":{"waveform":"Sine","frequency":1.0,"phase":0.0,"amplitude":1.0,"bipolar":true,"tempo_sync":false,"division":2,"phase_offset_degrees":0.0,"last_beat_phase":0.0}}}],"assignments":{"crossfader":[{"source_id":"lfo_1","amount":0.5,"component":null}]}}}"#;
        let state_old = MixerState::from_json(json_with_mod).unwrap();
        assert_eq!(state_old.modulation.source_count(), 1);
        assert!(state_old.modulation.has_modulation("crossfader"));
    }

    #[test]
    fn v1_migration_returns_legacy_modulation() {
        // v1 preset with a non-empty modulation block should be handed back to
        // the caller so it can be merged into EngineState.modulation.
        let json_v1 = r#"{"version":1,"crossfader":0.5,"channels":[
            {"uuid":"a","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false},
            {"uuid":"b","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false}
        ],"modulation":{"sources":[{"uuid":"lfo_1","source":{"LFO":{"waveform":"Sine","frequency":1.0,"phase":0.0,"amplitude":1.0,"bipolar":true,"tempo_sync":false,"division":2,"phase_offset_degrees":0.0,"last_beat_phase":0.0}}}],"assignments":{"crossfader":[{"source_id":"lfo_1","amount":0.5,"component":null}]}}}"#;
        let state = MixerState::from_json(json_v1).unwrap();
        let mut m = mixer_ab();
        let (matched, legacy) = m.apply_state(&state);
        assert_eq!(matched, 2);
        let legacy = legacy.expect("v1 preset with modulation should return legacy engine");
        assert_eq!(legacy.source_count(), 1);
        assert!(legacy.has_modulation("crossfader"));
    }

    #[test]
    fn v2_preset_returns_no_legacy_modulation() {
        // v2 (current) presets never carry modulation; apply_state returns None.
        let m = mixer_ab();
        let json = m.serialize_state().to_json().unwrap();
        let state = MixerState::from_json(&json).unwrap();
        let mut m2 = mixer_ab();
        let (_, legacy) = m2.apply_state(&state);
        assert!(legacy.is_none(), "current-version preset should not return legacy engine");
    }

    #[test]
    fn v1_preset_with_empty_modulation_returns_no_legacy() {
        // S2: v1 preset with an empty modulation block should return None,
        // exercising the version < VERSION && source_count() == 0 branch.
        let json_v1_empty = r#"{"version":1,"crossfader":0.5,"channels":[
            {"uuid":"a","opacity":1.0,"blend_mode":"Normal","solo":false,"mute":false}
        ],"modulation":{"sources":[],"assignments":{}}}"#;
        let state = MixerState::from_json(json_v1_empty).unwrap();
        let mut m = mixer_ab();
        let (_, legacy) = m.apply_state(&state);
        assert!(
            legacy.is_none(),
            "v1 preset with zero modulation sources should not return legacy engine"
        );
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
