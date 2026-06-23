//! Sample-rate converter using `rubato`.
//!
//! Converts any input sample rate to the mixer target (48 kHz).
//! Uses `FastFixedOut` for predictable output frame counts.

use crate::SampleProvider;
use rubato::{FastFixedOut, PolynomialDegree, Resampler};
use std::cell::UnsafeCell;

/// Resampling processor.
///
/// Wraps a source and converts its sample rate to `target_sample_rate`.
/// Input/output are interleaved; internally converts to rubato's planar format.
pub struct ResamplerProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<ResamplerInner>,
}

struct ResamplerInner {
    /// rubato resampler (planar I/O).
    resampler: FastFixedOut<f32>,
    /// Target sample rate.
    target_rate: u32,
    /// Source sample rate.
    source_rate: u32,
    /// Channel count.
    channels: u16,
    /// Planar input buffer [channel][frame].
    input_buf: Vec<Vec<f32>>,
    /// Planar output buffer [channel][frame].
    output_buf: Vec<Vec<f32>>,
    /// Interleaved output ring buffer.
    ring: Vec<f32>,
    /// Read position in ring buffer.
    ring_read: usize,
    /// Write position in ring buffer.
    ring_write: usize,
    /// Capacity of ring buffer (samples).
    ring_cap: usize,
}

impl ResamplerProcessor {
    pub fn new(source: Box<dyn SampleProvider>, target_rate: u32) -> Result<Self, rubato::ResamplerConstructionError> {
        let source_rate = source.sample_rate();
        let channels = source.channels();
        let ratio = target_rate as f64 / source_rate as f64;

        // Output chunk size: 1024 frames. This determines how often we call rubato.
        let chunk_size = 1024;
        let resampler = FastFixedOut::new(
            ratio,
            2.0,                    // max ratio variation
            PolynomialDegree::Septic, // good quality/CPU tradeoff
            chunk_size,
            channels as usize,
        )?;

        let input_buf = resampler.input_buffer_allocate(true);
        let output_buf = resampler.output_buffer_allocate(true);

        // Ring buffer: 4x chunk size to handle jitter
        let ring_cap = chunk_size * channels as usize * 4;
        let ring = vec![0.0f32; ring_cap];

        Ok(Self {
            source,
            inner: UnsafeCell::new(ResamplerInner {
                resampler,
                target_rate,
                source_rate,
                channels,
                input_buf,
                output_buf,
                ring,
                ring_read: 0,
                ring_write: 0,
                ring_cap,
            }),
        })
    }

    #[inline]
    fn inner(&self) -> &ResamplerInner {
        unsafe { &*self.inner.get() }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut ResamplerInner {
        unsafe { &mut *self.inner.get() }
    }

    /// Fill the ring buffer by reading from source and resampling.
    fn fill_ring(&self) {
        let inner = self.inner_mut();
        let channels = inner.channels as usize;

        loop {
            let available = (inner.ring_cap + inner.ring_write - inner.ring_read) % inner.ring_cap;
            let needed = inner.ring_cap / 2; // fill to half capacity
            if available >= needed {
                break;
            }

            // How many input frames does rubato need?
            let input_frames_needed = inner.resampler.input_frames_next();

            // Read interleaved from source
            let interleaved_needed = input_frames_needed * channels;
            let mut interleaved = vec![0.0f32; interleaved_needed];
            let read = self.source.read(&mut interleaved);
            if read == 0 {
                break; // EOF
            }
            let input_frames = read / channels;

            // Deinterleave into planar input buffer
            for ch in 0..channels {
                for i in 0..input_frames {
                    inner.input_buf[ch][i] = interleaved[i * channels + ch];
                }
            }

            // Zero-fill remaining input if we got less than needed
            for ch in 0..channels {
                for i in input_frames..input_frames_needed {
                    inner.input_buf[ch][i] = 0.0;
                }
            }

            // Process through rubato
            let (_, out_frames) = inner
                .resampler
                .process_into_buffer(&inner.input_buf, &mut inner.output_buf, None)
                .unwrap_or((0, 0));

            // Interleave output into ring buffer
            for frame in 0..out_frames {
                for ch in 0..channels {
                    let sample = inner.output_buf[ch][frame];
                    inner.ring[inner.ring_write] = sample;
                    inner.ring_write = (inner.ring_write + 1) % inner.ring_cap;
                }
            }
        }
    }
}

impl SampleProvider for ResamplerProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        self.fill_ring();

        let inner = self.inner_mut();
        let mut written = 0;

        while written < buffer.len() {
            let available = (inner.ring_cap + inner.ring_write - inner.ring_read) % inner.ring_cap;
            if available == 0 {
                break;
            }

            let to_copy = (buffer.len() - written).min(available);
            for i in 0..to_copy {
                buffer[written + i] = inner.ring[inner.ring_read];
                inner.ring_read = (inner.ring_read + 1) % inner.ring_cap;
            }
            written += to_copy;
        }

