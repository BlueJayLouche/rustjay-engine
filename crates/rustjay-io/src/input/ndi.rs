//! # NDI Input
//!
//! Network Device Interface video input receiver.

// Query helpers and accessors here are part of the NDI backend surface but not all
// are consumed yet; keep them available without warning.
#![allow(dead_code)]

use crossbeam::channel::{self, Receiver as CrossbeamReceiver, Sender};
use grafton_ndi::{
    Finder, FinderOptions, Receiver, ReceiverBandwidth, ReceiverColorFormat, ReceiverOptions, NDI,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Information about an available NDI source
#[derive(Debug, Clone)]
pub struct NdiSourceInfo {
    pub name: String,
    pub url: String,
}

/// How an [`NdiFrame`]'s bytes are laid out.
///
/// We ask senders for their native 4:2:2 and only get BGRA when the source
/// actually carries alpha, so consumers have to handle both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdiPixelLayout {
    /// Tightly packed BGRA, `width * 4` bytes per row.
    Bgra,
    /// Packed 4:2:2 (U Y0 V Y1), `width * 2` bytes per row. Upload as a
    /// half-width RGBA texture and unpack in a shader.
    Uyvy,
}

impl NdiPixelLayout {
    /// Bytes in one tightly-packed row of `width` pixels.
    pub fn row_bytes(self, width: u32) -> usize {
        match self {
            Self::Bgra => width as usize * 4,
            Self::Uyvy => width as usize * 2,
        }
    }
}

/// A frame the receive thread wrote straight into GPU-visible memory.
///
/// Rows are `bytes_per_row` apart — wgpu's copy alignment, not the frame's
/// own row length. The consumer encodes a `copy_buffer_to_texture` and then
/// hands the buffer back with [`NdiReceiver::remap_staged`].
pub struct StagedFrame {
    pub buffer: wgpu::Buffer,
    pub bytes_per_row: u32,
}

/// A received NDI video frame
pub struct NdiFrame {
    pub width: u32,
    pub height: u32,
    /// Pixel data in `layout`. Empty when `staged` carries the frame instead.
    pub data: Vec<u8>,
    /// Set when the consumer offered a buffer via
    /// [`NdiReceiver::provide_staging`]: the frame is already on the GPU side
    /// and `data` is empty.
    pub staged: Option<StagedFrame>,
    pub layout: NdiPixelLayout,
    pub timestamp: Instant,
}

/// NDI receiver that captures video frames from a source
pub struct NdiReceiver {
    source_name: String,
    receiver_thread: Option<JoinHandle<()>>,
    frame_tx: Sender<NdiFrame>,
    frame_rx: CrossbeamReceiver<NdiFrame>,
    running: Arc<AtomicBool>,
    /// Set when the source disappears (not found, or too many consecutive errors)
    source_lost: Arc<AtomicBool>,
    resolution: (u32, u32),
    /// Frame buffers handed back via [`Self::recycle`], refilled by the
    /// receive thread. A fresh multi-MB buffer per frame costs ~4.6× a reused
    /// one: the OS zero-fills and page-faults every new page.
    spare_tx: Sender<Vec<u8>>,
    spare_rx: CrossbeamReceiver<Vec<u8>>,
    /// Mapped buffers the consumer offers, for the receive thread to write
    /// frames into directly; see [`Self::provide_staging`].
    staging_tx: Sender<wgpu::Buffer>,
    staging_rx: CrossbeamReceiver<wgpu::Buffer>,
    /// Buffers the receive thread wrote but could not deliver, because the
    /// frame channel was full. They are unmapped, so they rejoin the ring
    /// through [`Self::remap_staged`] rather than going straight back.
    undelivered_tx: Sender<wgpu::Buffer>,
    undelivered_rx: CrossbeamReceiver<wgpu::Buffer>,
    /// What the buffers now in circulation were sized for.
    staging_key: Option<(u32, u32, NdiPixelLayout)>,
    /// How many buffers have been made for that size. Buffers leave the ring
    /// for good — dropped for the wrong size, or awaiting a re-map — so the
    /// count bounds how many replacements get made.
    staging_made: usize,
}

/// Buffers the ring starts with. Two are in flight at most (one being written,
/// one being copied); the third covers the frame a re-map is still pending on.
const STAGING_BUFFERS: usize = 3;

/// Ceiling on buffers for one frame size, so a path that keeps losing them
/// can't allocate without end. A sender faster than the renderer needs more
/// than the starting three: every frame it delivers between two renders takes
/// one, and each is out of circulation until its re-map lands.
const STAGING_MAX: usize = 8;

