//! # NDI Output Sender
//!
//! Sends video frames as an NDI stream.

// Accessors and availability checks here are part of the NDI backend surface but not all
// are consumed yet; keep them available without warning.
#![allow(dead_code)]

use grafton_ndi::{NDI, Sender, SenderOptions, VideoFrameBuilder, PixelFormat};
use crossbeam::channel::{self, Sender as ChannelSender, Receiver};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::Instant;

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
    running: Arc<AtomicBool>,
    is_owner: bool,
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl NdiOutputSender {
    /// Create and start a new NDI output sender
    pub fn new(name: impl Into<String>, width: u32, height: u32, include_alpha: bool) -> anyhow::Result<Self> {
        let name = name.into();
        
        if width == 0 || height == 0 {
            return Err(anyhow::anyhow!("Invalid dimensions: {}x{}", width, height));
        }
        
        let ndi = NDI::new()
            .map_err(|e| anyhow::anyhow!("Failed to initialize NDI: {:?}", e))?;
        
        let (frame_tx, frame_rx) = channel::bounded(2);
        
        let running = Arc::new(AtomicBool::new(true));
        let running_clone = Arc::clone(&running);
        
        let name_clone = name.clone();
        
        // Spawn send thread
        let thread_handle = thread::spawn(move || {
            Self::send_thread(
                ndi,
                name_clone,
                width,
                height,
                include_alpha,
                frame_rx,
                running_clone,
            );
        });
        
        Ok(Self {
            name,
            width,
            height,
            include_alpha,
            frame_tx,
            running,
            is_owner: true,
            thread_handle: Some(thread_handle),
        })
    }
    
    /// Send thread that owns the NDI sender and processes frames
    fn send_thread(
        ndi: NDI,
        name: String,
        _width: u32,
        _height: u32,
        include_alpha: bool,
        frame_rx: Receiver<FrameData>,
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
                return;
            }
        };
        
        let pixel_format = if include_alpha {
            PixelFormat::BGRA
        } else {
            PixelFormat::BGRX
        };
        
        let mut frame_count = 0u64;
        let mut last_log = Instant::now();
        
        while running.load(Ordering::SeqCst) {
            match frame_rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(frame_data) => {
                    frame_count += 1;
                    
                    let buffer_size = pixel_format.buffer_size(frame_data.width as i32, frame_data.height as i32);
                    
                    if frame_data.data.len() < buffer_size {
                        log::warn!("[NDI OUTPUT] Frame {} data too small", frame_count);
                        continue;
                    }
                    
                    let mut frame = match VideoFrameBuilder::new()
                        .resolution(frame_data.width as i32, frame_data.height as i32)
                        .pixel_format(pixel_format)
                        .frame_rate(60, 1)
                        .aspect_ratio(frame_data.width as f32 / frame_data.height as f32)
                        .build() {
                        Ok(f) => f,
                        Err(e) => {
                            log::error!("[NDI OUTPUT] Failed to build video frame: {:?}", e);
                            continue;
                        }
                    };
                    
                    let copy_len = buffer_size.min(frame.data.len());
                    frame.data[..copy_len].copy_from_slice(&frame_data.data[..copy_len]);
                    sender.send_video(&frame);
                    
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
    pub fn submit_frame(&self, bgra_data: &[u8], width: u32, height: u32) {
        if width != self.width || height != self.height {
            log::warn!("[NDI OUTPUT] Frame size mismatch");
            return;
        }
        
        if bgra_data.is_empty() {
            log::warn!("[NDI OUTPUT] Empty frame data received");
            return;
        }
        
        let frame = FrameData {
            width,
            height,
            data: bgra_data.to_vec(),
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
}

impl Clone for NdiOutputSender {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            width: self.width,
            height: self.height,
            include_alpha: self.include_alpha,
            frame_tx: self.frame_tx.clone(),
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
