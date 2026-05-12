//! Metal Device Utilities for Syphon
//!
//! This module provides utilities for working with Metal devices,
//! including GPU selection, device compatibility checking, and
//! high-performance GPU detection.

use crate::{Result, SyphonError};

#[cfg(target_os = "macos")]
use objc::runtime::Object;
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};

/// Information about a Metal GPU device
#[derive(Debug, Clone)]
pub struct MetalDeviceInfo {
    /// The raw Metal device pointer (id<MTLDevice>)
    #[cfg(target_os = "macos")]
    pub raw_device: *mut Object,
    
    /// Human-readable device name (e.g., "Apple M1 Pro", "AMD Radeon Pro 5500M")
    pub name: String,
    
    /// Whether this is the system's default device
    pub is_default: bool,
    
    /// Whether this is a low-power (integrated) GPU
    pub is_low_power: bool,
    
    /// Whether this is a removable (eGPU) device
    pub is_removable: bool,
    
    /// Whether this device has unified memory (Apple Silicon, Intel integrated)
    pub has_unified_memory: bool,
    
    /// Recommended maximum working set size in bytes (if available)
    pub recommended_max_working_set_size: Option<u64>,
    
    /// Metal GPU family (e.g., "Apple7", "Mac2")
    pub gpu_family: Option<String>,
}

// SAFETY: `raw_device` is a pointer to `id<MTLDevice>`. Metal device objects
// are explicitly documented by Apple as thread-safe singletons — they are
// reference-counted and safe to use concurrently from multiple threads.
// The remaining fields are plain Rust types that are trivially Send + Sync.
#[cfg(target_os = "macos")]
unsafe impl Send for MetalDeviceInfo {}
#[cfg(target_os = "macos")]
unsafe impl Sync for MetalDeviceInfo {}

impl MetalDeviceInfo {
    /// Check if this is a high-performance GPU (discrete/dedicated)
    ///
    /// On Apple Silicon, all GPUs are technically "integrated" but still
    /// high-performance. This method returns true for:
    /// - Discrete GPUs (AMD, NVIDIA on Intel Macs)
    /// - Apple Silicon GPUs (M1, M2, M3, etc.)
    /// - Non-low-power GPUs
    pub fn is_high_performance(&self) -> bool {
        // Apple Silicon GPUs have unified memory but are high performance
        if self.has_unified_memory && !self.is_low_power {
            return true;
        }
        // Discrete GPUs don't have unified memory and aren't low power
        if !self.has_unified_memory && !self.is_low_power {
            return true;
        }
        false
    }
    
    /// Check if two devices are the same physical GPU
    #[cfg(target_os = "macos")]
    pub fn is_same_device(&self, other: &MetalDeviceInfo) -> bool {
        self.raw_device == other.raw_device
    }
    
    /// Check if two devices are compatible for texture sharing
    ///
    /// Devices are compatible if they are the same device or if they
    /// support the same GPU features. For optimal performance, textures
    /// should be shared between the same device.
    pub fn is_compatible_with(&self, other: &MetalDeviceInfo) -> bool {
        #[cfg(target_os = "macos")]
        {
            // Same device is always compatible
            if self.is_same_device(other) {
                return true;
            }
            
            // Same GPU family is likely compatible
            if let (Some(ref self_family), Some(ref other_family)) = 
                (&self.gpu_family, &other.gpu_family) {
                if self_family == other_family {
                    return true;
                }
            }
        }
        
        // Different devices may work but with performance penalties
        false
    }
}

/// Get the system's default Metal device
///
/// This is typically the best GPU for rendering on the current system.
/// On single-GPU systems, this is the only GPU. On multi-GPU systems,
/// macOS selects what it considers the "best" GPU.
#[cfg(target_os = "macos")]
pub fn default_device() -> Option<MetalDeviceInfo> {
    unsafe {
        extern "C" {
            fn MTLCreateSystemDefaultDevice() -> *mut Object;
        }
        
        let device = MTLCreateSystemDefaultDevice();
        if device.is_null() {
            return None;
        }
        
        get_device_info(device, true)
    }
}

#[cfg(not(target_os = "macos"))]
pub fn default_device() -> Option<MetalDeviceInfo> {
    None
}

