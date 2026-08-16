//! Double-buffered sample provider — decouples slow file I/O from the audio callback.
//!
//! A background thread continuously reads from the wrapped source into a ring buffer.
//! The audio callback reads from the ring buffer without blocking.

use crate::SampleProvider;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

/// Default ring buffer size: 3 seconds at 48 kHz stereo.
const DEFAULT_RING_SECONDS: f32 = 3.0;

struct Inner {
    sample_rate: u32,
    channels: u16,
    length: Option<usize>,
    /// Ring consumer — the audio callback pops via `try_lock`, so it never
    /// blocks. The BG thread locks it only while holding the `seek_reset`
    /// write guard, which excludes the callback, so that lock is uncontended.
    cons: Mutex<HeapCons<f32>>,
    /// Samples popped by the callback since `position_base` (only advanced by
    /// the audio callback thread; reset by the BG thread during a seek reset).
    read_pos: AtomicUsize,
    /// EOF reached on source.
    eof: AtomicBool,
    /// Set on Drop so the background thread exits instead of looping forever.
    shutdown: AtomicBool,
    /// Latest seek target sample (set by the control thread, consumed by BG thread).
    seek_target: AtomicUsize,
    /// Requested/applied generations distinguish repeated seeks to the same sample.
    seek_generation: AtomicUsize,
    applied_seek_generation: AtomicUsize,
    /// Excludes the callback only while the background thread resets ring cursors.
    seek_reset: RwLock<()>,
    /// Absolute sample corresponding to ring-buffer read position zero.
    position_base: AtomicUsize,
    /// Source is behind a Mutex — only the BG thread ever locks it.
    source: Mutex<Box<dyn SampleProvider>>,
}

/// Buffered wrapper around a `SampleProvider`.
pub struct BufferedSource {
    inner: Arc<Inner>,
    _bg_thread: JoinHandle<()>,
}

impl BufferedSource {
    pub fn new(source: Box<dyn SampleProvider>) -> Self {
        let sr = source.sample_rate();
        let ch = source.channels();
        let length = source.length();
        let ring_samples = (sr as f32 * DEFAULT_RING_SECONDS * ch as f32).ceil() as usize;

        let ring = HeapRb::<f32>::new(ring_samples);
        let (prod, cons) = ring.split();

        let inner = Arc::new(Inner {
            sample_rate: sr,
            channels: ch,
            length,
            cons: Mutex::new(cons),
            read_pos: AtomicUsize::new(0),
            eof: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            seek_target: AtomicUsize::new(0),
            seek_generation: AtomicUsize::new(0),
            applied_seek_generation: AtomicUsize::new(0),
            seek_reset: RwLock::new(()),
            position_base: AtomicUsize::new(0),
            source: Mutex::new(source),
        });

        let inner2 = Arc::clone(&inner);
        let bg_thread = std::thread::spawn(move || {
            Self::bg_loop(inner2, prod);
        });

        Self {
            inner,
            _bg_thread: bg_thread,
        }
    }

    fn bg_loop(inner: Arc<Inner>, mut prod: HeapProd<f32>) {
        let mut temp = vec![0.0f32; 4096];
        loop {
            // Exit once the owning BufferedSource has been dropped (no thread leak).
            if inner.shutdown.load(Ordering::Acquire) {
                return;
            }

            // Check for pending seek
            let seek_generation = inner.seek_generation.load(Ordering::Acquire);
            if seek_generation != inner.applied_seek_generation.load(Ordering::Acquire) {
                let seek = inner.seek_target.load(Ordering::Acquire);
                let _reset = inner.seek_reset.write().unwrap_or_else(|e| e.into_inner());
                if let Ok(src) = inner.source.lock() {
                    src.seek(seek);
                }
                // The write guard above excludes the callback, so this lock is
                // always uncontended. Drop stale pre-seek samples.
                if let Ok(mut cons) = inner.cons.lock() {
                    cons.clear();
                }
                inner.position_base.store(seek, Ordering::Release);
                inner.read_pos.store(0, Ordering::Release);
                inner.eof.store(false, Ordering::Relaxed);
                inner
                    .applied_seek_generation
                    .store(seek_generation, Ordering::Release);
            }

            if inner.eof.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            let free = prod.vacant_len();
            if free == 0 {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }

            let to_read = temp.len().min(free);
            let n = if let Ok(src) = inner.source.lock() {
                src.read(&mut temp[..to_read])
            } else {
                0
            };

            if n == 0 {
                inner.eof.store(true, Ordering::Relaxed);
                continue;
            }

            // Non-blocking partial push; the BG thread is the sole producer.
            prod.push_slice(&temp[..n]);
        }
    }
}

