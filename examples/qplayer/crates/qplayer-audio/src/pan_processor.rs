//! Pan + per-cue fade-in/fade-out processor.
//!
//! Matches C# `PanFadeInOutProvider`. Applies:
//! 1. Per-cue fade-in (based on playback position)
//! 2. Per-cue fade-out (based on playback position)
//! 3. Stereo pan law (linear gain)
//! 4. Global volume multiplier

use crate::SampleProvider;
use qplayer_core::FadeType;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, Ordering};

/// Pan and fade processor.
pub struct PanProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<PanInner>,
    // Atomic parameters (written by control thread, read by audio thread)
    cmd_volume: AtomicU32,           // f32::to_bits
    cmd_pan: AtomicU32,              // f32::to_bits
    cmd_fade_in_frames: AtomicU32,
    cmd_fade_out_frames: AtomicU32,
    cmd_fade_out_start_frame: AtomicU32,
    cmd_fade_type: AtomicU8,
}

struct PanInner {
    volume: f32,
    pan: f32,
    fade_in_frames: u32,
    fade_out_frames: u32,
    fade_out_start_frame: u32,
    fade_type: FadeType,
    /// Local frame counter (independent of source position for reliability).
    frame_position: u64,
}

impl PanProcessor {
    pub fn new(source: Box<dyn SampleProvider>) -> Self {
        Self {
            source,
            inner: UnsafeCell::new(PanInner {
                volume: 1.0,
                pan: 0.0,
                fade_in_frames: 0,
                fade_out_frames: 0,
                fade_out_start_frame: u32::MAX,
                fade_type: FadeType::Linear,
                frame_position: 0,
            }),
            cmd_volume: AtomicU32::new(1.0f32.to_bits()),
            cmd_pan: AtomicU32::new(0.0f32.to_bits()),
            cmd_fade_in_frames: AtomicU32::new(0),
            cmd_fade_out_frames: AtomicU32::new(0),
            cmd_fade_out_start_frame: AtomicU32::new(u32::MAX),
            cmd_fade_type: AtomicU8::new(FadeType::Linear as u8),
        }
    }

    pub fn set_volume(&self, volume: f32) {
        self.cmd_volume.store(volume.to_bits(), Ordering::Relaxed);
    }

