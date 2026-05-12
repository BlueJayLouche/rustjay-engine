//! Extended IOSurface bindings not provided by the io_surface crate

use std::os::raw::c_void;
use io_surface::IOSurfaceRef;

#[link(name = "IOSurface", kind = "framework")]
extern "C" {
    /// Lock the IOSurface for access
    /// 
    /// # Arguments
    /// * `buffer` - The IOSurface reference
    /// * `options` - Lock options (0 for default, 1 for read-only)
    /// * `seed` - Pointer to store the seed value (can be null)
    /// 
    /// # Returns
    /// 0 on success, error code on failure
    pub fn IOSurfaceLock(buffer: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    
    /// Unlock the IOSurface
    /// 
    /// # Arguments
    /// * `buffer` - The IOSurface reference
    /// * `options` - Unlock options (0 for default)
    /// * `seed` - Pointer to store the seed value (can be null)
    /// 
    /// # Returns
    /// 0 on success, error code on failure
    pub fn IOSurfaceUnlock(buffer: IOSurfaceRef, options: u32, seed: *mut u32) -> i32;
    
    /// Get the base address of the IOSurface
    /// 
    /// # Safety
    /// The surface must be locked before accessing the base address
    pub fn IOSurfaceGetBaseAddress(buffer: IOSurfaceRef) -> *mut c_void;
    
    /// Get the bytes per row (stride) of the IOSurface
    pub fn IOSurfaceGetBytesPerRow(buffer: IOSurfaceRef) -> usize;
    
    /// Get the height of the IOSurface
    pub fn IOSurfaceGetHeight(buffer: IOSurfaceRef) -> usize;
    
    /// Get the width of the IOSurface
    pub fn IOSurfaceGetWidth(buffer: IOSurfaceRef) -> usize;
}

// Constants for lock options
pub const kIOSurfaceLockReadOnly: u32 = 0x1;
pub const kIOSurfaceLockAvoidSync: u32 = 0x2;