impl SampleProvider for BufferedSource {
    fn read(&self, buffer: &mut [f32]) -> usize {
        if self.inner.seek_generation.load(Ordering::Acquire)
            != self.inner.applied_seek_generation.load(Ordering::Acquire)
        {
            buffer.fill(0.0);
            return buffer.len();
        }
        let Ok(_reset) = self.inner.seek_reset.try_read() else {
            buffer.fill(0.0);
            return buffer.len();
        };
        if self.inner.seek_generation.load(Ordering::Acquire)
            != self.inner.applied_seek_generation.load(Ordering::Acquire)
        {
            buffer.fill(0.0);
            return buffer.len();
        }

        // try_lock: the callback must never block. Holding the seek_reset read
        // guard means the BG thread is not resetting, so this is uncontended.
        let Ok(mut cons) = self.inner.cons.try_lock() else {
            buffer.fill(0.0);
            return buffer.len();
        };

        let to_copy = buffer.len().min(cons.occupied_len());
        if to_copy == 0 {
            if self.inner.eof.load(Ordering::Acquire) {
                return 0;
            }
            // An empty ring while the background thread opens or re-seeks the
            // source is a temporary underflow, not EOF. Keep the mixer input
            // alive and render silence until decoded samples arrive.
            buffer.fill(0.0);
            return buffer.len();
        }

        let popped = cons.pop_slice(&mut buffer[..to_copy]);
        self.inner.read_pos.fetch_add(popped, Ordering::Release);
        popped
    }

    fn seek(&self, sample: usize) {
        self.inner.seek_target.store(sample, Ordering::Release);
        self.inner.seek_generation.fetch_add(1, Ordering::AcqRel);
    }

    fn position(&self) -> usize {
        if self.inner.seek_generation.load(Ordering::Acquire)
            != self.inner.applied_seek_generation.load(Ordering::Acquire)
        {
            self.inner.seek_target.load(Ordering::Acquire)
        } else {
            self.inner
                .position_base
                .load(Ordering::Acquire)
                .saturating_add(self.inner.read_pos.load(Ordering::Relaxed))
        }
    }

    fn length(&self) -> Option<usize> {
        self.inner.length
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate
    }

    fn channels(&self) -> u16 {
        self.inner.channels
    }
}

impl Drop for BufferedSource {
    fn drop(&mut self) {
        // Signal the background thread to exit. It drops its Arc<Inner> and ends,
        // so a cue's decode thread is reclaimed when the cue stops — not leaked.
        // Not joined: avoids blocking on an in-flight source.read().
        self.inner.shutdown.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Seekable mono ramp source: emitted value = absolute sample index as f32.
    struct RampSource {
        pos: Mutex<usize>,
    }

    impl RampSource {
        fn new() -> Self {
            Self { pos: Mutex::new(0) }
        }
    }

    impl SampleProvider for RampSource {
        fn read(&self, buffer: &mut [f32]) -> usize {
            let mut pos = self.pos.lock().unwrap();
            for (i, s) in buffer.iter_mut().enumerate() {
                *s = (*pos + i) as f32;
            }
            *pos += buffer.len();
            buffer.len()
        }
        fn seek(&self, sample: usize) {
            *self.pos.lock().unwrap() = sample;
        }
        fn position(&self) -> usize {
            *self.pos.lock().unwrap()
        }
        fn length(&self) -> Option<usize> {
            None
        }
        fn sample_rate(&self) -> u32 {
            48_000
        }
        fn channels(&self) -> u16 {
            1
        }
    }

    /// Read into `buf` until the source delivers real samples or the deadline
    /// passes. Returns the number of samples read on the final attempt.
    fn read_until_data(src: &BufferedSource, buf: &mut [f32], deadline: Instant) -> usize {
        loop {
            let n = src.read(buf);
            if buf.iter().any(|s| *s != 0.0) || Instant::now() > deadline {
                return n;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn delivers_source_samples_in_order() {
        let src = BufferedSource::new(Box::new(RampSource::new()));
        let mut buf = vec![-1.0f32; 256];
        let deadline = Instant::now() + Duration::from_secs(5);
        let n = read_until_data(&src, &mut buf, deadline);
        assert_eq!(n, 256);
        let base = buf[0];
        for (i, s) in buf.iter().enumerate() {
            assert_eq!(*s, base + i as f32, "sample {i} out of order");
        }
    }

    #[test]
    fn seek_discards_stale_ring_contents() {
        let src = BufferedSource::new(Box::new(RampSource::new()));
        let mut buf = vec![0.0f32; 256];
        let deadline = Instant::now() + Duration::from_secs(5);
        read_until_data(&src, &mut buf, deadline);

        src.seek(100_000);
        // First reads after a seek are silence while the BG thread re-seeks,
        // then data resumes at the seek target — never stale pre-seek samples.
        let deadline = Instant::now() + Duration::from_secs(5);
        let n = read_until_data(&src, &mut buf, deadline);
        assert_eq!(n, 256);
        assert_eq!(
            buf[0], 100_000.0,
            "first post-seek sample should be the seek target"
        );
        assert_eq!(src.position(), 100_000 + 256);
    }
}
