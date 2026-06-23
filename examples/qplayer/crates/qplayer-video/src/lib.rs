//! Video output crate — wgpu helpers for video playback.
//!
//! This crate provides:
//! - `Renderer`: simple textured quad blit pipeline
//! - `Texture`: double-buffered RGBA texture upload
//! - `VideoSource`: FFmpeg video decoder + `sws_scale` converter
//! - `OutputWindow`: winit window + wgpu surface helper
//!
//! The main application (in `qplayer`) wires these together inside its own
//! winit event loop, syncing video presentation to the audio master clock.

mod renderer;
mod texture;
mod video_source;
mod window;

pub use renderer::Renderer;
pub use texture::{Texture, VideoFrame};
pub use video_source::VideoSource;
pub use window::OutputWindow;
