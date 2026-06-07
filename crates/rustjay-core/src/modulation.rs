//! Parameter modulation engine for automating shader parameters.
//!
//! Supports LFOs, audio-reactive bands, ADSR envelopes, and step sequencers
//! with UUID-stable sources and multi-target assignments.
//!
//! This module is the evolutionary successor to the fixed `LfoBank` +
//! `RoutingMatrix` model in [`crate::lfo`] and [`crate::routing`].
//! Adapters on those legacy types let you convert into this richer model.

use crate::lfo::{beat_division_to_hz, BEAT_DIVISIONS};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_true() -> bool { true }

// ─── UUID helper ───────────────────────────────────────────────────────────

fn generate_short_uuid() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

// ─── Supporting enums ──────────────────────────────────────────────────────

/// LFO waveform types.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum LFOWaveform {
    /// Sinusoidal wave.
    #[default]
    Sine,
    /// Square wave.
    Square,
    /// Triangle wave.
    Triangle,
    /// Sawtooth wave (upward ramp).
    Sawtooth,
    /// Random sample-and-hold.
    Random,
}

/// How audio energy drives the modulation value.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum AudioReactMode {
    /// Direct: output = audio energy (standard envelope follower).
    #[default]
    Direct,
    /// Increase: audio energy sweeps the value upward (accumulates).
    Increase,
    /// Decrease: audio energy sweeps the value downward (accumulates).
    Decrease,
}

/// Audio frequency band presets (convenience for UI quick-select).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum AudioBandPreset {
    /// 20–250 Hz.
    #[default]
    Low,
    /// 250–2000 Hz.
    Mid,
    /// 2000–20000 Hz.
    High,
    /// 20–20000 Hz (overall level).
    Full,
}

impl AudioBandPreset {
    /// Get the frequency range for this preset.
    pub fn freq_range(self) -> (f32, f32) {
        match self {
            AudioBandPreset::Low => (20.0, 250.0),
            AudioBandPreset::Mid => (250.0, 2000.0),
            AudioBandPreset::High => (2000.0, 20000.0),
            AudioBandPreset::Full => (20.0, 20000.0),
        }
    }
}

/// ADSR envelope stage.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ADSRStage {
    /// Waiting for gate.
    #[default]
    Idle,
    /// Rising to peak.
    Attack,
    /// Falling to sustain.
    Decay,
    /// Holding at sustain level.
    Sustain,
    /// Falling to zero.
    Release,
}

/// Step sequencer interpolation mode.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum StepInterpolation {
    /// Hard steps, no interpolation.
    #[default]
    None,
    /// Linear interpolation between steps.
    Linear,
    /// Smooth cubic interpolation.
    Smooth,
}

// ─── Audio analysis values ─────────────────────────────────────────────────

/// Audio analysis values for a single source, passed to the modulation engine.
///
/// Uses a borrowed FFT slice (S1) to avoid per-frame allocation.
#[derive(Debug, Clone, Copy)]
pub struct AudioSourceValues<'a> {
    /// FFT magnitude data (one-sided spectrum).
    pub fft: &'a [f32],
    /// Overall audio level (RMS or peak).
    pub level: f32,
    /// Sample rate in Hz.
    pub sample_rate: f32,
}

impl<'a> Default for AudioSourceValues<'a> {
    fn default() -> Self {
        Self {
            fft: &[],
            level: 0.0,
            sample_rate: 0.0,
        }
    }
}

impl<'a> AudioSourceValues<'a> {
    /// Compute energy in a frequency range from the FFT data.
    ///
    /// Returns a perceptually-scaled value in roughly 0.0–1.0 range
    /// suitable for driving modulation (dB-based mapping).
    pub fn energy_in_range(&self, freq_low: f32, freq_high: f32) -> f32 {
        if self.fft.is_empty() || self.sample_rate <= 0.0 {
            return 0.0;
        }
        let fft_size = self.fft.len() * 2;
        let bin_width = self.sample_rate / fft_size as f32;
        let bin_low =
            ((freq_low / bin_width).floor() as usize).min(self.fft.len().saturating_sub(1));
        let bin_high = ((freq_high / bin_width).ceil() as usize).min(self.fft.len());
        if bin_high <= bin_low {
            return 0.0;
        }
        let slice = &self.fft[bin_low..bin_high];
        let rms = (slice.iter().map(|v| v * v).sum::<f32>() / slice.len() as f32).sqrt();
        if rms < 1e-6 {
            return 0.0;
        }
        let db = 20.0 * rms.log10();
        ((db + 60.0) / 60.0).clamp(0.0, 1.0)
    }
}

/// All audio source data for the current frame.
#[derive(Debug, Clone, Default)]
pub struct AudioValues<'a> {
    /// Per-source audio data, keyed by source id (`u32`).
    pub sources: HashMap<u32, AudioSourceValues<'a>>,
}

impl<'a> AudioValues<'a> {
    /// Get the first/primary source's data (lowest id).
    pub fn primary(&self) -> Option<&AudioSourceValues<'a>> {
        self.sources
            .iter()
            .min_by_key(|(id, _)| **id)
            .map(|(_, v)| v)
    }
}

// ─── Modulation assignment ─────────────────────────────────────────────────

/// Modulation assignment linking a source to a parameter.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParamModulation {
    /// UUID of the modulation source.
    pub source_id: String,
    /// Modulation depth/amount (-1.0 to 1.0, negative inverts).
    pub amount: f32,
    /// For color params: which component (0=R, 1=G, 2=B, 3=A), None for scalar.
    pub component: Option<usize>,
}

// ─── Modulation source entry ───────────────────────────────────────────────

/// A modulation source paired with a stable UUID identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulationSourceEntry {
    /// Stable UUID.
    pub uuid: String,
    /// The source configuration + runtime state.
    pub source: ModulationSource,
}

impl ModulationSourceEntry {
    /// Create a new entry with a generated UUID.
    pub fn new(source: ModulationSource) -> Self {
        Self {
            uuid: generate_short_uuid(),
            source,
        }
    }

    /// Create a new entry with a specific UUID (for preset loading).
    pub fn with_uuid(uuid: String, source: ModulationSource) -> Self {
        Self { uuid, source }
    }
}

// ─── Modulation source types ───────────────────────────────────────────────

fn default_noise_gate() -> f32 {
    0.1
}

/// Modulation source types and their computation logic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModulationSource {
    /// Low Frequency Oscillator.
    LFO {
        /// Waveform shape.
        waveform: LFOWaveform,
        /// Frequency in Hz (used when tempo_sync is false).
        frequency: f32,
        /// Phase offset (0–1).
        phase: f32,
        /// Amplitude multiplier.
        amplitude: f32,
        /// Whether output is bipolar (-1..1) or unipolar (0..1).
        bipolar: bool,
        /// Whether tempo sync is enabled.
        tempo_sync: bool,
        /// Beat division index (0-7, mapped to BEAT_DIVISIONS).
        division: usize,
        /// Phase offset in degrees (0-360).
        phase_offset_degrees: f32,
        /// Whether this LFO is active.
        #[serde(default = "default_true")]
        enabled: bool,
        /// Previous beat_phase sample — used to detect quantum-boundary crossings.
        #[serde(skip)]
        last_beat_phase: f32,
    },
    /// Audio FFT reactivity with custom frequency range.
    AudioBand {
        /// Optional audio source id (`None` = primary).
        source_id: Option<u32>,
        /// Low frequency bound in Hz.
        freq_low: f32,
        /// High frequency bound in Hz.
        freq_high: f32,
        /// Output gain multiplier.
        gain: f32,
        /// Release smoothing (0–0.99).
        smoothing: f32,
        /// React mode.
        #[serde(default)]
        mode: AudioReactMode,
        /// Noise gate threshold.
        #[serde(default = "default_noise_gate")]
        noise_gate: f32,
    },
    /// ADSR envelope generator.
    ADSR {
        /// Attack time in seconds.
        attack: f32,
        /// Decay time in seconds.
        decay: f32,
        /// Sustain level (0–1).
        sustain: f32,
        /// Release time in seconds.
        release: f32,
        /// Current stage (runtime).
        #[serde(skip)]
        stage: ADSRStage,
        /// Time spent in current stage (runtime).
        #[serde(skip)]
        stage_time: f32,
        /// Gate state (runtime).
        #[serde(skip)]
        gate: bool,
        /// Current output level (runtime).
        #[serde(skip)]
        current_level: f32,
    },
    /// Step sequencer.
    StepSequencer {
        /// Step values (0–1, or -1..1 in bipolar mode after scaling).
        steps: Vec<f32>,
        /// Rate in steps per second.
        rate: f32,
        /// Interpolation mode.
        interpolation: StepInterpolation,
        /// Whether output is bipolar.
        bipolar: bool,
    },
}

