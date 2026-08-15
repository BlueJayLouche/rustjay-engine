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
mod d3d12_zero_copy;
mod frame;
mod frame_lease;
mod frame_pool;
mod hap_converter;
mod pixel_sampler;
mod projection_renderer;
mod submit_queue;
mod video_source;
mod yuv_converter;
mod zero_copy;

pub use canvas_texture::CanvasTexture;
#[cfg(windows)]
pub use d3d12_zero_copy::{D3d12Frame, D3d12Handoff};
pub use frame_lease::{LeaseBudget, LeasePermit, MAX_ZERO_COPY_LEASES, SubmissionRetirement};
pub use frame_pool::FramePool;
#[cfg(windows)]
pub use submit_queue::DecodeFence;
pub use submit_queue::SharedQueue;

/// FFmpeg library version (libavutil, encoded major<<16 | minor<<8 | micro),
/// for the Status diagnostics window.
pub fn ffmpeg_version() -> u32 {
    ffmpeg_next::util::version()
}
pub use frame::{BitDepth, ChromaSubsample, FramePixels, VideoFrame, YuvPlane};
pub use hap_converter::HapConverter;
pub use pixel_sampler::PixelSampler;
pub use projection_renderer::ProjectionRenderer;
pub use video_source::{
    HapAcceleration, HapFallbackSession, VideoFrameTimings, VideoSource, ZeroCopyPreference,
};
pub use yuv_converter::YuvConverter;
pub use zero_copy::ZeroCopyAvailability;

/// Serializes GPU device creation/use across tests. With the default parallel
/// test harness, concurrent device create/destroy cycles from several test
/// threads can spin inside the NVIDIA user-mode driver when another process
/// (the venue soak) is actively rendering — observed wedging the suite for an
/// hour on ASHOF-PC02. Every GPU test takes this lock first.
#[cfg(test)]
pub(crate) fn gpu_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static GPU_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

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
        if !adapter.features().contains(required_features) {
            return None;
        }
        let device_queue = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features,
                ..Default::default()
            })
            .await
            .ok()?;
        Some(device_queue)
    })
}