impl NdiReceiver {
    /// Create a new NDI receiver (does not start receiving yet)
    pub fn new(source_name: impl Into<String>) -> Self {
        let (frame_tx, frame_rx) = channel::bounded(5);
        let (spare_tx, spare_rx) = channel::bounded(4);
        // Room for every buffer that can exist at once: a return refused here
        // is a buffer gone from the ring for good.
        let (staging_tx, staging_rx) = channel::bounded(STAGING_MAX);
        let (undelivered_tx, undelivered_rx) = channel::bounded(STAGING_MAX);

        Self {
            spare_tx,
            spare_rx,
            staging_tx,
            staging_rx,
            undelivered_tx,
            undelivered_rx,
            staging_key: None,
            staging_made: 0,
            source_name: source_name.into(),
            receiver_thread: None,
            frame_tx,
            frame_rx,
            running: Arc::new(AtomicBool::new(false)),
            source_lost: Arc::new(AtomicBool::new(false)),
            resolution: (1920, 1080),
        }
    }

    /// Returns true if the source has been lost (not found or repeated errors)
    pub fn is_source_lost(&self) -> bool {
        self.source_lost.load(Ordering::Relaxed)
    }

    /// Start receiving from the NDI source
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.receiver_thread.is_some() {
            return Err(anyhow::anyhow!("NDI receiver already started"));
        }

        let ndi = NDI::new().map_err(|e| anyhow::anyhow!("Failed to initialize NDI: {:?}", e))?;

        let source_name = self.source_name.clone();
        let frame_tx = self.frame_tx.clone();
        let spare_rx = self.spare_rx.clone();
        let staging_rx = self.staging_rx.clone();
        let undelivered_tx = self.undelivered_tx.clone();
        let running = Arc::clone(&self.running);
        let source_lost = Arc::clone(&self.source_lost);
        running.store(true, Ordering::SeqCst);
        source_lost.store(false, Ordering::Relaxed);