impl ModulationSource {
    /// Compare two sources by configuration fields only.
    ///
    /// Ignores ADSR runtime state (`stage`, `stage_time`, `gate`, `current_level`).
    pub fn config_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                ModulationSource::LFO {
                    waveform: w1,
                    frequency: f1,
                    phase: p1,
                    amplitude: a1,
                    bipolar: b1,
                    tempo_sync: t1,
                    division: d1,
                    phase_offset_degrees: po1,
                    ..
                },
                ModulationSource::LFO {
                    waveform: w2,
                    frequency: f2,
                    phase: p2,
                    amplitude: a2,
                    bipolar: b2,
                    tempo_sync: t2,
                    division: d2,
                    phase_offset_degrees: po2,
                    ..
                },
            ) => w1 == w2 && f1 == f2 && p1 == p2 && a1 == a2 && b1 == b2 && t1 == t2 && d1 == d2 && po1 == po2,
            (
                ModulationSource::AudioBand {
                    source_id: s1,
                    freq_low: fl1,
                    freq_high: fh1,
                    gain: g1,
                    smoothing: sm1,
                    mode: m1,
                    noise_gate: ng1,
                },
                ModulationSource::AudioBand {
                    source_id: s2,
                    freq_low: fl2,
                    freq_high: fh2,
                    gain: g2,
                    smoothing: sm2,
                    mode: m2,
                    noise_gate: ng2,
                },
            ) => {
                s1 == s2
                    && fl1 == fl2
                    && fh1 == fh2
                    && g1 == g2
                    && sm1 == sm2
                    && m1 == m2
                    && ng1 == ng2
            }
            (
                ModulationSource::ADSR {
                    attack: a1,
                    decay: d1,
                    sustain: s1,
                    release: r1,
                    ..
                },
                ModulationSource::ADSR {
                    attack: a2,
                    decay: d2,
                    sustain: s2,
                    release: r2,
                    ..
                },
            ) => a1 == a2 && d1 == d2 && s1 == s2 && r1 == r2,
            (
                ModulationSource::StepSequencer {
                    steps: s1,
                    rate: r1,
                    interpolation: i1,
                    bipolar: b1,
                },
                ModulationSource::StepSequencer {
                    steps: s2,
                    rate: r2,
                    interpolation: i2,
                    bipolar: b2,
                },
            ) => s1 == s2 && r1 == r2 && i1 == i2 && b1 == b2,
            _ => false,
        }
    }

    /// Create a sine LFO with the given frequency.
    pub fn sine_lfo(frequency: f32) -> Self {
        ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: false,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        }
    }

    /// Create an audio band source from a preset.
    pub fn audio_from_preset(preset: AudioBandPreset) -> Self {
        let (freq_low, freq_high) = preset.freq_range();
        ModulationSource::AudioBand {
            source_id: None,
            freq_low,
            freq_high,
            gain: 1.0,
            smoothing: 0.6,
            mode: AudioReactMode::Direct,
            noise_gate: 0.1,
        }
    }

    /// Create an ADSR envelope.
    pub fn adsr(attack: f32, decay: f32, sustain: f32, release: f32) -> Self {
        ModulationSource::ADSR {
            attack,
            decay,
            sustain,
            release,
            stage: ADSRStage::Idle,
            stage_time: 0.0,
            gate: false,
            current_level: 0.0,
        }
    }

    /// Create a step sequencer.
    pub fn step_sequencer(num_steps: usize, rate: f32) -> Self {
        ModulationSource::StepSequencer {
            steps: vec![0.0; num_steps.max(2)],
            rate,
            interpolation: StepInterpolation::None,
            bipolar: false,
        }
    }

    /// Gate on (for ADSR).
    pub fn gate_on(&mut self) {
        if let ModulationSource::ADSR {
            stage,
            stage_time,
            gate,
            ..
        } = self
        {
            *gate = true;
            *stage = ADSRStage::Attack;
            *stage_time = 0.0;
        }
    }

    /// Gate off (for ADSR).
    pub fn gate_off(&mut self) {
        if let ModulationSource::ADSR {
            stage,
            stage_time,
            gate,
            ..
        } = self
        {
            *gate = false;
            if *stage != ADSRStage::Idle {
                *stage = ADSRStage::Release;
                *stage_time = 0.0;
            }
        }
    }

    /// Calculate current value of this modulation source.
    ///
    /// Returns value in range `[-1, 1]` for bipolar or `[0, 1]` for unipolar.
    pub fn calculate(&mut self, time: f32, dt: f32, bpm: f32, beat_phase: f32, audio: &AudioValues<'_>, prev_value: f32) -> f32 {
        match self {
            ModulationSource::LFO {
                waveform,
                frequency,
                phase,
                amplitude,
                bipolar,
                tempo_sync,
                division,
                phase_offset_degrees,
                enabled,
                last_beat_phase,
            } => {
                if !*enabled {
                    return 0.0;
                }
                let raw_freq = if *tempo_sync {
                    let div = (*division).min(BEAT_DIVISIONS.len() - 1);
                    beat_division_to_hz(div, bpm)
                } else {
                    *frequency
                };
                // S3: NaN propagates through f32::clamp; guard it explicitly.
                let effective_freq = if raw_freq.is_finite() {
                    raw_freq.clamp(0.01, 20.0)
                } else {
                    1.0 // safe fallback
                };

                // Quantum-boundary phase snap: when beat_phase wraps from ~1 back to ~0,
                // reset phase for sub-beat/single-beat divisions so the LFO stays musically in phase.
                if *tempo_sync && beat_phase < *last_beat_phase - 0.5 {
                    let div = (*division).min(BEAT_DIVISIONS.len() - 1);
                    if BEAT_DIVISIONS[div] <= 1.0 {
                        *phase = 0.0;
                    }
                }
                *last_beat_phase = beat_phase;

                // Accumulate phase at the effective rate
                *phase = (*phase + effective_freq * dt) % 1.0;
                if !phase.is_finite() {
                    *phase = 0.0;
                }

                // Apply static phase offset (degrees → 0-1)
                let offset_normalized = *phase_offset_degrees / 360.0;
                let t = (*phase + offset_normalized) % 1.0;

                let raw = match waveform {
                    LFOWaveform::Sine => (t * std::f32::consts::TAU).sin(),
                    LFOWaveform::Square => {
                        if t < 0.5 {
                            1.0
                        } else {
                            -1.0
                        }
                    }
                    LFOWaveform::Triangle => 1.0 - 4.0 * (t - 0.5).abs(),
                    LFOWaveform::Sawtooth => 2.0 * t - 1.0,
                    LFOWaveform::Random => {
                        let seed = (time * effective_freq).floor() as u32;
                        let hash = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                        (hash as f32 / u32::MAX as f32) * 2.0 - 1.0
                    }
                };
                let scaled = raw * *amplitude;
                if *bipolar {
                    scaled
                } else {
                    scaled * 0.5 + 0.5
                }
            }
            ModulationSource::AudioBand {
                source_id,
                freq_low,
                freq_high,
                gain,
                smoothing,
                mode,
                noise_gate,
            } => {
                let source_vals = if let Some(id) = source_id {
                    audio.sources.get(id)
                } else {
                    audio.primary()
                };
                let raw_signal = if let Some(vals) = source_vals {
                    vals.energy_in_range(*freq_low, *freq_high) * *gain
                } else {
                    0.0
                };
                let raw = if raw_signal < *noise_gate {
                    0.0
                } else {
                    raw_signal
                };
                match mode {
                    AudioReactMode::Direct => {
                        if raw >= prev_value {
                            raw.clamp(0.0, 1.0)
                        } else {
                            let release_alpha = 1.0 - *smoothing;
                            (prev_value + release_alpha * (raw - prev_value)).clamp(0.0, 1.0)
                        }
                    }
                    AudioReactMode::Increase => {
                        if raw <= 0.0 {
                            prev_value
                        } else {
                            let speed = (1.0 - *smoothing * 0.9) * 4.0;
                            let step = raw * dt * speed;
                            let next = prev_value + step;
                            if next >= 1.0 {
                                next - 1.0
                            } else {
                                next
                            }
                        }
                    }
                    AudioReactMode::Decrease => {
                        if raw <= 0.0 {
                            prev_value
                        } else {
                            let speed = (1.0 - *smoothing * 0.9) * 4.0;
                            let step = raw * dt * speed;
                            let next = prev_value - step;
                            if next <= 0.0 {
                                next + 1.0
                            } else {
                                next
                            }
                        }
                    }
                }
            }
            ModulationSource::ADSR {
                attack,
                decay,
                sustain,
                release,
                stage,
                stage_time,
                current_level,
                ..
            } => {
                *stage_time += dt;
                match stage {
                    ADSRStage::Idle => {
                        *current_level = 0.0;
                    }
                    ADSRStage::Attack => {
                        let progress = if *attack > 0.001 {
                            *stage_time / *attack
                        } else {
                            1.0
                        };
                        if progress >= 1.0 {
                            *current_level = 1.0;
                            *stage = ADSRStage::Decay;
                            *stage_time = 0.0;
                        } else {
                            *current_level = progress;
                        }
                    }
                    ADSRStage::Decay => {
                        let progress = if *decay > 0.001 {
                            *stage_time / *decay
                        } else {
                            1.0
                        };
                        if progress >= 1.0 {
                            *current_level = *sustain;
                            *stage = ADSRStage::Sustain;
                            *stage_time = 0.0;
                        } else {
                            *current_level = 1.0 - (1.0 - *sustain) * progress;
                        }
                    }
                    ADSRStage::Sustain => {
                        *current_level = *sustain;
                    }
                    ADSRStage::Release => {
                        let start_level = *current_level;
                        let progress = if *release > 0.001 {
                            *stage_time / *release
                        } else {
                            1.0
                        };
                        if progress >= 1.0 {
                            *current_level = 0.0;
                            *stage = ADSRStage::Idle;
                            *stage_time = 0.0;
                        } else {
                            *current_level = start_level * (1.0 - progress);
                        }
                    }
                }
                *current_level
            }
            ModulationSource::StepSequencer {
                steps,
                rate,
                interpolation,
                bipolar,
            } => {
                if steps.is_empty() {
                    return 0.0;
                }
                let total_steps = steps.len() as f32;
                let position = (time * *rate) % total_steps;
                let current_idx = position.floor() as usize % steps.len();
                let raw = match interpolation {
                    StepInterpolation::None => steps[current_idx],
                    StepInterpolation::Linear => {
                        let next_idx = (current_idx + 1) % steps.len();
                        let frac = position.fract();
                        steps[current_idx] * (1.0 - frac) + steps[next_idx] * frac
                    }
                    StepInterpolation::Smooth => {
                        let next_idx = (current_idx + 1) % steps.len();
                        let frac = position.fract();
                        let t = frac * frac * (3.0 - 2.0 * frac);
                        steps[current_idx] * (1.0 - t) + steps[next_idx] * t
                    }
                };
                if *bipolar {
                    raw * 2.0 - 1.0
                } else {
                    raw
                }
            }
        }
    }
}

