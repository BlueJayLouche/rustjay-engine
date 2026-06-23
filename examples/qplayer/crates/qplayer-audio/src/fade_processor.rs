//! Real-time volume fade processor.
//!
//! Matches C# `FadingSampleProvider`. Supports linear, square, inverse-square,
//! and S-curve fades. Fades are triggered atomically from the control thread
//! and processed sample-accurately in the audio callback.

use crate::SampleProvider;
use qplayer_core::FadeType;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Volume fade processor.
pub struct FadeProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<FadeInner>,
    // Atomic command interface (written by control thread, read by audio thread)
    cmd_target_volume: AtomicU32, // f32::to_bits()
    cmd_duration_frames: AtomicU32,
    cmd_fade_type: AtomicU8,
    cmd_trigger: AtomicBool,
}

struct FadeInner {
    current_volume: f32,
    start_volume: f32,
    end_volume: f32,
    fade_type: FadeType,
    fade_duration_frames: u32,
    fade_remaining_frames: u32,
    is_fading: bool,
}

impl FadeProcessor {
    pub fn new(source: Box<dyn SampleProvider>, initial_volume: f32) -> Self {
        Self {
            source,
            inner: UnsafeCell::new(FadeInner {
                current_volume: initial_volume,
                start_volume: initial_volume,
                end_volume: initial_volume,
                fade_type: FadeType::Linear,
                fade_duration_frames: 0,
                fade_remaining_frames: 0,
                is_fading: false,
            }),
            cmd_target_volume: AtomicU32::new(initial_volume.to_bits()),
            cmd_duration_frames: AtomicU32::new(0),
            cmd_fade_type: AtomicU8::new(FadeType::Linear as u8),
            cmd_trigger: AtomicBool::new(false),
        }
    }

    /// Start a fade to `target_volume` over `duration_frames`.
    pub fn start_fade(&self, target_volume: f32, duration_frames: u32, fade_type: FadeType) {
        self.cmd_target_volume.store(target_volume.to_bits(), Ordering::Relaxed);
        self.cmd_duration_frames.store(duration_frames, Ordering::Relaxed);
        self.cmd_fade_type.store(fade_type as u8, Ordering::Relaxed);
        self.cmd_trigger.store(true, Ordering::Release);
    }

    /// Snap to a volume immediately (cancel any active fade).
    pub fn set_volume(&self, volume: f32) {
        self.cmd_target_volume.store(volume.to_bits(), Ordering::Relaxed);
        self.cmd_duration_frames.store(0, Ordering::Relaxed);
        self.cmd_trigger.store(true, Ordering::Release);
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut FadeInner {
        unsafe { &mut *self.inner.get() }
    }

    /// Apply fade curve to t in [0, 1].
    #[inline]
    fn curve(t: f32, fade_type: FadeType) -> f32 {
        match fade_type {
            FadeType::Linear => t,
            FadeType::Square => t * t,
            FadeType::InverseSquare => t.sqrt(),
            FadeType::SCurve => {
                // Cubic Hermite smoothstep: -2t³ + 3t²
                t * t * (3.0 - 2.0 * t)
            }
        }
    }
}

impl SampleProvider for FadeProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        // Check for pending fade command
        if self.cmd_trigger.swap(false, Ordering::Acquire) {
            let inner = self.inner_mut();
            let target = f32::from_bits(self.cmd_target_volume.load(Ordering::Relaxed));
            let duration = self.cmd_duration_frames.load(Ordering::Relaxed);
            let ft = self.cmd_fade_type.load(Ordering::Relaxed);

            if duration == 0 {
                // Instant snap
                inner.current_volume = target;
                inner.is_fading = false;
            } else {
                inner.start_volume = inner.current_volume;
                inner.end_volume = target;
                inner.fade_duration_frames = duration;
                inner.fade_remaining_frames = duration;
                inner.fade_type = fade_type_from_u8(ft);
                inner.is_fading = true;
            }
        }

        let read = self.source.read(buffer);
        let inner = self.inner_mut();
        let channels = self.source.channels() as usize;

        if !inner.is_fading {
            // Static volume — scalar multiply
            let vol = inner.current_volume;
            if vol != 1.0 {
                for sample in &mut buffer[..read] {
                    *sample *= vol;
                }
            }
            return read;
        }

        // Active fade: interpolate volume per-frame
        let frames = read / channels.max(1);
        let start_vol = inner.start_volume;
        let end_vol = inner.end_volume;
        let total_frames = inner.fade_duration_frames;
        let fade_type = inner.fade_type;

        for frame in 0..frames {
            let t = if inner.fade_remaining_frames == 0 {
                1.0f32
            } else {
                let elapsed = total_frames - inner.fade_remaining_frames;
                (elapsed as f32 / total_frames as f32).clamp(0.0, 1.0)
            };

            let gain = start_vol + Self::curve(t, fade_type) * (end_vol - start_vol);

            for ch in 0..channels {
                buffer[frame * channels + ch] *= gain;
            }

            if inner.fade_remaining_frames > 0 {
                inner.fade_remaining_frames -= 1;
            }
        }

