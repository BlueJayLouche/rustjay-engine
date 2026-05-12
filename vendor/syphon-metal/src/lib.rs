//! # Syphon Metal
//! 
//! Metal-specific utilities for Syphon, including IOSurface creation
//! and Metal texture interop for zero-copy GPU-to-GPU sharing.
//! 
//! ## Usage
//! 
//! ```ignore
//! use syphon_metal::{IOSurfacePool, MetalContext};
//! 
//! // Create a Metal context from a wgpu device (for zero-copy)
//! let ctx = MetalContext::from_wgpu_device(&wgpu_device)?;
//! 
//! // Create an IOSurface pool for efficient reuse
//! let pool = IOSurfacePool::new(1920, 1080, 3);
//! 
//! // Get a surface and create a Metal texture from it
//! let surface = pool.acquire().unwrap();
//! let texture = ctx.create_texture_from_iosurface(&surface, 1920, 1080)?;
//! ```

// Re-export io_surface
#[cfg(target_os = "macos")]
pub use io_surface::IOSurface;

/// A pool of reusable IOSurfaces for efficient frame publishing
#[cfg(target_os = "macos")]
pub struct IOSurfacePool {
    width: u32,
    height: u32,
    surfaces: Vec<io_surface::IOSurface>,
}

#[cfg(target_os = "macos")]
impl IOSurfacePool {
    /// Create a new pool with the specified capacity
    pub fn new(width: u32, height: u32, capacity: usize) -> Self {
        let mut surfaces = Vec::with_capacity(capacity);
        
        for _ in 0..capacity {
            if let Some(surface) = create_iosurface(width, height) {
                surfaces.push(surface);
            }
        }
        
        Self {
            width,
            height,
            surfaces,
        }
    }
    
    /// Acquire an IOSurface from the pool
    /// Returns None if all surfaces are in use
    pub fn acquire(&mut self) -> Option<io_surface::IOSurface> {
        self.surfaces.pop()
    }
    
    /// Return an IOSurface to the pool
    pub fn release(&mut self, surface: io_surface::IOSurface) {
        self.surfaces.push(surface);
    }
    
    /// Get pool capacity
    pub fn capacity(&self) -> usize {
        self.surfaces.capacity()
    }
    
    /// Get available surface count
    pub fn available(&self) -> usize {
        self.surfaces.len()
    }
    
    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Stub implementation for non-macOS platforms
#[cfg(not(target_os = "macos"))]
pub struct IOSurfacePool {
    width: u32,
    height: u32,
}

#[cfg(not(target_os = "macos"))]
impl IOSurfacePool {
    pub fn new(width: u32, height: u32, _capacity: usize) -> Self {
        Self { width, height }
    }
    pub fn capacity(&self) -> usize { 0 }
    pub fn available(&self) -> usize { 0 }
    pub fn dimensions(&self) -> (u32, u32) { (self.width, self.height) }
}

/// Create an IOSurface with the specified dimensions
#[cfg(target_os = "macos")]
fn create_iosurface(width: u32, height: u32) -> Option<io_surface::IOSurface> {
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    
    let width_num = CFNumber::from(width as i64);
    let height_num = CFNumber::from(height as i64);
    let bytes_per_elem = CFNumber::from(4i64); // RGBA8 = 4 bytes
    let pixel_format = CFNumber::from(0x42475241i64); // 'BGRA'
    
    let keys: Vec<CFString> = vec![
        CFString::from_static_string("IOSurfaceWidth"),
        CFString::from_static_string("IOSurfaceHeight"),
        CFString::from_static_string("IOSurfaceBytesPerElement"),
        CFString::from_static_string("IOSurfacePixelFormat"),
    ];
    
    // Create slices of references for the pairs
    let pairs: Vec<(CFString, CFType)> = vec![
        (keys[0].clone(), width_num.as_CFType().clone()),
        (keys[1].clone(), height_num.as_CFType().clone()),
        (keys[2].clone(), bytes_per_elem.as_CFType().clone()),
        (keys[3].clone(), pixel_format.as_CFType().clone()),
    ];
    
    let props = CFDictionary::from_CFType_pairs(&pairs);
    
    Some(io_surface::new(&props))
}

/// Metal context for zero-copy interop with wgpu
/// 
/// This holds the Metal device and command queue for creating
/// IOSurface-backed textures and performing GPU blits.
#[cfg(target_os = "macos")]
pub struct MetalContext {
    device: metal::Device,
    queue: metal::CommandQueue,
}

// SAFETY: `metal::Device` and `metal::CommandQueue` are both thread-safe per
// Apple's Metal documentation — they are reference-counted ObjC objects that
// can be used concurrently from multiple threads. Creating command buffers and
// textures from them is explicitly documented as thread-safe.
#[cfg(target_os = "macos")]
unsafe impl Send for MetalContext {}
#[cfg(target_os = "macos")]
unsafe impl Sync for MetalContext {}

#[cfg(target_os = "macos")]
impl MetalContext {
    /// Create a new Metal context from a raw Metal device
    /// 
    /// # Safety
    /// The device must be valid and remain valid for the lifetime of this context
    pub unsafe fn from_raw_device(device: metal::Device) -> Self {
        let queue = device.new_command_queue();
        Self { device, queue }
    }
    
