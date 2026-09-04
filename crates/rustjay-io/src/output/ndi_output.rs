//! # NDI Output Sender
//!
//! Sends video frames as an NDI stream.

// Accessors and availability checks here are part of the NDI backend surface but not all
// are consumed yet; keep them available without warning.
#![allow(dead_code)]

use crossbeam::channel::{self, Receiver, Sender as ChannelSender};
use grafton_ndi::{PixelFormat, Sender, SenderOptions, VideoFrame, VideoFrameBuilder, NDI};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

/// How often the send thread polls the SDK for receiver connections.
const CONNECTION_CHECK_INTERVAL: Duration = Duration::from_millis(500);

/// NDI video frame data (CPU side)
pub struct FrameData {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>, // BGRA or BGRX format
    pub has_alpha: bool,
    pub timestamp: Instant,
}

/// NDI output sender
pub struct NdiOutputSender {
    name: String,
    width: u32,
    height: u32,
    include_alpha: bool,
    frame_tx: ChannelSender<FrameData>,
    /// Buffers returned by the send thread for reuse (avoids a per-frame
    /// multi-MB allocation in `submit_frame`).
    return_rx: Receiver<Vec<u8>>,
    /// Updated by the send thread: whether any NDI receiver is connected.
    /// Starts optimistic so a receiver present at startup sees frames
    /// immediately; flips within `CONNECTION_CHECK_INTERVAL`.
    has_connections: Arc<AtomicBool>,
    running: Arc<AtomicBool>,
    is_owner: bool,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl NdiOutputSender {
    /// Create and start a new NDI output sender
    pub fn new(
        name: impl Into<String>,
        width: u32,
        height: u32,
        include_alpha: bool,
    ) -> anyhow::Result<Self> {
        let name = name.into();

        if width == 0 || height == 0 {
            return Err(anyhow::anyhow!("Invalid dimensions: {}x{}", width, height));
        }

        let ndi = NDI::new().map_err(|e| anyhow::anyhow!("Failed to initialize NDI: {:?}", e))?;

        let (frame_tx, frame_rx) = channel::bounded(2);
        let (return_tx, return_rx) = channel::bounded(2);

        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);

        let has_connections = Arc::new(AtomicBool::new(true));
        let has_connections_clone = Arc::clone(&has_connections);

        let name_clone = name.clone();

        // Spawn send thread
        let thread_handle = thread::spawn(move || {
            Self::send_thread(
                ndi,
                name_clone,
                include_alpha,
                frame_rx,
                return_tx,
                has_connections_clone,
                running_clone,
            );
        });

        Ok(Self {
            name,
            width,
            height,
            include_alpha,
            frame_tx,
            return_rx,
            has_connections,
            running,
            is_owner: true,
            thread_handle: Some(thread_handle),
        })
    }

