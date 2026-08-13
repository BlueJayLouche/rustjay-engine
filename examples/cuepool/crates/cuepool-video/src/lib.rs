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
#[cfg(windows)]
mod d3d11_zero_copy;
mod frame;
mod frame_lease;
mod frame_pool;
mod pixel_sampler;
mod projection_renderer;
mod video_source;
mod yuv_converter;
mod zero_copy;

pub use canvas_texture::CanvasTexture;
#[cfg(windows)]
pub use d3d11_zero_copy::{D3d11Frame, D3d11Handoff};
pub use frame_pool::FramePool;
pub use frame_lease::{LeaseBudget, LeasePermit, SubmissionRetirement, MAX_ZERO_COPY_LEASES};

/// FFmpeg library version (libavutil, encoded major<<16 | minor<<8 | micro),
/// for the Status diagnostics window.
pub fn ffmpeg_version() -> u32 {
    ffmpeg_next::util::version()
}
pub use pixel_sampler::PixelSampler;
pub use frame::{BitDepth, ChromaSubsample, FramePixels, VideoFrame, YuvPlane};
pub use projection_renderer::ProjectionRenderer;
pub use video_source::{VideoFrameTimings, VideoSource, ZeroCopyPreference};
pub use yuv_converter::YuvConverter;
pub use zero_copy::ZeroCopyAvailability;

#[cfg(test)]
pub(crate) fn test_device_queue(
    required_features: wgpu::Features,
) -> Option<(wgpu::Device, wgpu::Queue)> {
    pollster::block_on(async {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;
        let device_queue = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features,
                ..Default::default()
            })
            .await
            .expect("device");
        Some(device_queue)
    })
}