    /// Create a Metal context from the system default device
    pub fn system_default() -> Option<Self> {
        metal::Device::system_default().map(|device| {
            let queue = device.new_command_queue();
            Self { device, queue }
        })
    }
    
    /// Get the raw Metal device
    pub fn device(&self) -> &metal::Device {
        &self.device
    }
    
    /// Get the raw Metal command queue
    pub fn queue(&self) -> &metal::CommandQueue {
        &self.queue
    }
    
    /// Create a Metal texture from an IOSurface
    /// 
    /// This creates a texture that shares memory with the IOSurface,
    /// enabling zero-copy GPU-to-GPU transfers.
    /// 
    /// Uses raw Objective-C calls since metal-rs doesn't expose IOSurface support.
    pub fn create_texture_from_iosurface(
        &self,
        surface: &io_surface::IOSurface,
        width: u32,
        height: u32,
    ) -> Option<metal::Texture> {
        use objc::runtime::Object;
        use objc::class;
        use core_foundation::base::TCFType;
        use objc::{msg_send, sel, sel_impl};
        use cocoa::foundation::NSUInteger;
        use metal::{MTLStorageMode, MTLTextureUsage, MTLPixelFormat};
        use foreign_types_shared::ForeignType;
        
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
            let device_ptr = self.device.as_ptr() as *mut Object;
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
    
    /// Blit from a source Metal texture to a destination IOSurface-backed texture
    /// 
    /// This performs a GPU-to-GPU copy without involving the CPU.
    pub fn blit_to_iosurface(
        &self,
        src_texture: &metal::Texture,
        dest_surface: &io_surface::IOSurface,
        width: u32,
        height: u32,
    ) -> metal::CommandBuffer {
        // Create destination texture from IOSurface
        let dest_texture = self.create_texture_from_iosurface(dest_surface, width, height)
            .expect("Failed to create destination texture from IOSurface");
        
        // Create command buffer
        let cmd_buf = self.queue.new_command_buffer();
        
        // Create blit encoder
        let blit_encoder = cmd_buf.new_blit_command_encoder();
        
        // Copy from source to destination
        blit_encoder.copy_from_texture(
            src_texture,
            0, // source level
            0, // source slice
            metal::MTLOrigin { x: 0, y: 0, z: 0 },
            metal::MTLSize { 
                width: width as u64, 
                height: height as u64, 
                depth: 1 
            },
            &dest_texture,
            0, // destination level
            0, // destination slice
            metal::MTLOrigin { x: 0, y: 0, z: 0 },
        );
        
        blit_encoder.end_encoding();
        
        // We need to keep the dest_texture alive until the blit completes
        // Store it in the command buffer's user info (hacky but works)
        // A better approach would be to use a completion handler
        
        // Convert from &CommandBufferRef to CommandBuffer
        // cmd_buf is &CommandBufferRef, we need to convert to CommandBuffer
        // Use to_owned() to get an owned CommandBuffer
        cmd_buf.to_owned()
    }
    
    /// Blit from a wgpu texture to an IOSurface-backed texture
    /// 
    /// # Safety
    /// This requires access to wgpu's internal Metal texture through wgpu-hal.
    /// The source_texture_ptr must be a valid `id<MTLTexture>`.
    pub unsafe fn blit_wgpu_to_iosurface(
        &self,
        src_texture: &metal::Texture,
        dest_surface: &io_surface::IOSurface,
        width: u32,
        height: u32,
    ) -> metal::CommandBuffer {
        self.blit_to_iosurface(src_texture, dest_surface, width, height)
    }
}

/// Stub implementation for non-macOS platforms
#[cfg(not(target_os = "macos"))]
pub struct MetalContext;

#[cfg(not(target_os = "macos"))]
impl MetalContext {
    pub fn system_default() -> Option<Self> { None }
}

/// Helper to extract raw Metal handles from wgpu objects using wgpu-hal
///
/// Updated for wgpu 29: `as_hal` no longer accepts a closure; it returns
/// `Option<impl Deref<Target = ...>>` directly. `Queue::as_raw()` was removed —
/// the MTLCommandQueue is no longer publicly accessible via wgpu-hal.
#[cfg(all(feature = "wgpu", target_os = "macos"))]
pub mod wgpu_interop {
    use foreign_types_shared::{ForeignType, ForeignTypeRef};

