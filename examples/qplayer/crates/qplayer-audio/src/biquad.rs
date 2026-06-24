//! Biquad IIR filter — Direct Form I.
//!
//! Double-precision state to prevent drift; output cast to f32.
//! Matches C# `EQSampleProvider.FilterCoeffs` behavior.

use qplayer_core::{EQBand, EQBandShape, EQFilter, EQFilterOrder};

/// A single biquad section.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
    pub x1: f64,
    pub x2: f64,
    pub y1: f64,
    pub y2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self {
            b0: 1.0, b1: 0.0, b2: 0.0,
            a1: 0.0, a2: 0.0,
            x1: 0.0, x2: 0.0,
            y1: 0.0, y2: 0.0,
        }
    }
}

impl Biquad {
    pub const fn new() -> Self {
        Self {
            b0: 1.0, b1: 0.0, b2: 0.0,
            a1: 0.0, a2: 0.0,
            x1: 0.0, x2: 0.0,
            y1: 0.0, y2: 0.0,
        }
    }

    /// Process one sample.
    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
                - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Reset state.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    /// Compute coefficients for a peaking (bell) filter.
    pub fn bell(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        let a = 10.0f64.powf(gain_db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);

        let b0 = 1.0 + alpha * a;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 - alpha * a;
        let a0 = 1.0 + alpha / a;
        let a1 = b1;
        let a2 = 1.0 - alpha / a;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    /// Low shelf.
    pub fn low_shelf(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        let a = 10.0f64.powf(gain_db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);
        let sqrt_a = a.sqrt();

        let b0 = a * ((a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
        let b1 = 2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
        let a0 = (a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        let a1 = -2.0 * ((a - 1.0) + (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    /// High shelf.
    pub fn high_shelf(freq: f32, gain_db: f32, q: f32, sr: f32) -> Self {
        let a = 10.0f64.powf(gain_db as f64 / 40.0);
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);
        let sqrt_a = a.sqrt();

        let b0 = a * ((a + 1.0) + (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha);
        let b1 = -2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0);
        let b2 = a * ((a + 1.0) + (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha);
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + 2.0 * sqrt_a * alpha;
        let a1 = 2.0 * ((a - 1.0) - (a + 1.0) * cos_w0);
        let a2 = (a + 1.0) - (a - 1.0) * cos_w0 - 2.0 * sqrt_a * alpha;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    /// Notch.
    pub fn notch(freq: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);

        let b0 = 1.0;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0;
        let a0 = 1.0 + alpha;
        let a1 = b1;
        let a2 = 1.0 - alpha;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    /// Low pass.
    pub fn low_pass(freq: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);

        let b0 = (1.0 - cos_w0) / 2.0;
        let b1 = 1.0 - cos_w0;
        let b2 = b0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    /// High pass.
    pub fn high_pass(freq: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);

        let b0 = (1.0 + cos_w0) / 2.0;
        let b1 = -(1.0 + cos_w0);
        let b2 = b0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    /// All pass.
    pub fn all_pass(freq: f32, q: f32, sr: f32) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * freq as f64 / sr as f64;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * q as f64);

        let b0 = 1.0 - alpha;
        let b1 = -2.0 * cos_w0;
        let b2 = 1.0 + alpha;
        let a0 = b2;
        let a1 = b1;
        let a2 = b0;

        Self::from_raw(b0, b1, b2, a0, a1, a2)
    }

    #[inline]
    fn from_raw(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        let inv_a0 = 1.0 / a0;
        Self {
            b0: b0 * inv_a0,
            b1: b1 * inv_a0,
            b2: b2 * inv_a0,
            a1: a1 * inv_a0,
            a2: a2 * inv_a0,
            x1: 0.0, x2: 0.0, y1: 0.0, y2: 0.0,
        }
    }
}

/// Build a biquad from an `EQBand`.
pub fn biquad_from_band(band: &EQBand, sample_rate: f32) -> Option<Biquad> {
    if band.freq < 5.0 || band.freq > sample_rate * 0.49 {
        return None; // disabled / out of range
    }
    let q = band.q.max(0.01);
    let bq = match band.shape {
        EQBandShape::Bell => Biquad::bell(band.freq, band.gain, q, sample_rate),
        EQBandShape::LowShelf => Biquad::low_shelf(band.freq, band.gain, q, sample_rate),
        EQBandShape::HighShelf => Biquad::high_shelf(band.freq, band.gain, q, sample_rate),
        EQBandShape::Notch => Biquad::notch(band.freq, q, sample_rate),
        EQBandShape::LowPass => Biquad::low_pass(band.freq, q, sample_rate),
        EQBandShape::HighPass => Biquad::high_pass(band.freq, q, sample_rate),
        EQBandShape::AllPass => Biquad::all_pass(band.freq, q, sample_rate),
    };
    Some(bq)
}

/// Build HPF/LPF biquad(s) from an `EQFilter`.
/// Returns up to 2 biquads (for 24 dB/oct).
pub fn biquads_from_filter(filter: &EQFilter, sample_rate: f32) -> Vec<Biquad> {
    match filter.order {
        EQFilterOrder::Disabled => Vec::new(),
        EQFilterOrder::_12dBOct => {
            if filter.frequency > 5.0 && filter.frequency < sample_rate * 0.49 {
                vec![Biquad::high_pass(filter.frequency, 0.7, sample_rate)]
            } else {
                Vec::new()
            }
        }
        EQFilterOrder::_24dBOct => {
            if filter.frequency > 5.0 && filter.frequency < sample_rate * 0.49 {
                vec![
                    Biquad::high_pass(filter.frequency, 0.7, sample_rate),
                    Biquad::high_pass(filter.frequency, 0.7, sample_rate),
                ]
            } else {
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bell_passes_unity_at_0db() {
        let mut bq = Biquad::bell(1000.0, 0.0, 1.0, 48000.0);
        // With 0 dB gain, bell should be unity at all frequencies
        let mut out = [0.0f64; 100];
        for i in 0..100 {
            out[i] = bq.process(1.0);
        }
        // After transient, should settle to ~1.0
        assert!((out[99] - 1.0).abs() < 0.001, "unity gain bell should pass DC, got {}", out[99]);
    }

    #[test]
    fn test_low_pass_attenuates_high_freq() {
        // Feed a high frequency sine (close to Nyquist) through LP
        let mut bq = Biquad::low_pass(1000.0, 0.7, 48000.0);
        let sr = 48000.0f64;
        let freq = 12000.0f64;
        let mut max_out = 0.0f64;
        let mut phase = 0.0f64;
        for _ in 0..1000 {
            let sample = phase.sin();
            let out = bq.process(sample);
            max_out = max_out.max(out.abs());
            phase += 2.0 * std::f64::consts::PI * freq / sr;
        }
        // Should be significantly attenuated (> 12 dB)
        assert!(max_out < 0.25, "LP should attenuate 12kHz, got peak {}", max_out);
    }

    #[test]
    fn test_high_pass_blocks_dc() {
        let mut bq = Biquad::high_pass(100.0, 0.7, 48000.0);
        let mut out = [0.0f64; 1000];
        for i in 0..1000 {
            out[i] = bq.process(1.0);
        }
        // After transient, DC should be blocked
        assert!(out[999].abs() < 0.01, "HP should block DC, got {}", out[999]);
    }
}
