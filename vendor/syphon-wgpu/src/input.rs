//! Syphon wgpu input receiver
//!
//! ## Zero-copy path (default on Metal)
//!
//! When the wgpu device is backed by Metal, frames are transferred via a GPU
//! blit from the IOSurface-backed Metal texture directly into the output wgpu
//! texture — no CPU involvement at all.
//!
//! The output texture is kept alive across frames and initialized through
//! wgpu once on creation so that wgpu's texture-initialization tracking does
//! not zero it out before the first shader use.
//!
//! ## CPU fallback
//!
//! If the Metal HAL is unavailable (e.g. wgpu Vulkan/DX12), the frame is
//! locked on the CPU and uploaded via `queue.write_texture`.

use syphon_core::{SyphonClient, Result, ServerInfo};
#[cfg(target_os = "macos")]
use crate::metal_interop;

pub struct SyphonWgpuInput {
    client: Option<SyphonClient>,
    connected_server: Option<String>,
    /// Persistent output texture — initialized via wgpu on creation so
    /// wgpu's init-tracking never zeroes it after an external Metal blit.
    output_texture: Option<wgpu::Texture>,
    output_width: u32,
    output_height: u32,
    /// Metal context created from wgpu's underlying Metal device.
    /// Present only when wgpu is backed by Metal.
    #[cfg(target_os = "macos")]
    metal_ctx: Option<syphon_metal::MetalContext>,
}

impl SyphonWgpuInput {
    /// Create a new input receiver.
    ///
    /// Extracts the underlying Metal device from `device` (if Metal-backed) so
    /// the zero-copy blit path is available immediately.
    pub fn new(device: &wgpu::Device, _queue: &wgpu::Queue) -> Self {
        #[cfg(target_os = "macos")]
        let metal_ctx = Self::build_metal_ctx(device);

        Self {
            client: None,
            connected_server: None,
            output_texture: None,
            output_width: 0,
            output_height: 0,
            #[cfg(target_os = "macos")]
            metal_ctx,
        }
    }

    #[cfg(target_os = "macos")]
    fn build_metal_ctx(device: &wgpu::Device) -> Option<syphon_metal::MetalContext> {
        let ctx = metal_interop::extract_metal_device(device)
            .map(|raw| unsafe { syphon_metal::MetalContext::from_raw_device(raw) });
        if ctx.is_none() {
            log::warn!("[SyphonWgpuInput] wgpu device is not Metal-backed; will use CPU fallback");
        }
        ctx
    }

    /// Connect to a Syphon server by display name.
    ///
    /// Returns [`SyphonError::AmbiguousServerName`] when multiple servers share
    /// the same name. In that case use [`connect_by_info`](Self::connect_by_info).
    pub fn connect(&mut self, server_name: &str) -> Result<()> {
        log::info!("[SyphonWgpuInput] Connecting to '{}'", server_name);
        let client = SyphonClient::connect(server_name)?;
        self.client = Some(client);
        self.connected_server = Some(server_name.to_string());
        log::info!("[SyphonWgpuInput] Connected");
        Ok(())
    }

    /// Connect using a [`ServerInfo`] obtained from `SyphonServerDirectory`.
    /// Matches by UUID — unambiguous even when names collide.
    pub fn connect_by_info(&mut self, info: &ServerInfo) -> Result<()> {
        log::info!("[SyphonWgpuInput] Connecting to '{}' (uuid={})", info.display_name(), info.uuid);
        let client = SyphonClient::connect_by_info(info)?;
        self.connected_server = Some(info.display_name().to_string());
        self.client = Some(client);
        log::info!("[SyphonWgpuInput] Connected");
        Ok(())
    }

    /// Connect with push-based delivery via a channel.
    ///
    /// Returns `((), receiver)`. The receiver yields `()` each time the server
    /// publishes a new frame — no polling needed. Call [`receive_texture`](Self::receive_texture)
    /// after waking on the channel.
    pub fn connect_with_channel(
        &mut self,
        server_name: &str,
    ) -> Result<std::sync::mpsc::Receiver<()>> {
        log::info!("[SyphonWgpuInput] Connecting to '{}' (push mode)", server_name);
        let (client, rx) = SyphonClient::connect_with_channel(server_name)?;
        self.connected_server = Some(server_name.to_string());
        self.client = Some(client);
        log::info!("[SyphonWgpuInput] Connected (push mode)");
        Ok(rx)
    }

    /// Connect by [`ServerInfo`] with push-based delivery.
    ///
    /// UUID-based — unambiguous even when names collide.
    pub fn connect_by_info_with_channel(
        &mut self,
        info: &ServerInfo,
    ) -> Result<std::sync::mpsc::Receiver<()>> {
        log::info!("[SyphonWgpuInput] Connecting to '{}' (uuid={}, push mode)", info.display_name(), info.uuid);
        let (client, rx) = SyphonClient::connect_by_info_with_channel(info)?;
        self.connected_server = Some(info.display_name().to_string());
        self.client = Some(client);
        log::info!("[SyphonWgpuInput] Connected (push mode)");
        Ok(rx)
    }

    pub fn disconnect(&mut self) {
        self.client = None;
        self.connected_server = None;
        self.output_texture = None;
        log::info!("[SyphonWgpuInput] Disconnected");
    }

    pub fn is_connected(&self) -> bool {
        self.client.as_ref().map_or(false, |c| {
            #[cfg(target_os = "macos")]
            { c.is_connected() }
            #[cfg(not(target_os = "macos"))]
            { true }
        })
    }

