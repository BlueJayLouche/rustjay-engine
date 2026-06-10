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

/// A received NDI video frame
pub struct NdiFrame {
    pub width: u32,
    pub height: u32,
    /// BGRA pixel data
    pub data: Vec<u8>,
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
}

impl NdiReceiver {
    /// Create a new NDI receiver (does not start receiving yet)
    pub fn new(source_name: impl Into<String>) -> Self {
        let (frame_tx, frame_rx) = channel::bounded(5);

        Self {
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

            // Create receiver with BGRA format
            let options = ReceiverOptions::builder(source)
                .color(ReceiverColorFormat::BGRX_BGRA)
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
            while running.load(Ordering::SeqCst) {
                match receiver.capture_video_ref(Duration::from_millis(100)) {
                    Ok(Some(video_frame)) => {
                        consecutive_errors = 0;
                        let width = video_frame.width() as u32;
                        let height = video_frame.height() as u32;
                        let frame_data = video_frame.data();

                        // Strip NDI row stride/padding to produce tightly-packed BGRA
                        // matching bytes_per_row = width * 4 expected by the GPU upload.
                        let frame = NdiFrame {
                            width,
                            height,
                            data: strip_stride_bgra(frame_data, width, height),
                            timestamp: Instant::now(),
                        };

                        let _ = frame_tx.try_send(frame);
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
            latest = Some(frame);
        }
        latest
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
/// This produces tightly-packed BGRA bytes ready to upload to a `Bgra8Unorm` wgpu
/// texture with `bytes_per_row = width * 4`. No channel swap needed since
/// `ReceiverColorFormat::BGRX_BGRA` already matches `Bgra8Unorm`.
fn strip_stride_bgra(data: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row_bytes = width as usize * 4;
    let mut out = vec![0u8; row_bytes * height as usize];

    let actual_stride = if height > 0 {
        data.len() / height as usize
    } else {
        row_bytes
    };

    for y in 0..height as usize {
        let src = y * actual_stride;
        let dst = y * row_bytes;
        if src + row_bytes <= data.len() {
            out[dst..dst + row_bytes].copy_from_slice(&data[src..src + row_bytes]);
        }
    }

    out
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
