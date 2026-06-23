//! Loop and trim processor.
//!
//! Matches C# `LoopingSampleProvider`. Adds start/end trim points and seamless
//! looping on top of a `SampleProvider`. Tracks a `total_position` that counts
//! continuously across loops (for display / metering).

use crate::SampleProvider;
use qplayer_core::LoopMode;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

/// Loop and trim processor.
pub struct LoopProcessor {
    source: Box<dyn SampleProvider>,
    inner: UnsafeCell<LoopInner>,
    // Atomic parameters (written by control thread, read by audio thread)
    cmd_start_frame: AtomicU64,
    cmd_end_frame: AtomicU64, // 0 = play to end
    cmd_loop_mode: AtomicU8,
    cmd_loop_count: AtomicU32,
    /// Optional external counter incremented each time a loop boundary is crossed.
    /// Allows the main thread to synchronise video loops without locking.
    loop_counter: Option<Arc<AtomicU32>>,
}

struct LoopInner {
    start_frame: u64,
    end_frame: u64,
    loop_mode: LoopMode,
    loop_count: u32,
    /// Continuous position counter (in frames) for display.
    total_frames: u64,
    /// Number of completed loops.
    played_loops: u32,
    /// Whether the source has been exhausted.
    exhausted: bool,
}

impl LoopProcessor {
    pub fn new(source: Box<dyn SampleProvider>) -> Self {
        Self {
            source,
            inner: UnsafeCell::new(LoopInner {
                start_frame: 0,
                end_frame: 0,
                loop_mode: LoopMode::OneShot,
                loop_count: 1,
                total_frames: 0,
                played_loops: 0,
                exhausted: false,
            }),
            cmd_start_frame: AtomicU64::new(0),
            cmd_end_frame: AtomicU64::new(0),
            cmd_loop_mode: AtomicU8::new(LoopMode::OneShot as u8),
            cmd_loop_count: AtomicU32::new(1),
            loop_counter: None,
        }
    }

    /// Attach an external atomic counter that will be incremented on every loop.
    pub fn with_loop_counter(mut self, counter: Arc<AtomicU32>) -> Self {
        self.loop_counter = Some(counter);
        self
    }

    pub fn set_loop(&self, start_frame: u64, end_frame: u64, mode: LoopMode, count: u32) {
        self.cmd_start_frame.store(start_frame, Ordering::Relaxed);
        self.cmd_end_frame.store(end_frame, Ordering::Relaxed);
        self.cmd_loop_mode.store(mode as u8, Ordering::Relaxed);
        self.cmd_loop_count.store(count, Ordering::Relaxed);
    }

    /// Current total position in frames (counts across loops).
    pub fn total_frames(&self) -> u64 {
        self.inner().total_frames
    }

    /// Number of completed loops.
    pub fn played_loops(&self) -> u32 {
        self.inner().played_loops
    }

    #[inline]
    fn inner(&self) -> &LoopInner {
        unsafe { &*self.inner.get() }
    }

    #[inline]
    #[allow(clippy::mut_from_ref)]
    fn inner_mut(&self) -> &mut LoopInner {
        unsafe { &mut *self.inner.get() }
    }

    /// Convert sample position to frame position.
    #[inline]
    fn samples_to_frames(&self, samples: usize) -> u64 {
        (samples / self.source.channels().max(1) as usize) as u64
    }

    /// Convert frame position to sample position.
    #[inline]
    fn frames_to_samples(&self, frames: u64) -> usize {
        (frames * self.source.channels() as u64) as usize
    }
}

impl SampleProvider for LoopProcessor {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let inner = self.inner_mut();
        let channels = self.source.channels() as usize;

        // Refresh loop parameters
        inner.start_frame = self.cmd_start_frame.load(Ordering::Relaxed);
        let cmd_end = self.cmd_end_frame.load(Ordering::Relaxed);
        inner.end_frame = if cmd_end == 0 {
            // Auto-detect from source length
            self.source
                .length()
                .map(|len| self.samples_to_frames(len))
                .unwrap_or(u64::MAX)
        } else {
            cmd_end
        };
        inner.loop_mode = loop_mode_from_u8(self.cmd_loop_mode.load(Ordering::Relaxed));
        inner.loop_count = self.cmd_loop_count.load(Ordering::Relaxed);

