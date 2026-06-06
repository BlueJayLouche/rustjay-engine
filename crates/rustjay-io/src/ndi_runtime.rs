//! # NDI Runtime Path Setup (Windows)
//!
//! This module ensures the NDI runtime DLL can be found on Windows.
//! It's a no-op on macOS and Linux.
//!
//! ## Why This Exists
//!
//! The NDI SDK on Windows installs runtime DLLs to a versioned subdirectory
//! (e.g., `C:\Program Files\NDI\NDI 6 Runtime\v6\`). These paths are not
//! automatically added to the system PATH, causing "DLL not found" errors
//! when running the application.
//!
//! ## Solution
//!
//! On Windows, we search common NDI installation paths and add the runtime
//! directory to the DLL search path using `SetDllDirectoryW`.
//!
//! ## Platform Notes
//!
//! - **Windows**: Searches for NDI runtime and sets DLL directory
//! - **macOS**: No-op (NDI runtime is in standard library paths)
//! - **Linux**: No-op (NDI runtime is in standard library paths)

// Most of this module is only exercised on Windows; keep the helpers on other targets.
#![allow(dead_code)]

#[cfg(target_os = "windows")]
use std::path::Path;

/// NDI DLL name (Windows x64).
#[cfg(target_os = "windows")]
const NDI_DLL: &str = "Processing.NDI.Lib.x64.dll";

/// Directories to search for the NDI runtime, most common first.
#[cfg(target_os = "windows")]
const SEARCH_DIRS: &[&str] = &[
    // NDI 6 Runtime
    "C:\\Program Files\\NDI\\NDI 6 Runtime\\v6",
    "C:\\Program Files (x86)\\NDI\\NDI 6 Runtime\\v6",
    // NDI 5 Runtime
    "C:\\Program Files\\NDI\\NDI 5 Runtime\\v5",
    "C:\\Program Files (x86)\\NDI\\NDI 5 Runtime\\v5",
    // Older versions
    "C:\\Program Files\\NDI\\NDI 4 Runtime\\v4",
    "C:\\Program Files (x86)\\NDI\\NDI 4 Runtime\\v4",
    // SDK paths (development fallback)
    "C:\\Program Files\\NDI\\NDI 6 SDK\\Bin\\x64",
    "C:\\Program Files\\NDI\\NDI 5 SDK\\Bin\\x64",
];

/// Initialize NDI runtime path.
///
/// On Windows, searches for NDI runtime installation and adds it to the
/// DLL search path. On other platforms, this is a no-op.
///
/// # Returns
/// - `Ok(())` if successful or not needed on this platform
/// - `Err(String)` if NDI runtime cannot be found on Windows
pub fn init() -> Result<(), String> {
    init_internal()
}

#[cfg(target_os = "windows")]
fn init_internal() -> Result<(), String> {
    use windows::Win32::System::LibraryLoader::SetDllDirectoryW;

    for path_str in SEARCH_DIRS {
        let path = Path::new(path_str);
        if path.join(NDI_DLL).exists() {
            let wide_path: Vec<u16> = path_str.encode_utf16().chain(std::iter::once(0)).collect();

            unsafe {
                if SetDllDirectoryW(windows::core::PCWSTR(wide_path.as_ptr())).is_ok() {
                    log::info!("[NDI Runtime] Added to DLL search path: {}", path_str);
                    return Ok(());
                }
            }
        }
    }

    Err(format!(
        "NDI runtime DLL not found. Searched paths:\n{}\n\n\
         Please install NDI Tools from https://ndi.tv/tools/",
        SEARCH_DIRS.join("\n")
    ))
}

#[cfg(not(target_os = "windows"))]
fn init_internal() -> Result<(), String> {
    Ok(())
}

/// Check if NDI runtime is available without modifying paths.
pub fn is_available() -> bool {
    is_available_internal()
}

#[cfg(target_os = "windows")]
fn is_available_internal() -> bool {
    SEARCH_DIRS
        .iter()
        .any(|dir| Path::new(dir).join(NDI_DLL).exists())
}

#[cfg(not(target_os = "windows"))]
fn is_available_internal() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_does_not_panic() {
        // This should not panic on any platform
        let result = init();
        // On Windows without NDI installed, this will fail
        // On other platforms, it should succeed
        if cfg!(target_os = "windows") {
            // Result depends on whether NDI is installed
            println!("NDI init result: {:?}", result);
        } else {
            assert!(result.is_ok());
        }
    }

    #[test]
    fn test_is_available() {
        // This should not panic
        let available = is_available();
        println!("NDI available: {}", available);
    }
}
