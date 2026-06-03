//! # Output Module
//!
//! Video output to other applications via:
//! - NDI (cross-platform network)
//! - Syphon (macOS GPU texture sharing)
//! - Spout (Windows GPU texture sharing)
//! - v4l2loopback (Linux virtual camera) - TODO
//!
//! GPU readback uses a double-buffered staging pool so the render thread
//! never blocks waiting for a GPU→CPU copy to complete.  Each frame the
//! render thread submits a copy into the *current* staging slot and harvests
//! the *previous* slot's data (which has had a full frame to finish mapping).

use std::sync::Arc;

/// Commands for output stream control
// Superseded by `rustjay_core::OutputCommand`; kept as the io-layer's own descriptor.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputCommand {
    None,
    #[cfg(feature = "ndi")]
    StartNdi,
    #[cfg(feature = "ndi")]
    StopNdi,
    #[cfg(target_os = "macos")]
    StartSyphon,
    #[cfg(target_os = "macos")]
    StopSyphon,
    #[cfg(target_os = "windows")]
    StartSpout { sender_name: String },
    #[cfg(target_os = "windows")]
    StopSpout,
    #[cfg(target_os = "linux")]
    StartV4l2 { device_path: String },
    #[cfg(target_os = "linux")]
    StopV4l2,
    ResizeOutput,
}

#[cfg(feature = "ndi")]
pub mod ndi_output;
#[cfg(target_os = "macos")]
pub mod syphon_output;
#[cfg(target_os = "windows")]
pub mod spout_output;
#[cfg(target_os = "linux")]
pub mod v4l2_output;

#[cfg(feature = "ndi")]
use ndi_output::NdiOutputSender;

// ---------------------------------------------------------------------------
// Async GPU readback pool
// ---------------------------------------------------------------------------

/// Number of staging buffers in the pool.  Two is enough: one being filled
/// by the GPU while the CPU reads the other.
const READBACK_SLOTS: usize = 2;

/// State of a single staging buffer slot.
enum SlotState {
    /// Buffer is idle and available for a new copy.
    /// May hold a cached buffer (unmapped, ready to reuse) from the previous frame.
    Available { cached: Option<(wgpu::Buffer, u64)> },
    /// A copy has been submitted and `map_async` requested; waiting for GPU.
    Pending {
        buffer: wgpu::Buffer,
        width: u32,
        height: u32,
        ready: std::sync::mpsc::Receiver<bool>,
    },
}

/// Double-buffered staging pool for non-blocking GPU→CPU readback.
struct ReadbackPool {
    slots: Vec<SlotState>,
    /// Index of the slot to write into this frame.
    current: usize,
}

impl ReadbackPool {
    fn new() -> Self {
        let mut slots = Vec::with_capacity(READBACK_SLOTS);
        for _ in 0..READBACK_SLOTS {
            slots.push(SlotState::Available { cached: None });
        }
        Self { slots, current: 0 }
    }

    /// Harvest the *previous* slot if its map has completed, returning the
    /// BGRA pixel data.  This never blocks — if the GPU hasn't finished yet
    /// we simply skip this frame's readback.
    fn harvest_previous(&mut self) -> Option<(Vec<u8>, u32, u32)> {
        let prev = (self.current + READBACK_SLOTS - 1) % READBACK_SLOTS;
        let slot = &mut self.slots[prev];

        match slot {
            SlotState::Pending { buffer, width, height, ready } => {
                // Non-blocking check — is the map complete?
                match ready.try_recv() {
                    Ok(true) => {
                        let w = *width;
                        let h = *height;
                        let data = buffer.slice(..).get_mapped_range().to_vec();
                        buffer.unmap();
                        // Return the unmapped buffer to the slot cache so submit_copy
                        // can reuse it next frame without a GPU allocator call.
                        let buf_size = (w as u64) * 4 * (h as u64);
                        let buf = match std::mem::replace(slot, SlotState::Available { cached: None }) {
                            SlotState::Pending { buffer, .. } => buffer,
                            _ => {
                                log::error!("[ReadbackPool] Expected Pending slot but found another state");
                                return None;
                            }
                        };
                        *slot = SlotState::Available { cached: Some((buf, buf_size)) };
                        Some((data, w, h))
                    }
                    _ => None,
                }
            }
            SlotState::Available { .. } => None,
        }
    }

    /// Submit a non-blocking copy from `texture` into the current staging
    /// slot and request an async map.
    fn submit_copy(
        &mut self,
        texture: &wgpu::Texture,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) {
        let width = texture.width();
        let height = texture.height();
        let bytes_per_row = width * 4;
        let buffer_size = (bytes_per_row * height) as u64;

        // Extract any cached buffer from this slot (discarding Pending if GPU is too slow).
        let cached = match std::mem::replace(&mut self.slots[self.current], SlotState::Available { cached: None }) {
            SlotState::Available { cached } => cached,
            SlotState::Pending { .. } => {
                log::debug!("Readback slot {} overwritten (GPU too slow)", self.current);
                None
            }
        };

        // Reuse cached buffer if it matches the required size, else allocate.
        let staging_buffer = match cached {
            Some((buf, sz)) if sz == buffer_size => buf,
            _ => device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Readback Staging"),
                size: buffer_size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Readback Copy"),
        });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        queue.submit(std::iter::once(encoder.finish()));

