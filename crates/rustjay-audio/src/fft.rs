//! Lock-free audio I/O types and real-time FFT processing.
//!
//! All types in this module are safe to use from the real-time audio callback:
//! no allocations, no mutexes — only atomics.

// `process_audio_frame` takes pre-allocated `&mut Vec` buffers (documented) to
// avoid RT-thread allocation, and threads many params for the same reason.
#![allow(clippy::too_many_arguments, clippy::ptr_arg)]

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Lock-free audio output (written by real-time callback, read by main thread)
// ---------------------------------------------------------------------------

pub(crate) struct AudioOutput {
    pub fft: [AtomicU32; 8],
    /// Full per-bin FFT spectrum (dB-mapped 0–1, length = fft_size/2+1).
    pub spectrum: Vec<AtomicU32>,
    pub volume: AtomicU32,
    /// Set true by callback; atomically swapped false when read by main thread
    pub beat: AtomicBool,
    pub beat_phase: AtomicU32,
}

impl AudioOutput {
    pub fn new(fft_size: usize) -> Self {
        let spectrum_len = fft_size / 2 + 1;
        Self {
            fft: std::array::from_fn(|_| AtomicU32::new(0.0f32.to_bits())),
            spectrum: (0..spectrum_len)
                .map(|_| AtomicU32::new(0.0f32.to_bits()))
                .collect(),
            volume: AtomicU32::new(0.0f32.to_bits()),
            beat: AtomicBool::new(false),
            beat_phase: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn reset(&self) {
        for f in &self.fft {
            f.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
        for s in &self.spectrum {
            s.store(0.0f32.to_bits(), Ordering::Relaxed);
        }
        self.volume.store(0.0f32.to_bits(), Ordering::Relaxed);
        self.beat.store(false, Ordering::Relaxed);
        self.beat_phase.store(0.0f32.to_bits(), Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// Lock-free audio config (written by main thread, read by real-time callback)
// ---------------------------------------------------------------------------

pub(crate) struct AudioConfig {
    pub amplitude: AtomicU32,
    pub smoothing: AtomicU32,
    pub normalize: AtomicBool,
    pub pink_noise_shaping: AtomicBool,
}

impl AudioConfig {
    pub fn new() -> Self {
        Self {
            amplitude: AtomicU32::new(1.0f32.to_bits()),
            smoothing: AtomicU32::new(0.5f32.to_bits()),
            normalize: AtomicBool::new(true),
            pink_noise_shaping: AtomicBool::new(false),
        }
    }

    pub fn amplitude(&self) -> f32 {
        f32::from_bits(self.amplitude.load(Ordering::Relaxed))
    }

    pub fn smoothing(&self) -> f32 {
        f32::from_bits(self.smoothing.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Available FFT sizes
// ---------------------------------------------------------------------------

pub const FFT_SIZES: &[usize] = &[1024, 2048, 4096, 8192];
pub const DEFAULT_FFT_SIZE: usize = 4096;
pub const FFT_SIZE_LABELS: &[&str] = &[
    "1024  (43 Hz, 23ms)",
    "2048  (21 Hz, 46ms)",
    "4096  (11 Hz, 93ms)",
    "8192  (5 Hz, 186ms)",
];

// ---------------------------------------------------------------------------
// Real-time audio frame processing
// ---------------------------------------------------------------------------

/// Real-time callback frame processor — pre-allocated bufs avoid RT-thread allocation.
pub fn process_audio_frame(
    frame: &[f32],
    sample_rate: f32,
    hann_window: &[f32],
    r2c: &std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    scratch: &mut [rustfft::num_complex::Complex<f32>],
    windowed_buf: &mut Vec<f32>,
    spectrum_buf: &mut Vec<rustfft::num_complex::Complex<f32>>,
    magnitudes_buf: &mut Vec<f32>,
    beat_energy: &mut f32,
    beat_history: &mut VecDeque<f32>,
    beat_counter: &mut u32,
    norm_peak: &mut f32,
    output: &Arc<AudioOutput>,
    config: &Arc<AudioConfig>,
) {
    // Apply pre-computed Hann window in-place (no allocation, no transcendental calls)
    for ((s, w), w_out) in frame
        .iter()
        .zip(hann_window.iter())
        .zip(windowed_buf.iter_mut())
    {
        *w_out = s * w;
    }

    if r2c
        .process_with_scratch(windowed_buf, spectrum_buf, scratch)
        .is_err()
    {
        return;
    }

    // Read config atomically (no mutex, no blocking)
    let smoothing = config.smoothing();
    let amplitude = config.amplitude();
    let normalize = config.normalize.load(Ordering::Relaxed);
    let pink_noise_shaping = config.pink_noise_shaping.load(Ordering::Relaxed);

    // Compute per-bin magnitudes in dB-normalized 0-1 range (no allocation).
    //   1. Normalize by fft_size (realfft does NOT divide by N)
    //   2. Apply amplitude as gain (before dB — preserves dynamic range)
    //   3. Convert to dB, then map [-60 dB, 0 dB] → [0, 1]
    let fft_size = hann_window.len();
    let fft_norm = 1.0 / fft_size as f32;
    let gain = amplitude.max(0.0001);

    for (m, c) in magnitudes_buf.iter_mut().zip(spectrum_buf.iter()) {
        let raw_mag = c.norm() * fft_norm * gain;
        let db = 20.0 * (raw_mag + 1e-10).log10();
        *m = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    }

    let mut bands = calculate_bands(magnitudes_buf, sample_rate);

    // Pink noise compensation: scale each band by a multiplier derived from
    // +3dB per octave relative to Sub Bass (~40 Hz). Multiplicative so silence
    // stays at zero — only actual signal is boosted.
    if pink_noise_shaping {
        const PINK_GAINS: [f32; 8] = [
            1.0,  // Sub Bass  ~40 Hz (reference)
            1.15, // Bass      ~90 Hz  (+1.2 octaves)
            1.30, // Low Mid   ~185 Hz (+2.2 octaves)
            1.50, // Mid       ~375 Hz (+3.2 octaves)
            1.80, // High Mid  ~1000 Hz (+4.6 octaves)
            2.20, // High      ~3000 Hz (+6.2 octaves)
            2.60, // Very High ~6000 Hz (+7.2 octaves)
            3.00, // Presence  ~12000 Hz (+8.2 octaves)
        ];
        for (band, &g) in bands.iter_mut().zip(PINK_GAINS.iter()) {
            *band = (*band * g).min(1.0);
        }
    }
    let volume: f32 = frame.iter().map(|&s| s * s).sum::<f32>() / hann_window.len() as f32;
    let rms_volume = volume.sqrt();

    // Beat detection — O(1) front removal via VecDeque
    let instant_energy: f32 = bands.iter().sum();
    beat_history.push_back(instant_energy);
    if beat_history.len() > 43 {
        beat_history.pop_front();
    }

    let local_average = if beat_history.len() >= 10 {
        beat_history.iter().sum::<f32>() / beat_history.len() as f32
    } else {
        instant_energy
    };

    let is_beat = instant_energy > local_average * 1.3 && instant_energy > 0.1;

    if is_beat {
        *beat_counter += 1;
        *beat_energy = instant_energy;
    }

    let phase =
        ((*beat_counter as f32 + (instant_energy / beat_energy.max(0.001)).min(1.0)) * 0.1) % 1.0;

    // Normalization: track a single slow-decaying global peak across all bands.
    // All bands are scaled by the same factor → no per-band transient inversion.
    let current_max = bands.iter().cloned().fold(0.0f32, f32::max);
    if current_max > *norm_peak {
        // Slow attack — don't let a single spike dominate
        *norm_peak = *norm_peak * 0.9 + current_max * 0.1;
    } else {
        // Very slow decay
        *norm_peak *= 0.999;
    }
    *norm_peak = norm_peak.max(0.01); // Floor to prevent division by near-zero

    for (i, &band) in bands.iter().enumerate() {
        let scaled = if normalize {
            (band / *norm_peak).min(1.0)
        } else {
            band
        };

        let prev = f32::from_bits(output.fft[i].load(Ordering::Relaxed));
        let smoothed = prev * smoothing + scaled * (1.0 - smoothing);

        output.fft[i].store(smoothed.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    // Write full spectrum with the same smoothing / normalization / pink shaping.
    // Per-bin pink gain is derived from the 8-band table by bin center frequency.
    let spectrum_len = magnitudes_buf.len();
    if output.spectrum.len() == spectrum_len {
        let bin_hz = sample_rate / fft_size as f32;
        for (i, (m, s)) in magnitudes_buf.iter().zip(output.spectrum.iter()).enumerate() {
            let freq = i as f32 * bin_hz;
            let pink_gain = if pink_noise_shaping {
                pink_gain_for_freq(freq)
            } else {
                1.0
            };

            let scaled = (*m * pink_gain).min(1.0);
            let normalized = if normalize {
                (scaled / *norm_peak).min(1.0)
            } else {
                scaled
            };

            let prev = f32::from_bits(s.load(Ordering::Relaxed));
            let smoothed = prev * smoothing + normalized * (1.0 - smoothing);
            s.store(smoothed.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    // Volume: simple EMA, no amplitude feedback loop
    let prev_volume = f32::from_bits(output.volume.load(Ordering::Relaxed));
    let smoothed_volume = prev_volume * smoothing + rms_volume * (1.0 - smoothing);
    output
        .volume
        .store(smoothed_volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);

    if is_beat {
        output.beat.store(true, Ordering::Relaxed);
    }

    output.beat_phase.store(phase.to_bits(), Ordering::Relaxed);
}

/// Peak (max) per band — immune to bin-count dilution across octaves.
pub fn calculate_bands(magnitudes: &[f32], sample_rate: f32) -> [f32; 8] {
    let mut bands = [0.0f32; 8];
    let nyquist = sample_rate / 2.0;
    let bins_per_hz = magnitudes.len() as f32 / nyquist;

    let ranges = [
        (20.0, 60.0),
        (60.0, 120.0),
        (120.0, 250.0),
        (250.0, 500.0),
        (500.0, 2000.0),
        (2000.0, 4000.0),
        (4000.0, 8000.0),
        (8000.0, 16000.0),
    ];

    for (i, (low, high)) in ranges.iter().enumerate() {
        let low_bin = (low * bins_per_hz) as usize;
        let high_bin = ((high * bins_per_hz) as usize).min(magnitudes.len());

        if high_bin > low_bin {
            bands[i] = magnitudes[low_bin..high_bin]
                .iter()
                .cloned()
                .fold(0.0f32, f32::max);
        }
    }

    bands
}

/// Returns 1.0 for frequencies outside 20 Hz–16 kHz (no boost).
pub fn pink_gain_for_freq(freq: f32) -> f32 {
    const PINK_GAINS: [f32; 8] = [
        1.0, 1.15, 1.30, 1.50, 1.80, 2.20, 2.60, 3.00,
    ];
    let band = if freq < 20.0 {
        None
    } else if freq < 60.0 {
        Some(0)
    } else if freq < 120.0 {
        Some(1)
    } else if freq < 250.0 {
        Some(2)
    } else if freq < 500.0 {
        Some(3)
    } else if freq < 2000.0 {
        Some(4)
    } else if freq < 4000.0 {
        Some(5)
    } else if freq < 8000.0 {
        Some(6)
    } else if freq < 16000.0 {
        Some(7)
    } else {
        None
    };
    band.map(|idx| PINK_GAINS[idx]).unwrap_or(1.0)
}
