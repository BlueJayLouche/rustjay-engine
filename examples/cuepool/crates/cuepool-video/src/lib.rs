//! Video output crate — wgpu helpers for video playback.
//!
//! This crate provides:
//! - `VideoFrame`: a decoded RGBA8 frame
//! - `VideoSource`: FFmpeg video decoder + `sws_scale` converter
//! - `CanvasTexture`: the projection canvas frame buffer
//! - `ProjectionRenderer`: slice + edge-blend renderer for one projector output
//!
//! The main application (in `cuepool`) wires these together inside its own
//! winit event loop, syncing video presentation to the audio master clock.

mod canvas_texture;
mod frame;
mod frame_pool;
mod pixel_sampler;
mod projection_renderer;
mod video_source;
mod yuv_converter;

pub use canvas_texture::CanvasTexture;
pub use frame_pool::FramePool;

/// FFmpeg library version (libavutil, encoded major<<16 | minor<<8 | micro),
/// for the Status diagnostics window.
pub fn ffmpeg_version() -> u32 {
    ffmpeg_next::util::version()
}
pub use pixel_sampler::PixelSampler;
pub use frame::{BitDepth, ChromaSubsample, FramePixels, VideoFrame, YuvPlane};
pub use projection_renderer::ProjectionRenderer;
pub use video_source::VideoSource;
pub use yuv_converter::YuvConverter;