        // Request async map — the callback signals via channel.
        let (tx, rx) = std::sync::mpsc::channel::<bool>();
        staging_buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = tx.send(result.is_ok());
            });

        self.slots[self.current] = SlotState::Pending {
            buffer: staging_buffer,
            width,
            height,
            ready: rx,
        };

        self.current = (self.current + 1) % READBACK_SLOTS;
    }

    /// Drain any pending slots (used during shutdown / output stop).
    fn drain(&mut self, device: &wgpu::Device) {
        for slot in &mut self.slots {
            if matches!(slot, SlotState::Pending { .. }) {
                // Poll once to let the GPU finish, then discard.
                device.poll(wgpu::PollType::wait_indefinitely()).ok();
                if let SlotState::Pending { buffer, .. } =
                    std::mem::replace(slot, SlotState::Available { cached: None })
                {
                    drop(buffer);
                }
            }
            // Available slots may hold a cached buffer; dropped when ReadbackPool drops.
        }
    }
}

// ---------------------------------------------------------------------------
// OutputManager
// ---------------------------------------------------------------------------

/// Manages all video outputs
pub struct OutputManager {
    /// NDI network output
    #[cfg(feature = "ndi")]
    ndi_output: Option<NdiOutputSender>,

    /// Syphon output (macOS)
    #[cfg(target_os = "macos")]
    syphon_output: Option<syphon_output::SyphonOutput>,

    /// Spout output (Windows).
    #[cfg(target_os = "windows")]
    spout_output: Option<spout_output::SpoutOutput>,

    /// V4L2 loopback output (Linux)
    #[cfg(target_os = "linux")]
    v4l2_output: Option<v4l2_output::V4l2LoopbackOutput>,

    /// Async readback pool for CPU-path outputs (NDI, V4L2).
    readback_pool: ReadbackPool,

    frame_count: u64,
}

impl OutputManager {
    /// Create a new output manager
    pub fn new() -> Self {
        Self {
            #[cfg(feature = "ndi")]
            ndi_output: None,
            #[cfg(target_os = "macos")]
            syphon_output: None,
            #[cfg(target_os = "windows")]
            spout_output: None,
            #[cfg(target_os = "linux")]
            v4l2_output: None,
            readback_pool: ReadbackPool::new(),
            frame_count: 0,
        }
    }

    /// Start NDI output
    #[cfg(feature = "ndi")]
    pub fn start_ndi(
        &mut self,
        name: &str,
        width: u32,
        height: u32,
        include_alpha: bool,
    ) -> anyhow::Result<()> {
        let sender = NdiOutputSender::new(name, width, height, include_alpha)?;
        self.ndi_output = Some(sender);
        log::info!("NDI output started: {} ({}x{})", name, width, height);
        Ok(())
    }