        let thread_handle = thread::spawn(move || {
            // Find the source
            let options = FinderOptions::builder().show_local_sources(true).build();

            let finder = match Finder::new(&ndi, &options) {
                Ok(f) => f,
                Err(e) => {
                    log::error!("[NDI] Failed to create finder: {:?}", e);
                    return;
                }
            };

            // Wait for the specific source.
            // Strategy: call wait_for_sources() (which blocks until the source
            // list *changes*), then snapshot with current_sources(). This avoids
            // the race in sources(timeout) where the change event is consumed by
            // wait_for_sources but the subsequent get returns stale data.
            let mut found_source = None;
            let search_start = Instant::now();
            const SEARCH_TIMEOUT_SECS: u64 = 30;

            #[cfg(target_os = "windows")]
            log::warn!(
                "[NDI] If no sources are found, check Windows Firewall: \
                 the NDI runtime needs UDP access (ports 5353, 5960-5969) \
                 for the app executable."
            );

            while running.load(Ordering::SeqCst)
                && search_start.elapsed().as_secs() < SEARCH_TIMEOUT_SECS
            {
                // Block up to 500 ms for any change in the source list.
                let _ = finder.wait_for_sources(Duration::from_millis(500));

                match finder.current_sources() {
                    Ok(sources) => {
                        if !sources.is_empty() {
                            log::debug!(
                                "[NDI] Visible sources: {}",
                                sources.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")
                            );
                        }
                        for source in sources {
                            if source.name == source_name {
                                found_source = Some(source);
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        log::debug!("[NDI] Error listing sources: {:?}", e);
                    }
                }

                if found_source.is_some() {
                    break;
                }

            }

            let source = match found_source {
                Some(s) => s,
                None => {
                    log::error!(
                        "[NDI] Could not find source '{}' within timeout",
                        source_name
                    );
                    source_lost.store(true, Ordering::Relaxed);
                    return;
                }
            };

            // Ask for the sender's native 4:2:2 (BGRA only when it has alpha).
            // Requesting BGRA made libndi convert on the CPU via vImage, whose
            // dispatch_apply left ~14 worker threads spinning in root-queue
            // contention — 25x more time yielding than converting. The unpack is
            // a shader's job.
            let options = ReceiverOptions::builder(source)
                .color(ReceiverColorFormat::UYVY_BGRA)
                .bandwidth(ReceiverBandwidth::Highest)
                .build();

            let receiver = match Receiver::new(&ndi, &options) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("[NDI] Failed to create receiver: {:?}", e);
                    return;
                }
            };

            log::info!("[NDI] Connected to: {}", source_name);

            // Receive loop
            let mut consecutive_errors = 0u32;
            // Which path frames are taking, said once each way round.
            let mut on_gpu_path = false;
            let mut fallbacks = 0u64;
            while running.load(Ordering::SeqCst) {
                match receiver.capture_video_ref(Duration::from_millis(100)) {
                    Ok(Some(video_frame)) => {
                        consecutive_errors = 0;
                        let width = video_frame.width() as u32;
                        let height = video_frame.height() as u32;
                        let frame_data = video_frame.data();
                        let layout = match video_frame.pixel_format() {
                            grafton_ndi::PixelFormat::UYVY => NdiPixelLayout::Uyvy,
                            _ => NdiPixelLayout::Bgra,
                        };

                        // Straight into GPU-visible memory when the consumer
                        // has offered a buffer the right size for this frame:
                        // then nothing ever copies it again on the CPU. A
                        // buffer left over from another size is dropped.
                        let bytes_per_row = (layout.row_bytes(width) as u32)
                            .next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
                        let staged_size = u64::from(bytes_per_row) * u64::from(height);
                        let staged = staging_rx
                            .try_recv()
                            .ok()
                            .filter(|buffer| {
                                let fits = buffer.size() == staged_size;
                                if !fits {
                                    log::debug!(
                                        "[NDI] staging buffer is {} bytes, this frame needs \
                                         {staged_size} — dropped",
                                        buffer.size()
                                    );
                                }
                                fits
                            })
                            .and_then(|buffer| {
                                match buffer.get_mapped_range_mut(..) {
                                    Ok(mut view) => {
                                        strip_stride_into_mapped(
                                            &mut view,
                                            frame_data,
                                            width,
                                            height,
                                            layout,
                                            bytes_per_row,
                                        );
                                    }
                                    Err(e) => {
                                        log::warn!("[NDI] staging buffer not mapped: {e}");
                                        return None;
                                    }
                                }
                                buffer.unmap();
                                Some(StagedFrame {
                                    buffer,
                                    bytes_per_row,
                                })
                            });

                        // Otherwise strip NDI's row padding into a recycled
                        // Vec, for the consumer to upload itself.
                        let mut data = Vec::new();
                        if staged.is_none() {
                            data = spare_rx.try_recv().unwrap_or_default();
                            strip_stride(&mut data, frame_data, width, height, layout);
                            fallbacks += 1;
                            if on_gpu_path || fallbacks == 120 {
                                on_gpu_path = false;
                                log::info!(
                                    "[NDI] no staging buffer free — frames are going through \
                                     the CPU upload path"
                                );
                            }
                        } else if !on_gpu_path {
                            on_gpu_path = true;
                            fallbacks = 0;
                            log::info!("[NDI] frames are landing straight in GPU memory");
                        }

                        // A frame the consumer is too busy to take is dropped,
                        // but its buffer is not: without this the ring bleeds
                        // dry exactly when the sender outruns the renderer,
                        // which is when staging is worth the most.
                        if let Err(channel::TrySendError::Full(rejected)) =
                            frame_tx.try_send(NdiFrame {
                                width,
                                height,
                                data,
                                staged,
                                layout,
                                timestamp: Instant::now(),
                            })
                            && let Some(staged) = rejected.staged
                        {
                            let _ = undelivered_tx.try_send(staged.buffer);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        consecutive_errors += 1;
                        log::error!(
                            "[NDI] Frame capture error ({}/50): {:?}",
                            consecutive_errors,
                            e
                        );
                        // After ~5s of continuous errors, declare the source lost
                        if consecutive_errors >= 50 {
                            log::warn!(
                                "[NDI] Source '{}' considered lost after repeated errors",
                                source_name
                            );
                            source_lost.store(true, Ordering::Relaxed);
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                }
            }
        });

        self.receiver_thread = Some(thread_handle);
        Ok(())
    }

    /// Stop receiving frames
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);

        if let Some(handle) = self.receiver_thread.take() {
            let _ = handle.join();
        }

        log::info!("[NDI] Receiver stopped for source: {}", self.source_name);
    }