    pub fn set_pan(&self, pan: f32) {
        self.cmd_pan.store(pan.clamp(-1.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn set_fade_in(&self, frames: u32) {
        self.cmd_fade_in_frames.store(frames, Ordering::Relaxed);
    }

    pub fn set_fade_out(&self, frames: u32, start_frame: u32) {
        self.cmd_fade_out_frames.store(frames, Ordering::Relaxed);
        self.cmd_fade_out_start_frame.store(start_frame, Ordering::Relaxed);
    }

    pub fn set_fade_type(&self, fade_type: FadeType) {
        self.cmd_fade_type.store(fade_type as u8, Ordering::Relaxed);
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut PanInner {
        unsafe { &mut *self.inner.get() }
    }

    /// Compute fade gain for a frame at `position`.
    #[inline]
    fn fade_gain(&self, inner: &PanInner, position: u64) -> f32 {
        let mut gain = 1.0f32;

        // Fade in
        if inner.fade_in_frames > 0 && position < inner.fade_in_frames as u64 {
            let t = position as f32 / inner.fade_in_frames as f32;
            gain *= Self::curve(t, inner.fade_type);
        }

        // Fade out
        if inner.fade_out_frames > 0 && position >= inner.fade_out_start_frame as u64 {
            let elapsed = position - inner.fade_out_start_frame as u64;
            if elapsed >= inner.fade_out_frames as u64 {
                gain = 0.0;
            } else {
                let t = 1.0 - (elapsed as f32 / inner.fade_out_frames as f32);
                gain *= Self::curve(t, inner.fade_type);
            }
        }

        gain
    }

    /// Apply fade curve to t in [0, 1].
    #[inline]
    fn curve(t: f32, fade_type: FadeType) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match fade_type {
            FadeType::Linear => t,
            FadeType::Square => t * t,
            FadeType::InverseSquare => t.sqrt(),
            FadeType::SCurve => t * t * (3.0 - 2.0 * t),
        }
    }
}

impl SampleProvider for PanProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let read = self.source.read(buffer);
        let inner = self.inner_mut();
        let channels = self.source.channels() as usize;

        // Refresh parameters from atomics
        inner.volume = f32::from_bits(self.cmd_volume.load(Ordering::Relaxed));
        inner.pan = f32::from_bits(self.cmd_pan.load(Ordering::Relaxed));
        inner.fade_in_frames = self.cmd_fade_in_frames.load(Ordering::Relaxed);
        inner.fade_out_frames = self.cmd_fade_out_frames.load(Ordering::Relaxed);
        inner.fade_out_start_frame = self.cmd_fade_out_start_frame.load(Ordering::Relaxed);
        inner.fade_type = fade_type_from_u8(self.cmd_fade_type.load(Ordering::Relaxed));

        let frames = read / channels.max(1);
        let is_stereo = channels == 2;

        // Precompute pan gains (linear law)
        let pan = inner.pan;
        let pan_l = if is_stereo { 1.0 - pan.max(0.0) } else { 1.0 };
        let pan_r = if is_stereo { 1.0 + pan.min(0.0) } else { 1.0 };

        for frame in 0..frames {
            let pos = inner.frame_position + frame as u64;
            let fade = self.fade_gain(inner, pos);
            let vol = inner.volume * fade;

            if is_stereo {
                buffer[frame * 2] *= vol * pan_l;
                buffer[frame * 2 + 1] *= vol * pan_r;
            } else {
                for ch in 0..channels {
                    buffer[frame * channels + ch] *= vol;
                }
            }
        }

        inner.frame_position += frames as u64;
        read
    }

    fn seek(&self, sample: usize) {
        self.source.seek(sample);
        self.inner_mut().frame_position = (sample / self.source.channels().max(1) as usize) as u64;
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

unsafe impl Send for PanProcessor {}
unsafe impl Sync for PanProcessor {}

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

    fn dc_source(val: f32) -> Box<dyn SampleProvider> {
        Box::new(FnSource::new(
            move |buf| {
                for s in buf.iter_mut() {
                    *s = val;
                }
                buf.len()
            },
            48000,
            2,
        ))
    }

    #[test]
    fn test_pan_center() {
        let pan = PanProcessor::new(dc_source(1.0));
        pan.set_volume(0.5);
        pan.set_pan(0.0);

        let mut buf = vec![0.0f32; 4];
        let read = pan.read(&mut buf);
        assert_eq!(read, 4);
        assert_eq!(buf, vec![0.5, 0.5, 0.5, 0.5]);
    }

    #[test]
    fn test_pan_left() {
        let pan = PanProcessor::new(dc_source(1.0));
        pan.set_pan(-1.0); // full left

        let mut buf = vec![0.0f32; 4];
        pan.read(&mut buf);
        // Left channel = 1.0, Right channel = 0.0
        assert_eq!(buf[0], 1.0);
        assert_eq!(buf[1], 0.0);
    }

    #[test]
    fn test_pan_right() {
        let pan = PanProcessor::new(dc_source(1.0));
        pan.set_pan(1.0); // full right

        let mut buf = vec![0.0f32; 4];
        pan.read(&mut buf);
        // Left channel = 0.0, Right channel = 1.0
        assert_eq!(buf[0], 0.0);
        assert_eq!(buf[1], 1.0);
    }

    #[test]
    fn test_fade_in() {
        let pan = PanProcessor::new(dc_source(1.0));
        pan.set_fade_in(4);
        pan.set_fade_type(FadeType::Linear);

        let mut buf = vec![0.0f32; 8]; // 4 stereo frames
        pan.read(&mut buf);

        // Frame 0: t=0/4=0, gain=0
        assert!(buf[0].abs() < 0.01, "frame 0 should be silent, got {}", buf[0]);
        // Frame 1: t=1/4=0.25, gain=0.25
        assert!((buf[2] - 0.25).abs() < 0.01, "frame 1 should be 0.25, got {}", buf[2]);
        // Frame 2: t=2/4=0.5, gain=0.5
        assert!((buf[4] - 0.5).abs() < 0.01, "frame 2 should be 0.5, got {}", buf[4]);
        // Frame 3: t=3/4=0.75, gain=0.75
        assert!((buf[6] - 0.75).abs() < 0.01, "frame 3 should be 0.75, got {}", buf[6]);
    }

    #[test]
    fn test_fade_out() {
        let pan = PanProcessor::new(dc_source(1.0));
        pan.set_fade_out(4, 0); // fade out over 4 frames, starting at frame 0
        pan.set_fade_type(FadeType::Linear);

        let mut buf = vec![0.0f32; 8]; // 4 stereo frames
        pan.read(&mut buf);

        // Frame 0: t=1-0/4=1.0, gain=1.0
        assert!((buf[0] - 1.0).abs() < 0.01, "frame 0 should be full, got {}", buf[0]);
        // Frame 1: t=1-1/4=0.75, gain=0.75
        assert!((buf[2] - 0.75).abs() < 0.01, "frame 1 should be 0.75, got {}", buf[2]);
        // Frame 2: t=1-2/4=0.5, gain=0.5
        assert!((buf[4] - 0.5).abs() < 0.01, "frame 2 should be 0.5, got {}", buf[4]);
        // Frame 3: t=1-3/4=0.25, gain=0.25
        assert!((buf[6] - 0.25).abs() < 0.01, "frame 3 should be 0.25, got {}", buf[6]);
    }

    #[test]
    fn test_seek_resets_position() {
        let pan = PanProcessor::new(dc_source(1.0));
        pan.set_fade_in(10);

        let mut buf = vec![0.0f32; 4];
        pan.read(&mut buf);
        // After reading 2 frames, position is 2

        pan.seek(0); // seek back to start
        let mut buf2 = vec![0.0f32; 4];
        pan.read(&mut buf2);
        // Should get the same fade-in behavior from position 0
        assert!(buf2[0].abs() < 0.01, "after seek, frame 0 should be silent");
    }
}