    /// Extract the raw Metal device from a wgpu device.
    ///
    /// # Safety
    /// The caller must not use the returned `metal::Device` past the point where
    /// the wgpu device is dropped. The device is retained internally, so it is
    /// safe to hold onto independently.
    pub unsafe fn extract_metal_device(device: &wgpu::Device) -> Option<metal::Device> {
        // Retain via direct libobjc call to avoid objc crate's sel! macro scoping.
        extern "C" {
            fn objc_retain(value: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
        }

        let hal_guard = device.as_hal::<wgpu_hal::api::Metal>()?;
        // raw_device() → &Retained<ProtocolObject<dyn MTLDevice>>
        // ProtocolObject<P> is #[repr(C)] over zero-sized AnyObject; the pointer
        // is a thin ObjC id<MTLDevice>. Both metal-rs and objc2 wrap the same type.
        let proto_ref = &**hal_guard.raw_device();
        let raw_ptr = proto_ref as *const _ as *mut metal::MTLDevice;
        // Retain before handing to metal-rs so its drop doesn't over-release.
        objc_retain(raw_ptr as *mut _);
        Some(metal::Device::from_ptr(raw_ptr))
    }

    /// Call `f` with the raw `MTLTextureRef` backing a wgpu texture.
    ///
    /// `f` receives `None` when the texture is not Metal-backed.
    /// The reference is only valid for the duration of `f`.
    ///
    /// # Safety
    /// The wgpu texture must remain valid (not dropped) while `f` runs.
    pub unsafe fn with_metal_texture<F, R>(texture: &wgpu::Texture, f: F) -> R
    where
        F: FnOnce(Option<&metal::TextureRef>) -> R,
    {
        match texture.as_hal::<wgpu_hal::api::Metal>() {
            Some(hal_guard) => {
                // raw_handle() → &ProtocolObject<dyn MTLTexture> (thin pointer)
                let proto_ref = hal_guard.raw_handle();
                let raw_ptr = proto_ref as *const _ as *const metal::MTLTexture;
                f(Some(metal::TextureRef::from_ptr(raw_ptr as *mut _)))
            }
            None => f(None),
        }
    }

    /// Get the raw `id<MTLTexture>` pointer from a wgpu texture.
    ///
    /// Returns `None` if the texture is not Metal-backed.
    /// The pointer is only valid while the wgpu texture is alive.
    ///
    /// # Safety
    /// The wgpu texture must remain alive while the returned pointer is in use.
    pub unsafe fn get_metal_texture_ptr(
        texture: &wgpu::Texture,
    ) -> Option<*mut objc::runtime::Object> {
        with_metal_texture(texture, |mtl_tex| {
            mtl_tex.map(|t| t.as_ptr() as *mut objc::runtime::Object)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_surface_pool() {
        let mut pool = IOSurfacePool::new(640, 480, 3);
        assert_eq!(pool.capacity(), 3);
        
        #[cfg(target_os = "macos")]
        {
            assert_eq!(pool.available(), 3);
            
            let surface = pool.acquire();
            assert!(surface.is_some());
            assert_eq!(pool.available(), 2);
            
            pool.release(surface.unwrap());
            assert_eq!(pool.available(), 3);
        }
    }
    
    #[test]
    fn test_metal_context_creation() {
        // This test only works on macOS with Metal available
        #[cfg(target_os = "macos")]
        {
            if let Some(_ctx) = MetalContext::system_default() {
                println!("Metal context created successfully");
            } else {
                println!("Metal not available on this system");
            }
        }
    }
}
