//! Video output crate — wgpu helpers for video playback.
//!
//! This crate provides:
//! - `VideoFrame`: a decoded RGBA8 frame
//! - `VideoSource`: FFmpeg video decoder + `sws_scale` converter
//! - `CanvasTexture`: the projection canvas frame buffer
//! - `ProjectionRenderer`: slice + edge-blend renderer for one projector output
//!
//! The main application (in `qplayer`) wires these together inside its own
//! winit event loop, syncing video presentation to the audio master clock.

mod canvas_texture;
mod frame;
mod projection_renderer;
mod video_source;

pub use canvas_texture::CanvasTexture;
pub use frame::VideoFrame;
pub use projection_renderer::ProjectionRenderer;
pub use video_source::VideoSource;
