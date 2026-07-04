//! Lookahead limiter with soft-knee compression.
//!
//! Simplified from C# `AudioLimiterSampleProvider` while preserving core
//! behavior: lookahead delay, gain-reduction envelope, stereo linking,
//! and hard clip to threshold.

use crate::SampleProvider;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Core lookahead limiter state (can be used standalone or inside SampleProvider).
pub struct Limiter {
    enabled: bool,
    pub threshold: f32,
    input_gain: f32,
    channels: u16,

    // Lookahead delay (ring buffer)
    delay: Vec<f32>,
    delay_write: usize,
    delay_read: usize,
    delay_size: usize,

    // Envelope follower state
    envelope: f32,
    /// Attack coefficient (smoothing)
    attack_coef: f32,
    /// Release coefficient
    release_coef: f32,
    /// Hold counter
    hold_counter: u32,
    /// Hold duration in samples
    hold_samples: u32,

    // Gain reduction metering
    pub gr_db: f32,
}

/// Lookahead limiter processor wrapping a SampleProvider.
pub struct LimiterProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<Limiter>,
    // Atomic parameters
    cmd_threshold: AtomicU32,     // f32::to_bits
    cmd_input_gain: AtomicU32,    // f32::to_bits
    cmd_enabled: AtomicBool,
    // Gain reduction metering (updated by audio thread, read by main thread)
    gr_db_atomic: AtomicU32,      // f32::to_bits of GR in dB
}

impl Limiter {
    /// Create a standalone limiter core. `threshold` is linear gain.
    pub fn new(threshold: f32, sample_rate: u32, channels: u16) -> Self {
        let delay_ms = 5.0f32;
        let delay_samples = ((sample_rate as f32 * delay_ms / 1000.0) * channels as f32).ceil() as usize;
        let delay_size = delay_samples.next_power_of_two();

        Self {
            enabled: true,
            threshold: threshold.clamp(0.01, 1.0),
            input_gain: 1.0,
            channels,
            delay: vec![0.0f32; delay_size],
            delay_write: 0,
            delay_read: 0,
            delay_size,
            envelope: 1.0,
            attack_coef: Self::time_to_coef(2.0, sample_rate),
            release_coef: Self::time_to_coef(50.0, sample_rate),
            hold_counter: 0,
            hold_samples: (sample_rate as f32 * 10.0 / 1000.0) as u32,
            gr_db: 0.0,
        }
    }

    #[inline]
    fn time_to_coef(ms: f32, sr: u32) -> f32 {
        let samples = ms * sr as f32 / 1000.0;
        (-1.0 / samples.max(1.0)).exp()
    }

    /// Process a buffer in-place. Returns the minimum envelope (most reduction) observed.
    pub fn process(&mut self, buffer: &mut [f32]) -> f32 {
        let channels = self.channels as usize;
        if !self.enabled || self.threshold >= 1.0 || channels == 0 {
            return 1.0;
        }

        let mask = self.delay_size - 1;
        let threshold = self.threshold;
        let input_gain = self.input_gain;
        let attack_coef = self.attack_coef;
        let release_coef = self.release_coef;
        let hold_samples = self.hold_samples;

        let frames = buffer.len() / channels;
        let mut min_envelope = 1.0f32;

        for frame in 0..frames {
            let mut peak_l = 0.0f32;
            let mut peak_r = 0.0f32;

            for ch in 0..channels {
                let s = buffer[frame * channels + ch] * input_gain;
                let abs_s = s.abs();
                if ch == 0 {
                    peak_l = abs_s;
                } else if ch == 1 {
                    peak_r = abs_s;
                }
                self.delay[self.delay_write & mask] = s;
                self.delay_write += 1;
            }

            let peak = if channels >= 2 { peak_l.max(peak_r) } else { peak_l };

            let target_gr = if peak > threshold {
                threshold / peak
            } else {
                1.0
            };

            if target_gr < self.envelope {
                self.envelope = attack_coef * self.envelope + (1.0 - attack_coef) * target_gr;
                self.hold_counter = hold_samples;
            } else {
                if self.hold_counter > 0 {
                    self.hold_counter -= 1;
                } else {
                    self.envelope = release_coef * self.envelope + (1.0 - release_coef) * target_gr;
                }
            }

            self.envelope = self.envelope.clamp(0.0, 1.0);
            if self.envelope < min_envelope {
                min_envelope = self.envelope;
            }

            for ch in 0..channels {
                let delayed = self.delay[self.delay_read & mask];
                self.delay_read += 1;
                let mut out = delayed * self.envelope;
                out = out.clamp(-threshold, threshold);
                buffer[frame * channels + ch] = out;
            }
        }

        self.gr_db = if min_envelope > 0.0 {
            20.0 * min_envelope.log10()
        } else {
            -96.0
        };

        min_envelope
    }

