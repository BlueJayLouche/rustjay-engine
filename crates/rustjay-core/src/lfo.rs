//! # LFO (Low Frequency Oscillator) System
//!
//! 3 LFOs - one for each HSB parameter (Hue, Saturation, Brightness)
//! Tempo-syncable with phase offset support

use crate::params::{ParamType, ParameterDescriptor};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::f32::consts::PI;

/// Beat division multipliers for tempo sync
/// Represent cycle duration in beats (smaller = faster)
pub const BEAT_DIVISIONS: [f32; 8] = [
    0.0625, // 1/16
    0.125,  // 1/8
    0.25,   // 1/4
    0.5,    // 1/2
    1.0,    // 1 beat
    2.0,    // 2 beats
    4.0,    // 4 beats
    8.0,    // 8 beats
];

/// Beat division names for UI
pub const BEAT_DIVISION_NAMES: [&str; 8] = ["1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"];

/// Convert beat division index to frequency in Hz for a given BPM
pub fn beat_division_to_hz(division: usize, bpm: f32) -> f32 {
    let division = division.min(BEAT_DIVISIONS.len() - 1);
    let beats_per_cycle = BEAT_DIVISIONS[division];
    let beat_duration = 60.0 / bpm.max(1.0);
    let cycle_duration = beat_duration * beats_per_cycle;
    1.0 / cycle_duration
}

/// LFO Waveforms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Waveform {
    /// Sinusoidal wave.
    #[default]
    Sine = 0,
    /// Triangle wave.
    Triangle = 1,
    /// Upward ramp (0 → 1).
    Ramp = 2,
    /// Downward saw (1 → -1).
    Saw = 3,
    /// Square wave.
    Square = 4,
}

impl Waveform {
    /// Human-readable waveform name.
    pub fn name(&self) -> &'static str {
        match self {
            Waveform::Sine => "Sine",
            Waveform::Triangle => "Triangle",
            Waveform::Ramp => "Ramp",
            Waveform::Saw => "Saw",
            Waveform::Square => "Square",
        }
    }

    /// All supported waveforms.
    pub fn all() -> &'static [Waveform] {
        &[
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Ramp,
            Waveform::Saw,
            Waveform::Square,
        ]
    }
}

/// Target parameter for LFO modulation.
///
/// Uses `#[repr(i8)]` so that explicit discriminants work alongside
/// the `Custom(String)` variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(i8)]
#[derive(Default)]
pub enum LfoTarget {
    /// No modulation target.
    #[default]
    None = -1,
    /// Modulate hue shift.
    HueShift = 0,
    /// Modulate saturation.
    Saturation = 1,
    /// Modulate brightness.
    Brightness = 2,
    /// Modulate an effect-declared custom parameter.
    Custom(String),
    /// Unrecognised target from an older preset file.
    /// Treated as `None` — the LFO is preserved but has no effect.
    #[serde(other)]
    Unknown,
}

impl LfoTarget {
    /// Human-readable target name.
    pub fn name(&self) -> String {
        match self {
            LfoTarget::None => "None".to_string(),
            LfoTarget::HueShift => "Hue Shift".to_string(),
            LfoTarget::Saturation => "Saturation".to_string(),
            LfoTarget::Brightness => "Brightness".to_string(),
            LfoTarget::Custom(id) => id.clone(),
            LfoTarget::Unknown => "(unknown)".to_string(),
        }
    }

    /// All static modulation targets (HSB only, excludes `None` and `Unknown`).
    /// For backward compatibility.
    pub fn all() -> &'static [LfoTarget] {
        &[
            LfoTarget::HueShift,
            LfoTarget::Saturation,
            LfoTarget::Brightness,
        ]
    }

    /// Generate the full list of LFO targets for a set of parameter descriptors.
    /// Includes HSB targets + one target per modulatable custom parameter.
    pub fn all_for(descriptors: &[ParameterDescriptor]) -> Vec<LfoTarget> {
        let mut targets: Vec<LfoTarget> = Self::all().to_vec();
        for d in descriptors {
            if matches!(d.param_type, ParamType::Float | ParamType::Int) {
                targets.push(LfoTarget::Custom(d.id.clone()));
            }
        }
        targets.push(LfoTarget::None);
        targets
    }

    /// Get the parameter id for this target (if it's a custom target).
    pub fn param_id(&self) -> Option<&str> {
        match self {
            LfoTarget::HueShift => Some("hue_shift"),
            LfoTarget::Saturation => Some("saturation"),
            LfoTarget::Brightness => Some("brightness"),
            LfoTarget::Custom(id) => Some(id),
            _ => None,
        }
    }
}

