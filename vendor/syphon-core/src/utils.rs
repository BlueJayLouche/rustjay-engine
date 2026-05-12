//! Utility functions for Objective-C interop

use crate::{Result, SyphonError};
use std::ffi::{CStr, CString};

#[cfg(target_os = "macos")]
use objc::runtime::Object;

/// Convert a Rust string to an NSString
///
/// # Safety
/// The returned pointer must be released when done
#[cfg(target_os = "macos")]
pub fn to_nsstring(s: &str) -> Result<*mut Object> {
    use objc::runtime::Class;
    use objc::{msg_send, sel, sel_impl};
    
    let c_string = CString::new(s).map_err(|e| {
        SyphonError::InvalidParameter(format!("Invalid string: {}", e))
    })?;
    
    unsafe {
        let cls = Class::get("NSString")
            .ok_or_else(|| SyphonError::FrameworkNotFound(
                "NSString class not found".to_string()
            ))?;
        
        let obj: *mut Object = msg_send![
            cls,
            stringWithUTF8String: c_string.as_ptr()
        ];
        
        if obj.is_null() {
            return Err(SyphonError::CreateFailed(
                "Failed to create NSString".to_string()
            ));
        }
        
        Ok(obj)
    }
}

/// Convert an NSString to a Rust String
#[cfg(target_os = "macos")]
pub fn from_nsstring(obj: *mut Object) -> String {
    use objc::{msg_send, sel, sel_impl};
    
    unsafe {
        if obj.is_null() {
            return String::new();
        }
        
        let cstr: *const i8 = msg_send![obj, UTF8String];
        CStr::from_ptr(cstr)
            .to_string_lossy()
            .into_owned()
    }
}

/// Get the Objective-C class name of an object
#[cfg(target_os = "macos")]
pub fn class_name(obj: *mut Object) -> String {
    use objc::{msg_send, sel, sel_impl};
    
    unsafe {
        if obj.is_null() {
            return "null".to_string();
        }
        
        let cls: *mut Object = msg_send![obj, class];
        let name: *const i8 = msg_send![cls, className];
        
        CStr::from_ptr(name)
            .to_string_lossy()
            .into_owned()
    }
}

/// Log debug info about an Objective-C object
#[cfg(target_os = "macos")]
pub fn log_object_info(obj: *mut Object) {
    use objc::{msg_send, sel, sel_impl};
    
    unsafe {
        let class_name_str = class_name(obj);
        let retain_count: usize = msg_send![obj, retainCount];
        let hash: usize = msg_send![obj, hash];
        
        log::debug!(
            "Object {:p}: class={}, retainCount={}, hash={}",
            obj, class_name_str, retain_count, hash
        );
    }
}

/// Check if a class exists (for framework availability testing)
#[cfg(target_os = "macos")]
pub fn class_exists(name: &str) -> bool {
    use objc::runtime::Class;
    Class::get(name).is_some()
}

/// Wrap an Objective-C exception in a Result
///
/// # Safety
/// This catches Objective-C exceptions and converts them to Rust errors
#[cfg(target_os = "macos")]
pub fn objc_try<F, R>(f: F) -> Result<R>
where
    F: FnOnce() -> R,
{
    use std::panic::catch_unwind;
    
    // Note: This catches panics but not Objective-C exceptions
    // For full @try/@catch support, you'd need objc_exception crate
    match catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => Ok(result),
        Err(_) => Err(SyphonError::ObjcException),
    }
}

/// Create a pixel format fourcc code from characters
pub const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    ((a as u32) << 24) | 
    ((b as u32) << 16) | 
    ((c as u32) << 8) | 
    (d as u32)
}

/// Common pixel formats
pub mod pixel_format {
    use super::fourcc;
    
    pub const BGRA: u32 = fourcc(b'B', b'G', b'R', b'A');
    pub const ARGB: u32 = fourcc(b'A', b'R', b'G', b'B');
    pub const RGBA: u32 = fourcc(b'R', b'G', b'B', b'A');
    pub const YUVS: u32 = fourcc(b'Y', b'U', b'V', b'S');
}