// ─── ModulationEngine ──────────────────────────────────────────────────────

/// Manages sources, assignments, and per-frame evaluation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModulationEngine {
    /// Available modulation sources (with stable UUIDs).
    pub sources: Vec<ModulationSourceEntry>,
    /// Map from parameter name to list of modulations.
    pub assignments: HashMap<String, Vec<ParamModulation>>,
    /// UUID → index cache for O(1) lookups during tick.
    #[serde(skip)]
    uuid_to_idx: HashMap<String, usize>,
    #[serde(skip)]
    prev_values: Vec<f32>,
    #[serde(skip)]
    current_values: Vec<f32>,
    #[serde(skip)]
    prev_time: Option<f32>,
    /// True if any assignment key starts with `"mod:"` — gates `apply_mod_on_mod`
    /// and `evaluation_order` caching (PERF-3).
    #[serde(skip)]
    has_mod_on_mod: bool,
    /// Cached evaluation order; invalidated when assignments or sources change.
    #[serde(skip)]
    cached_evaluation_order: Option<Vec<usize>>,
}

impl ModulationEngine {
    /// Create an empty engine.
    pub fn new() -> Self {
        Self::default()
    }

    fn rebuild_uuid_index(&mut self) {
        self.uuid_to_idx.clear();
        for (i, entry) in self.sources.iter().enumerate() {
            self.uuid_to_idx.insert(entry.uuid.clone(), i);
        }
    }

    fn rebuild_mod_on_mod_flag(&mut self) {
        self.has_mod_on_mod = self.assignments.keys().any(|k| k.starts_with("mod:"));
    }

    fn invalidate_evaluation_cache(&mut self) {
        self.cached_evaluation_order = None;
    }

    /// Ensure `uuid_to_idx` is populated (needed after deserialization).
    pub fn ensure_index(&mut self) {
        if self.uuid_to_idx.len() != self.sources.len() {
            self.rebuild_uuid_index();
        }
    }

    /// Add a new source, returns its UUID.
    pub fn add_source(&mut self, source: ModulationSource) -> String {
        let entry = ModulationSourceEntry::new(source);
        let uuid = entry.uuid.clone();
        self.sources.push(entry);
        self.prev_values.push(0.0);
        self.current_values.push(0.0);
        self.uuid_to_idx
            .insert(uuid.clone(), self.sources.len() - 1);
        self.invalidate_evaluation_cache();
        uuid
    }

    /// Add a source with a specific UUID (for preset loading).
    pub fn add_source_with_uuid(&mut self, uuid: String, source: ModulationSource) -> String {
        let entry = ModulationSourceEntry::with_uuid(uuid.clone(), source);
        self.sources.push(entry);
        self.prev_values.push(0.0);
        self.current_values.push(0.0);
        self.uuid_to_idx
            .insert(uuid.clone(), self.sources.len() - 1);
        self.invalidate_evaluation_cache();
        uuid
    }

    /// Remove a source by UUID.
    pub fn remove_source(&mut self, uuid: &str) {
        if let Some(idx) = self.uuid_to_idx.get(uuid).copied() {
            self.sources.remove(idx);
            if idx < self.prev_values.len() {
                self.prev_values.remove(idx);
            }
            if idx < self.current_values.len() {
                self.current_values.remove(idx);
            }
            // Remove assignments referencing this source
            for mods in self.assignments.values_mut() {
                mods.retain(|m| m.source_id != uuid);
            }
            // Remove mod-on-mod assignments targeting this source
            let mod_prefix = format!("mod:{}:", uuid);
            self.assignments.retain(|k, _| !k.starts_with(&mod_prefix));
            self.rebuild_uuid_index();
            self.rebuild_mod_on_mod_flag();
            self.invalidate_evaluation_cache();
        }
    }

    /// Remove all assignments whose key starts with the given prefix.
    ///
    /// Used to clean up orphaned assignments when a deck or effect is removed.
    pub fn remove_assignments_with_prefix(&mut self, prefix: &str) {
        let before = self.assignments.len();
        self.assignments.retain(|k, _| !k.starts_with(prefix));
        let removed = before - self.assignments.len();
        if removed > 0 {
            log::info!(
                "Removed {} orphaned modulation assignments with prefix '{}'",
                removed,
                prefix
            );
        }
        self.rebuild_mod_on_mod_flag();
        self.invalidate_evaluation_cache();
    }

    /// Assign a source to modulate a parameter.
    pub fn assign(
        &mut self,
        param_name: &str,
        source_id: &str,
        amount: f32,
        component: Option<usize>,
    ) {
        if !self.uuid_to_idx.contains_key(source_id) {
            self.ensure_index();
            if !self.uuid_to_idx.contains_key(source_id) {
                return;
            }
        }
        let modulation = ParamModulation {
            source_id: source_id.to_string(),
            amount,
            component,
        };
        self.assignments
            .entry(param_name.to_string())
            .or_default()
            .push(modulation);
        self.rebuild_mod_on_mod_flag();
        self.invalidate_evaluation_cache();
    }

    /// Assign a source to modulate another source's parameter (mod-on-mod).
    pub fn assign_mod_on_mod(
        &mut self,
        target_uuid: &str,
        param_name: &str,
        modulator_uuid: &str,
        amount: f32,
    ) {
        let key = format!("mod:{}:{}", target_uuid, param_name);
        self.assign(&key, modulator_uuid, amount, None);
    }

    /// Clear mod-on-mod assignments for a target source parameter.
    pub fn clear_mod_on_mod(&mut self, target_uuid: &str, param_name: &str) {
        let key = format!("mod:{}:{}", target_uuid, param_name);
        self.assignments.remove(&key);
        self.rebuild_mod_on_mod_flag();
        self.invalidate_evaluation_cache();
    }

    /// Clear all assignments for a parameter.
    pub fn clear_assignments(&mut self, param_name: &str) {
        self.assignments.remove(param_name);
        self.rebuild_mod_on_mod_flag();
        self.invalidate_evaluation_cache();
    }

    /// Trigger ADSR gate on.
    pub fn trigger_adsr(&mut self, uuid: &str) {
        if let Some(&idx) = self.uuid_to_idx.get(uuid) {
            self.sources[idx].source.gate_on();
        }
    }

    /// Trigger ADSR gate off.
    pub fn release_adsr(&mut self, uuid: &str) {
        if let Some(&idx) = self.uuid_to_idx.get(uuid) {
            self.sources[idx].source.gate_off();
        }
    }

    /// Get a mutable reference to a source by UUID.
    pub fn source_mut(&mut self, uuid: &str) -> Option<&mut ModulationSource> {
        self.ensure_index();
        self.uuid_to_idx
            .get(uuid)
            .copied()
            .map(|idx| &mut self.sources[idx].source)
    }

    /// Check if a source exists.
    pub fn has_source(&self, uuid: &str) -> bool {
        self.sources.iter().any(|e| e.uuid == uuid)
    }

    fn source_idx(&self, uuid: &str) -> Option<usize> {
        self.sources.iter().position(|e| e.uuid == uuid)
    }

    fn get_mod_source_offset(&self, source_uuid: &str, param_name: &str) -> f32 {
        let key = format!("mod:{}:{}", source_uuid, param_name);
        self.get_modulation(&key)
    }

    fn apply_mod_on_mod(&self, idx: usize, source: &ModulationSource) -> ModulationSource {
        let uuid = &self.sources[idx].uuid;
        let mut modified = source.clone();
        match &mut modified {
            ModulationSource::LFO {
                frequency,
                phase,
                amplitude,
                ..
            } => {
                *frequency =
                    (*frequency + self.get_mod_source_offset(uuid, "frequency")).max(0.001);
                *phase = (*phase + self.get_mod_source_offset(uuid, "phase")).clamp(0.0, 1.0);
                *amplitude =
                    (*amplitude + self.get_mod_source_offset(uuid, "amplitude")).clamp(0.0, 1.0);
            }
            ModulationSource::AudioBand {
                gain, smoothing, ..
            } => {
                *gain = (*gain + self.get_mod_source_offset(uuid, "gain")).max(0.0);
                *smoothing =
                    (*smoothing + self.get_mod_source_offset(uuid, "smoothing")).clamp(0.0, 0.99);
            }
            ModulationSource::ADSR {
                attack,
                decay,
                sustain,
                release,
                ..
            } => {
                *attack = (*attack + self.get_mod_source_offset(uuid, "attack")).max(0.001);
                *decay = (*decay + self.get_mod_source_offset(uuid, "decay")).max(0.001);
                *sustain = (*sustain + self.get_mod_source_offset(uuid, "sustain")).clamp(0.0, 1.0);
                *release = (*release + self.get_mod_source_offset(uuid, "release")).max(0.001);
            }
            ModulationSource::StepSequencer { rate, .. } => {
                *rate = (*rate + self.get_mod_source_offset(uuid, "rate")).max(0.01);
            }
        }
        modified
    }

