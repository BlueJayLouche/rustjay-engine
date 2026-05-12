#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

pub mod input;
pub mod output;
pub mod ndi_runtime;
pub mod texture_utils;

#[cfg(target_os = "linux")]
pub mod v4l2_devices;

pub use input::InputManager;
pub use output::OutputManager;
