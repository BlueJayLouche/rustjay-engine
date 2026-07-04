use ffmpeg_next::{codec, color, format, frame, media::Type, software::scaling, threading};
use crate::frame::{BitDepth, ChromaSubsample, VideoFrame, YuvPlane};

/// Wraps an FFmpeg video stream decoder and produces `VideoFrame`s.
pub struct VideoSource {
    ictx: format::context::Input,
    decoder: codec::decoder::Video,
    stream_index: usize,
    time_base: f64,
    /// `None` for GPU-path YUV sources; `Some` when the pixel format needs a CPU
    /// convert to RGBA (the universal swscale fallback).
    scaler: Option<scaling::Context>,
    width: u32,
    height: u32,
    dst_width: u32,
    dst_height: u32,
    decoded_frame: frame::Video,
    rgb_frame: frame::Video,
    eof: bool,
}

/// Formats that upload straight to the GPU and convert in-shader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuYuvFormat {
    Yuv420,
    Yuv422,
    Yuv444,
    Nv12,
    Yuv420P10,
}

/// Single source of truth for "GPU path vs swscale fallback".
fn gpu_format_class(fmt: format::Pixel) -> Option<GpuYuvFormat> {
    use format::Pixel;
    match fmt {
        Pixel::YUV420P | Pixel::YUVJ420P => Some(GpuYuvFormat::Yuv420),
        Pixel::YUV422P | Pixel::YUVJ422P => Some(GpuYuvFormat::Yuv422),
        Pixel::YUV444P | Pixel::YUVJ444P => Some(GpuYuvFormat::Yuv444),
        Pixel::NV12 => Some(GpuYuvFormat::Nv12),
        Pixel::YUV420P10LE => Some(GpuYuvFormat::Yuv420P10),
        _ => None,
    }
}

fn plane(frame: &frame::Video, i: usize) -> YuvPlane {
    YuvPlane {
        data: frame.data(i).to_vec(),
        stride: frame.stride(i) as u32,
        width: frame.plane_width(i),
        height: frame.plane_height(i),
    }
}

fn is_full_range(frame: &frame::Video) -> bool {
    use format::Pixel;
    matches!(frame.color_range(), color::Range::JPEG)
        || matches!(
            frame.format(),
            Pixel::YUVJ420P | Pixel::YUVJ422P | Pixel::YUVJ444P
        )
}

fn is_bt709(frame: &frame::Video, height: u32) -> bool {
    match frame.color_space() {
        color::Space::BT709 => true,
        color::Space::BT470BG | color::Space::SMPTE170M => false,
        // Unspecified: HD heuristic (>576 lines ⇒ BT.709).
        _ => height > 576,
    }
}

impl VideoSource {
    /// Open a video file and initialise the decoder (+ a CPU scaler only if the
    /// pixel format is not handled natively by the GPU converter).
    ///
    /// Frames are produced at the source's **native resolution**; aspect-ratio
    /// fitting is the canvas's job, so forcing a fixed size here would pre-stretch
    /// non-matching sources.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        ffmpeg_next::init()?;

        let ictx = format::input(path)?;
        let input = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream found"))?;
        let stream_index = input.index();
        let time_base = f64::from(input.time_base());

        let mut context = codec::Context::from_parameters(input.parameters())?;
        // Frame-parallel decoding (count 0 = auto / one per core). Single-threaded
        // decode can't sustain large frames at high fps (e.g. 5400x1080@50), which
        // shows up as dropped frames downstream.
        context.set_threading(threading::Config::kind(threading::Type::Frame));
        let mut decoder = context.decoder().video()?;
        decoder.set_parameters(input.parameters())?;

        let width = decoder.width();
        let height = decoder.height();
        let (dst_width, dst_height) = (width, height);

        // GPU-native YUV → no scaler. Everything else → RGBA via swscale fallback.
        let scaler = if gpu_format_class(decoder.format()).is_some() {
            None
        } else {
            Some(scaling::Context::get(
                decoder.format(),
                width,
                height,
                format::Pixel::RGBA,
                dst_width,
                dst_height,
                scaling::Flags::BILINEAR,
            )?)
        };

        let decoded_frame = frame::Video::empty();
        let mut rgb_frame = frame::Video::empty();
        rgb_frame.set_format(format::Pixel::RGBA);
        rgb_frame.set_width(dst_width);
        rgb_frame.set_height(dst_height);