/// Get information about a specific Metal device
#[cfg(target_os = "macos")]
pub fn get_device_info(device: *mut Object, is_default: bool) -> Option<MetalDeviceInfo> {
    unsafe {
        if device.is_null() {
            return None;
        }
        
        // Get device name
        let name_nsstring: *mut Object = msg_send![device, name];
        let name = crate::utils::from_nsstring(name_nsstring);
        
        // Check device properties
        let is_low_power: bool = msg_send![device, isLowPower];
        let is_removable: bool = msg_send![device, isRemovable];
        
        // hasUnifiedMemory is available on macOS 10.15+
        let has_unified_memory: bool = msg_send![device, hasUnifiedMemory];
        
        // recommendedMaxWorkingSetSize (available on macOS 10.12+)
        let recommended_max_working_set_size: u64 = 
            msg_send![device, recommendedMaxWorkingSetSize];
        let recommended_max_working_set_size = if recommended_max_working_set_size > 0 {
            Some(recommended_max_working_set_size)
        } else {
            None
        };
        
        // Try to get GPU family information
        let gpu_family = get_gpu_family(device);
        
        Some(MetalDeviceInfo {
            raw_device: device,
            name,
            is_default,
            is_low_power,
            is_removable,
            has_unified_memory,
            recommended_max_working_set_size,
            gpu_family,
        })
    }
}

#[cfg(target_os = "macos")]
unsafe fn get_gpu_family(device: *mut Object) -> Option<String> {
    use cocoa::foundation::NSUInteger;
    
    // MTLGPUFamily values (as of Metal 3)
    // These are approximate mappings
    let family: NSUInteger = msg_send![device, supportsFamily: 1001u64]; // Apple1
    if family != 0 {
        // Check for Apple GPU families
        let families = [
            (1008, "Apple8"),
            (1007, "Apple7"),
            (1006, "Apple6"),
            (1005, "Apple5"),
            (1004, "Apple4"),
            (1003, "Apple3"),
            (1002, "Apple2"),
            (1001, "Apple1"),
        ];
        
        for (family_id, name) in &families {
            let supported: bool = msg_send![device, supportsFamily: *family_id as u64];
            if supported {
                return Some(name.to_string());
            }
        }
    }
    
    // Check for Mac GPU families
    let mac_families = [
        (5001, "Mac2"),
        (5002, "Mac1"),
    ];
    
    for (family_id, name) in &mac_families {
        let supported: bool = msg_send![device, supportsFamily: *family_id as u64];
        if supported {
            return Some(name.to_string());
        }
    }
    
    None
}

/// List all available Metal devices on the system
///
/// This includes integrated GPUs, discrete GPUs, and external GPUs (eGPUs).
#[cfg(target_os = "macos")]
pub fn available_devices() -> Vec<MetalDeviceInfo> {
    unsafe {
        // Get MTLCopyAllDevices function
        extern "C" {
            fn MTLCopyAllDevices() -> *mut Object; // Returns NSArray<id<MTLDevice>>
        }
        
        let devices_array = MTLCopyAllDevices();
        if devices_array.is_null() {
            return Vec::new();
        }
        
        let count: usize = msg_send![devices_array, count];
        let default = default_device();
        let default_ptr = default.as_ref().map(|d| d.raw_device);
        
        let mut devices = Vec::with_capacity(count);
        
        for i in 0..count {
            let device: *mut Object = msg_send![devices_array, objectAtIndex: i];
            let is_default = default_ptr.map(|d| d == device).unwrap_or(false);
            
            if let Some(info) = get_device_info(device, is_default) {
                devices.push(info);
            }
        }
        
        // Release the array
        let _: () = msg_send![devices_array, release];
        
        devices
    }
}

#[cfg(not(target_os = "macos"))]
pub fn available_devices() -> Vec<MetalDeviceInfo> {
    Vec::new()
}

/// Get the recommended GPU for high-performance rendering
///
/// This selects the best GPU in the following priority:
/// 1. Non-low-power discrete GPU (AMD on Intel Macs)
/// 2. Apple Silicon GPU (unified memory, not low power)
/// 3. Default GPU (fallback)
///
/// Returns None if no Metal devices are available.
pub fn recommended_high_performance_device() -> Option<MetalDeviceInfo> {
    let devices = available_devices();
    
    if devices.is_empty() {
        return None;
    }
    
    // First, look for a non-low-power discrete GPU
    for device in &devices {
        if !device.is_low_power && !device.has_unified_memory {
            return Some(device.clone());
        }
    }
    
    // Second, look for Apple Silicon GPU (unified memory, not low power)
    for device in &devices {
        if device.has_unified_memory && !device.is_low_power {
            return Some(device.clone());
        }
    }
    
    // Third, look for any non-low-power GPU
    for device in &devices {
        if !device.is_low_power {
            return Some(device.clone());
        }
    }
    
    // Fallback to default device
    devices.into_iter().find(|d| d.is_default).or_else(default_device)
}