    /// Start NDI output (errors; NDI feature disabled).
    #[cfg(not(feature = "ndi"))]
    pub fn start_ndi(
        &mut self,
        _name: &str,
        _width: u32,
        _height: u32,
        _include_alpha: bool,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("NDI support not compiled. Enable the 'ndi' feature."))
    }

    /// Stop NDI output
    #[cfg(feature = "ndi")]
    pub fn stop_ndi(&mut self) {
        if self.ndi_output.take().is_some() {
            log::info!("NDI output stopped");
        }
    }

    /// Stop NDI output (no-op; NDI feature disabled).
    #[cfg(not(feature = "ndi"))]
    pub fn stop_ndi(&mut self) {}

    /// Start Syphon output (macOS only)
    #[cfg(target_os = "macos")]
    pub fn start_syphon(
        &mut self,
        server_name: &str,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> anyhow::Result<()> {
        let syphon = syphon_output::SyphonOutput::new(server_name, device, queue)?;
        self.syphon_output = Some(syphon);
        log::info!("Syphon output started: {}", server_name);
        Ok(())
    }

    /// Stop Syphon output (macOS only)
    #[cfg(target_os = "macos")]
    pub fn stop_syphon(&mut self) {
        if self.syphon_output.take().is_some() {
            log::info!("Syphon output stopped");
        }
    }

    /// Start Spout output (Windows only)
    #[cfg(target_os = "windows")]
    pub fn start_spout(
        &mut self,
        sender_name: &str,
    ) -> anyhow::Result<()> {
        let spout = spout_output::SpoutOutput::new(sender_name)?;
        self.spout_output = Some(spout);
        log::info!("Spout output started: {}", sender_name);
        Ok(())
    }

    /// Stop Spout output (Windows only)
    #[cfg(target_os = "windows")]
    pub fn stop_spout(&mut self) {
        if self.spout_output.take().is_some() {
            log::info!("Spout output stopped");
        }
    }

    /// Check if Spout is active (Windows only)
    #[cfg(target_os = "windows")]
    pub fn is_spout_active(&self) -> bool {
        self.spout_output.is_some()
    }

    /// Whether Spout output is active (always `false` on non-Windows).
    #[cfg(not(target_os = "windows"))]
    pub fn is_spout_active(&self) -> bool {
        false
    }

    /// Start V4L2 loopback output (Linux only).
    #[cfg(target_os = "linux")]
    pub fn start_v4l2(&mut self, device_path: &str, width: u32, height: u32) -> anyhow::Result<()> {
        let output = v4l2_output::V4l2LoopbackOutput::new(device_path, width, height)?;
        self.v4l2_output = Some(output);
        log::info!("V4L2 output started on {}", device_path);
        Ok(())
    }

    /// Stop V4L2 loopback output (Linux only)
    #[cfg(target_os = "linux")]
    pub fn stop_v4l2(&mut self) {
        if self.v4l2_output.take().is_some() {
            log::info!("V4L2 output stopped");
        }
    }

    /// Check if V4L2 is active (Linux only)
    #[cfg(target_os = "linux")]
    pub fn is_v4l2_active(&self) -> bool {
        self.v4l2_output.is_some()
    }

    /// Whether V4L2 output is active (always `false` on non-Linux).
    #[cfg(not(target_os = "linux"))]
    pub fn is_v4l2_active(&self) -> bool {
        false
    }

    /// Returns true if any CPU-path output (NDI, V4L2, Spout) needs readback.
    fn needs_readback(&self) -> bool {
        #[cfg(feature = "ndi")]
        if self.ndi_output.is_some() {
            return true;
        }
        #[cfg(target_os = "windows")]
        if self.spout_output.is_some() {
            return true;
        }
        #[cfg(target_os = "linux")]
        if self.v4l2_output.is_some() {
            return true;
        }
        false
    }

    /// Submit frame to all active outputs.
    ///
    /// GPU-path outputs (Syphon, Spout) receive the texture directly.
    /// CPU-path outputs (NDI, V4L2) use the async readback pool — the
    /// render thread never blocks waiting for a GPU→CPU copy.
    pub fn submit_frame(&mut self, texture: &wgpu::Texture, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.frame_count += 1;

        // CPU-path outputs: harvest previous frame's readback, then submit
        // a new copy for this frame.
        if self.needs_readback() {
            // Non-blocking poll to nudge the GPU — helps the previous
            // frame's map_async complete before we try to harvest.
            device.poll(wgpu::PollType::Poll).ok();

            // Harvest the previous frame's data (never blocks).
            if let Some((_data, _width, _height)) = self.readback_pool.harvest_previous() {
                #[cfg(feature = "ndi")]
                if let Some(ref sender) = self.ndi_output {
                    sender.submit_frame(&_data, _width, _height);
                }

                #[cfg(target_os = "windows")]
                if let Some(ref mut spout) = self.spout_output {
                    if let Err(e) = spout.submit_bytes(&data, width, height) {
                        log::error!("Spout output error: {}", e);
                    }
                }

                #[cfg(target_os = "linux")]
                if let Some(ref mut v4l2) = self.v4l2_output {
                    if let Err(e) = v4l2.send_frame(&data, width, height) {
                        log::error!("V4L2 output error: {}", e);
                    }
                }
            }

            // Submit a non-blocking copy for *this* frame.
            self.readback_pool.submit_copy(texture, device, queue);
        }

        // Syphon output (zero-copy on macOS)
        #[cfg(target_os = "macos")]
        if let Some(ref mut syphon) = self.syphon_output {
            if let Err(e) = syphon.submit_frame(texture, device, queue) {
                log::error!("Syphon output error: {}", e);
            }
        }

        // Note: Spout is now fed from the readback pool above (CPU path),
        // not directly from the GPU texture.
    }

    /// Check if NDI is active
    #[cfg(feature = "ndi")]
    pub fn is_ndi_active(&self) -> bool {
        self.ndi_output.is_some()
    }

    /// Whether NDI output is active (always false; NDI feature disabled).
    #[cfg(not(feature = "ndi"))]
    pub fn is_ndi_active(&self) -> bool {
        false
    }

    /// Check if Syphon is active (macOS only)
    #[cfg(target_os = "macos")]
    pub fn is_syphon_active(&self) -> bool {
        self.syphon_output.is_some()
    }

    #[cfg(not(target_os = "macos"))]
    pub fn is_syphon_active(&self) -> bool {
        false
    }

    /// Shutdown all outputs
    pub fn shutdown(&mut self) {
        self.stop_ndi();
        #[cfg(target_os = "macos")]
        self.stop_syphon();
        #[cfg(target_os = "windows")]
        self.stop_spout();
        #[cfg(target_os = "linux")]
        self.stop_v4l2();
    }

    /// Drain readback pool (call when GPU device is still alive).
    pub fn drain_readback(&mut self, device: &wgpu::Device) {
        self.readback_pool.drain(device);
    }
}

impl Default for OutputManager {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for OutputManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}
