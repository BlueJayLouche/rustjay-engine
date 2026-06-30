/// One plane of a planar YUV frame, described by its own stride/size.
#[derive(Debug, Clone)]
pub struct YuvPlane {
    pub data: Vec<u8>, // length == stride * height (FFmpeg linesize padding kept)
    pub stride: u32,
    pub width: u32,
    pub height: u32,
}

/// Pixel payload of a decoded frame. Video decodes to `Yuv420` (planes uploaded
/// to the GPU and converted in a shader); images/text stay `Rgba`.
#[derive(Debug, Clone)]
pub enum FramePixels {
    Rgba(Vec<u8>),
    Yuv420 {
        y: YuvPlane,
        u: YuvPlane,
        v: YuvPlane,
        full_range: bool, // JPEG/full vs MPEG/limited levels
        bt709: bool,      // BT.709 vs BT.601 matrix
    },
}

/// A decoded video frame ready for GPU upload.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pts: f64, // seconds
    pub pixels: FramePixels,
}

impl VideoFrame {
    /// RGBA8 frame (images, text cues, tests).
    pub fn new(width: u32, height: u32, data: Vec<u8>, pts: f64) -> Self {
        debug_assert_eq!(data.len(), (width * height * 4) as usize);
        Self { width, height, pts, pixels: FramePixels::Rgba(data) }
    }

    /// Planar YUV420 frame (the decode hot path — no CPU colorspace convert).
    #[allow(clippy::too_many_arguments)]
    pub fn yuv420(
        width: u32,
        height: u32,
        pts: f64,
        y: YuvPlane,
        u: YuvPlane,
        v: YuvPlane,
        full_range: bool,
        bt709: bool,
    ) -> Self {
        Self { width, height, pts, pixels: FramePixels::Yuv420 { y, u, v, full_range, bt709 } }
    }

    /// RGBA bytes if this is an RGBA frame (the CPU canvas-compose path).
    pub fn rgba(&self) -> Option<&[u8]> {
        match &self.pixels {
            FramePixels::Rgba(d) => Some(d),
            FramePixels::Yuv420 { .. } => None,
        }
    }
}
