//! Render utilities and wgpu engine for RustJay.
//!
//! Provides the `WgpuEngine`, texture helpers, and plugin renderer used by
//! the real-time visual engine.

#![warn(missing_docs)]

/// Screen blit pipeline.
pub mod blit;
/// Plugin-aware renderer.
pub mod plugin_renderer;
/// Main wgpu renderer.
pub mod renderer;
/// Texture utilities.
pub mod texture;

/// Re-export of the main wgpu rendering engine.
pub use renderer::WgpuEngine;
/// Re-export of the generic texture type.
pub use texture::Texture;
/// Re-export of the input texture wrapper.
pub use texture::InputTexture;
/// Re-export of the feedback texture wrapper.
pub use texture::PreviousFrameTexture;
