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
#[cfg(feature = "hap")]
mod hap_source;

pub use camera_source::CameraSource;
#[cfg(feature = "ffmpeg")]
pub use ffmpeg_source::FfmpegSource;
#[cfg(feature = "hap")]
pub use hap_source::HapSource;
pub use image_source::ImageSource;
pub use registry::{Registry, SourceEntry, SourceKind};
pub use solid_color_source::SolidColorSource;
pub use watcher::ShaderWatcher;