    /// Compute evaluation order for sources respecting mod-on-mod dependencies.
    pub(crate) fn evaluation_order(&self) -> Vec<usize> {
        const MAX_MOD_DEPTH: usize = 4;
        let n = self.sources.len();
        if n == 0 {
            return vec![];
        }

        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (key, mods) in &self.assignments {
            if let Some(target_uuid) = Self::parse_mod_target(key) {
                if let Some(target_idx) = self.source_idx(target_uuid) {
                    for m in mods {
                        if let Some(src_idx) = self.source_idx(&m.source_id) {
                            if src_idx != target_idx {
                                deps[target_idx].push(src_idx);
                            }
                        }
                    }
                }
            }
        }

        let mut order = Vec::with_capacity(n);
        let mut evaluated = vec![false; n];
        for _pass in 0..MAX_MOD_DEPTH {
            let mut progress = false;
            for i in 0..n {
                if evaluated[i] {
                    continue;
                }
                if deps[i].iter().all(|&d| evaluated[d]) {
                    order.push(i);
                    evaluated[i] = true;
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        for (i, &is_evaluated) in evaluated.iter().enumerate().take(n) {
            if !is_evaluated {
                order.push(i);
            }
        }
        order
    }

    /// Parse mod-on-mod key: `"mod:{uuid}:{param}"` → `Some(uuid)`.
    pub(crate) fn parse_mod_target(key: &str) -> Option<&str> {
        let parts: Vec<&str> = key.splitn(3, ':').collect();
        if parts.len() >= 2 && parts[0] == "mod" {
            Some(parts[1])
        } else {
            None
        }
    }

    /// Update all source values for the current frame.
    pub fn update(&mut self, time: f32, bpm: f32, beat_phase: f32, audio: &AudioValues<'_>) {
        self.ensure_index();
        let dt = self.prev_time.map_or(0.016, |prev| time - prev);
        self.prev_time = Some(time);

        while self.prev_values.len() < self.sources.len() {
            self.prev_values.push(0.0);
        }
        while self.current_values.len() < self.sources.len() {
            self.current_values.push(0.0);
        }

        if self.cached_evaluation_order.is_none() {
            let o = self.evaluation_order();
            self.cached_evaluation_order = Some(o);
        }
        let order = self.cached_evaluation_order.as_ref().unwrap();
        for &i in order {
            let value = if self.has_mod_on_mod {
                let mut effective = self.apply_mod_on_mod(i, &self.sources[i].source);
                let value = effective.calculate(time, dt, bpm, beat_phase, audio, self.prev_values[i]);
                // Copy back mutable state changes (ADSR stage progression)
                if let (
                    ModulationSource::ADSR {
                        stage,
                        stage_time,
                        current_level,
                        ..
                    },
                    ModulationSource::ADSR {
                        stage: eff_stage,
                        stage_time: eff_st,
                        current_level: eff_cl,
                        ..
                    },
                ) = (&mut self.sources[i].source, &effective)
                {
                    *stage = *eff_stage;
                    *stage_time = *eff_st;
                    *current_level = *eff_cl;
                }
                value
            } else {
                // Fast path: mutate source in place, no clone (S4).
                self.sources[i].source.calculate(time, dt, bpm, beat_phase, audio, self.prev_values[i])
            };

            self.current_values[i] = value;
            self.prev_values[i] = value;
        }
    }

    /// Get the total modulation offset for a scalar parameter.
    pub fn get_modulation(&self, param_name: &str) -> f32 {
        self.get_modulation_for_component(param_name, None)
    }

    /// Get the total modulation offset for a specific component (color params).
    pub fn get_modulation_for_component(&self, param_name: &str, component: Option<usize>) -> f32 {
        let Some(mods) = self.assignments.get(param_name) else {
            return 0.0;
        };
        let mut total = 0.0;
        for m in mods {
            if m.component == component {
                let idx = if let Some(&i) = self.uuid_to_idx.get(&m.source_id) {
                    i
                } else {
                    // Fallback: linear scan (handles deserialized state before ensure_index)
                    match self.sources.iter().position(|e| e.uuid == m.source_id) {
                        Some(i) => i,
                        None => continue,
                    }
                };
                if idx < self.current_values.len() {
                    total += self.current_values[idx] * m.amount;
                }
            }
        }
        total
    }

    /// Check if a parameter has any modulations assigned.
    pub fn has_modulation(&self, param_name: &str) -> bool {
        self.assignments
            .get(param_name)
            .is_some_and(|v| !v.is_empty())
    }

    /// Number of sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    /// Current computed values for all sources (for UI visualization).
    pub fn current_values(&self) -> &[f32] {
        &self.current_values
    }

    /// Current value for a source by UUID.
    pub fn current_value_for(&self, uuid: &str) -> f32 {
        self.sources
            .iter()
            .position(|e| e.uuid == uuid)
            .and_then(|idx| self.current_values.get(idx).copied())
            .unwrap_or(0.0)
    }

    /// Find an existing source by UUID.
    pub fn find_source_by_uuid(&self, uuid: &str) -> Option<&ModulationSourceEntry> {
        self.sources.iter().find(|e| e.uuid == uuid)
    }

    /// Iterate over all assignments.
    pub fn assignments_iter(&self) -> impl Iterator<Item = (&String, &Vec<ParamModulation>)> {
        self.assignments.iter()
    }

    /// Map LFO sources back to legacy `Lfo` structs for web-protocol backward compat.
    /// Non-LFO sources are ignored. Targets are inferred from assignments.
    pub fn to_lfo_vec(&self) -> Vec<crate::lfo::Lfo> {
        use crate::lfo::{Lfo, LfoTarget, Waveform};
        let mut out = Vec::new();
        for (i, entry) in self.sources.iter().enumerate() {
            let ModulationSource::LFO {
                waveform: wf,
                frequency,
                phase,
                amplitude,
                tempo_sync,
                division,
                phase_offset_degrees,
                enabled,
                last_beat_phase,
                ..
            } = &entry.source else { continue };

            // Infer target from assignments: find the first param this source modulates.
            let target = self
                .assignments
                .iter()
                .find_map(|(param_id, mods)| {
                    mods.iter()
                        .find(|m| m.source_id == entry.uuid)
                        .map(|_| param_id.as_str())
                })
                .map(|pid| match pid {
                    "hue_shift" => LfoTarget::HueShift,
                    "saturation" => LfoTarget::Saturation,
                    "brightness" => LfoTarget::Brightness,
                    _ => LfoTarget::Custom(pid.to_string()),
                })
                .unwrap_or(LfoTarget::None);

            let waveform = match wf {
                LFOWaveform::Sine => Waveform::Sine,
                LFOWaveform::Square => Waveform::Square,
                LFOWaveform::Triangle => Waveform::Triangle,
                LFOWaveform::Sawtooth => Waveform::Saw,
                LFOWaveform::Random => Waveform::Sine, // no Random in legacy; map to Sine
            };

            out.push(Lfo {
                index: i,
                enabled: *enabled,
                target,
                waveform,
                amplitude: *amplitude,
                tempo_sync: *tempo_sync,
                division: *division,
                rate: *frequency,
                phase_offset: *phase_offset_degrees,
                phase: *phase,
                output: self.current_values.get(i).copied().unwrap_or(0.0),
                last_beat_phase: *last_beat_phase,
            });
        }
        out
    }
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_audio() -> AudioValues<'static> {
        AudioValues::default()
    }

    // ── LFO waveform tests ───────────────────────────────────────────

    #[test]
    fn lfo_sine_unipolar_range() {
        let mut lfo = ModulationSource::sine_lfo(1.0);
        let audio = empty_audio();
        for i in 0..100 {
            let t = i as f32 / 100.0;
            let val = lfo.calculate(t, 0.01, 120.0, 0.0, &audio, 0.0);
            assert!(
                (0.0..=1.0).contains(&val),
                "Sine unipolar out of range: {val} at t={t}"
            );
        }
    }

    #[test]
    fn lfo_sine_bipolar_range() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        for i in 0..100 {
            let t = i as f32 / 100.0;
            let val = lfo.calculate(t, 0.01, 120.0, 0.0, &audio, 0.0);
            assert!(
                (-1.0..=1.0).contains(&val),
                "Sine bipolar out of range: {val}"
            );
        }
    }

    #[test]
    fn lfo_square_values() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Square,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val_first = lfo.calculate(0.0, 0.1, 120.0, 0.0, &audio, 0.0);
        let val_second = lfo.calculate(0.0, 0.5, 120.0, 0.0, &audio, 0.0);
        assert!((val_first - 1.0).abs() < 1e-5);
        assert!((val_second - (-1.0)).abs() < 1e-5);
    }

