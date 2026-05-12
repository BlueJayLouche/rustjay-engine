//! # LFO (Low Frequency Oscillator) System
//!
//! 3 LFOs - one for each HSB parameter (Hue, Saturation, Brightness)
//! Tempo-syncable with phase offset support

use serde::{Deserialize, Serialize};
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
pub const BEAT_DIVISION_NAMES: [&str; 8] = [
    "1/16", "1/8", "1/4", "1/2", "1", "2", "4", "8"
];

/// Convert beat division index to frequency in Hz for a given BPM
pub fn beat_division_to_hz(division: usize, bpm: f32) -> f32 {
    let division = division.min(BEAT_DIVISIONS.len() - 1);
    let beats_per_cycle = BEAT_DIVISIONS[division];
    let beat_duration = 60.0 / bpm.max(1.0);
    let cycle_duration = beat_duration * beats_per_cycle;
    1.0 / cycle_duration
}

/// LFO Waveforms
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Waveform {
    Sine = 0,
    Triangle = 1,
    Ramp = 2,    // Upward ramp
    Saw = 3,     // Downward saw
    Square = 4,
}

impl Waveform {
    pub fn name(&self) -> &'static str {
        match self {
            Waveform::Sine => "Sine",
            Waveform::Triangle => "Triangle",
            Waveform::Ramp => "Ramp",
            Waveform::Saw => "Saw",
            Waveform::Square => "Square",
        }
    }
    
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

impl Default for Waveform {
    fn default() -> Self {
        Waveform::Sine
    }
}

/// Target parameter for LFO modulation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum LfoTarget {
    None = -1,
    HueShift = 0,
    Saturation = 1,
    Brightness = 2,
}

impl LfoTarget {
    pub fn name(&self) -> &'static str {
        match self {
            LfoTarget::None => "None",
            LfoTarget::HueShift => "Hue Shift",
            LfoTarget::Saturation => "Saturation",
            LfoTarget::Brightness => "Brightness",
        }
    }
    
    pub fn all() -> &'static [LfoTarget] {
        &[
            LfoTarget::HueShift,
            LfoTarget::Saturation,
            LfoTarget::Brightness,
        ]
    }
}

impl Default for LfoTarget {
    fn default() -> Self {
        LfoTarget::None
    }
}

/// Single LFO configuration and state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lfo {
    /// LFO index (0, 1, 2)
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
        }
    }
    
    /// Calculate the LFO output at current phase
    pub fn calculate_value(phase: f32, waveform: Waveform) -> f32 {
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
            Waveform::Ramp => 2.0 * phase - 1.0,     // -1 to 1 upward
            Waveform::Saw => 1.0 - 2.0 * phase,       // 1 to -1 downward
            Waveform::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
    
    /// Update LFO phase based on time/BPM
    /// 
    /// # Arguments
    /// * `bpm` - Current BPM
    /// * `delta_time` - Time since last frame in seconds
    /// * `beat_phase` - Current beat phase (0-1) from tap tempo, for syncing at 0°
    pub fn update(&mut self, bpm: f32, delta_time: f32, beat_phase: f32) {
        if !self.enabled || self.target == LfoTarget::None {
            self.output = 0.0;
            return;
        }
        
        // Calculate effective rate
        let rate_hz = if self.tempo_sync {
            // Calculate frequency from BPM and beat division
            let division = self.division.clamp(0, BEAT_DIVISIONS.len() - 1);
            let beat_duration = 60.0 / bpm.max(1.0); // seconds per beat
            let cycle_duration = beat_duration * BEAT_DIVISIONS[division];
            1.0 / cycle_duration
        } else {
            self.rate.clamp(0.01, 20.0)
        };
        
        // Update phase
        self.phase += rate_hz * delta_time;
        self.phase = self.phase % 1.0;
        
        // Calculate phase with offset
        // Phase offset: 0° = no offset, aligns with beat_phase at 0
        let offset_normalized = self.phase_offset / 360.0;
        let effective_phase = (self.phase + offset_normalized) % 1.0;
        
        // When phase_offset is 0, we want LFO to be at start of cycle when beat_phase is 0
        // This means: effective_phase should align with beat_phase
        let synced_phase = (effective_phase + beat_phase) % 1.0;
        
        // Calculate output
        let raw_value = Self::calculate_value(synced_phase, self.waveform);
        self.output = raw_value * self.amplitude;
    }
    
    /// Reset phase to 0
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.output = 0.0;
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

/// Collection of 3 LFOs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LfoBank {
    pub lfos: [Lfo; 3],
}

impl LfoBank {
    pub fn new() -> Self {
        Self {
            lfos: [
                Lfo::new(0),
                Lfo::new(1),
                Lfo::new(2),
            ],
        }
    }
    
    /// Update all LFOs
    pub fn update(&mut self, bpm: f32, delta_time: f32, beat_phase: f32) {
        for lfo in &mut self.lfos {
            lfo.update(bpm, delta_time, beat_phase);
        }
    }
    
    /// Get modulation values for HSB parameters
    /// Returns (hue_mod, sat_mod, bright_mod)
    pub fn get_hsb_modulations(&self) -> (f32, f32, f32) {
        let mut hue = 0.0;
        let mut sat = 0.0;
        let mut bright = 0.0;
        
        for lfo in &self.lfos {
            if !lfo.enabled {
                continue;
            }
            match lfo.target {
                LfoTarget::HueShift => hue = lfo.output,
                LfoTarget::Saturation => sat = lfo.output,
                LfoTarget::Brightness => bright = lfo.output,
                LfoTarget::None => {}
            }
        }
        
        (hue, sat, bright)
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
}

impl Default for LfoBank {
    fn default() -> Self {
        Self::new()
    }
}

/// LFO state for the app
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct LfoState {
    pub bank: LfoBank,
    /// Whether LFO window is shown
    #[serde(skip)]
    pub show_window: bool,
}

impl LfoState {
    pub fn new() -> Self {
        Self {
            bank: LfoBank::new(),
            show_window: false,
        }
    }
    
    /// Apply LFO modulations to base HSB values
    /// Returns modulated (hue, saturation, brightness)
    pub fn apply_to_hsb(&self, base_hue: f32, base_sat: f32, base_bright: f32) -> (f32, f32, f32) {
        let (hue_mod, sat_mod, bright_mod) = self.bank.get_hsb_modulations();
        
        // Apply modulation with appropriate ranges
        // Hue: add modulation * 180 degrees (full range)
        // Sat/Bright: add modulation * 2.0 (full range)
        let hue = (base_hue + hue_mod * 180.0).clamp(-180.0, 180.0);
        let sat = (base_sat + sat_mod * 2.0).clamp(0.0, 2.0);
        let bright = (base_bright + bright_mod * 2.0).clamp(0.0, 2.0);
        
        (hue, sat, bright)
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