/// Single LFO configuration and state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lfo {
    /// LFO index (0–7)
    pub index: usize,
    /// Whether this LFO is enabled
    pub enabled: bool,
    /// Target parameter to modulate
    pub target: LfoTarget,
    /// Waveform type
    pub waveform: Waveform,
    /// Amplitude (-1.0 to 1.0)
    pub amplitude: f32,
    /// Whether tempo sync is enabled
    pub tempo_sync: bool,
    /// Beat division index (0-7)
    pub division: usize,
    /// Free rate in Hz (when not tempo synced)
    pub rate: f32,
    /// Phase offset in degrees (0-360)
    pub phase_offset: f32,
    /// Current phase (0-1), not serialized
    #[serde(skip)]
    pub phase: f32,
    /// Current output value (-1.0 to 1.0), not serialized
    #[serde(skip)]
    pub output: f32,
    /// Previous beat_phase sample — used to detect quantum-boundary crossings
    #[serde(skip)]
    pub last_beat_phase: f32,
}

impl Lfo {
    /// Create a new LFO with default settings
    pub fn new(index: usize) -> Self {
        let target = match index {
            0 => LfoTarget::HueShift,
            1 => LfoTarget::Saturation,
            2 => LfoTarget::Brightness,
            _ => LfoTarget::None,
        };

        Self {
            index,
            enabled: false,
            target,
            waveform: Waveform::Sine,
            amplitude: 0.5,
            tempo_sync: true,
            division: 2, // 1/4 note default
            rate: 1.0,   // 1 Hz default
            phase_offset: 0.0,
            phase: 0.0,
            output: 0.0,
            last_beat_phase: 0.0,
        }
    }

    /// Calculate the LFO output at current phase
    pub fn calculate_value(phase: f32, waveform: Waveform) -> f32 {
        if !phase.is_finite() {
            return 0.0;
        }
        let phase = phase % 1.0;

        match waveform {
            Waveform::Sine => (phase * 2.0 * PI).sin(),
            Waveform::Triangle => {
                if phase < 0.25 {
                    4.0 * phase
                } else if phase < 0.75 {
                    2.0 - 4.0 * phase
                } else {
                    4.0 * phase - 4.0
                }
            }
            Waveform::Ramp => 2.0 * phase - 1.0, // -1 to 1 upward
            Waveform::Saw => 1.0 - 2.0 * phase,  // 1 to -1 downward
            Waveform::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }

    /// Update LFO phase based on time/BPM.
    ///
    /// `beat_phase` is a 0→1 ramp from the active sync source (Link quantum,
    /// ProDJ bar, or audio beat detector).  It is used only as a snap trigger:
    /// when it wraps from ~1 back to ~0 (a new quantum started) we realign
    /// `self.phase` for sub-beat/single-beat divisions so the LFO stays
    /// musically in phase.  It is NOT added to `self.phase` — doing so would
    /// make the LFO run faster than the selected division because both
    /// `self.phase` and `beat_phase` advance at the tempo rate.
    pub fn update(&mut self, bpm: f32, delta_time: f32, beat_phase: f32) {
        if !self.enabled || self.target == LfoTarget::None {
            self.output = 0.0;
            return;
        }

        let division = self.division.clamp(0, BEAT_DIVISIONS.len() - 1);

        if !bpm.is_finite() || !delta_time.is_finite() || !beat_phase.is_finite() {
            self.output = 0.0;
            return;
        }

        // Calculate effective rate
        let rate_hz = if self.tempo_sync {
            let beat_duration = 60.0 / bpm.max(1.0);
            let cycle_duration = beat_duration * BEAT_DIVISIONS[division];
            1.0 / cycle_duration
        } else {
            self.rate.clamp(0.01, 20.0)
        };

        // Snap to beat on quantum boundary crossing (beat_phase wrapped ≈ 1→0).
        // Only snap for divisions ≤ 1 beat; longer cycles accumulate freely
        // so they don't get disrupted on every bar.
        if self.tempo_sync
            && beat_phase < self.last_beat_phase - 0.5
            && BEAT_DIVISIONS[division] <= 1.0
        {
            self.phase = 0.0;
        }
        self.last_beat_phase = beat_phase;

        // Accumulate phase at the correct rate
        self.phase = (self.phase + rate_hz * delta_time) % 1.0;
        if !self.phase.is_finite() {
            self.phase = 0.0;
        }

        // Apply static phase offset (degrees → 0-1)
        let offset_normalized = self.phase_offset / 360.0;
        let effective_phase = (self.phase + offset_normalized) % 1.0;

        let raw_value = Self::calculate_value(effective_phase, self.waveform);
        self.output = raw_value * self.amplitude;
    }

    /// Reset phase to 0
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.output = 0.0;
        self.last_beat_phase = 0.0;
    }