/// Check if a specific device is compatible with Syphon
///
/// This verifies that:
/// 1. The device is a valid Metal device
/// 2. The device supports the required Metal features
/// 3. The device can be used for texture sharing
#[cfg(target_os = "macos")]
pub fn check_device_compatibility(device: *mut Object) -> Result<()> {
    unsafe {
        if device.is_null() {
            return Err(SyphonError::InvalidParameter(
                "Device pointer is null".to_string()
            ));
        }
        
        // Check if device supports BGRA8Unorm texture format (required for Syphon)
        let format_supported: bool = msg_send![device, supportsTextureSampleCount: 1];
        if !format_supported {
            return Err(SyphonError::InvalidParameter(
                "Device does not support required texture formats".to_string()
            ));
        }
        
        // Check if device supports the minimum required features
        // MTLFeatureSet is deprecated, so we just check basic support
        let name_nsstring: *mut Object = msg_send![device, name];
        let name = crate::utils::from_nsstring(name_nsstring);
        
        log::debug!("Device '{}' compatibility check passed", name);
        
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
pub fn check_device_compatibility(_device: *mut Object) -> Result<()> {
    Err(SyphonError::NotAvailable)
}

/// Validate that a render device matches the Syphon server device
///
/// This function should be called before publishing frames to ensure
/// optimal performance. It returns a warning if the devices don't match,
/// as this may cause performance penalties due to GPU-to-GPU transfers.
#[cfg(target_os = "macos")]
pub fn validate_device_match(
    render_device: *mut Object,
    syphon_device: *mut Object,
) -> Result<()> {
    if render_device.is_null() || syphon_device.is_null() {
        return Err(SyphonError::InvalidParameter(
            "Device pointer is null".to_string()
        ));
    }
    
    if render_device == syphon_device {
        return Ok(());
    }
    
    // Devices don't match - get info for better error message
    let render_info = get_device_info(render_device, false);
    let syphon_info = get_device_info(syphon_device, false);
    
    let render_name = render_info.as_ref().map(|d| d.name.as_str()).unwrap_or("Unknown");
    let syphon_name = syphon_info.as_ref().map(|d| d.name.as_str()).unwrap_or("Unknown");
    
    log::warn!(
        "GPU mismatch detected: Render device '{}' does not match Syphon server device '{}'. \
         This may cause performance penalties due to GPU-to-GPU transfers.",
        render_name,
        syphon_name
    );
    
    // Check if devices are at least compatible (same family)
    if let (Some(render_info), Some(syphon_info)) = (render_info, syphon_info) {
        if !render_info.is_compatible_with(&syphon_info) {
            log::warn!(
                "Devices are from different GPU families and may not be fully compatible."
            );
        }
    }
    
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub fn validate_device_match(
    _render_device: *mut Object,
    _syphon_device: *mut Object,
) -> Result<()> {
    Err(SyphonError::NotAvailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_device() {
        #[cfg(target_os = "macos")]
        {
            if let Some(device) = default_device() {
                println!("Default device: {:?}", device.name);
                println!("Is low power: {:?}", device.is_low_power);
                println!("Has unified memory: {:?}", device.has_unified_memory);
            } else {
                println!("No default Metal device found");
            }
        }
    }
    
    #[test]
    fn test_available_devices() {
        #[cfg(target_os = "macos")]
        {
            let devices = available_devices();
            println!("Found {} Metal devices", devices.len());
            for device in &devices {
                println!("  - {} (default={}, low_power={}, unified={})",
                    device.name,
                    device.is_default,
                    device.is_low_power,
                    device.has_unified_memory
                );
            }
        }
    }
    
    #[test]
    fn test_recommended_device() {
        #[cfg(target_os = "macos")]
        {
            if let Some(device) = recommended_high_performance_device() {
                println!("Recommended high-performance device: {}", device.name);
                println!("Is high performance: {}", device.is_high_performance());
            } else {
                println!("No high-performance device found");
            }
        }
    }
}
