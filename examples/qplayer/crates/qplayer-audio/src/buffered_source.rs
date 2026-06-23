//! Double-buffered sample provider — decouples slow file I/O from the audio callback.
//!
//! A background thread continuously reads from the wrapped source into a ring buffer.
//! The audio callback reads from the ring buffer without blocking.

use crate::SampleProvider;
use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Default ring buffer size: 3 seconds at 48 kHz stereo.
const DEFAULT_RING_SECONDS: f32 = 3.0;

struct Inner {
    sample_rate: u32,
    channels: u16,
    // Ring buffer — written only by BG thread, read only by audio thread.
    ring: UnsafeCell<Vec<f32>>,
    /// Write position (only advanced by background thread).
    write_pos: AtomicUsize,
    /// Read position (only advanced by audio callback thread).
    read_pos: AtomicUsize,
    /// EOF reached on source.
    eof: AtomicBool,
    /// Seek target sample (set by audio thread, consumed by BG thread).
    seek_target: AtomicUsize,
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
        let ring_samples = (sr as f32 * DEFAULT_RING_SECONDS * ch as f32).ceil() as usize;

        let inner = Arc::new(Inner {
            sample_rate: sr,
            channels: ch,
            ring: UnsafeCell::new(vec![0.0f32; ring_samples]),
            write_pos: AtomicUsize::new(0),
            read_pos: AtomicUsize::new(0),
            eof: AtomicBool::new(false),
            seek_target: AtomicUsize::new(usize::MAX), // MAX = no seek pending
            source: Mutex::new(source),
        });

        let inner2 = Arc::clone(&inner);
        let bg_thread = std::thread::spawn(move || {
            Self::bg_loop(inner2);
        });

        Self {
            inner,
            _bg_thread: bg_thread,
        }
    }

    fn bg_loop(inner: Arc<Inner>) {
        let mut temp = vec![0.0f32; 4096];
        loop {
            // Check for pending seek
            let seek = inner.seek_target.swap(usize::MAX, Ordering::Acquire);
            if seek != usize::MAX {
                if let Ok(src) = inner.source.lock() {
                    src.seek(seek);
                }
                // Reset ring
                inner.read_pos.store(0, Ordering::Release);
                inner.write_pos.store(0, Ordering::Release);
                inner.eof.store(false, Ordering::Relaxed);
            }

            if inner.eof.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(10));
                continue;
            }

            let ring_size = unsafe { (*inner.ring.get()).len() };
            let wp = inner.write_pos.load(Ordering::Relaxed);
            let rp = inner.read_pos.load(Ordering::Acquire);
            let used = wp.saturating_sub(rp);
            let free = ring_size.saturating_sub(used);

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

            // Copy into ring buffer (BG thread is the sole writer)
            let ring = unsafe { &mut *inner.ring.get() };
            let write_idx = wp % ring_size;
            let end = (write_idx + n).min(ring_size);
            let first_len = end - write_idx;
            ring[write_idx..end].copy_from_slice(&temp[..first_len]);
            if n > first_len {
                let second_len = n - first_len;
                ring[..second_len].copy_from_slice(&temp[first_len..n]);
            }

            inner.write_pos.store(wp + n, Ordering::Release);
        }
    }
}

impl SampleProvider for BufferedSource {
    fn read(&self, buffer: &mut [f32]) -> usize {
        let ring_size = unsafe { (*self.inner.ring.get()).len() };
        let rp = self.inner.read_pos.load(Ordering::Relaxed);
        let wp = self.inner.write_pos.load(Ordering::Acquire);
        let available = wp.saturating_sub(rp);
        let to_copy = buffer.len().min(available);

        if to_copy == 0 {
            return 0;
        }

        let ring = unsafe { &*self.inner.ring.get() };
        let read_idx = rp % ring_size;
        let end = (read_idx + to_copy).min(ring_size);
        let first_len = end - read_idx;
        buffer[..first_len].copy_from_slice(&ring[read_idx..end]);
        if to_copy > first_len {
            let second_len = to_copy - first_len;
            buffer[first_len..to_copy].copy_from_slice(&ring[..second_len]);
        }

        self.inner.read_pos.store(rp + to_copy, Ordering::Release);
        to_copy
    }

    fn seek(&self, sample: usize) {
        self.inner.seek_target.store(sample, Ordering::Release);
        // Spin briefly to let the BG thread pick up the seek
        for _ in 0..100 {
            if self.inner.seek_target.load(Ordering::Acquire) == usize::MAX {
                return;
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        }
    }

    fn position(&self) -> usize {
        self.inner.read_pos.load(Ordering::Relaxed)
    }

    fn length(&self) -> Option<usize> {
        if let Ok(src) = self.inner.source.lock() {
            src.length()
        } else {
            None
        }
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate
    }

    fn channels(&self) -> u16 {
        self.inner.channels
    }
}

unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}
unsafe impl Send for BufferedSource {}
unsafe impl Sync for BufferedSource {}
