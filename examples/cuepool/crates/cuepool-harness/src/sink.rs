//! Null audio sink — pulls the mixer exactly as `cuepool_audio::engine` does from
//! the cpal callback, but into a plain buffer. No device, no driver, no thread.

use cuepool_audio::{Mixer, SampleProvider};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct NullSink {
    mixer: Arc<Mixer>,
    buffer: Vec<f32>,
}

impl NullSink {
    pub fn new(mixer: Arc<Mixer>, block_frames: usize) -> Self {
        let channels = mixer.channels() as usize;
        Self {
            mixer,
            buffer: vec![0.0; block_frames * channels],
        }
    }

    /// One audio-callback's worth of render. Returns the filled block.
    pub fn render_block(&mut self) -> &[f32] {
        self.buffer.fill(0.0);
        self.mixer.render(&mut self.buffer);
        &self.buffer
    }
}

/// A finite source emitting a deterministic ramp, so a dropped or duplicated
/// block is visible in the output rather than hiding in silence.
pub struct RampSource {
    sample_rate: u32,
    channels: u16,
    len_samples: usize,
    pos: AtomicUsize,
}

impl RampSource {
    pub fn new(sample_rate: u32, channels: u16, len_samples: usize) -> Self {
        Self {
            sample_rate,
            channels,
            len_samples,
            pos: AtomicUsize::new(0),
        }
    }
}

impl SampleProvider for RampSource {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let start = self.pos.load(Ordering::Relaxed);
        let n = buffer.len().min(self.len_samples.saturating_sub(start));
        for (i, slot) in buffer.iter_mut().take(n).enumerate() {
            *slot = ((start + i) % 1000) as f32 / 1000.0 - 0.5;
        }
        self.pos.store(start + n, Ordering::Relaxed);
        n
    }
    fn seek(&self, sample: usize) {
        self.pos
            .store(sample.min(self.len_samples), Ordering::Relaxed);
    }
    fn position(&self) -> usize {
        self.pos.load(Ordering::Relaxed)
    }
    fn length(&self) -> Option<usize> {
        Some(self.len_samples)
    }
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}
