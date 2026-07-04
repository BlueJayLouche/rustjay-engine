//! Block-based peak/RMS metering processor.
//!
//! Matches C# `MeteringSampleProviderVec`. Passes audio through unchanged
//! while extracting per-channel peak and RMS over a configurable interval.

use crate::SampleProvider;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Metering data for one notification interval.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MeterData {
    pub peak_l: f32,
    pub peak_r: f32,
    pub rms_l: f32,
    pub rms_r: f32,
    /// Frame count this measurement covers.
    pub frames: u32,
}

/// Pass-through metering processor.
pub struct MeteringProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<MeterInner>,
    // Atomic output (updated by audio thread, read by control thread)
    out_peak_l: AtomicU32,
    out_peak_r: AtomicU32,
    out_rms_l: AtomicU32,
    out_rms_r: AtomicU32,
    out_frames: AtomicU32,
    out_version: AtomicU64,
}

struct MeterInner {
    interval_frames: u32,
    peak_l: f32,
    peak_r: f32,
    sum_sq_l: f64,
    sum_sq_r: f64,
    frame_count: u32,
}

impl MeteringProcessor {
    pub fn new(source: Box<dyn SampleProvider>) -> Self {
        let sr = source.sample_rate();
        let interval = (sr / 10).max(1); // default 100 ms
        Self {
            source,
            inner: UnsafeCell::new(MeterInner {
                interval_frames: interval,
                peak_l: 0.0,
                peak_r: 0.0,
                sum_sq_l: 0.0,
                sum_sq_r: 0.0,
                frame_count: 0,
            }),
            out_peak_l: AtomicU32::new(0),
            out_peak_r: AtomicU32::new(0),
            out_rms_l: AtomicU32::new(0),
            out_rms_r: AtomicU32::new(0),
            out_frames: AtomicU32::new(0),
            out_version: AtomicU64::new(0),
        }
    }

    /// Set the metering interval in frames.
    pub fn set_interval_frames(&self, frames: u32) {
        self.inner_mut().interval_frames = frames.max(1);
    }

    /// Read the latest meter data (non-blocking, may read stale data).
    pub fn read_meters(&self) -> MeterData {
        MeterData {
            peak_l: f32::from_bits(self.out_peak_l.load(Ordering::Relaxed)),
            peak_r: f32::from_bits(self.out_peak_r.load(Ordering::Relaxed)),
            rms_l: f32::from_bits(self.out_rms_l.load(Ordering::Relaxed)),
            rms_r: f32::from_bits(self.out_rms_r.load(Ordering::Relaxed)),
            frames: self.out_frames.load(Ordering::Relaxed),
        }
    }

    /// Check if new meter data is available since `last_version`.
    pub fn version(&self) -> u64 {
        self.out_version.load(Ordering::Acquire)
    }

    /// Analyze an already-mixed buffer directly, bypassing the inner source.
    ///
    /// Use this from the audio callback after mixing and limiting, where the
    /// samples to meter are already in a buffer rather than pulled from a chain.
    pub fn analyze(&self, data: &[f32]) {
        let inner = self.inner_mut();
        let channels = self.source.channels() as usize;
        let frames = data.len() / channels.max(1);

        if channels == 2 {
            for frame in 0..frames {
                let l = data[frame * 2];
                let r = data[frame * 2 + 1];
                inner.peak_l = inner.peak_l.max(l.abs());
                inner.peak_r = inner.peak_r.max(r.abs());
                inner.sum_sq_l += (l as f64).powi(2);
                inner.sum_sq_r += (r as f64).powi(2);
            }
        } else {
            for frame in 0..frames {
                for ch in 0..channels {
                    let s = data[frame * channels + ch];
                    inner.peak_l = inner.peak_l.max(s.abs());
                    inner.sum_sq_l += (s as f64).powi(2);
                }
            }
        }

        inner.frame_count += frames as u32;
        if inner.frame_count >= inner.interval_frames {
            self.publish(inner);
            inner.peak_l = 0.0;
            inner.peak_r = 0.0;
            inner.sum_sq_l = 0.0;
            inner.sum_sq_r = 0.0;
            inner.frame_count = 0;
        }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut MeterInner {
        unsafe { &mut *self.inner.get() }
    }

    fn publish(&self, inner: &MeterInner) {
        let frames = inner.frame_count.max(1);
        let rms_l = (inner.sum_sq_l / frames as f64).sqrt() as f32;
        let rms_r = (inner.sum_sq_r / frames as f64).sqrt() as f32;

        self.out_peak_l.store(inner.peak_l.to_bits(), Ordering::Relaxed);
        self.out_peak_r.store(inner.peak_r.to_bits(), Ordering::Relaxed);
        self.out_rms_l.store(rms_l.to_bits(), Ordering::Relaxed);
        self.out_rms_r.store(rms_r.to_bits(), Ordering::Relaxed);
        self.out_frames.store(frames, Ordering::Relaxed);
        self.out_version.fetch_add(1, Ordering::Release);
    }
}

impl SampleProvider for MeteringProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let read = self.source.read(buffer);
        let inner = self.inner_mut();
        let channels = self.source.channels() as usize;
        let frames = read / channels.max(1);