    /// Get the waveform value at a specific phase (for visualization)
    pub fn get_waveform_value_at(&self, phase: f32) -> f32 {
        Self::calculate_value(phase, self.waveform)
    }
}

impl Default for Lfo {
    fn default() -> Self {
        Self::new(0)
    }
}

/// Collection of LFOs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LfoBank {
    /// The LFOs (default capacity 8).
    pub lfos: Vec<Lfo>,
    /// Pre-allocated per-frame accumulation buffer: (param_index, summed_mod_value).
    /// Cleared at the start of each apply call; never deallocated between frames.
    #[serde(skip)]
    mod_accum: Vec<(usize, f32)>,
}

impl LfoBank {
    /// Create a new bank with eight default LFOs.
    pub fn new() -> Self {
        Self {
            lfos: (0..8).map(Lfo::new).collect(),
            mod_accum: Vec::with_capacity(8),
        }
    }

    /// Update all LFOs
    pub fn update(&mut self, bpm: f32, delta_time: f32, beat_phase: f32) {
        for lfo in &mut self.lfos {
            lfo.update(bpm, delta_time, beat_phase);
        }
    }

    /// Get modulation values for all targets.
    /// Returns a map of `param_id → modulation_value`.
    ///
    /// Allocates on every call — use [`fill_modulations`](Self::fill_modulations) +
    /// [`mod_accum`](Self::mod_accum) on the hot path.
    pub fn get_modulations(&self) -> HashMap<String, f32> {
        let mut mods = HashMap::new();
        for lfo in &self.lfos {
            if !lfo.enabled {
                continue;
            }
            if let Some(id) = lfo.target.param_id() {
                // Sum modulations from multiple LFOs targeting the same param
                let entry = mods.entry(id.to_string()).or_insert(0.0);
                *entry += lfo.output;
            }
        }
        mods
    }

    /// Populate the internal accumulation buffer with summed LFO outputs per descriptor index.
    ///
    /// Call once per frame.  The result is then read back via [`mod_accum`](Self::mod_accum)
    /// in a separate step so the caller can simultaneously borrow other state while iterating.
    /// No heap allocation occurs after the first call (the Vec retains its capacity).
    pub fn fill_modulations(&mut self, descriptors: &[crate::params::ParameterDescriptor]) {
        self.mod_accum.clear();
        for lfo in &self.lfos {
            if !lfo.enabled {
                continue;
            }
            let LfoTarget::Custom(id) = &lfo.target else {
                continue;
            };
            let Some(idx) = descriptors.iter().position(|d| d.id == *id) else {
                continue;
            };
            if let Some(entry) = self.mod_accum.iter_mut().find(|(i, _)| *i == idx) {
                entry.1 += lfo.output;
            } else {
                self.mod_accum.push((idx, lfo.output));
            }
        }
    }

    /// Read the last accumulated (param_index, summed_mod_value) pairs.
    pub fn mod_accum(&self) -> &[(usize, f32)] {
        &self.mod_accum
    }

    /// Reset all LFO phases
    pub fn reset_all(&mut self) {
        for lfo in &mut self.lfos {
            lfo.reset();
        }
    }

    /// Get LFO by index
    pub fn get(&self, index: usize) -> Option<&Lfo> {
        self.lfos.get(index)
    }

    /// Get mutable LFO by index
    pub fn get_mut(&mut self, index: usize) -> Option<&mut Lfo> {
        self.lfos.get_mut(index)
    }

