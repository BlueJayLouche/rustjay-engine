//! `SampleProvider` trait — the core abstraction for the audio pipeline.
//!
//! Replaces C# `ISamplePositionProvider`. Every stage in the audio chain
//! implements this trait: decoders, loopers, resamplers, EQ, fades, pan, mixer.
//!
//! # Thread Safety Contract
//!
//! The audio callback thread is the **only** thread that calls `read()`.
//! Control methods (`seek`, `set_volume`, etc.) are called from the main thread.
//! Implementations must ensure `read()` never blocks, never allocates, and never
//! locks a mutex. Use atomics and lock-free queues for cross-thread state.

/// A source of audio samples.
///
/// All methods except `read` are called from the main/control thread.
/// `read` is called from the real-time audio callback.
pub trait SampleProvider: Send + Sync {
    /// Fill `buffer` with interleaved f32 samples (LRLR...).
    ///
    /// Returns the number of **samples** (not frames) written.
    /// If the source has reached EOF, returns fewer samples than requested.
    /// The caller must zero-fill any unfilled portion of the buffer.
    fn read(&self, buffer: &mut [f32]) -> usize;

    /// Sample-accurate seek.
    fn seek(&self, sample: usize);

    /// Current position in samples (mono sample count, not frames).
    fn position(&self) -> usize;

    /// Total length in samples, if known.
    fn length(&self) -> Option<usize>;

    /// Sample rate of this source.
    fn sample_rate(&self) -> u32;

    /// Channel count.
    fn channels(&self) -> u16;
}

/// A `SampleProvider` that wraps a closure — useful for testing and adapters.
pub struct FnSource<F: Fn(&mut [f32]) -> usize + Send + Sync> {
    f: F,
    sample_rate: u32,
    channels: u16,
}

impl<F: Fn(&mut [f32]) -> usize + Send + Sync> FnSource<F> {
    pub fn new(f: F, sample_rate: u32, channels: u16) -> Self {
        Self { f, sample_rate, channels }
    }
}

impl<F: Fn(&mut [f32]) -> usize + Send + Sync> SampleProvider for FnSource<F> {
    fn read(&self, buffer: &mut [f32]) -> usize {
        (self.f)(buffer)
    }
    fn seek(&self, _sample: usize) {}
    fn position(&self) -> usize { 0 }
    fn length(&self) -> Option<usize> { None }
    fn sample_rate(&self) -> u32 { self.sample_rate }
    fn channels(&self) -> u16 { self.channels }
}
