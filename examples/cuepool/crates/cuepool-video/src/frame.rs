/// One plane of a planar YUV frame, described by its own stride/size.
/// For 10-bit formats the data is the raw little-endian `u16` bytes and
/// `stride` is still in bytes.
#[derive(Debug, Clone)]
pub struct YuvPlane {
    pub data: Vec<u8>, // length == stride * height (FFmpeg linesize padding kept)
    pub stride: u32,
    pub width: u32,
    pub height: u32,
}

/// Chroma subsampling of a planar YUV frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaSubsample {
    Cs420,
    Cs422,
    Cs444,
}

/// Bit depth of a planar YUV frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitDepth {
    B8,
    B10,
}

/// Pixel payload of a decoded frame. GPU-path YUV variants are uploaded straight
/// to the converter; images/text stay `Rgba`; everything else is converted to
/// `Rgba` on the CPU by swscale before reaching here.
#[derive(Debug, Clone)]
pub enum FramePixels {
    Rgba(Vec<u8>),
    /// Decompressed BC1/BC3 blocks from a HAP packet. The dimensions on
    /// `VideoFrame` remain the logical image size; the upload is padded to
    /// complete 4x4 blocks.
    Hap {
        format: hap_parser::TextureFormat,
        data: Vec<u8>,
        padded_width: u32,
        padded_height: u32,
    },
    YuvPlanar {
        subsample: ChromaSubsample,
        bit_depth: BitDepth,
        y: YuvPlane,
        u: YuvPlane,
        v: YuvPlane,
        full_range: bool, // JPEG/full vs MPEG/limited levels
        bt709: bool,      // BT.709 vs BT.601 matrix
    },
    Nv12 {
        y: YuvPlane,
        uv: YuvPlane,
        full_range: bool,
        bt709: bool,
    },
    #[cfg(windows)]
    D3d12Nv12(crate::D3d12Frame),
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
        Self {
            width,
            height,
            pts,
            pixels: FramePixels::Rgba(data),
        }
    }

    pub(crate) fn hap(
        width: u32,
        height: u32,
        pts: f64,
        format: hap_parser::TextureFormat,
        data: Vec<u8>,
    ) -> Self {
        let padded_width = width.div_ceil(4) * 4;
        let padded_height = height.div_ceil(4) * 4;
        debug_assert_eq!(data.len(), format.frame_size(width, height));
        Self {
            width,
            height,
            pts,
            pixels: FramePixels::Hap {
                format,
                data,
                padded_width,
                padded_height,
            },
        }
    }

    /// Planar YUV frame (any 4:2:0/4:2:2/4:4:4, 8 or 10-bit — GPU path).
    #[allow(clippy::too_many_arguments)]
    pub fn yuv_planar(
        width: u32,
        height: u32,
        pts: f64,
        subsample: ChromaSubsample,
        bit_depth: BitDepth,
        y: YuvPlane,
        u: YuvPlane,
        v: YuvPlane,
        full_range: bool,
        bt709: bool,
    ) -> Self {
        Self {
            width,
            height,
            pts,
            pixels: FramePixels::YuvPlanar {
                subsample,
                bit_depth,
                y,
                u,
                v,
                full_range,
                bt709,
            },
        }
    }

    /// NV12 semi-planar frame (Y + interleaved UV — GPU path).
    pub fn nv12(
        width: u32,
        height: u32,
        pts: f64,
        y: YuvPlane,
        uv: YuvPlane,
        full_range: bool,
        bt709: bool,
    ) -> Self {
        Self {
            width,
            height,
            pts,
            pixels: FramePixels::Nv12 {
                y,
                uv,
                full_range,
                bt709,
            },
        }
    }

    #[cfg(windows)]
    pub(crate) fn d3d12_nv12(width: u32, height: u32, pts: f64, frame: crate::D3d12Frame) -> Self {
        Self {
            width,
            height,
            pts,
            pixels: FramePixels::D3d12Nv12(frame),
        }
    }

    #[cfg(windows)]
    pub fn d3d12_handoff(&self) -> Option<crate::D3d12Handoff> {
        match &self.pixels {
            FramePixels::D3d12Nv12(frame) => Some(frame.handoff()),
            _ => None,
        }
    }

    /// RGBA bytes if this is an RGBA frame (the CPU canvas-compose path).
    pub fn rgba(&self) -> Option<&[u8]> {
        match &self.pixels {
            FramePixels::Rgba(d) => Some(d),
            _ => None,
        }
    }
}