        if channels == 2 {
            for frame in 0..frames {
                let l = buffer[frame * 2];
                let r = buffer[frame * 2 + 1];
                inner.peak_l = inner.peak_l.max(l.abs());
                inner.peak_r = inner.peak_r.max(r.abs());
                inner.sum_sq_l += (l as f64).powi(2);
                inner.sum_sq_r += (r as f64).powi(2);
            }
        } else {
            for frame in 0..frames {
                for ch in 0..channels {
                    let s = buffer[frame * channels + ch];
                    inner.peak_l = inner.peak_l.max(s.abs());
                    inner.sum_sq_l += (s as f64).powi(2);
                }
            }
        }

        inner.frame_count += frames as u32;

        if inner.frame_count >= inner.interval_frames {
            self.publish(inner);
            inner.peak_l = 0.0;
            inner.peak_r = 0.0;
            inner.sum_sq_l = 0.0;
            inner.sum_sq_r = 0.0;
            inner.frame_count = 0;
        }

        read
    }

    fn seek(&self, sample: usize) {
        self.source.seek(sample);
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

unsafe impl Send for MeteringProcessor {}
unsafe impl Sync for MeteringProcessor {}

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
    fn test_metering_peak() {
        let meter = MeteringProcessor::new(dc_source(0.5));
        meter.set_interval_frames(10);

        let mut buf = vec![0.0f32; 20]; // 10 stereo frames
        meter.read(&mut buf);

        let data = meter.read_meters();
        assert_eq!(data.frames, 10);
        assert!((data.peak_l - 0.5).abs() < 0.001);
        assert!((data.peak_r - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_metering_rms() {
        let meter = MeteringProcessor::new(dc_source(0.5));
        meter.set_interval_frames(10);

        let mut buf = vec![0.0f32; 20];
        meter.read(&mut buf);

        let data = meter.read_meters();
        // RMS of constant 0.5 = 0.5
        assert!((data.rms_l - 0.5).abs() < 0.001);
        assert!((data.rms_r - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_metering_interval_split() {
        // Interval = 10 frames, but we read 5 frames at a time
        let meter = MeteringProcessor::new(dc_source(0.8));
        meter.set_interval_frames(10);

        let mut buf = vec![0.0f32; 10]; // 5 stereo frames
        meter.read(&mut buf);

        // Not enough frames yet — no publish
        let v1 = meter.version();

        meter.read(&mut buf);
        // Now we have 10 frames — should publish
        let v2 = meter.version();
        assert!(v2 > v1, "version should increment after interval");

        let data = meter.read_meters();
        assert_eq!(data.frames, 10);
        assert!((data.peak_l - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_metering_zero() {
        let meter = MeteringProcessor::new(dc_source(0.0));
        meter.set_interval_frames(4);

        let mut buf = vec![0.0f32; 8];
        meter.read(&mut buf);

        let data = meter.read_meters();
        assert_eq!(data.peak_l, 0.0);
        assert_eq!(data.rms_l, 0.0);
    }
}