    /// Send thread that owns the NDI sender and processes frames
    fn send_thread(
        ndi: NDI,
        name: String,
        include_alpha: bool,
        frame_rx: Receiver<FrameData>,
        return_tx: ChannelSender<Vec<u8>>,
        has_connections: Arc<AtomicBool>,
        running: Arc<AtomicBool>,
    ) {
        let options = SenderOptions::builder(&name)
            .clock_video(true)
            .clock_audio(false)
            .build();

        let sender = match Sender::new(&ndi, &options) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[NDI OUTPUT] Failed to create NDI sender: {:?}", e);
                has_connections.store(false, Ordering::SeqCst);
                return;
            }
        };

        let pixel_format = if include_alpha {
            PixelFormat::BGRA
        } else {
            PixelFormat::BGRX
        };

        // Persistent NDI frame: rebuilt only on resolution change, so we
        // don't alloc+memset a full-size buffer every frame.
        let mut video_frame: Option<VideoFrame> = None;
        let mut frame_count = 0u64;
        let mut last_log = Instant::now();
        // Check on the first loop iteration.
        let mut last_conn_check = Instant::now() - CONNECTION_CHECK_INTERVAL;

        while running.load(Ordering::SeqCst) {
            if last_conn_check.elapsed() >= CONNECTION_CHECK_INTERVAL {
                // Zero timeout = last known count, never blocks. On SDK error
                // assume connected (keep streaming) rather than go dark.
                let count = sender.connection_count(Duration::ZERO).unwrap_or(1);
                let connected = count > 0;
                if has_connections.swap(connected, Ordering::SeqCst) != connected {
                    log::info!(
                        "[NDI OUTPUT] '{}' receiver connections: {}",
                        name,
                        count
                    );
                }
                last_conn_check = Instant::now();
            }

            match frame_rx.recv_timeout(Duration::from_millis(100)) {
                Ok(frame_data) => {
                    // No receivers: drop the frame (buffer still recycled) —
                    // encoding to nobody is the dominant idle cost.
                    if !has_connections.load(Ordering::SeqCst) {
                        let _ = return_tx.try_send(frame_data.data);
                        continue;
                    }

                    frame_count += 1;

                    let buffer_size =
                        pixel_format.buffer_size(frame_data.width as i32, frame_data.height as i32);

                    if frame_data.data.len() < buffer_size {
                        log::warn!("[NDI OUTPUT] Frame {} data too small", frame_count);
                        let _ = return_tx.try_send(frame_data.data);
                        continue;
                    }

                    let rebuild = match &video_frame {
                        Some(f) => {
                            f.width != frame_data.width as i32
                                || f.height != frame_data.height as i32
                        }
                        None => true,
                    };
                    if rebuild {
                        video_frame = match VideoFrameBuilder::new()
                            .resolution(frame_data.width as i32, frame_data.height as i32)
                            .pixel_format(pixel_format)
                            .frame_rate(60, 1)
                            .aspect_ratio(frame_data.width as f32 / frame_data.height as f32)
                            .build()
                        {
                            Ok(f) => Some(f),
                            Err(e) => {
                                log::error!("[NDI OUTPUT] Failed to build video frame: {:?}", e);
                                let _ = return_tx.try_send(frame_data.data);
                                continue;
                            }
                        };
                    }

                    let frame = video_frame.as_mut().expect("video frame just built");
                    let copy_len = buffer_size.min(frame.data.len());
                    frame.data[..copy_len].copy_from_slice(&frame_data.data[..copy_len]);
                    sender.send_video(frame);

                    let _ = return_tx.try_send(frame_data.data);

                    if last_log.elapsed().as_secs() >= 30 {
                        log::info!("[NDI OUTPUT] {} frames sent to '{}'", frame_count, name);
                        last_log = Instant::now();
                    }
                }
                Err(channel::RecvTimeoutError::Timeout) => {}
                Err(channel::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    /// Submit a frame for sending
    pub fn submit_frame(
        &mut self,
        bgra_data: &[u8],
        width: u32,
        height: u32,
    ) -> anyhow::Result<()> {
        if width != self.width || height != self.height {
            log::info!(
                "[NDI OUTPUT] Resolution change: {}x{} -> {}x{} (restarting '{}')",
                self.width,
                self.height,
                width,
                height,
                self.name
            );
            let name = self.name.clone();
            let include_alpha = self.include_alpha;
            self.stop();
            *self = Self::new(name, width, height, include_alpha)?;
        }

        if bgra_data.is_empty() {
            log::warn!("[NDI OUTPUT] Empty frame data received");
            return Ok(());
        }

        // No receivers connected: skip the copy + channel entirely. The
        // OutputManager normally gates the readback upstream via
        // `has_connections()`; this guards direct callers.
        if !self.has_connections() {
            return Ok(());
        }

        let len = bgra_data.len();
        // Reuse a returned buffer when one is available and big enough.
        let mut data = match self.return_rx.try_recv() {
            Ok(mut buf) if buf.capacity() >= len => {
                buf.clear();
                buf
            }
            _ => Vec::with_capacity(len),
        };
        data.extend_from_slice(bgra_data);

        let frame = FrameData {
            width,
            height,
            data,
            has_alpha: self.include_alpha,
            timestamp: Instant::now(),
        };

        match self.frame_tx.try_send(frame) {
            Ok(_) => {}
            Err(channel::TrySendError::Full(_)) => {
                log::debug!("[NDI OUTPUT] Frame dropped - channel full");
            }
            Err(channel::TrySendError::Disconnected(_)) => {
                log::warn!("[NDI OUTPUT] Frame channel disconnected");
            }
        }

        Ok(())
    }

    /// Stop the NDI sender
    pub fn stop(&mut self) {
        if !self.is_owner {
            return;
        }
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    /// Check if sender is running
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Whether any NDI receiver is currently connected (polled by the send
    /// thread every `CONNECTION_CHECK_INTERVAL`; optimistic until first poll).
    pub fn has_connections(&self) -> bool {
        self.has_connections.load(Ordering::SeqCst)
    }
}

impl Clone for NdiOutputSender {
    fn clone(&self) -> Self {
        // Clones don't participate in buffer recycling — they get a dead-end
        // receiver and simply allocate per submit.
        let (_dead_tx, dead_rx) = channel::bounded(0);
        Self {
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            include_alpha: self.include_alpha,
            frame_tx: self.frame_tx.clone(),
            return_rx: dead_rx,
            has_connections: Arc::clone(&self.has_connections),
            running: Arc::clone(&self.running),
            is_owner: false,
            thread_handle: None,
        }
    }
}

impl Drop for NdiOutputSender {
    fn drop(&mut self) {
        if self.is_owner {
            self.stop();
        }
    }
}

/// Check if NDI output is available
pub fn is_ndi_output_available() -> bool {
    NDI::new().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no receivers connected, the send thread's connection poll must flip `has_connections` off within about
    /// one check interval — otherwise idle NDI keeps copying + encoding frames to nobody.
    /// Skips when the NDI runtime is unavailable.
    #[test]
    fn connection_gating_flips_off_without_receivers() {
        if !is_ndi_output_available() {
            eprintln!("NDI runtime unavailable — skipping");
            return;
        }
        let mut sender =
            NdiOutputSender::new("rustjay-ndi-gating-test", 64, 64, false).expect("failed to create NDI sender");
        assert!(
            sender.has_connections(),
            "connection state should start optimistic"
        );

        let frame = vec![0u8; 64 * 64 * 4];
        for _ in 0..10 {
            sender.submit_frame(&frame, 64, 64).unwrap();
        }

        std::thread::sleep(CONNECTION_CHECK_INTERVAL * 3);

        assert!(
            !sender.has_connections(),
            "no receivers on 'rustjay-ndi-gating-test' — gating flag should have flipped off"
        );
        // Post-gate submit must still succeed (early return) and never block.
        sender.submit_frame(&frame, 64, 64).unwrap();
    }
}