    #[test]
    fn lfo_triangle_symmetry() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Triangle,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val_start = lfo.calculate(0.0, 0.0, 120.0, 0.0, &audio, 0.0);
        let val_mid = lfo.calculate(0.0, 0.5, 120.0, 0.0, &audio, 0.0);
        assert!(
            (val_start - (-1.0)).abs() < 1e-5,
            "Triangle at 0: {val_start}"
        );
        assert!((val_mid - 1.0).abs() < 1e-5, "Triangle at 0.5: {val_mid}");
    }

    #[test]
    fn lfo_sawtooth_ramp() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sawtooth,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val_0 = lfo.calculate(0.0, 0.0, 120.0, 0.0, &audio, 0.0);
        let val_half = lfo.calculate(0.0, 0.5, 120.0, 0.0, &audio, 0.0);
        assert!((val_0 - (-1.0)).abs() < 1e-5);
        assert!((val_half - 0.0).abs() < 1e-5);
    }

    #[test]
    fn lfo_amplitude_scales() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 0.5,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        for i in 0..100 {
            let t = i as f32 / 100.0;
            let val = lfo.calculate(t, 0.01, 120.0, 0.0, &audio, 0.0);
            assert!((-0.5..=0.5).contains(&val), "Amplitude scaling off: {val}");
        }
    }

    #[test]
    fn lfo_frequency_affects_period() {
        let mut lfo_slow = ModulationSource::sine_lfo(1.0);
        let mut lfo_fast = ModulationSource::sine_lfo(2.0);
        let audio = empty_audio();
        let slow = lfo_slow.calculate(0.0, 0.25, 120.0, 0.0, &audio, 0.0);
        let fast = lfo_fast.calculate(0.0, 0.25, 120.0, 0.0, &audio, 0.0);
        assert!((slow - fast).abs() > 0.1);
    }

    #[test]
    fn tempo_sync_lfo_frequency() {
        // Direct helper check: 1 beat at 120 BPM = 2 Hz
        assert!((beat_division_to_hz(4, 120.0) - 2.0).abs() < 0.001);

        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: true,
            division: 4, // 1 beat
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        // At 120 BPM, 1 beat = 0.5 s cycle → 2 Hz.
        // After 0.25 s, phase = 0.5 → sin(π) = 0.
        let val = lfo.calculate(0.0, 0.25, 120.0, 0.0, &audio, 0.0);
        assert!(
            (val - 0.0).abs() < 0.01,
            "Tempo-sync LFO at 120 BPM/1 beat should be at zero crossing at 0.25s, got {val}"
        );
    }

    #[test]
    fn quantum_snap_resets_phase() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 1.0,
            phase: 0.25,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: true,
            division: 2, // ≤ 1 beat, so snap applies
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.9,
        };
        let audio = empty_audio();
        // beat_phase wraps from 0.9 to 0.1 → snap should reset phase to 0.0
        let val = lfo.calculate(0.0, 0.0, 120.0, 0.1, &audio, 0.0);
        assert!((val - 0.0).abs() < 0.01, "Phase should snap to 0 on quantum boundary, got {val}");
    }

    #[test]
    fn lfo_random_deterministic() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Random,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val1 = lfo.calculate(0.3, 0.01, 120.0, 0.0, &audio, 0.0);
        let val2 = lfo.calculate(0.3, 0.01, 120.0, 0.0, &audio, 0.0);
        assert_eq!(
            val1, val2,
            "Random LFO should be deterministic for same time"
        );
    }

    // ── ADSR tests ───────────────────────────────────────────────────

    #[test]
    fn adsr_idle_is_zero() {
        let mut adsr = ModulationSource::adsr(0.1, 0.1, 0.5, 0.1);
        let audio = empty_audio();
        let val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, 0.0);
        assert_eq!(val, 0.0);
    }

    #[test]
    fn adsr_attack_reaches_peak() {
        let mut adsr = ModulationSource::adsr(0.1, 0.1, 0.5, 0.1);
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..20 {
            val = adsr.calculate(0.0, 0.01, 120.0, 0.0, &audio, val);
        }
        assert!(
            val > 0.4,
            "ADSR should reach significant level during attack: {val}"
        );
    }

    #[test]
    fn adsr_sustain_holds() {
        let mut adsr = ModulationSource::adsr(0.01, 0.01, 0.7, 0.01);
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..100 {
            val = adsr.calculate(0.0, 0.01, 120.0, 0.0, &audio, val);
        }
        assert!(
            (val - 0.7).abs() < 0.05,
            "ADSR should hold at sustain level: {val}"
        );
    }

    #[test]
    fn adsr_release_to_zero() {
        let mut adsr = ModulationSource::adsr(0.01, 0.01, 0.7, 0.05);
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..50 {
            val = adsr.calculate(0.0, 0.01, 120.0, 0.0, &audio, val);
        }
        adsr.gate_off();
        for _ in 0..50 {
            val = adsr.calculate(0.0, 0.01, 120.0, 0.0, &audio, val);
        }
        assert!(val < 0.05, "ADSR should release to near zero: {val}");
    }

    #[test]
    fn adsr_gate_off_noop_when_idle() {
        let mut adsr = ModulationSource::adsr(0.1, 0.1, 0.5, 0.1);
        adsr.gate_off();
        let audio = empty_audio();
        let val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, 0.0);
        assert_eq!(val, 0.0);
    }

    // ── StepSequencer tests ──────────────────────────────────────────

    #[test]
    fn step_sequencer_basic() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.0, 0.5, 1.0, 0.5],
            rate: 4.0,
            interpolation: StepInterpolation::None,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(0.0, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!((val - 0.0).abs() < 1e-5);
        let val = seq.calculate(0.25, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!((val - 0.5).abs() < 1e-5);
    }

    #[test]
    fn step_sequencer_linear_interpolation() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.0, 1.0],
            rate: 1.0,
            interpolation: StepInterpolation::Linear,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(0.5, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!((val - 0.5).abs() < 0.01, "Linear interp mid: {val}");
    }

    #[test]
    fn step_sequencer_bipolar() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.0, 1.0],
            rate: 1.0,
            interpolation: StepInterpolation::None,
            bipolar: true,
        };
        let audio = empty_audio();
        let val = seq.calculate(0.0, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!((val - (-1.0)).abs() < 1e-5);
        let val = seq.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!((val - 1.0).abs() < 1e-5);
    }

    #[test]
    fn step_sequencer_empty_returns_zero() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![],
            rate: 1.0,
            interpolation: StepInterpolation::None,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(0.5, 0.01, 120.0, 0.0, &audio, 0.0);
        assert_eq!(val, 0.0);
    }

    #[test]
    fn step_sequencer_smooth_interpolation() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.0, 1.0],
            rate: 1.0,
            interpolation: StepInterpolation::Smooth,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(0.5, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!(val > 0.0 && val < 1.0, "Smooth interp: {val}");
        assert!(
            (val - 0.5).abs() < 0.01,
            "Smoothstep at 0.5 should be 0.5: {val}"
        );
    }

    // ── AudioSourceValues tests ──────────────────────────────────────

    #[test]
    fn audio_energy_empty_fft() {
        let source = AudioSourceValues {
            fft: &[],
            level: 0.0,
            sample_rate: 48000.0,
        };
        assert_eq!(source.energy_in_range(20.0, 250.0), 0.0);
    }

    #[test]
    fn audio_energy_zero_sample_rate() {
        let source = AudioSourceValues {
            fft: &[0.5; 256],
            level: 0.5,
            sample_rate: 0.0,
        };
        assert_eq!(source.energy_in_range(20.0, 250.0), 0.0);
    }

    #[test]
    fn audio_energy_silent() {
        let source = AudioSourceValues {
            fft: &[0.0; 256],
            level: 0.0,
            sample_rate: 48000.0,
        };
        assert_eq!(source.energy_in_range(20.0, 250.0), 0.0);
    }

    #[test]
    fn audio_energy_loud_signal() {
        let source = AudioSourceValues {
            fft: &[1.0; 256],
            level: 1.0,
            sample_rate: 48000.0,
        };
        let energy = source.energy_in_range(20.0, 20000.0);
        assert!((energy - 1.0).abs() < 0.01, "Full signal energy: {energy}");
    }

    #[test]
    fn audio_values_primary_returns_lowest_id() {
        let mut av = AudioValues::default();
        av.sources.insert(
            5,
            AudioSourceValues {
                fft: &[],
                level: 0.5,
                sample_rate: 48000.0,
            },
        );
        av.sources.insert(
            2,
            AudioSourceValues {
                fft: &[],
                level: 0.8,
                sample_rate: 48000.0,
            },
        );
        let primary = av.primary().unwrap();
        assert!((primary.level - 0.8).abs() < 1e-5);
    }

    #[test]
    fn audio_values_primary_none_when_empty() {
        let av = AudioValues::default();
        assert!(av.primary().is_none());
    }

    // ── ModulationEngine tests ───────────────────────────────────────

    #[test]
    fn engine_add_source_returns_uuid() {
        let mut engine = ModulationEngine::new();
        let uuid0 = engine.add_source(ModulationSource::sine_lfo(1.0));
        let uuid1 = engine.add_source(ModulationSource::sine_lfo(2.0));
        assert_ne!(uuid0, uuid1);
        assert_eq!(engine.source_count(), 2);
    }

    #[test]
    fn engine_remove_source_cleans_assignments() {
        let mut engine = ModulationEngine::new();
        let uuid0 = engine.add_source(ModulationSource::sine_lfo(1.0));
        engine.add_source(ModulationSource::sine_lfo(2.0));
        let uuid2 = engine.add_source(ModulationSource::sine_lfo(3.0));
        engine.assign("param_a", &uuid0, 1.0, None);
        engine.assign("param_b", &uuid2, 0.5, None);
        engine.remove_source(&uuid0);
        assert!(!engine.has_modulation("param_a"));
        assert!(engine.has_modulation("param_b"));
        assert_eq!(engine.source_count(), 2);
    }

    #[test]
    fn engine_assign_and_get_modulation() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::sine_lfo(1.0));
        engine.update(0.25, 120.0, 0.0, &empty_audio());
        engine.assign("brightness", &uuid, 1.0, None);
        let _mod_val = engine.get_modulation("brightness");
    }

    #[test]
    fn engine_clear_assignments() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::sine_lfo(1.0));
        engine.assign("brightness", &uuid, 1.0, None);
        assert!(engine.has_modulation("brightness"));
        engine.clear_assignments("brightness");
        assert!(!engine.has_modulation("brightness"));
    }

    #[test]
    fn engine_update_computes_values() {
        let mut engine = ModulationEngine::new();
        engine.add_source(ModulationSource::sine_lfo(1.0));
        engine.update(0.0, 120.0, 0.0, &empty_audio());
        let values = engine.current_values();
        assert_eq!(values.len(), 1);
    }

    #[test]
    fn engine_mod_on_mod() {
        let mut engine = ModulationEngine::new();
        let lfo0 = engine.add_source(ModulationSource::sine_lfo(1.0));
        let lfo1 = engine.add_source(ModulationSource::sine_lfo(2.0));
        engine.assign_mod_on_mod(&lfo0, "frequency", &lfo1, 0.5);
        engine.update(1.0, 120.0, 0.0, &empty_audio());
        assert!(engine.current_values().len() == 2);
    }

    #[test]
    fn engine_clear_mod_on_mod() {
        let mut engine = ModulationEngine::new();
        let lfo0 = engine.add_source(ModulationSource::sine_lfo(1.0));
        let lfo1 = engine.add_source(ModulationSource::sine_lfo(2.0));
        engine.assign_mod_on_mod(&lfo0, "frequency", &lfo1, 0.5);
        assert!(engine.has_modulation(&format!("mod:{}:frequency", lfo0)));
        engine.clear_mod_on_mod(&lfo0, "frequency");
        assert!(!engine.has_modulation(&format!("mod:{}:frequency", lfo0)));
    }

    #[test]
    fn engine_trigger_adsr() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::adsr(0.01, 0.01, 0.5, 0.01));
        engine.trigger_adsr(&uuid);
        for i in 0..20 {
            engine.update(i as f32 * 0.01, 120.0, 0.0, &empty_audio());
        }
        let val = engine.current_value_for(&uuid);
        assert!(val > 0.0, "ADSR should produce non-zero after trigger");
    }

    #[test]
    fn engine_release_adsr() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::adsr(0.01, 0.01, 0.5, 0.01));
        engine.trigger_adsr(&uuid);
        for i in 0..30 {
            engine.update(i as f32 * 0.01, 120.0, 0.0, &empty_audio());
        }
        engine.release_adsr(&uuid);
        for i in 30..80 {
            engine.update(i as f32 * 0.01, 120.0, 0.0, &empty_audio());
        }
        let val = engine.current_value_for(&uuid);
        assert!(val < 0.1, "ADSR should be near zero after release: {}", val);
    }

    #[test]
    fn engine_get_modulation_nonexistent_param() {
        let engine = ModulationEngine::new();
        assert_eq!(engine.get_modulation("nonexistent"), 0.0);
    }

    #[test]
    fn engine_evaluation_order_no_deps() {
        let mut engine = ModulationEngine::new();
        engine.add_source(ModulationSource::sine_lfo(1.0));
        engine.add_source(ModulationSource::sine_lfo(2.0));
        let order = engine.evaluation_order();
        assert_eq!(order.len(), 2);
    }

    #[test]
    fn engine_component_modulation() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::sine_lfo(1.0));
        engine.update(0.25, 120.0, 0.0, &empty_audio());
        engine.assign("color", &uuid, 1.0, Some(0));
        engine.assign("color", &uuid, 0.5, Some(1));
        let r_mod = engine.get_modulation_for_component("color", Some(0));
        let g_mod = engine.get_modulation_for_component("color", Some(1));
        let no_mod = engine.get_modulation_for_component("color", Some(2));
        // Unassigned component has no modulation.
        assert_eq!(no_mod, 0.0);
        // Same source drives both assigned components; component 1's scale (0.5)
        // is half of component 0's (1.0), so its modulation is half.
        assert!(
            (g_mod - 0.5 * r_mod).abs() < 1e-6,
            "g_mod={g_mod} r_mod={r_mod}"
        );
    }

    // ── AudioBandPreset tests ────────────────────────────────────────

    #[test]
    fn audio_band_preset_ranges() {
        assert_eq!(AudioBandPreset::Low.freq_range(), (20.0, 250.0));
        assert_eq!(AudioBandPreset::Mid.freq_range(), (250.0, 2000.0));
        assert_eq!(AudioBandPreset::High.freq_range(), (2000.0, 20000.0));
        assert_eq!(AudioBandPreset::Full.freq_range(), (20.0, 20000.0));
    }

    #[test]
    fn audio_band_from_preset_creates_valid_source() {
        let source = ModulationSource::audio_from_preset(AudioBandPreset::Low);
        match source {
            ModulationSource::AudioBand {
                freq_low,
                freq_high,
                gain,
                ..
            } => {
                assert_eq!(freq_low, 20.0);
                assert_eq!(freq_high, 250.0);
                assert_eq!(gain, 1.0);
            }
            _ => panic!("Expected AudioBand"),
        }
    }

    // ── Constructor tests ────────────────────────────────────────────

    #[test]
    fn step_sequencer_min_steps() {
        let seq = ModulationSource::step_sequencer(1, 1.0);
        match seq {
            ModulationSource::StepSequencer { steps, .. } => {
                assert_eq!(steps.len(), 2);
            }
            _ => panic!("Expected StepSequencer"),
        }
    }

    #[test]
    fn parse_mod_target_valid() {
        assert_eq!(
            ModulationEngine::parse_mod_target("mod:abc123:frequency"),
            Some("abc123")
        );
        assert_eq!(
            ModulationEngine::parse_mod_target("mod:def456:phase"),
            Some("def456")
        );
    }

    #[test]
    fn parse_mod_target_invalid() {
        assert_eq!(ModulationEngine::parse_mod_target("brightness"), None);
        assert_eq!(ModulationEngine::parse_mod_target("deck0:param"), None);
    }

    // ── Audio band with noise gate ───────────────────────────────────

    #[test]
    fn audio_band_noise_gate() {
        let mut source = ModulationSource::AudioBand {
            source_id: Some(0),
            freq_low: 20.0,
            freq_high: 250.0,
            gain: 1.0,
            smoothing: 0.0,
            mode: AudioReactMode::Direct,
            noise_gate: 0.5,
        };
        let mut audio = AudioValues::default();
        audio.sources.insert(
            0,
            AudioSourceValues {
                fft: &[0.001; 256],
                level: 0.001,
                sample_rate: 48000.0,
            },
        );
        let val = source.calculate(0.0, 0.01, 120.0, 0.0, &audio, 0.0);
        assert_eq!(val, 0.0, "Below noise gate should be silent");
    }

    // ── config_eq tests ──────────────────────────────────────────────

    #[test]
    fn config_eq_lfo_same() {
        let a = ModulationSource::sine_lfo(2.0);
        let b = ModulationSource::sine_lfo(2.0);
        assert!(a.config_eq(&b));
    }

    #[test]
    fn config_eq_lfo_different_freq() {
        let a = ModulationSource::sine_lfo(2.0);
        let b = ModulationSource::sine_lfo(3.0);
        assert!(!a.config_eq(&b));
    }

    #[test]
    fn config_eq_adsr_ignores_runtime() {
        let a = ModulationSource::ADSR {
            attack: 0.1,
            decay: 0.2,
            sustain: 0.7,
            release: 0.3,
            stage: ADSRStage::Idle,
            stage_time: 0.0,
            gate: false,
            current_level: 0.0,
        };
        let b = ModulationSource::ADSR {
            attack: 0.1,
            decay: 0.2,
            sustain: 0.7,
            release: 0.3,
            stage: ADSRStage::Attack,
            stage_time: 1.5,
            gate: true,
            current_level: 0.8,
        };
        assert!(a.config_eq(&b));
    }

    #[test]
    fn config_eq_different_variants() {
        let a = ModulationSource::sine_lfo(2.0);
        let b = ModulationSource::adsr(0.1, 0.2, 0.7, 0.3);
        assert!(!a.config_eq(&b));
    }

    // ── find_source_by_uuid tests ───────────────────────────────────

    #[test]
    fn find_source_by_uuid_found() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::sine_lfo(2.0));
        assert!(engine.find_source_by_uuid(&uuid).is_some());
    }

    #[test]
    fn find_source_by_uuid_not_found() {
        let engine = ModulationEngine::new();
        assert!(engine.find_source_by_uuid("nonexistent").is_none());
    }

    #[test]
    fn add_source_with_uuid_preserves_uuid() {
        let mut engine = ModulationEngine::new();
        let uuid =
            engine.add_source_with_uuid("custom01".to_string(), ModulationSource::sine_lfo(2.0));
        assert_eq!(uuid, "custom01");
        assert!(engine.has_source("custom01"));
    }

    // ── Gap coverage: chains, removal, edge cases ───────────────────

    #[test]
    fn circular_mod_on_mod_no_hang() {
        let mut engine = ModulationEngine::new();
        let a = engine.add_source(ModulationSource::sine_lfo(1.0));
        let b = engine.add_source(ModulationSource::sine_lfo(2.0));
        let c = engine.add_source(ModulationSource::sine_lfo(3.0));
        // A modulates B, B modulates C, C modulates A (cycle)
        engine.assign_mod_on_mod(&b, "frequency", &a, 0.5);
        engine.assign_mod_on_mod(&c, "frequency", &b, 0.5);
        engine.assign_mod_on_mod(&a, "frequency", &c, 0.5);
        // Must complete without hanging, values must be finite
        let audio = AudioValues::default();
        engine.update(1.0, 120.0, 0.0, &audio);
        for v in engine.current_values() {
            assert!(v.is_finite(), "circular chain produced non-finite value");
        }
    }

    #[test]
    fn deep_chain_fallback() {
        let mut engine = ModulationEngine::new();
        let mut uuids = Vec::new();
        for i in 0..5 {
            uuids.push(engine.add_source(ModulationSource::sine_lfo((i + 1) as f32)));
        }
        // Chain: 0→1→2→3→4
        for i in 0..4 {
            engine.assign_mod_on_mod(&uuids[i + 1], "frequency", &uuids[i], 0.1);
        }
        let audio = AudioValues::default();
        engine.update(1.0, 120.0, 0.0, &audio);
        // All 5 sources should have been evaluated
        assert_eq!(engine.current_values().len(), 5);
        for v in engine.current_values() {
            assert!(v.is_finite());
        }
    }

    #[test]
    fn evaluation_order_respects_deps() {
        let mut engine = ModulationEngine::new();
        let a = engine.add_source(ModulationSource::sine_lfo(1.0));
        let b = engine.add_source(ModulationSource::sine_lfo(2.0));
        // A modulates B → A must be evaluated before B
        engine.assign_mod_on_mod(&b, "frequency", &a, 0.5);
        let order = engine.evaluation_order();
        let a_pos = order
            .iter()
            .position(|&i| i == engine.sources.iter().position(|e| e.uuid == a).unwrap())
            .unwrap();
        let b_pos = order
            .iter()
            .position(|&i| i == engine.sources.iter().position(|e| e.uuid == b).unwrap())
            .unwrap();
        assert!(
            a_pos < b_pos,
            "dependency A should be evaluated before target B"
        );
    }

    #[test]
    fn remove_source_mid_chain() {
        let mut engine = ModulationEngine::new();
        let a = engine.add_source(ModulationSource::sine_lfo(1.0));
        let b = engine.add_source(ModulationSource::sine_lfo(2.0));
        let c = engine.add_source(ModulationSource::sine_lfo(3.0));
        engine.assign_mod_on_mod(&b, "frequency", &a, 0.5);
        engine.assign_mod_on_mod(&c, "frequency", &b, 0.5);
        // Remove the middle source
        engine.remove_source(&b);
        assert_eq!(engine.source_count(), 2);
        // Should still update without panic
        let audio = AudioValues::default();
        engine.update(1.0, 120.0, 0.0, &audio);
        assert!(engine.has_source(&a));
        assert!(engine.has_source(&c));
    }

    #[test]
    fn index_consistency_after_removal() {
        let mut engine = ModulationEngine::new();
        let a = engine.add_source(ModulationSource::sine_lfo(1.0));
        let _b = engine.add_source(ModulationSource::sine_lfo(2.0));
        let c = engine.add_source(ModulationSource::sine_lfo(3.0));
        engine.remove_source(&_b);
        // UUIDs a and c should still resolve correctly
        assert!(engine.find_source_by_uuid(&a).is_some());
        assert!(engine.find_source_by_uuid(&c).is_some());
        assert_eq!(engine.source_count(), 2);
    }

    #[test]
    fn empty_source_list_update() {
        let mut engine = ModulationEngine::new();
        let audio = AudioValues::default();
        // Update with 0 sources → no crash
        engine.update(0.0, 120.0, 0.0, &audio);
        assert_eq!(engine.source_count(), 0);
        assert!(engine.current_values().is_empty());
    }

    #[test]
    fn mod_on_mod_removed_target() {
        let mut engine = ModulationEngine::new();
        let a = engine.add_source(ModulationSource::sine_lfo(1.0));
        let b = engine.add_source(ModulationSource::sine_lfo(2.0));
        engine.assign_mod_on_mod(&a, "frequency", &b, 0.5);
        // Remove the target — assignments should be cleaned up
        engine.remove_source(&a);
        assert!(!engine.has_source(&a));
        // The mod-on-mod key "mod:{a}:frequency" should have been purged
        for key in engine.assignments_iter().map(|(k, _)| k) {
            assert!(
                !key.contains(&a),
                "stale mod-on-mod key found after target removal"
            );
        }
    }

    #[test]
    fn assign_nonexistent_source_ignored() {
        let mut engine = ModulationEngine::new();
        engine.assign("some_param", "bogus_uuid", 1.0, None);
        // No assignment should have been created
        assert!(!engine.has_modulation("some_param"));
    }

    // ── Chaos Tests: LFO edge values ─────────────────────────────────

    #[test]
    fn chaos_lfo_zero_frequency_does_not_nan() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 0.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        for i in 0..100 {
            let val = lfo.calculate(i as f32 * 0.01, 0.01, 120.0, 0.0, &audio, 0.0);
            assert!(val.is_finite(), "LFO freq=0 produced non-finite: {val}");
        }
    }

    #[test]
    fn chaos_lfo_infinity_frequency_does_not_panic() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: f32::INFINITY,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val = lfo.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        // (Inf * 1.0 + 0.0) % 1.0 = NaN — document this
        let _ = val; // must not panic
    }

    #[test]
    fn chaos_lfo_nan_frequency_does_not_panic() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: f32::NAN,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val = lfo.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        let _ = val; // must not panic
    }

    #[test]
    fn chaos_lfo_nan_amplitude_does_not_panic() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Triangle,
            frequency: 1.0,
            phase: 0.0,
            amplitude: f32::NAN,
            bipolar: false,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val = lfo.calculate(0.5, 0.01, 120.0, 0.0, &audio, 0.0);
        let _ = val; // must not panic
    }

    #[test]
    fn chaos_lfo_negative_frequency_does_not_panic() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sawtooth,
            frequency: -10.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val = lfo.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!(
            val.is_finite(),
            "negative freq should produce finite: {val}"
        );
    }

    #[test]
    fn chaos_lfo_all_waveforms_at_extreme_time() {
        let audio = empty_audio();
        for waveform in [
            LFOWaveform::Sine,
            LFOWaveform::Square,
            LFOWaveform::Triangle,
            LFOWaveform::Sawtooth,
            LFOWaveform::Random,
        ] {
            let mut lfo = ModulationSource::LFO {
                waveform,
                frequency: 1e6,
                phase: 0.0,
                amplitude: 1.0,
                bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
            };
            let val = lfo.calculate(1e10, 0.01, 120.0, 0.0, &audio, 0.0);
            let _ = val; // must not panic
        }
    }

    // ── Chaos Tests: Step Sequencer edge cases ───────────────────────

    #[test]
    fn chaos_step_sequencer_single_step() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.75],
            rate: 1.0,
            interpolation: StepInterpolation::Linear,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(0.5, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!(val.is_finite(), "single step produced non-finite: {val}");
    }

    #[test]
    fn chaos_step_sequencer_nan_rate_does_not_panic() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.0, 0.5, 1.0],
            rate: f32::NAN,
            interpolation: StepInterpolation::None,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        let _ = val; // must not panic
    }

    #[test]
    fn chaos_step_sequencer_infinity_rate_does_not_panic() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.0, 1.0],
            rate: f32::INFINITY,
            interpolation: StepInterpolation::Smooth,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        let _ = val; // must not panic
    }

    #[test]
    fn chaos_step_sequencer_zero_rate() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![0.2, 0.8],
            rate: 0.0,
            interpolation: StepInterpolation::Linear,
            bipolar: false,
        };
        let audio = empty_audio();
        let val = seq.calculate(1.0, 0.01, 120.0, 0.0, &audio, 0.0);
        assert!(val.is_finite(), "zero rate produced non-finite: {val}");
    }

    #[test]
    fn chaos_step_sequencer_nan_step_values() {
        let mut seq = ModulationSource::StepSequencer {
            steps: vec![f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5],
            rate: 1.0,
            interpolation: StepInterpolation::Linear,
            bipolar: false,
        };
        let audio = empty_audio();
        for i in 0..20 {
            let val = seq.calculate(i as f32 * 0.25, 0.01, 120.0, 0.0, &audio, 0.0);
            let _ = val; // must not panic
        }
    }

    // ── Chaos Tests: ADSR edge cases ─────────────────────────────────

    #[test]
    fn chaos_adsr_zero_all_times() {
        let mut adsr = ModulationSource::adsr(0.0, 0.0, 0.5, 0.0);
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..50 {
            val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, val);
            assert!(val.is_finite(), "zero-time ADSR produced non-finite: {val}");
        }
        adsr.gate_off();
        for _ in 0..50 {
            val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, val);
            assert!(val.is_finite(), "zero-time ADSR release non-finite: {val}");
        }
    }

    #[test]
    fn chaos_adsr_nan_attack_does_not_panic() {
        let mut adsr = ModulationSource::ADSR {
            attack: f32::NAN,
            decay: 0.1,
            sustain: 0.5,
            release: 0.1,
            stage: ADSRStage::Idle,
            stage_time: 0.0,
            gate: false,
            current_level: 0.0,
        };
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..20 {
            val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, val);
        }
        // must not panic
    }

    #[test]
    fn chaos_adsr_negative_sustain() {
        let mut adsr = ModulationSource::adsr(0.01, 0.01, -1.0, 0.01);
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..100 {
            val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, val);
        }
        // Sustain = -1.0 may produce negative values — document, must not panic
    }

    #[test]
    fn chaos_adsr_infinity_release() {
        let mut adsr = ModulationSource::adsr(0.01, 0.01, 0.5, f32::INFINITY);
        adsr.gate_on();
        let audio = empty_audio();
        let mut val = 0.0;
        for _ in 0..50 {
            val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, val);
        }
        adsr.gate_off();
        for _ in 0..50 {
            val = adsr.calculate(0.0, 0.016, 120.0, 0.0, &audio, val);
            // progress = stage_time / INFINITY = 0 — never completes release
        }
        // must not panic
    }

    // ── Serialize round-trip test (REQ-06.1) ─────────────────────────

    #[test]
    fn serialize_round_trip() {
        let mut engine = ModulationEngine::new();
        let lfo = engine.add_source_with_uuid("lfo01".to_string(), ModulationSource::sine_lfo(1.0));
        let adsr = engine.add_source_with_uuid(
            "adsr01".to_string(),
            ModulationSource::adsr(0.1, 0.2, 0.7, 0.3),
        );
        let seq = engine.add_source_with_uuid(
            "seq01".to_string(),
            ModulationSource::step_sequencer(4, 2.0),
        );
        engine.assign("brightness", &lfo, 0.5, None);
        engine.assign("color", &lfo, 1.0, Some(0));
        engine.assign("color", &adsr, 0.25, Some(1));
        engine.assign_mod_on_mod(&lfo, "frequency", &seq, 0.1);

        let json = serde_json::to_string(&engine).expect("serialize");
        let mut restored: ModulationEngine = serde_json::from_str(&json).expect("deserialize");
        restored.ensure_index();

        assert_eq!(restored.source_count(), 3);
        assert!(restored.has_source("lfo01"));
        assert!(restored.has_source("adsr01"));
        assert!(restored.has_source("seq01"));
        assert!(restored.has_modulation("brightness"));
        assert!(restored.has_modulation("color"));
        assert!(restored.has_modulation(&format!("mod:{}:frequency", lfo)));

        // Verify source configs match
        let original_lfo = engine.find_source_by_uuid("lfo01").unwrap();
        let restored_lfo = restored.find_source_by_uuid("lfo01").unwrap();
        assert!(original_lfo.source.config_eq(&restored_lfo.source));
    }

    // ── Vector stability test (REQ-06.2) ────────────────────────────
    // NOTE: this checks that `current_values` vector length is stable,
    // not that `update()` is allocation-free. See PERF-3: `evaluation_order`
    // and `apply_mod_on_mod` still allocate per tick when modulation is active.

    #[test]
    fn update_does_not_grow_vectors_after_first_tick() {
        let mut engine = ModulationEngine::new();
        for i in 0..16 {
            engine.add_source(ModulationSource::sine_lfo(i as f32 + 1.0));
        }
        // First tick may grow vectors
        engine.update(0.0, 120.0, 0.0, &empty_audio());
        let prev_len = engine.current_values().len();
        assert_eq!(prev_len, 16);

        // Subsequent ticks must not change length (no push calls)
        for i in 1..100 {
            engine.update(i as f32 * 0.016, 120.0, 0.0, &empty_audio());
            assert_eq!(
                engine.current_values().len(),
                prev_len,
                "current_values changed length at tick {i}"
            );
        }
    }

    // ── UUID generation format test ──────────────────────────────────

    #[test]
    fn generated_uuid_format() {
        let entry = ModulationSourceEntry::new(ModulationSource::sine_lfo(1.0));
        assert_eq!(entry.uuid.len(), 8, "UUID should be 8 chars");
        assert!(
            entry.uuid.chars().all(|c| c.is_ascii_hexdigit()),
            "UUID should be hex: {}",
            entry.uuid
        );
    }

    #[test]
    fn generated_uuid_unique() {
        let ids: Vec<String> = (0..100)
            .map(|_| ModulationSourceEntry::new(ModulationSource::sine_lfo(1.0)).uuid)
            .collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 100, "100 UUIDs should all be unique");
    }

    // ── Adapter tests ────────────────────────────────────────────────

    #[test]
    fn lfo_bank_adapter_preserves_8_sources() {
        let bank = crate::LfoBank::new();
        let sources = bank.to_modulation_sources();
        assert_eq!(sources.len(), 8, "LfoBank has 8 fixed LFOs");
    }

    #[test]
    fn lfo_bank_adapter_maps_default_targets() {
        let bank = crate::LfoBank::new();
        let engine = bank.to_modulation_engine(120.0);
        assert!(engine.has_modulation("hue_shift"));
        assert!(engine.has_modulation("saturation"));
        assert!(engine.has_modulation("brightness"));
    }

    #[test]
    fn routing_matrix_adapter_maps_routes() {
        let matrix = crate::RoutingMatrix::with_defaults();
        let engine = matrix.to_modulation_engine();
        // with_defaults() adds Bass→Brightness and High→Saturation
        assert!(engine.has_modulation("brightness"));
        assert!(engine.has_modulation("saturation"));
    }

    // ── Enabled / web-remote integration tests (F2 + S2) ─────────────

    #[test]
    fn disabled_lfo_returns_zero() {
        let mut lfo = ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: false,
            last_beat_phase: 0.0,
        };
        let audio = empty_audio();
        let val = lfo.calculate(0.0, 0.1, 120.0, 0.0, &audio, 0.0);
        assert_eq!(val, 0.0, "Disabled LFO must return 0.0");
    }

    #[test]
    fn engine_respects_lfo_enabled_flag() {
        let mut engine = ModulationEngine::new();
        let uuid = engine.add_source(ModulationSource::LFO {
            waveform: LFOWaveform::Sine,
            frequency: 1.0,
            phase: 0.0,
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 2,
            phase_offset_degrees: 0.0,
            enabled: true,
            last_beat_phase: 0.0,
        });
        engine.assign("test_param", &uuid, 1.0, None);

        let audio = empty_audio();
        engine.update(0.0, 120.0, 0.0, &audio);
        let active_val = engine.get_modulation("test_param");
        assert!(active_val.abs() > 0.01, "Enabled LFO should produce non-zero modulation");

        // Disable the source (simulating web LfoEnable { slot: 0, enabled: false })
        if let Some(entry) = engine.sources.iter_mut().find(|s| s.uuid == uuid) {
            if let ModulationSource::LFO { ref mut enabled, .. } = entry.source {
                *enabled = false;
            }
        }
        engine.update(0.1, 120.0, 0.0, &audio);
        let disabled_val = engine.get_modulation("test_param");
        assert_eq!(disabled_val, 0.0, "Disabled LFO should produce zero modulation");
    }

    #[test]
    fn web_lfo_set_replaces_source_preserving_phase() {
        // Simulates the F2 web command handler logic: replace source at lfo_0
        // while preserving runtime phase and last_beat_phase.
        let mut engine = ModulationEngine::new();
        engine.add_source_with_uuid(
            "lfo_0".to_string(),
            ModulationSource::LFO {
                waveform: LFOWaveform::Sine,
                frequency: 1.0,
                phase: 0.5,
                amplitude: 0.5,
                bipolar: true,
                tempo_sync: false,
                division: 2,
                phase_offset_degrees: 0.0,
                enabled: true,
                last_beat_phase: 0.8,
            },
        );

        // Simulate web LfoSet replacing the source
        let mut new_source = ModulationSource::LFO {
            waveform: LFOWaveform::Triangle,
            frequency: 2.0,
            phase: 0.0, // should be overwritten with existing
            amplitude: 1.0,
            bipolar: true,
            tempo_sync: false,
            division: 4,
            phase_offset_degrees: 90.0,
            enabled: true,
            last_beat_phase: 0.0, // should be overwritten with existing
        };
        if let Some(idx) = engine.sources.iter().position(|s| s.uuid == "lfo_0") {
            let existing_phase = if let ModulationSource::LFO { phase, .. } = engine.sources[idx].source { phase } else { 0.0 };
            let existing_last = if let ModulationSource::LFO { last_beat_phase, .. } = engine.sources[idx].source { last_beat_phase } else { 0.0 };
            engine.sources[idx].source = match new_source {
                ModulationSource::LFO { ref mut phase, ref mut last_beat_phase, .. } => {
                    *phase = existing_phase;
                    *last_beat_phase = existing_last;
                    new_source
                }
                _ => new_source,
            };
        }

        if let ModulationSource::LFO { phase, last_beat_phase, waveform, .. } = &engine.sources[0].source {
            assert_eq!(*phase, 0.5, "Phase should be preserved");
            assert_eq!(*last_beat_phase, 0.8, "last_beat_phase should be preserved");
            assert_eq!(*waveform, LFOWaveform::Triangle, "Waveform should be updated");
        } else {
            panic!("Expected LFO source");
        }
    }
}