    /// Get the latest frame (non-blocking, consumes the frame)
    pub fn get_latest_frame(&mut self) -> Option<NdiFrame> {
        let mut latest: Option<NdiFrame> = None;
        while let Ok(frame) = self.frame_rx.try_recv() {
            self.resolution = (frame.width, frame.height);
            if let Some(skipped) = latest.replace(frame) {
                // A frame nobody will see still holds a buffer. Both kinds go
                // back: dropping a staged one takes it out of the ring for
                // good, and the ring bleeds dry within a second or two.
                match skipped.staged {
                    Some(staged) => self.remap_staged(staged.buffer),
                    None => self.recycle(skipped.data),
                }
            }
        }
        latest
    }

    /// Hand a frame's buffer back once it has been uploaded, so the receive
    /// thread refills it instead of allocating. Dropping it instead is fine,
    /// just slower.
    pub fn recycle(&self, data: Vec<u8>) {
        let _ = self.spare_tx.try_send(data);
    }

    /// Offer the receive thread mapped buffers to write frames into, so the
    /// pixels never pass through a `Vec` and the render thread only encodes a
    /// copy instead of paying for one ([`NdiFrame::staged`]).
    ///
    /// Call it each frame with the size now arriving: it rebuilds the ring
    /// when that changes and does nothing when it hasn't. Buffers left from an
    /// earlier size are dropped by the receive thread, which falls back to the
    /// `Vec` path until the new ones arrive.
    pub fn provide_staging(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        layout: NdiPixelLayout,
    ) {
        if width == 0 || height == 0 {
            return;
        }
        // Buffers the receive thread wrote but couldn't deliver are unmapped;
        // map them again and they rejoin the ring.
        while let Ok(buffer) = self.undelivered_rx.try_recv() {
            self.remap_staged(buffer);
        }

        let key = (width, height, layout);
        let fresh = self.staging_key != Some(key);
        if fresh {
            // A new size: buffers for the old one are dropped by the receive
            // thread as they come round.
            self.staging_key = Some(key);
            self.staging_made = 0;
        } else if !self.staging_rx.is_empty() || self.staging_made >= STAGING_MAX {
            // Buffers are already waiting, or enough are in circulation.
            return;
        }
        // Refill when the ring runs dry, not only when the size changes: a
        // buffer is gone for good once the thread drops it for the wrong size,
        // and a re-map can lag. Without this the ring empties once and every
        // frame falls back to the Vec path for good.
        let bytes_per_row =
            (layout.row_bytes(width) as u32).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        for _ in 0..if fresh { STAGING_BUFFERS } else { 1 } {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("NDI staging"),
                size: u64::from(bytes_per_row) * u64::from(height),
                // MAP_WRITE pairs only with COPY_SRC, which is all we need.
                usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: true,
            });
            if self.staging_tx.try_send(buffer).is_err() {
                break;
            }
            self.staging_made += 1;
        }
    }

    /// Hand a staged buffer back, once its copy has been *submitted*: it is
    /// mapped again in the background and rejoins the pool. The callback only
    /// runs while the device is polled, so poll it each frame.
    pub fn remap_staged(&self, buffer: wgpu::Buffer) {
        let tx = self.staging_tx.clone();
        let returned = buffer.clone();
        buffer.map_async(wgpu::MapMode::Write, .., move |result| {
            match result {
                // The channel holds every buffer that can exist, so a refusal
                // here means the accounting is wrong — and a silent one would
                // look like the ring mysteriously starving.
                Ok(()) => {
                    if tx.try_send(returned).is_err() {
                        log::warn!("[NDI] staging ring full on return — buffer dropped");
                    }
                }
                Err(e) => log::warn!("[NDI] staging buffer could not be mapped again: {e}"),
            }
        });
    }

    /// Check if a new frame is available
    pub fn has_frame(&self) -> bool {
        !self.frame_rx.is_empty()
    }

    /// Get current resolution
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }

    /// Check if receiver is running
    pub fn is_running(&self) -> bool {
        self.receiver_thread.is_some() && self.running.load(Ordering::SeqCst)
    }
}

