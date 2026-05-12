//! wgpu ↔ Metal interop helpers
//!
//! **All `wgpu-hal` version-specific code lives here.**
//! When upgrading wgpu, edit only this module.
//!
//! Current wgpu version: 29.0.3
//!
//! ## Changes from wgpu 25
//!
//! - `as_hal` no longer takes a closure; it returns `Option<impl Deref<...>>` directly.
//! - `wgpu_hal::metal::Queue` no longer exposes the raw `MTLCommandQueue`.
//!   The zero-copy blit path now uses the `metal::CommandQueue` stored in
//!   `SyphonWgpuOutput` (which comes from the same MTLDevice as wgpu's).
//!   `device.poll(PollType::Wait)` is called before the blit to ensure wgpu's
//!   prior render commands have finished on the GPU.
//! - `wgpu_hal::metal::Device::raw_device()` returns
//!   `&Retained<ProtocolObject<dyn MTLDevice>>` instead of `&Mutex<metal::Device>`.
//!   `ProtocolObject<P>` is `#[repr(C)]` over a zero-sized `AnyObject`, so
//!   `*const ProtocolObject<_>` is always a thin (non-fat) ObjC `id` pointer.

#[cfg(target_os = "macos")]
use metal::foreign_types::{ForeignType, ForeignTypeRef};

/// Extract the underlying `metal::Device` from a wgpu device.
///
/// Returns `None` when the wgpu device is not backed by Metal (e.g. Vulkan).
#[cfg(target_os = "macos")]
pub fn extract_metal_device(device: &wgpu::Device) -> Option<metal::Device> {
    // Retain via direct libobjc call to avoid objc crate's sel! macro scoping.
    extern "C" {
        fn objc_retain(value: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    unsafe {
        let hal_guard = device.as_hal::<wgpu_hal::api::Metal>()?;
        // raw_device() → &Retained<ProtocolObject<dyn MTLDevice>>
        // ProtocolObject<P> is repr(C) over zero-sized AnyObject; the pointer is
        // a thin ObjC id<MTLDevice>. Cast is safe — same machine word.
        let proto_ref = &**hal_guard.raw_device();
        let raw_ptr = proto_ref as *const _ as *mut metal::MTLDevice;
        // Retain so that metal-rs's drop doesn't over-release the device that
        // wgpu still holds.
        objc_retain(raw_ptr as *mut _);
        Some(metal::Device::from_ptr(raw_ptr))
    }
}

/// Call `f` with the raw `MTLTextureRef` backing a wgpu texture.
///
/// `f` is not called if the wgpu texture is not Metal-backed.
/// The reference is valid only for the duration of `f`.
#[cfg(target_os = "macos")]
pub fn with_metal_texture<F>(texture: &wgpu::Texture, f: F)
where
    F: FnOnce(&metal::TextureRef),
{
    unsafe {
        if let Some(hal_guard) = texture.as_hal::<wgpu_hal::api::Metal>() {
            // raw_handle() → &ProtocolObject<dyn MTLTexture> (thin pointer)
            let proto_ref = hal_guard.raw_handle();
            let raw_ptr = proto_ref as *const _ as *const metal::MTLTexture;
            f(metal::TextureRef::from_ptr(raw_ptr as *mut _));
        }
    }
}
