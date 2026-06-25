//! A loaded HAP clip: metadata + in/out points + frame decode.
//!
//! Built directly on hap-wgpu's frame decoder (`QtHapReader` + `HapTexture`) — no
//! `HapPlayer`, no `Arc<Mutex>`. The owning `Pad` drives `current_frame`; this just
//! decodes the requested frame (cached when unchanged) and reports the colour space.
#![allow(dead_code)] // model API — path/set_range consumed by Phase 1b persistence

use std::path::{Path, PathBuf};

use hap_wgpu::{padded_dimensions, HapTexture, QtHapReader, TextureFormat};

/// How the decoded BCn texture must be interpreted before compositing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorSpace {
    /// Sample as-is (Hap1/Hap5/BC7/BC6H).
    Rgb,
    /// YCoCg→RGB convert needed (HapY / HAP Q).
    YcoCg,
}

pub struct Sample {
    pub name: String,
    pub path: PathBuf,
    pub dims: (u32, u32),
    pub padded: (u32, u32),
    pub format: TextureFormat,
    pub frame_count: u32,
    pub fps: f32,
    /// Playback range (frame-accurate, inclusive).
    pub in_point: u32,
    pub out_point: u32,

    reader: QtHapReader,
    last_frame: Option<u32>,
    cached: Option<HapTexture>,
}

impl Sample {
    pub fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let reader = QtHapReader::open(&path)?;
        let dims = reader.resolution();
        let frame_count = reader.frame_count();
        let format = reader.texture_format();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip")
            .to_string();
        Ok(Self {
            name,
            path,
            dims,
            padded: padded_dimensions(dims.0, dims.1),
            format,
            frame_count,
            fps: reader.fps().max(1.0),
            in_point: 0,
            out_point: frame_count.saturating_sub(1),
            reader,
            last_frame: None,
            cached: None,
        })
    }

    pub fn color_space(&self) -> ColorSpace {
        if self.format == TextureFormat::YcoCgDxt5 {
            ColorSpace::YcoCg
        } else {
            ColorSpace::Rgb
        }
    }

    /// Crop the padded (multiple-of-4) BC texture back to the real image.
    pub fn uv_scale(&self) -> [f32; 2] {
        [
            self.dims.0 as f32 / self.padded.0.max(1) as f32,
            self.dims.1 as f32 / self.padded.1.max(1) as f32,
        ]
    }

    pub fn set_range(&mut self, in_point: u32, out_point: u32) {
        let max = self.frame_count.saturating_sub(1);
        self.in_point = in_point.min(max);
        self.out_point = out_point.min(max);
        if self.in_point > self.out_point {
            std::mem::swap(&mut self.in_point, &mut self.out_point);
        }
    }

    /// Decode `frame` (clamped to in/out) to a GPU texture, cached if unchanged.
    pub fn frame_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: u32,
    ) -> Option<&HapTexture> {
        let frame = frame.clamp(self.in_point, self.out_point);
        if self.last_frame != Some(frame) || self.cached.is_none() {
            match self.reader.read_frame(frame) {
                Ok(hf) => {
                    self.cached = Some(HapTexture::from_dxt_data(
                        device,
                        queue,
                        self.padded.0,
                        self.padded.1,
                        hf.format,
                        &hf.data,
                        frame,
                    ));
                    self.last_frame = Some(frame);
                }
                Err(e) => log::warn!("Sample '{}' read_frame {frame}: {e}", self.name),
            }
        }
        self.cached.as_ref()
    }
}