    /// Convert this bank into modulation source entries.
    ///
    /// Each LFO becomes a [`crate::modulation::ModulationSource::LFO`] entry.
    /// Waveforms are mapped to the nearest [`crate::modulation::LFOWaveform`];
    /// `Ramp` and `Saw` both map to `Sawtooth`.
    #[cfg(feature = "modulation")]
    pub fn to_modulation_sources(&self) -> Vec<crate::modulation::ModulationSourceEntry> {
        use crate::modulation::{LFOWaveform, ModulationSource, ModulationSourceEntry};
        self.lfos
            .iter()
            .map(|lfo| {
                let waveform = match lfo.waveform {
                    Waveform::Sine => LFOWaveform::Sine,
                    Waveform::Triangle => LFOWaveform::Triangle,
                    Waveform::Square => LFOWaveform::Square,
                    Waveform::Ramp | Waveform::Saw => LFOWaveform::Sawtooth,
                };
                let frequency = if lfo.tempo_sync {
                    beat_division_to_hz(lfo.division, 120.0)
                } else {
                    lfo.rate
                };
                let phase = lfo.phase_offset / 360.0;
                let source = ModulationSource::LFO {
                    waveform,
                    frequency,
                    phase,
                    amplitude: lfo.amplitude,
                    bipolar: true,
                };
                ModulationSourceEntry::with_uuid(format!("lfo_{}", lfo.index), source)
            })
            .collect()
    }

    /// Convert this bank into a full [`crate::modulation::ModulationEngine`].
    ///
    /// `bpm` is used to resolve tempo-synced frequencies.
    #[cfg(feature = "modulation")]
    pub fn to_modulation_engine(&self, bpm: f32) -> crate::modulation::ModulationEngine {
        use crate::modulation::{LFOWaveform, ModulationEngine, ModulationSource};
        let mut engine = ModulationEngine::new();
        for lfo in &self.lfos {
            let waveform = match lfo.waveform {
                Waveform::Sine => LFOWaveform::Sine,
                Waveform::Triangle => LFOWaveform::Triangle,
                Waveform::Square => LFOWaveform::Square,
                Waveform::Ramp | Waveform::Saw => LFOWaveform::Sawtooth,
            };
            let frequency = if lfo.tempo_sync {
                beat_division_to_hz(lfo.division, bpm)
            } else {
                lfo.rate
            };
            let phase = lfo.phase_offset / 360.0;
            let source = ModulationSource::LFO {
                waveform,
                frequency,
                phase,
                amplitude: lfo.amplitude,
                bipolar: true,
            };
            let uuid = engine.add_source_with_uuid(format!("lfo_{}", lfo.index), source);
            if let Some(param_id) = lfo.target.param_id() {
                engine.assign(param_id, &uuid, 1.0, None);
            }
        }
        engine
    }
}

impl Default for LfoBank {
    fn default() -> Self {
        Self::new()
    }
}

/// LFO state for the app
/// LFO state container.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LfoState {
    /// The three LFOs.
    pub bank: LfoBank,
    /// Whether the LFO control window is visible.
    #[serde(skip)]
    pub show_window: bool,
}

impl LfoState {
    /// Create a new LFO state with default settings.
    pub fn new() -> Self {
        Self {
            bank: LfoBank::new(),
            show_window: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sine_waveform() {
        assert!((Lfo::calculate_value(0.0, Waveform::Sine) - 0.0).abs() < 0.001);
        assert!((Lfo::calculate_value(0.25, Waveform::Sine) - 1.0).abs() < 0.001);
        assert!((Lfo::calculate_value(0.5, Waveform::Sine) - 0.0).abs() < 0.001);
        assert!((Lfo::calculate_value(0.75, Waveform::Sine) - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn test_square_waveform() {
        assert_eq!(Lfo::calculate_value(0.0, Waveform::Square), 1.0);
        assert_eq!(Lfo::calculate_value(0.25, Waveform::Square), 1.0);
        assert_eq!(Lfo::calculate_value(0.5, Waveform::Square), -1.0);
        assert_eq!(Lfo::calculate_value(0.75, Waveform::Square), -1.0);
    }

    #[test]
    fn test_lfo_update() {
        let mut lfo = Lfo::new(0);
        lfo.enabled = true;
        lfo.tempo_sync = false;
        lfo.rate = 1.0; // 1 Hz = 1 cycle per second

        // Update for 0.25 seconds should advance phase by 0.25
        lfo.update(120.0, 0.25, 0.0);
        assert!((lfo.phase - 0.25).abs() < 0.01);
    }
}
