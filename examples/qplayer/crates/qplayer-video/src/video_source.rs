use ffmpeg_next::{codec, color, format, frame, media::Type, software::scaling, threading};
use crate::frame::{VideoFrame, YuvPlane};

/// Wraps an FFmpeg video stream decoder and produces `VideoFrame`s.
pub struct VideoSource {
    ictx: format::context::Input,
    decoder: codec::decoder::Video,
    stream_index: usize,
    time_base: f64,
    /// `None` for planar-YUV sources (uploaded straight to the GPU); `Some` only
    /// when the source pixel format needs a CPU convert to RGBA (rare fallback).
    scaler: Option<scaling::Context>,
    width: u32,
    height: u32,
    dst_width: u32,
    dst_height: u32,
    decoded_frame: frame::Video,
    rgb_frame: frame::Video,
    eof: bool,
}

/// 8-bit planar YUV 4:2:0 — the common H.264/HEVC decode output. These upload as
/// three R8 textures and convert in the shader, so they skip swscale entirely.
fn is_yuv420p(fmt: format::Pixel) -> bool {
    matches!(fmt, format::Pixel::YUV420P | format::Pixel::YUVJ420P)
}

fn plane(frame: &frame::Video, i: usize) -> YuvPlane {
    YuvPlane {
        data: frame.data(i).to_vec(),
        stride: frame.stride(i) as u32,
        width: frame.plane_width(i),
        height: frame.plane_height(i),
    }
}

impl VideoSource {
    /// Open a video file and initialise the decoder (+ a CPU scaler only if the
    /// pixel format is not planar YUV420).
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

        // YUV420 → GPU shader convert (no scaler). Anything else → RGBA via swscale.
        let scaler = if is_yuv420p(decoder.format()) {
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
            // Planar YUV420: hand the three planes to the GPU as-is.
            let full_range = matches!(self.decoded_frame.color_range(), color::Range::JPEG)
                || matches!(self.decoded_frame.format(), format::Pixel::YUVJ420P);
            let bt709 = match self.decoded_frame.color_space() {
                color::Space::BT709 => true,
                color::Space::BT470BG | color::Space::SMPTE170M => false,
                // Unspecified: HD heuristic (>576 lines ⇒ BT.709).
                _ => self.height > 576,
            };
            return Some(VideoFrame::yuv420(
                self.dst_width,
                self.dst_height,
                pts,
                plane(&self.decoded_frame, 0),
                plane(&self.decoded_frame, 1),
                plane(&self.decoded_frame, 2),
                full_range,
                bt709,
            ));
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