        if inner.fade_remaining_frames == 0 {
            inner.current_volume = end_vol;
            inner.is_fading = false;
        } else {
            // Update current_volume to where we left off for next block
            let t = (total_frames - inner.fade_remaining_frames) as f32 / total_frames as f32;
            inner.current_volume = start_vol + Self::curve(t, fade_type) * (end_vol - start_vol);
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

unsafe impl Send for FadeProcessor {}
unsafe impl Sync for FadeProcessor {}

/// Simple atomic u8 storage for `FadeType`.
struct AtomicU8 {
    inner: std::sync::atomic::AtomicU8,
}

impl AtomicU8 {
    fn new(v: u8) -> Self {
        Self {
            inner: std::sync::atomic::AtomicU8::new(v),
        }
    }
    fn load(&self, ordering: Ordering) -> u8 {
        self.inner.load(ordering)
    }
    fn store(&self, v: u8, ordering: Ordering) {
        self.inner.store(v, ordering);
    }
}

fn fade_type_from_u8(v: u8) -> FadeType {
    match v {
        0 => FadeType::Linear,
        1 => FadeType::SCurve,
        2 => FadeType::Square,
        3 => FadeType::InverseSquare,
        _ => FadeType::Linear,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FnSource;

    fn sine_source() -> Box<dyn SampleProvider> {
        Box::new(FnSource::new(
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
        ))
    }

    #[test]
    fn test_fade_linear() {
        let source = Box::new(FnSource::new(
            |buf| {
                for s in buf.iter_mut() { *s = 1.0; }
                buf.len()
            },
            48000,
            2,
        ));
        let fade = FadeProcessor::new(source, 1.0);

        let mut buf = vec![0.0f32; 480 * 2]; // 480 stereo frames
        fade.start_fade(0.0, 480, FadeType::Linear); // fade to silence over 480 frames

        let read = fade.read(&mut buf);
        assert_eq!(read, 960);

        // First frame should be near full volume (gain ~1.0)
        assert!((buf[0] - 1.0).abs() < 0.01, "first frame should be near full volume, got {}", buf[0]);
        // Last frame: at t = 479/480 ≈ 0.998, gain ≈ 0.002
        let last_l = buf[958];
        assert!(last_l < 0.05, "last frame should be near silence, got {}", last_l);
    }

    #[test]
    fn test_fade_s_curve() {
        let source = Box::new(FnSource::new(
            |buf| {
                for s in buf.iter_mut() {
                    *s = 1.0;
                }
                buf.len()
            },
            48000,
            1,
        ));
        let fade = FadeProcessor::new(source, 0.0);

        let mut buf = vec![0.0f32; 4];
        fade.start_fade(1.0, 4, FadeType::SCurve);

        fade.read(&mut buf);

        // 4 frames over 4-frame fade: t = 0/4, 1/4, 2/4, 3/4
        // smoothstep: 0, 0.15625, 0.5, 0.84375
        assert!(buf[0].abs() < 0.01, "start should be ~0, got {}", buf[0]);
        assert!((buf[1] - 0.15625).abs() < 0.01, "quarter should be ~0.156, got {}", buf[1]);
        assert!((buf[2] - 0.5).abs() < 0.01, "half should be ~0.5, got {}", buf[2]);
        assert!((buf[3] - 0.84375).abs() < 0.01, "three-quarter should be ~0.844, got {}", buf[3]);
    }

    #[test]
    fn test_fade_square() {
        let source = Box::new(FnSource::new(
            |buf| {
                for s in buf.iter_mut() {
                    *s = 1.0;
                }
                buf.len()
            },
            48000,
            1,
        ));
        let fade = FadeProcessor::new(source, 0.0);

        let mut buf = vec![0.0f32; 4];
        fade.start_fade(1.0, 4, FadeType::Square);

        fade.read(&mut buf);

        // Square: 0, 0.0625, 0.25, 0.5625, 1.0
        assert!(buf[0].abs() < 0.01);
        assert!((buf[1] - 0.0625).abs() < 0.05, "got {}", buf[1]);
        assert!((buf[2] - 0.25).abs() < 0.05, "got {}", buf[2]);
        assert!((buf[3] - 0.5625).abs() < 0.1, "got {}", buf[3]);
    }

    #[test]
    fn test_set_volume_instant() {
        let source = Box::new(FnSource::new(
            |buf| {
                for s in buf.iter_mut() {
                    *s = 1.0;
                }
                buf.len()
            },
            48000,
            2,
        ));
        let fade = FadeProcessor::new(source, 1.0);
        fade.set_volume(0.5);

        let mut buf = vec![0.0f32; 4];
        fade.read(&mut buf);

        assert_eq!(buf, vec![0.5, 0.5, 0.5, 0.5]);
    }
}