        Ok(Self {
            ictx,
            decoder,
            stream_index,
            time_base,
            scaler,
            width,
            height,
            dst_width,
            dst_height,
            decoded_frame,
            rgb_frame,
            eof: false,
        })
    }

    /// Build a `VideoFrame` from the currently-decoded frame.
    fn convert_current(&mut self) -> Option<VideoFrame> {
        let pts = self.decoded_frame.timestamp().unwrap_or(0) as f64 * self.time_base;

        if self.scaler.is_none() {
            let full_range = is_full_range(&self.decoded_frame);
            let bt709 = is_bt709(&self.decoded_frame, self.height);
            let fmt = gpu_format_class(self.decoded_frame.format())?;
            return Some(match fmt {
                GpuYuvFormat::Nv12 => VideoFrame::nv12(
                    self.dst_width,
                    self.dst_height,
                    pts,
                    plane(&self.decoded_frame, 0),
                    plane(&self.decoded_frame, 1),
                    full_range,
                    bt709,
                ),
                GpuYuvFormat::Yuv420P10 => VideoFrame::yuv_planar(
                    self.dst_width,
                    self.dst_height,
                    pts,
                    ChromaSubsample::Cs420,
                    BitDepth::B10,
                    plane(&self.decoded_frame, 0),
                    plane(&self.decoded_frame, 1),
                    plane(&self.decoded_frame, 2),
                    full_range,
                    bt709,
                ),
                GpuYuvFormat::Yuv420 => VideoFrame::yuv_planar(
                    self.dst_width,
                    self.dst_height,
                    pts,
                    ChromaSubsample::Cs420,
                    BitDepth::B8,
                    plane(&self.decoded_frame, 0),
                    plane(&self.decoded_frame, 1),
                    plane(&self.decoded_frame, 2),
                    full_range,
                    bt709,
                ),
                GpuYuvFormat::Yuv422 => VideoFrame::yuv_planar(
                    self.dst_width,
                    self.dst_height,
                    pts,
                    ChromaSubsample::Cs422,
                    BitDepth::B8,
                    plane(&self.decoded_frame, 0),
                    plane(&self.decoded_frame, 1),
                    plane(&self.decoded_frame, 2),
                    full_range,
                    bt709,
                ),
                GpuYuvFormat::Yuv444 => VideoFrame::yuv_planar(
                    self.dst_width,
                    self.dst_height,
                    pts,
                    ChromaSubsample::Cs444,
                    BitDepth::B8,
                    plane(&self.decoded_frame, 0),
                    plane(&self.decoded_frame, 1),
                    plane(&self.decoded_frame, 2),
                    full_range,
                    bt709,
                ),
            });
        }

        let scaler = self.scaler.as_mut().unwrap();
        scaler.run(&self.decoded_frame, &mut self.rgb_frame).ok()?;
        let data = self.rgb_frame.data(0).to_vec();
        Some(VideoFrame::new(self.dst_width, self.dst_height, data, pts))
    }

    /// Read the next frame and return it with PTS in seconds. `None` at EOF.
    pub fn read_frame(&mut self) -> Option<VideoFrame> {
        if self.eof {
            return None;
        }

        // Try draining already-decoded frames first.
        if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            return self.convert_current();
        }

        for (stream, packet) in self.ictx.packets() {
            if stream.index() == self.stream_index {
                if self.decoder.send_packet(&packet).is_err() {
                    continue;
                }
                if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                    return self.convert_current();
                }
            }
        }

        // Flush decoder
        let _ = self.decoder.send_eof();
        if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            self.eof = true;
            return self.convert_current();
        }
        self.eof = true;
        None
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn dst_width(&self) -> u32 { self.dst_width }
    pub fn dst_height(&self) -> u32 { self.dst_height }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_format_class_maps_native_and_fallback_formats() {
        use format::Pixel;
        assert!(gpu_format_class(Pixel::YUV420P).is_some());
        assert!(gpu_format_class(Pixel::YUVJ420P).is_some());
        assert!(gpu_format_class(Pixel::YUV422P).is_some());
        assert!(gpu_format_class(Pixel::YUVJ422P).is_some());
        assert!(gpu_format_class(Pixel::YUV444P).is_some());
        assert!(gpu_format_class(Pixel::YUVJ444P).is_some());
        assert!(gpu_format_class(Pixel::NV12).is_some());
        assert!(gpu_format_class(Pixel::YUV420P10LE).is_some());

        // Long-tail / packed / RGB / alpha formats fall back to swscale.
        assert!(gpu_format_class(Pixel::YUYV422).is_none());
        assert!(gpu_format_class(Pixel::UYVY422).is_none());
        assert!(gpu_format_class(Pixel::RGB24).is_none());
        assert!(gpu_format_class(Pixel::BGRA).is_none());
        assert!(gpu_format_class(Pixel::YUV420P16LE).is_none());
    }
}
