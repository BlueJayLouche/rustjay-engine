//! Video decode is deferred in v1 — the engine owns video output
//! (NDI/Syphon/Spout/projection) at the video-cue milestone. This stub keeps the
//! `VideoSource` API so the binary compiles without FFmpeg: `open` returns an
//! error, so video cues log-and-skip while audio playback is unaffected.
//!
//! This is the seam where the engine's video decode plugs in later.

use crate::texture::VideoFrame;

pub struct VideoSource;

impl VideoSource {
    // ponytail: video decode intentionally unimplemented in v1 — engine owns it later.
    pub fn open(_path: &str, _dst_width: u32, _dst_height: u32) -> anyhow::Result<Self> {
        anyhow::bail!("video cues are deferred in this build (audio-only)")
    }

    pub fn read_frame(&mut self) -> Option<VideoFrame> {
        None
    }

    pub fn width(&self) -> u32 { 0 }
    pub fn height(&self) -> u32 { 0 }
    pub fn dst_width(&self) -> u32 { 0 }
    pub fn dst_height(&self) -> u32 { 0 }
}