        written
    }

    fn seek(&self, sample: usize) {
        self.source.seek(sample);
        let inner = self.inner_mut();
        inner.ring_read = 0;
        inner.ring_write = 0;
        // Re-create resampler to reset state
        let ratio = inner.target_rate as f64 / inner.source_rate as f64;
        if let Ok(new_r) = FastFixedOut::new(
            ratio,
            2.0,
            PolynomialDegree::Septic,
            1024,
            inner.channels as usize,
        ) {
            inner.resampler = new_r;
            inner.input_buf = inner.resampler.input_buffer_allocate(true);
            inner.output_buf = inner.resampler.output_buffer_allocate(true);
        }
    }

    fn position(&self) -> usize {
        // Report in target-rate samples to stay consistent with `length()`.
        // (Currently shadowed by BufferedSource's own read position, but keep the
        // trait contract self-consistent for any direct consumer.)
        let inner = self.inner();
        (self.source.position() as f64 * inner.target_rate as f64 / inner.source_rate as f64) as usize
    }

    fn length(&self) -> Option<usize> {
        self.source.length().map(|len| {
            (len as f64 * self.inner().target_rate as f64 / self.inner().source_rate as f64) as usize
        })
    }

    fn sample_rate(&self) -> u32 {
        self.inner().target_rate
    }

    fn channels(&self) -> u16 {
        self.inner().channels
    }
}

unsafe impl Send for ResamplerProcessor {}
unsafe impl Sync for ResamplerProcessor {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FnSource;

    #[test]
    fn test_resampler_44100_to_48000() {
        // Generate a 1kHz sine wave at 44100 Hz
        let source = Box::new(FnSource::new(
            |buf| {
                static mut PHASE: f32 = 0.0;
                const FREQ: f32 = 1000.0;
                const SR: f32 = 44100.0;
                for i in 0..buf.len() / 2 {
                    let sample = (unsafe { PHASE }).sin();
                    buf[i * 2] = sample;
                    buf[i * 2 + 1] = sample;
                    unsafe {
                        PHASE += 2.0 * std::f32::consts::PI * FREQ / SR;
                    }
                }
                buf.len()
            },
            44100,
            2,
        ));

        let resampler = ResamplerProcessor::new(source, 48000).unwrap();
        assert_eq!(resampler.sample_rate(), 48000);
        assert_eq!(resampler.channels(), 2);

        // Read in chunks, accumulate ~1 second of 48kHz audio
        let mut total = 0usize;
        let mut buf = vec![0.0f32; 4096];
        for _ in 0..30 {
            let read = resampler.read(&mut buf);
            if read == 0 {
                break;
            }
            total += read;
        }
        // Should get approximately 1 second of audio (96000 samples stereo)
        assert!(total > 90000, "expected ~96000 samples total, got {}", total);
    }

    /// Distortion guard: a clean 1 kHz sine resampled 44.1k→48k must stay a clean
    /// 1 kHz sine. Catches gross chain bugs (channel mixing, frame misalignment,
    /// garbage) without audio hardware or an FFT — checks bounds, energy, and
    /// dominant frequency via zero-crossing rate. This is the test that should
    /// fail while playback is "very distorted" and pass once the chain is fixed.
    #[test]
    fn test_resampler_sine_stays_clean() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        const FREQ: f64 = 1000.0;
        const SRC_SR: f64 = 44100.0;
        const AMP: f32 = 0.5;

        let n = AtomicUsize::new(0);
        let source = Box::new(FnSource::new(
            move |buf| {
                for f in 0..buf.len() / 2 {
                    let i = n.fetch_add(1, Ordering::Relaxed) as f64;
                    let s = (AMP as f64 * (2.0 * std::f64::consts::PI * FREQ * i / SRC_SR).sin()) as f32;
                    buf[f * 2] = s;
                    buf[f * 2 + 1] = s;
                }
                buf.len()
            },
            44100,
            2,
        ));

        let rs = ResamplerProcessor::new(source, 48000).unwrap();

        // Collect ~0.5 s of 48k stereo.
        let mut out = Vec::new();
        let mut buf = vec![0.0f32; 4096];
        while out.len() < 48_000 {
            let read = rs.read(&mut buf);
            if read == 0 {
                break;
            }
            out.extend_from_slice(&buf[..read]);
        }
        assert!(out.len() >= 48_000, "resampler underran: {} samples", out.len());

        // Steady-state left channel (skip warm-up; index 4096 is even = left).
        let left: Vec<f32> = out[4096..].iter().step_by(2).copied().collect();

        let peak = left.iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak <= 1.05, "output blew up — peak {}", peak);
        assert!(peak > 0.30, "output collapsed — peak {} (expected ~{})", peak, AMP);

        let rms = (left.iter().map(|s| s * s).sum::<f32>() / left.len() as f32).sqrt();
        let ideal_rms = AMP / std::f32::consts::SQRT_2; // ~0.354
        assert!(
            (rms - ideal_rms).abs() < 0.08,
            "RMS {} far from sine ideal {} — energy is wrong (distorted)",
            rms,
            ideal_rms
        );

        let zc = left.windows(2).filter(|w| w[0] * w[1] < 0.0).count();
        let dur_s = left.len() as f32 / 48_000.0;
        let dominant_hz = zc as f32 / 2.0 / dur_s;
        assert!(
            (900.0..1100.0).contains(&dominant_hz),
            "dominant {} Hz, expected ~1000 — frequency content is wrong (distorted)",
            dominant_hz
        );
    }
}
