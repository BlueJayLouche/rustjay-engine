//! Sources — ISF, video, image, camera, NDI, streams.
//!
//! Delegates to engine crates where possible:
//! - ISF      → `rustjay-isf`
//! - Camera   → `rustjay-io/input` (webcam)
//! - NDI      → `rustjay-io/ndi_runtime`
//! - Video decode / HAP / SRT / HLS / DASH / RTMP → coverage gaps;
//!   see `PARITY.md` Phase 2 / 9 / 10 probes.

mod camera_source;
mod image_source;
pub mod registry;
mod solid_color_source;
mod watcher;

#[cfg(feature = "ffmpeg")]
mod ffmpeg_source;
#[cfg(feature = "ffmpeg")]
mod stream_source;
#[cfg(feature = "hap")]
mod hap_source;
#[cfg(feature = "ndi")]
mod ndi_source;
#[cfg(target_os = "macos")]
mod syphon_source;

pub use camera_source::CameraSource;
#[cfg(feature = "ffmpeg")]
pub use ffmpeg_source::FfmpegSource;
#[cfg(feature = "ffmpeg")]
pub use stream_source::StreamSource;
#[cfg(feature = "hap")]
pub use hap_source::HapSource;
pub use image_source::ImageSource;
#[cfg(feature = "ndi")]
pub use ndi_source::NdiSource;
pub use registry::{Registry, SourceEntry, SourceKind};
pub use solid_color_source::SolidColorSource;
#[cfg(target_os = "macos")]
pub use syphon_source::SyphonSource;
pub use watcher::ShaderWatcher;