    /// Try to receive a frame into the persistent output texture.
    ///
    /// Returns `true` when a new frame was written; `false` when no new frame
    /// is available.  Access the result with [`output_texture`](Self::output_texture).
    ///
    /// On Metal, performs a GPU-to-GPU blit with zero CPU copies.
    /// On other backends, falls back to CPU upload.
    pub fn receive_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        let client = match self.client.as_ref() {
            Some(c) => c,
            None => return false,
        };

        #[cfg(target_os = "macos")]
        {
            if !client.has_new_frame() { return false; }

            let mut frame = match client.try_receive() {
                Ok(Some(f)) => f,
                _ => return false,
            };

            let w = frame.width;
            let h = frame.height;

            // Create or resize the persistent output texture.
            // Zero-initialise via wgpu so its init-tracking marks it as "written"
            // — otherwise wgpu clears it before the first shader use, overwriting
            // any data the external Metal blit wrote.
            if self.output_texture.is_none() || self.output_width != w || self.output_height != h {
                log::info!("[SyphonWgpuInput] Creating output texture: {}x{}", w, h);
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("syphon_input"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                });
                // Write zeros through wgpu to mark the texture as initialized.
                let zeros = vec![0u8; (w * h * 4) as usize];
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &zeros,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w * 4),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
                self.output_texture = Some(tex);
                self.output_width = w;
                self.output_height = h;
            }

            let output = self.output_texture.as_ref().unwrap();

            // Attempt zero-copy GPU blit; fall back to CPU on failure.
            // Poll wgpu before the Metal blit to ensure prior render work is done
            // (wgpu 29 no longer exposes its MTLCommandQueue for cross-queue ordering).
            let used_gpu = if let Some(ref ctx) = self.metal_ctx {
                let _ = device.poll(wgpu::PollType::wait_indefinitely());
                Self::gpu_blit(&frame, output, ctx.queue())
            } else {
                false
            };

            if !used_gpu {
                log::warn!("[SyphonWgpuInput] GPU blit unavailable, using CPU fallback");
                let stride = frame.bytes_per_row() as u32;
                let data = match frame.to_vec() {
                    Ok(d) => d,
                    Err(e) => { log::warn!("[SyphonWgpuInput] CPU read failed: {}", e); return false; }
                };
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: output,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &data,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(stride),
                        rows_per_image: Some(h),
                    },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
            }

            return true;
        }

        #[cfg(not(target_os = "macos"))]
        { false }
    }

    /// The persistent output texture, valid after [`receive_texture`](Self::receive_texture)
    /// returns `true`.
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        self.output_texture.as_ref()
    }

    /// GPU-to-GPU blit: Syphon frame texture → output wgpu texture, zero CPU copies.
    ///
    /// Uses `frame.metal_texture_ptr()` — the `id<MTLTexture>` returned by
    /// `SyphonMetalClient::newFrameImage`.  This texture was created on the
    /// *same* Metal device as the Syphon client, so no cross-device IOSurface
    /// re-wrapping is needed.
    ///
    /// Submitted on wgpu's own Metal command queue so Metal's queue-ordering
    /// guarantee ensures the blit completes before any subsequent wgpu commands
    /// that read `output`.
    /// GPU-to-GPU blit using a dedicated Metal command queue.
    ///
    /// In wgpu 29, `Queue::as_hal` no longer exposes the internal `MTLCommandQueue`,
    /// so we use the queue from `MetalContext` instead. The caller must call
    /// `device.poll(PollType::wait_indefinitely())` before invoking this to ensure
    /// all prior wgpu rendering is complete on the GPU.
    #[cfg(target_os = "macos")]
    fn gpu_blit(
        frame: &syphon_core::Frame,
        output: &wgpu::Texture,
        metal_queue: &metal::CommandQueue,
    ) -> bool {
        use metal::foreign_types::ForeignType;
        use std::mem::ManuallyDrop;

        let frame_tex_ptr = frame.metal_texture_ptr();
        if frame_tex_ptr.is_null() {
            log::warn!("[SyphonWgpuInput] newFrameImage returned nil, cannot GPU blit");
            return false;
        }

        // Wrap the pointer as metal::Texture so we can pass &*src to the blit
        // encoder.  ManuallyDrop prevents the implicit ObjC release on drop —
        // Frame::drop already owns the matching retain.
        let src = ManuallyDrop::new(unsafe {
            metal::Texture::from_ptr(frame_tex_ptr as *mut _)
        });

        let mut ok = false;

        unsafe {
            objc::rc::autoreleasepool(|| {
                metal_interop::with_metal_texture(output, |dst| {
                    let cmd = metal_queue.new_command_buffer();
                    let enc = cmd.new_blit_command_encoder();
                    enc.copy_from_texture(
                        &src,
                        0, 0,
                        metal::MTLOrigin { x: 0, y: 0, z: 0 },
                        metal::MTLSize {
                            width:  frame.width  as u64,
                            height: frame.height as u64,
                            depth:  1,
                        },
                        dst,
                        0, 0,
                        metal::MTLOrigin { x: 0, y: 0, z: 0 },
                    );
                    enc.end_encoding();
                    cmd.commit();
                    ok = true;
                });
            });
        }

        ok
    }

    pub fn server_name(&self) -> Option<&str> {
        self.connected_server.as_deref()
    }
}

impl Drop for SyphonWgpuInput {
    fn drop(&mut self) {
        self.disconnect();
    }
}
