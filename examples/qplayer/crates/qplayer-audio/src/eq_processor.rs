//! 4-band semi-parametric EQ processor.
//!
//! Matches C# `EQSampleProvider`. Processes 4 bands in series, plus optional
//! HPF/LPF. Coefficients recalculated when settings change.

use crate::biquad::{biquad_from_band, biquads_from_filter, Biquad};
use crate::SampleProvider;
use qplayer_core::EQSettings;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};

/// EQ processor.
pub struct EqProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<EqInner>,
    // Lock-free settings update
    settings: UnsafeCell<EQSettings>,
    settings_version: AtomicU64,
}

struct EqInner {
    local_version: u64,
    sample_rate: u32,
    channels: u16,
    /// One biquad per band per channel.
    bands: [[Option<Biquad>; 4]; MAX_CHANNELS],
    /// HPF biquads per channel.
    hpf: [Vec<Biquad>; MAX_CHANNELS],
    /// LPF biquads per channel.
    lpf: [Vec<Biquad>; MAX_CHANNELS],
}

const MAX_CHANNELS: usize = 2; // stereo max

impl EqProcessor {
    pub fn new(source: Box<dyn SampleProvider>, settings: EQSettings) -> Self {
        let sample_rate = source.sample_rate();
        let channels = source.channels();
        let mut inner = EqInner {
            local_version: 0,
            sample_rate,
            channels,
            bands: Default::default(),
            hpf: Default::default(),
            lpf: Default::default(),
        };
        inner.rebuild(&settings);

        Self {
            source,
            inner: UnsafeCell::new(inner),
            settings: UnsafeCell::new(settings),
            settings_version: AtomicU64::new(1),
        }
    }

    /// Update EQ settings from the control thread.
    pub fn update_settings(&self, settings: EQSettings) {
        unsafe {
            *self.settings.get() = settings;
        }
        self.settings_version.fetch_add(1, Ordering::Release);
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut EqInner {
        unsafe { &mut *self.inner.get() }
    }
}

impl SampleProvider for EqProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let read = self.source.read(buffer);
        let inner = self.inner_mut();
        let channels = inner.channels as usize;

        // Check for updated settings
        let new_version = self.settings_version.load(Ordering::Acquire);
        if inner.local_version != new_version {
            let settings = unsafe { &*self.settings.get() };
            inner.rebuild(settings);
            inner.local_version = new_version;
        }

        if !inner.is_active() {
            return read;
        }

        let frames = read / channels.max(1);

        for frame in 0..frames {
            for ch in 0..channels.min(MAX_CHANNELS) {
                let sample = buffer[frame * channels + ch] as f64;

                // HPF
                let mut s = sample;
                for bq in &mut inner.hpf[ch] {
                    s = bq.process(s);
                }

                // 4 bands
                for band_opt in inner.bands[ch].iter_mut().flatten() {
                    s = band_opt.process(s);
                }

                // LPF
                for bq in &mut inner.lpf[ch] {
                    s = bq.process(s);
                }

                buffer[frame * channels + ch] = s as f32;
            }
        }