impl Drop for NdiReceiver {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Strip NDI row stride/padding from raw frame data.
///
/// NDI frames may have row-aligned padding (e.g. IOSurface stride alignment on macOS).
/// This produces tightly-packed rows ready to upload. BGRA needs no channel
/// swap — it already matches `Bgra8Unorm` — and UYVY is uploaded as-is for the
/// shader to unpack.
///
/// Fills `out` in place, reusing its allocation (see [`NdiReceiver::recycle`]).
fn strip_stride(out: &mut Vec<u8>, data: &[u8], width: u32, height: u32, layout: NdiPixelLayout) {
    let row_bytes = layout.row_bytes(width);
    let rows = height as usize;
    let actual_stride = if rows > 0 { data.len() / rows } else { row_bytes };
    out.clear();

    // Unpadded — most senders: one copy.
    if actual_stride == row_bytes {
        out.extend_from_slice(&data[..row_bytes * rows]);
        return;
    }

    out.reserve(row_bytes * rows);
    for y in 0..rows {
        let src = y * actual_stride;
        match data.get(src..src + row_bytes) {
            Some(row) => out.extend_from_slice(row),
            // A short frame keeps its size; the missing rows are black.
            None => out.resize(out.len() + row_bytes, 0),
        }
    }
}

/// [`strip_stride`], writing into mapped GPU memory instead of a `Vec`.
///
/// Rows land `bytes_per_row` apart, the copy alignment wgpu requires, which is
/// usually wider than the frame's own rows. Writes only, never reads: the
/// memory is typically write-combined, where reading is glacial.
fn strip_stride_into_mapped(
    out: &mut wgpu::BufferViewMut,
    data: &[u8],
    width: u32,
    height: u32,
    layout: NdiPixelLayout,
    bytes_per_row: u32,
) {
    let row_bytes = layout.row_bytes(width);
    let rows = height as usize;
    let stride = if rows > 0 { data.len() / rows } else { row_bytes };
    for y in 0..rows {
        let Some(row) = data.get(y * stride..y * stride + row_bytes) else {
            // A short frame leaves the rest of the buffer as it was.
            break;
        };
        let dst = y * bytes_per_row as usize;
        out.slice(dst..dst + row_bytes).copy_from_slice(row);
    }
}

#[cfg(test)]
mod strip_stride_tests {
    use super::{strip_stride, NdiPixelLayout};

    fn stripped(data: &[u8], w: u32, h: u32, layout: NdiPixelLayout, reuse: &mut Vec<u8>) -> Vec<u8> {
        strip_stride(reuse, data, w, h, layout);
        reuse.clone()
    }

    #[test]
    fn strips_padding_and_passes_tight_frames_through() {
        // One buffer across all three, as the receive thread reuses it; a
        // leftover byte from an earlier, bigger frame must not survive.
        let mut buf = vec![0xAA; 64];

        // 2×2 BGRA: 8-byte rows padded to 12.
        let padded: Vec<u8> = (0..24).collect();
        let expected: Vec<u8> = (0..8).chain(12..20).collect();
        assert_eq!(stripped(&padded, 2, 2, NdiPixelLayout::Bgra, &mut buf), expected);

        let tight: Vec<u8> = (0..16).collect();
        assert_eq!(stripped(&tight, 2, 2, NdiPixelLayout::Bgra, &mut buf), tight);

        // UYVY rows are width * 2.
        let uyvy: Vec<u8> = (0..8).collect();
        assert_eq!(stripped(&uyvy, 2, 2, NdiPixelLayout::Uyvy, &mut buf), uyvy);
    }
}

/// Global NDI availability check
pub fn is_ndi_available() -> bool {
    NDI::new().is_ok()
}

/// Quick function to list available NDI sources
pub fn list_ndi_sources(timeout_ms: u32) -> Vec<String> {
    let ndi = match NDI::new() {
        Ok(ndi) => ndi,
        Err(e) => {
            log::error!("Failed to initialize NDI: {:?}", e);
            return Vec::new();
        }
    };

    let options = FinderOptions::builder().show_local_sources(true).build();

    let finder = match Finder::new(&ndi, &options) {
        Ok(f) => f,
        Err(e) => {
            log::error!("Failed to create NDI finder: {:?}", e);
            return Vec::new();
        }
    };

    // Poll using wait_for_sources() + current_sources() in a loop.
    // A single sources(timeout_ms) call only returns after one change event;
    // if sources are already known or no new sources arrive, it may time out
    // without returning everything the SDK has discovered. The loop below
    // keeps draining change events until the deadline, then returns a final
    // snapshot so we don't miss sources announced just before the deadline.
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms as u64);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let wait = remaining.min(Duration::from_millis(500));
        let _ = finder.wait_for_sources(wait);
    }

    match finder.current_sources() {
        Ok(sources) => {
            for s in &sources {
                log::info!("[NDI] Discovered source: \"{}\"", s.name);
            }
            sources.into_iter().map(|s| s.name).collect()
        }
        Err(e) => {
            log::error!("Failed to get NDI sources: {:?}", e);
            Vec::new()
        }
    }
}
