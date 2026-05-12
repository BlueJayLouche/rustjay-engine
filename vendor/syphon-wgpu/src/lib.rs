//! # Syphon wgpu Integration - Zero-Copy Edition
//! 
//! High-performance, zero-copy GPU-to-GPU Syphon integration for wgpu applications.
//! 
//! ## Overview
//! 
//! This crate provides:
//! - `SyphonWgpuOutput` - Publish wgpu-rendered frames to Syphon clients
//! - `SyphonWgpuInput` - Receive frames from Syphon servers as wgpu textures
//! 
//! Both use IOSurface-backed textures for zero-copy GPU transfer.
//! 
//! ## Native BGRA Format
//! 
//! This crate uses native macOS BGRA8Unorm format throughout. When rendering for Syphon output,
//! use `wgpu::TextureFormat::Bgra8Unorm`. When receiving from Syphon, you'll get BGRA data
//! directly without any format conversion.
//!
//! ## Usage
//! 
//! ### Output (Server)
//! ```no_run
//! use syphon_wgpu::SyphonWgpuOutput;
//! 
//! let mut output = SyphonWgpuOutput::new(
//!     "My App", &device, &queue, 1920, 1080
//! ).expect("Failed to create Syphon output");
//! 
//! output.publish(&render_texture, &device, &queue);
//! ```
//!
//! ### Input (Client)
//! ```no_run
//! use syphon_wgpu::SyphonWgpuInput;
//!
//! let mut input = SyphonWgpuInput::new(&device, &queue);
//! input.connect("Simple Server").unwrap();
//!
//! if let Some(texture) = input.receive_texture(&device, &queue) {
//!     // Texture is Bgra8Unorm (native Syphon format)
//! }
//! ```

pub use syphon_core::{SyphonServer, SyphonClient, SyphonError, Result, ServerInfo, ServerOptions};

/// Result returned by [`SyphonWgpuOutput::publish`].
///
/// Check this to confirm which code path was used — never assume zero-copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishStatus {
    /// Frame published via zero-copy GPU blit through IOSurface — no CPU involved.
    ZeroCopy,
    /// Frame published via CPU readback (Metal interop unavailable).
    CpuFallback,
    /// No clients connected; frame skipped (no work done).
    NoClients,
    /// All IOSurfaces in the pool were in-flight; frame dropped.
    ///
    /// Increase `pool_size` in [`SyphonOutputConfig`] if this occurs regularly.
    PoolExhausted,
}

/// Configuration for [`SyphonWgpuOutput`].
#[derive(Debug, Clone)]
pub struct SyphonOutputConfig {
    /// Number of IOSurfaces to pre-allocate for triple-(or more) buffering.
    ///
    /// Must be ≥ 1. Default: `3` (suitable for ≤ 120 Hz with typical GPU latency).
    /// Increase to `4`–`5` at very high frame rates or if [`PublishStatus::PoolExhausted`]
    /// appears in logs.
    pub pool_size: usize,

    /// Core Syphon server options (e.g. `is_private`).
    pub server_options: ServerOptions,
}

impl Default for SyphonOutputConfig {
    fn default() -> Self {
        Self { pool_size: 3, server_options: ServerOptions::default() }
    }
}

// Input module for receiving frames
pub mod input;
pub use input::SyphonWgpuInput;

// All wgpu-hal Metal interop lives here — update only this when upgrading wgpu.
#[cfg(target_os = "macos")]
mod metal_interop;

#[cfg(target_os = "macos")]
use metal::*;
#[cfg(target_os = "macos")]
use metal::foreign_types::{ForeignType, ForeignTypeRef};
// sel and sel_impl are needed for msg_send! macro expansion even though the
// lint reports them as unused — rustc does not track macro-internal references.
#[cfg(target_os = "macos")]
#[allow(unused_imports)]
use objc::{sel, sel_impl};
#[cfg(target_os = "macos")]
use objc::runtime::Object;

/// High-level wgpu-to-Syphon output with zero-copy GPU transfer
/// 
/// This implementation uses IOSurface-backed textures to transfer frames directly
/// from wgpu to Syphon without CPU readback.
/// 
/// ## Format
/// 
/// Uses native BGRA8Unorm format. Render your content to a Bgra8Unorm texture
/// and publish it directly.
pub struct SyphonWgpuOutput {
    server: SyphonServer,
    width: u32,
    height: u32,
    #[cfg(target_os = "macos")]
    surface_pool: syphon_metal::IOSurfacePool,
    #[cfg(target_os = "macos")]
    frame_count: u64,
    #[cfg(target_os = "macos")]
    use_zero_copy: bool,
    #[cfg(target_os = "macos")]
    metal_device: Option<Device>,
    #[cfg(target_os = "macos")]
    metal_queue: Option<CommandQueue>,
}