    pub fn reset(&mut self) {
        self.delay_write = 0;
        self.delay_read = 0;
        self.envelope = 1.0;
        self.hold_counter = 0;
        self.delay.fill(0.0);
        self.gr_db = 0.0;
    }
}

impl LimiterProcessor {
    /// Create a limiter wrapping a source. `threshold` is linear gain (e.g., 0.95 = -0.45 dB).
    pub fn new(source: Box<dyn SampleProvider>, threshold: f32) -> Self {
        let sr = source.sample_rate();
        let ch = source.channels();
        Self {
            source,
            inner: UnsafeCell::new(Limiter::new(threshold, sr, ch)),
            cmd_threshold: AtomicU32::new(threshold.clamp(0.01, 1.0).to_bits()),
            cmd_input_gain: AtomicU32::new(1.0f32.to_bits()),
            cmd_enabled: AtomicBool::new(true),
            gr_db_atomic: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.cmd_enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn set_threshold(&self, threshold: f32) {
        self.cmd_threshold
            .store(threshold.clamp(0.01, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_input_gain(&self, gain: f32) {
        self.cmd_input_gain.store(gain.max(0.0).to_bits(), Ordering::Relaxed);
    }

    /// Read current gain reduction in dB (0.0 = no reduction, negative = reducing).
    pub fn gr_db(&self) -> f32 {
        f32::from_bits(self.gr_db_atomic.load(Ordering::Relaxed))
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut Limiter {
        unsafe { &mut *self.inner.get() }
    }
}

impl SampleProvider for LimiterProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let read = self.source.read(buffer);
        let inner = self.inner_mut();

        // Refresh parameters
        inner.enabled = self.cmd_enabled.load(Ordering::Relaxed);
        inner.threshold = f32::from_bits(self.cmd_threshold.load(Ordering::Relaxed));
        inner.input_gain = f32::from_bits(self.cmd_input_gain.load(Ordering::Relaxed));

        inner.process(&mut buffer[..read]);
        self.gr_db_atomic.store(inner.gr_db.to_bits(), Ordering::Relaxed);

        read
    }

    fn seek(&self, sample: usize) {
        self.source.seek(sample);
        self.inner_mut().reset();
    }

    fn position(&self) -> usize {
        self.source.position()
    }

    fn length(&self) -> Option<usize> {
        self.source.length()
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn channels(&self) -> u16 {
        self.source.channels()
    }
}

unsafe impl Send for LimiterProcessor {}
unsafe impl Sync for LimiterProcessor {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FnSource;

    fn dc_source(val: f32) -> Box<dyn SampleProvider> {
        Box::new(FnSource::new(
            move |buf| {
                for s in buf.iter_mut() { *s = val; }
                buf.len()
            },
            48000,
            2,
        ))
    }

    #[test]
    fn test_limiter_disabled_passes_through() {
        let limiter = LimiterProcessor::new(dc_source(1.0), 0.95);
        limiter.set_enabled(false);

        let mut buf = vec![0.0f32; 4];
        limiter.read(&mut buf);
        assert_eq!(buf, vec![1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn test_limiter_clips_above_threshold() {
        // Input = 2.0, threshold = 0.5 → should be limited to ~0.5
        let limiter = LimiterProcessor::new(dc_source(2.0), 0.5);

        // Need enough samples to fill the lookahead delay first
        let mut buf = vec![0.0f32; 4096];
        limiter.read(&mut buf);

        // After the delay, samples should be clamped to threshold
        let tail = &buf[2048..];
        let max_val = tail.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max_val <= 0.55,
            "limiter should clamp to ~0.5, got max {}",
            max_val
        );
    }

    #[test]
    fn test_limiter_does_not_affect_below_threshold() {
        // Input = 0.3, threshold = 0.5 → should pass through
        let limiter = LimiterProcessor::new(dc_source(0.3), 0.5);

        let mut buf = vec![0.0f32; 4096];
        limiter.read(&mut buf);

        let tail = &buf[2048..];
        let min_val = tail.iter().map(|s| s.abs()).fold(f32::MAX, f32::min);
        assert!(
            min_val > 0.25,
            "limiter should not affect signals below threshold, got min {}",
            min_val
        );
    }

    #[test]
    fn test_seek_resets() {
        let limiter = LimiterProcessor::new(dc_source(2.0), 0.5);
        let mut buf = vec![0.0f32; 4096];
        limiter.read(&mut buf);

        limiter.seek(0);
        let mut buf2 = vec![0.0f32; 4096];
        limiter.read(&mut buf2);

        // After seek, the delay should be reset so first samples pass through
        assert!(buf2[0] > 0.1, "after seek, initial samples should pass through delay");
    }
}