        read
    }

    fn seek(&self, sample: usize) {
        self.source.seek(sample);
        // Reset filter state to avoid artifacts after seek
        let inner = self.inner_mut();
        for ch in 0..MAX_CHANNELS {
            for band_opt in inner.bands[ch].iter_mut().flatten() {
                band_opt.reset();
            }
            for bq in &mut inner.hpf[ch] {
                bq.reset();
            }
            for bq in &mut inner.lpf[ch] {
                bq.reset();
            }
        }
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

unsafe impl Send for EqProcessor {}
unsafe impl Sync for EqProcessor {}

impl EqInner {
    fn rebuild(&mut self, settings: &EQSettings) {
        let sr = self.sample_rate;
        let ch = self.channels as usize;

        for c in 0..ch.min(MAX_CHANNELS) {
            // Rebuild 4 bands
            let bands = [settings.band1, settings.band2, settings.band3, settings.band4];
            for (i, band) in bands.iter().enumerate() {
                self.bands[c][i] = if settings.enabled {
                    biquad_from_band(band, sr as f32)
                } else {
                    None
                };
            }

            // HPF
            self.hpf[c] = if settings.enabled {
                biquads_from_filter(&settings.hpf, sr as f32)
            } else {
                Vec::new()
            };

            // LPF
            self.lpf[c] = if settings.enabled {
                biquads_from_filter(&settings.lpf, sr as f32)
            } else {
                Vec::new()
            };
        }
    }

    fn is_active(&self) -> bool {
        self.bands.iter().any(|ch| ch.iter().any(|b| b.is_some()))
            || self.hpf.iter().any(|v| !v.is_empty())
            || self.lpf.iter().any(|v| !v.is_empty())
    }
}

// Need Default for [Vec<Biquad>; MAX_CHANNELS] and [[Option<Biquad>; 4]; MAX_CHANNELS]
impl Default for EqInner {
    fn default() -> Self {
        Self {
            local_version: 0,
            sample_rate: 48000,
            channels: 2,
            bands: Default::default(),
            hpf: Default::default(),
            lpf: Default::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biquad::Biquad;
    use qplayer_core::{EQBand, EQBandShape};

    fn dc_source(val: f32) -> Box<dyn SampleProvider> {
        Box::new(crate::FnSource::new(
            move |buf| {
                for s in buf.iter_mut() { *s = val; }
                buf.len()
            },
            48000,
            2,
        ))
    }

    #[test]
    fn test_eq_disabled_passes_through() {
        let settings = EQSettings {
            enabled: false,
            ..Default::default()
        };
        let eq = EqProcessor::new(dc_source(1.0), settings);
        let mut buf = vec![0.0f32; 4];
        let read = eq.read(&mut buf);
        assert_eq!(read, 4);
        assert_eq!(buf, vec![1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn test_eq_bell_boost_at_center_freq() {
        // Feed a 1kHz sine through a +6dB bell at 1kHz — should boost
        let source = Box::new(crate::FnSource::new(
            |buf| {
                static mut PHASE: f32 = 0.0;
                const FREQ: f32 = 1000.0;
                const SR: f32 = 48000.0;
                for i in 0..buf.len() / 2 {
                    let s = unsafe { PHASE }.sin();
                    buf[i * 2] = s;
                    buf[i * 2 + 1] = s;
                    unsafe { PHASE += 2.0 * std::f32::consts::PI * FREQ / SR }
                }
                buf.len()
            },
            48000,
            2,
        ));
        let settings = EQSettings {
            enabled: true,
            band1: EQBand {
                freq: 1000.0,
                gain: 6.0,
                q: 1.0,
                shape: EQBandShape::Bell,
            },
            ..Default::default()
        };
        let eq = EqProcessor::new(source, settings);

        // Process enough samples for the filter to settle (several cycles)
        let mut buf = vec![0.0f32; 512];
        for _ in 0..50 {
            eq.read(&mut buf);
        }

        // Measure peak amplitude of output
        let peak = buf.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        // +6dB boost ≈ 1.995x amplitude
        assert!(
            peak > 1.5 && peak < 2.5,
            "+6dB bell at 1kHz should boost 1kHz sine significantly, got peak {}",
            peak
        );
    }

    #[test]
    fn test_eq_high_pass_blocks_dc() {
        let settings = EQSettings {
            enabled: true,
            hpf: qplayer_core::EQFilter {
                frequency: 100.0,
                order: qplayer_core::EQFilterOrder::_12dBOct,
            },
            ..Default::default()
        };
        let eq = EqProcessor::new(dc_source(1.0), settings);
        let mut buf = vec![0.0f32; 4];
        for _ in 0..500 {
            eq.read(&mut buf);
        }
        let out = buf[0];
        assert!(out.abs() < 0.05, "HPF should block DC, got {}", out);
    }

    #[test]
    fn test_settings_update() {
        let mut settings = EQSettings {
            enabled: false,
            ..Default::default()
        };
        let eq = EqProcessor::new(dc_source(1.0), settings.clone());

        // Initially disabled
        let mut buf = vec![0.0f32; 4];
        eq.read(&mut buf);
        assert_eq!(buf[0], 1.0);

        // Enable HPF — this WILL affect DC
        settings.enabled = true;
        settings.hpf = qplayer_core::EQFilter {
            frequency: 100.0,
            order: qplayer_core::EQFilterOrder::_12dBOct,
        };
        eq.update_settings(settings);

        // Process until filter settles
        for _ in 0..500 {
            eq.read(&mut buf);
        }
        let out = buf[0];
        assert!(out.abs() < 0.05, "HPF should block DC after update, got {}", out);
    }
}