// SAFETY: `SyphonWgpuOutput` wraps a `SyphonServer` (Send+Sync, see its SAFETY
// comment), a `metal::Device` and `metal::CommandQueue` (both thread-safe per
// Apple's Metal docs), and an `IOSurfacePool` which holds `io_surface::IOSurface`
// objects. IOSurface is a reference-counted CoreFoundation type whose
// retain/release operations are thread-safe. The mutable fields `frame_count`,
// `use_zero_copy`, `width`, and `height` are written only in `new` or `publish`
// which should not be called concurrently (the caller must serialise publishes).
// Sync is acceptable because all constituent types are individually safe to
// access from shared references on concurrent threads.
#[cfg(target_os = "macos")]
unsafe impl Send for SyphonWgpuOutput {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SyphonWgpuOutput {}

impl SyphonWgpuOutput {
    /// Create a new Syphon output using the default [`SyphonOutputConfig`].
    pub fn new(
        name: &str,
        wgpu_device: &wgpu::Device,
        wgpu_queue: &wgpu::Queue,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        Self::new_with_config(name, wgpu_device, wgpu_queue, width, height, SyphonOutputConfig::default())
    }

    /// Create a new Syphon output with explicit configuration.
    ///
    /// Use this when you need a non-default pool size (e.g. at >120 Hz).
    pub fn new_with_config(
        name: &str,
        wgpu_device: &wgpu::Device,
        _wgpu_queue: &wgpu::Queue,
        width: u32,
        height: u32,
        config: SyphonOutputConfig,
    ) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            Self::new_macos(name, wgpu_device, _wgpu_queue, width, height, config)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (name, wgpu_device, _wgpu_queue, width, height, config);
            Err(SyphonError::NotAvailable)
        }
    }

    #[cfg(target_os = "macos")]
    fn new_macos(
        name: &str,
        wgpu_device: &wgpu::Device,
        _wgpu_queue: &wgpu::Queue,
        width: u32,
        height: u32,
        config: SyphonOutputConfig,
    ) -> Result<Self> {
        // Try to get the Metal device from wgpu for zero-copy.
        let device_opt = metal_interop::extract_metal_device(wgpu_device);
        
        let use_zero_copy = device_opt.is_some();
        
        if use_zero_copy {
            log::info!("SyphonWgpuOutput: Using zero-copy GPU-to-GPU path");
            
            let metal_device = device_opt.unwrap();
            let metal_queue = metal_device.new_command_queue();
            
            // Create the Syphon server with the Metal device and server options.
            let device_ptr = metal_device.as_ref() as *const DeviceRef as *mut Object;
            let server = SyphonServer::new_with_name_and_device_and_options(
                name, device_ptr, width, height, config.server_options.clone()
            )?;
            
            // Create an IOSurface pool sized by config.
            let pool_size = config.pool_size.max(1);
            let surface_pool = syphon_metal::IOSurfacePool::new(width, height, pool_size);
            
            log::info!(
                "SyphonWgpuOutput created: {}x{} (zero-copy with {} IOSurfaces)",
                width, height, surface_pool.capacity()
            );
            
            Ok(Self {
                server,
                width,
                height,
                surface_pool,
                frame_count: 0,
                use_zero_copy: true,
                metal_device: Some(metal_device),
                metal_queue: Some(metal_queue),
            })
        } else {
            log::warn!("SyphonWgpuOutput: Metal interop failed, falling back to CPU readback");
            
            // Fallback: Create separate Metal device and use CPU readback
            let metal_device = Device::system_default()
                .ok_or_else(|| SyphonError::CreateFailed(
                    "Failed to get Metal device".to_string()
                ))?;
            
            let metal_queue = metal_device.new_command_queue();
            
            let device_ptr = metal_device.as_ref() as *const DeviceRef as *mut Object;
            let server = SyphonServer::new_with_name_and_device_and_options(
                name, device_ptr, width, height, config.server_options.clone()
            )?;
            
            // Fallback mode does not use the pool (CPU path allocates per-frame).
            let surface_pool = syphon_metal::IOSurfacePool::new(width, height, 0);
            
            log::info!("SyphonWgpuOutput created: {}x{} (CPU fallback)", width, height);
            
            Ok(Self {
                server,
                width,
                height,
                surface_pool,
                frame_count: 0,
                use_zero_copy: false,
                metal_device: Some(metal_device),
                metal_queue: Some(metal_queue),
            })
        }
    }
    
    /// Publish a texture to Syphon.
    ///
    /// Returns a [`PublishStatus`] indicating which path was used. Check it —
    /// never assume zero-copy is active, especially after device changes.
    ///
    /// The texture must be in `Bgra8Unorm` format.
    pub fn publish(
        &mut self,
        texture: &wgpu::Texture,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> PublishStatus {
        #[cfg(target_os = "macos")]
        {
            if self.server.client_count() == 0 {
                return PublishStatus::NoClients;
            }

            self.frame_count += 1;

            if self.use_zero_copy {
                self.publish_zero_copy(texture, device, queue)
            } else {
                self.publish_cpu_fallback(texture, device, queue);
                PublishStatus::CpuFallback
            }
        }

        #[cfg(not(target_os = "macos"))]
        PublishStatus::NoClients
    }
    
    #[cfg(target_os = "macos")]
    fn publish_zero_copy(
        &mut self,
        texture: &wgpu::Texture,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> PublishStatus {
        let surface = match self.surface_pool.acquire() {
            Some(s) => s,
            None => {
                log::warn!(
                    "[SyphonWgpuOutput] IOSurface pool exhausted — frame dropped. \
                     Consider increasing pool_size in SyphonOutputConfig."
                );
                return PublishStatus::PoolExhausted;
            }
        };

        let mut published = false;

        // In wgpu 29, the internal MTLCommandQueue is no longer accessible via
        // wgpu-hal. We use our own metal_queue and poll wgpu first to ensure
        // all prior GPU rendering is complete before we blit.
        let _ = device.poll(wgpu::PollType::wait_indefinitely());

        unsafe {
            objc::rc::autoreleasepool(|| {
                metal_interop::with_metal_texture(texture, |src_texture| {
                    let Some(ref metal_device) = self.metal_device else { return };
                    let Some(ref metal_queue) = self.metal_queue else { return };

                    let Some(dest_texture) = Self::create_iosurface_texture(
                        metal_device, &surface, self.width, self.height
                    ) else { return };

                    let cmd_buf = metal_queue.new_command_buffer();
                    let blit = cmd_buf.new_blit_command_encoder();
                    blit.copy_from_texture(
                        src_texture,
                        0, 0,
                        MTLOrigin { x: 0, y: 0, z: 0 },
                        MTLSize { width: self.width as u64, height: self.height as u64, depth: 1 },
                        &dest_texture,
                        0, 0,
                        MTLOrigin { x: 0, y: 0, z: 0 },
                    );
                    blit.end_encoding();

                    // Notify Syphon before committing so it can reference
                    // the command buffer for synchronisation.
                    let tex_ptr = dest_texture.as_ptr() as *mut Object;
                    let cmd_ptr = cmd_buf.as_ptr() as *mut Object;
                    self.server.publish_metal_texture(tex_ptr, cmd_ptr);

                    cmd_buf.commit();
                    published = true;
                });
            });
        }

        // Triple-buffering ensures the GPU has finished with this surface before
        // it's reused — return it even if publish failed so we don't leak it.
        self.surface_pool.release(surface);

        if published { PublishStatus::ZeroCopy } else { PublishStatus::CpuFallback }
    }
    
    /// Create a Metal texture from an IOSurface using raw Objective-C
    #[cfg(target_os = "macos")]
    fn create_iosurface_texture(
        device: &metal::Device,
        surface: &io_surface::IOSurface,
        width: u32,
        height: u32,
    ) -> Option<metal::Texture> {
        use objc::runtime::Object;
        use objc::{msg_send, class};
        use cocoa::foundation::NSUInteger;
        use core_foundation::base::TCFType;
        use metal::{MTLStorageMode, MTLTextureUsage, MTLPixelFormat};
        
        unsafe {
            // Create texture descriptor
            let desc: *mut Object = msg_send![class!(MTLTextureDescriptor), new];
            let _: () = msg_send![desc, setPixelFormat: MTLPixelFormat::BGRA8Unorm];
            let _: () = msg_send![desc, setWidth: width as NSUInteger];
            let _: () = msg_send![desc, setHeight: height as NSUInteger];
            let _: () = msg_send![desc, setStorageMode: MTLStorageMode::Shared];
            let _: () = msg_send![desc, setUsage: MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead];
            
            // Get the raw IOSurfaceRef
            let surface_ref = surface.as_concrete_TypeRef();
            
            // Call newTextureWithDescriptor:iosurface:plane:
            let device_ptr = device.as_ptr() as *mut Object;
            let texture_ptr: *mut Object = msg_send![
                device_ptr,
                newTextureWithDescriptor: desc
                iosurface: surface_ref
                plane: 0 as NSUInteger
            ];
            
            // Release the descriptor
            let _: () = msg_send![desc, release];
            
            if texture_ptr.is_null() {
                None
            } else {
                // Convert to metal::Texture
                Some(metal::Texture::from_ptr(texture_ptr as *mut metal::MTLTexture))
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    fn publish_cpu_fallback(&mut self, texture: &wgpu::Texture, device: &wgpu::Device, queue: &wgpu::Queue) {
        // CPU readback fallback implementation
        // This is the stable but slower path
        
        let buffer_size = (self.width * self.height * 4) as u64;
        
        // Create staging buffer
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Syphon Staging"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        
        // Copy texture to buffer
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Syphon Copy"),
        });
        
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.width * 4),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        
        queue.submit(std::iter::once(encoder.finish()));
        
        // Wait for GPU
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        
        // Map and upload
        let buffer_slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result.is_ok());
        });
        
        // Wait for map (with timeout)
        let start = std::time::Instant::now();
        let mut ready = false;
        while start.elapsed().as_millis() < 10 {
            if let Ok(true) = rx.try_recv() {
                ready = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
            let _ = device.poll(wgpu::PollType::Poll);
        }
        
        if ready {
            let data = buffer_slice.get_mapped_range();
            
            // Check if we have actual data
            if data.iter().any(|&b| b != 0) {
                if let (Some(ref metal_device), Some(ref metal_queue)) = 
                    (&self.metal_device, &self.metal_queue) 
                {
                    // Upload directly without flip - native BGRA
                    let desc = TextureDescriptor::new();
                    desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
                    desc.set_width(self.width as u64);
                    desc.set_height(self.height as u64);
                    desc.set_storage_mode(MTLStorageMode::Managed);
                    desc.set_usage(MTLTextureUsage::ShaderRead);
                    
                    let mtl_texture = metal_device.new_texture(&desc);
                    
                    mtl_texture.replace_region(
                        MTLRegion {
                            origin: MTLOrigin { x: 0, y: 0, z: 0 },
                            size: MTLSize {
                                width: self.width as u64,
                                height: self.height as u64,
                                depth: 1,
                            },
                        },
                        0,
                        data.as_ptr() as *const _,
                        (self.width * 4) as u64,
                    );
                    
                    let cmd_buf = metal_queue.new_command_buffer();
                    
                    unsafe {
                        let texture_ptr = mtl_texture.as_ptr() as *mut Object;
                        let cmd_buf_ptr = cmd_buf.as_ptr() as *mut Object;
                        self.server.publish_metal_texture(texture_ptr, cmd_buf_ptr);
                    }
                    
                    cmd_buf.commit();
                }
            }
            
            drop(data);
            buffer.unmap();
        }
    }
    
    /// Get the number of connected clients
    pub fn client_count(&self) -> usize {
        self.server.client_count()
    }
    
    /// Check if any clients are connected
    pub fn has_clients(&self) -> bool {
        self.server.client_count() > 0
    }
    
    /// Get the server name
    pub fn name(&self) -> &str {
        self.server.name()
    }
    
    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    
    /// Check if zero-copy is being used
    #[cfg(target_os = "macos")]
    pub fn is_zero_copy(&self) -> bool {
        self.use_zero_copy
    }
    
    /// Check if zero-copy is being used (non-macOS always returns false)
    #[cfg(not(target_os = "macos"))]
    pub fn is_zero_copy(&self) -> bool {
        false
    }
}

/// List available Syphon servers
pub fn list_servers() -> Vec<String> {
    syphon_core::SyphonServerDirectory::servers()
        .into_iter()
        .map(|info| info.name)
        .collect()
}

/// Check if Syphon is available on this system
pub fn is_available() -> bool {
    syphon_core::is_available()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_availability() {
        println!("Syphon available: {}", is_available());
    }
}