        if inner.exhausted {
            return 0;
        }

        let mut total_read = 0;
        let target_frames = buffer.len() / channels.max(1);

        while total_read < target_frames {
            // Ensure source is at the right position (trim start)
            let current_src_frame = self.samples_to_frames(self.source.position());
            if current_src_frame < inner.start_frame {
                self.source
                    .seek(self.frames_to_samples(inner.start_frame));
            }

            let current_src_frame = self.samples_to_frames(self.source.position());
            let remaining_in_loop = inner.end_frame.saturating_sub(current_src_frame);

            let frames_to_read = (target_frames - total_read).min(remaining_in_loop as usize);
            let samples_to_read = frames_to_read * channels;

            if samples_to_read == 0 {
                // Hit loop boundary — decide what to do
                match inner.loop_mode {
                    LoopMode::OneShot | LoopMode::HoldLast => {
                        inner.exhausted = true;
                        break;
                    }
                    LoopMode::Looped => {
                        if inner.played_loops + 1 >= inner.loop_count {
                            inner.exhausted = true;
                            break;
                        }
                        inner.played_loops += 1;
                        if let Some(ref counter) = self.loop_counter {
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                        self.source
                            .seek(self.frames_to_samples(inner.start_frame));
                        continue;
                    }
                    LoopMode::LoopedInfinite => {
                        inner.played_loops += 1;
                        if let Some(ref counter) = self.loop_counter {
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                        self.source
                            .seek(self.frames_to_samples(inner.start_frame));
                        continue;
                    }
                }
            }

            let buf_start = total_read * channels;
            let read = self.source.read(&mut buffer[buf_start..buf_start + samples_to_read]);
            if read == 0 {
                // Source exhausted unexpectedly
                inner.exhausted = true;
                break;
            }

            let read_frames = read / channels;
            total_read += read_frames;
            inner.total_frames += read_frames as u64;
        }

        total_read * channels
    }

    fn seek(&self, sample: usize) {
        let inner = self.inner_mut();
        let frame = self.samples_to_frames(sample);
        inner.total_frames = frame;
        inner.exhausted = false;
        // Adjust for trim start
        let seek_frame = inner.start_frame + frame;
        self.source.seek(self.frames_to_samples(seek_frame));
    }

    fn position(&self) -> usize {
        self.frames_to_samples(self.inner().total_frames)
    }

    fn length(&self) -> Option<usize> {
        // Length in total samples, accounting for loops.
        // Read from atomics so this is correct even before the first read() call.
        let inner = self.inner();
        let source_len = self.source.length()?;
        let source_frames = self.samples_to_frames(source_len);
        let cmd_end = self.cmd_end_frame.load(Ordering::Relaxed);
        let effective_end = if cmd_end == 0 {
            source_frames
        } else {
            cmd_end.min(source_frames)
        };
        let start = self.cmd_start_frame.load(Ordering::Relaxed).min(effective_end);
        let loop_frames = effective_end.saturating_sub(start);

        match inner.loop_mode {
            LoopMode::OneShot | LoopMode::HoldLast => Some(self.frames_to_samples(loop_frames)),
            LoopMode::Looped => {
                let total_frames = loop_frames * inner.loop_count as u64;
                Some(self.frames_to_samples(total_frames.min(source_frames)))
            }
            LoopMode::LoopedInfinite => None,
        }
    }

    fn sample_rate(&self) -> u32 {
        self.source.sample_rate()
    }

    fn channels(&self) -> u16 {
        self.source.channels()
    }
}

unsafe impl Send for LoopProcessor {}
unsafe impl Sync for LoopProcessor {}

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

fn loop_mode_from_u8(v: u8) -> LoopMode {
    match v {
        0 => LoopMode::OneShot,
        1 => LoopMode::Looped,
        2 => LoopMode::LoopedInfinite,
        3 => LoopMode::HoldLast,
        _ => LoopMode::OneShot,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::UnsafeCell;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A test source that stores fixed samples and supports seek.
    struct TestSource {
        data: UnsafeCell<Vec<f32>>,
        pos: AtomicUsize,
        sample_rate: u32,
        channels: u16,
    }

    impl TestSource {
        fn new(data: Vec<f32>, sample_rate: u32, channels: u16) -> Self {
            Self {
                data: UnsafeCell::new(data),
                pos: AtomicUsize::new(0),
                sample_rate,
                channels,
            }
        }
    }

    impl SampleProvider for TestSource {
        fn read(&self, buffer: &mut [f32]) -> usize {
            let data = unsafe { &*self.data.get() };
            let pos = self.pos.load(Ordering::Relaxed);
            let len = data.len();
            let to_copy = buffer.len().min(len.saturating_sub(pos));
            buffer[..to_copy].copy_from_slice(&data[pos..pos + to_copy]);
            self.pos.store(pos + to_copy, Ordering::Relaxed);
            to_copy
        }
        fn seek(&self, sample: usize) {
            self.pos.store(sample, Ordering::Relaxed);
        }
        fn position(&self) -> usize {
            self.pos.load(Ordering::Relaxed)
        }
        fn length(&self) -> Option<usize> {
            Some(unsafe { &*self.data.get() }.len())
        }
        fn sample_rate(&self) -> u32 {
            self.sample_rate
        }
        fn channels(&self) -> u16 {
            self.channels
        }
    }

    unsafe impl Send for TestSource {}
    unsafe impl Sync for TestSource {}

    /// 10 stereo frames: samples 0..19
    fn finite_source() -> Box<dyn SampleProvider> {
        Box::new(TestSource::new(
            (0..20).map(|i| i as f32).collect(),
            48000,
            2,
        ))
    }

    #[test]
    fn test_oneshot_trim() {
        let source = finite_source();
        let loop_proc = LoopProcessor::new(source);
        loop_proc.set_loop(2, 5, LoopMode::OneShot, 1); // frames 2-4

        let mut buf = vec![0.0f32; 20]; // request 10 frames
        let read = loop_proc.read(&mut buf);
        assert_eq!(read, 6); // 3 frames * 2 ch = 6 samples (frames 2,3,4)
        // Source values at frame 2 = samples 4,5 -> 4.0, 5.0
        assert_eq!(buf[0], 4.0);
        assert_eq!(buf[1], 5.0);
    }

    #[test]
    fn test_loop_twice() {
        let source = finite_source();
        let loop_proc = LoopProcessor::new(source);
        loop_proc.set_loop(0, 3, LoopMode::Looped, 2); // frames 0-2, loop 2x

        let mut buf = vec![0.0f32; 24]; // request 12 frames
        let read = loop_proc.read(&mut buf);
        // 3 frames * 2 loops = 6 frames total = 12 samples
        assert_eq!(read, 12);
        // First loop: frames 0,1,2 -> samples 0,1,2,3,4,5
        assert_eq!(buf[0], 0.0);
        assert_eq!(buf[5], 5.0);
        // Second loop: should seek back to 0, so same values
        assert_eq!(buf[6], 0.0);
        assert_eq!(buf[11], 5.0);
    }

    #[test]
    fn test_total_position_counts_across_loops() {
        let source = finite_source();
        let loop_proc = LoopProcessor::new(source);
        loop_proc.set_loop(0, 2, LoopMode::LoopedInfinite, 1); // 2-frame loops

        let mut buf = vec![0.0f32; 8]; // 4 frames
        loop_proc.read(&mut buf);

        assert_eq!(loop_proc.total_frames(), 4);
        // We read 4 frames with 2-frame loops -> seek back once
        assert_eq!(loop_proc.played_loops(), 1);
    }

    #[test]
    fn test_seek_resets() {
        let source = finite_source();
        let loop_proc = LoopProcessor::new(source);
        loop_proc.set_loop(0, 3, LoopMode::LoopedInfinite, 1);

        let mut buf = vec![0.0f32; 4]; // 2 frames
        loop_proc.read(&mut buf);
        assert_eq!(loop_proc.total_frames(), 2);

        loop_proc.seek(0);
        assert_eq!(loop_proc.total_frames(), 0);

        let mut buf2 = vec![0.0f32; 4];
        loop_proc.read(&mut buf2);
        assert_eq!(loop_proc.total_frames(), 2);
    }
}
