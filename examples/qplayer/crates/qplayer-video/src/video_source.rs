use ffmpeg_next::{codec, format, frame, media::Type, software::scaling};
use crate::texture::VideoFrame;

/// Wraps an FFmpeg video stream decoder and produces `VideoFrame`s.
pub struct VideoSource {
    ictx: format::context::Input,
    decoder: codec::decoder::Video,
    stream_index: usize,
    time_base: f64,
    scaler: scaling::Context,
    width: u32,
    height: u32,
    dst_width: u32,
    dst_height: u32,
    decoded_frame: frame::Video,
    rgb_frame: frame::Video,
    eof: bool,
}

impl VideoSource {
    /// Open a video file and initialise the decoder + scaler.
    pub fn open(path: &str, dst_width: u32, dst_height: u32) -> anyhow::Result<Self> {
        ffmpeg_next::init()?;

        let ictx = format::input(path)?;
        let input = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("no video stream found"))?;
        let stream_index = input.index();
        let time_base = f64::from(input.time_base());

        let context = codec::Context::from_parameters(input.parameters())?;
        let mut decoder = context.decoder().video()?;
        decoder.set_parameters(input.parameters())?;

        let width = decoder.width();
        let height = decoder.height();

        let scaler = scaling::Context::get(
            decoder.format(),
            width,
            height,
            format::Pixel::RGBA,
            dst_width,
            dst_height,
            scaling::Flags::BILINEAR,
        )?;

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

    /// Read the next frame, converting to RGBA and returning it with PTS in seconds.
    /// Returns `None` at EOF.
    pub fn read_frame(&mut self) -> Option<VideoFrame> {
        if self.eof {
            return None;
        }

        // Try draining already-decoded frames first.
        match self.decoder.receive_frame(&mut self.decoded_frame) {
            Ok(()) => {
                self.scaler.run(&self.decoded_frame, &mut self.rgb_frame).ok()?;
                let pts = self.decoded_frame.timestamp().unwrap_or(0) as f64 * self.time_base;
                let data = self.rgb_frame.data(0).to_vec();
                return Some(VideoFrame::new(self.dst_width, self.dst_height, data, pts));
            }
            Err(_) => {}
        }

        for (stream, packet) in self.ictx.packets() {
            if stream.index() == self.stream_index {
                if self.decoder.send_packet(&packet).is_err() {
                    continue;
                }
                match self.decoder.receive_frame(&mut self.decoded_frame) {
                    Ok(()) => {
                        self.scaler.run(&self.decoded_frame, &mut self.rgb_frame).ok()?;
                        let pts = self.decoded_frame.timestamp().unwrap_or(0) as f64 * self.time_base;
                        let data = self.rgb_frame.data(0).to_vec();
                        return Some(VideoFrame::new(self.dst_width, self.dst_height, data, pts));
                    }
                    Err(_) => {}
                }
            }
        }

        // Flush decoder
        let _ = self.decoder.send_eof();
        match self.decoder.receive_frame(&mut self.decoded_frame) {
            Ok(()) => {
                self.scaler.run(&self.decoded_frame, &mut self.rgb_frame).ok()?;
                let pts = self.decoded_frame.timestamp().unwrap_or(0) as f64 * self.time_base;
                let data = self.rgb_frame.data(0).to_vec();
                self.eof = true;
                Some(VideoFrame::new(self.dst_width, self.dst_height, data, pts))
            }
            Err(_) => {
                self.eof = true;
                None
            }
        }
    }

    pub fn width(&self) -> u32 { self.width }
    pub fn height(&self) -> u32 { self.height }
    pub fn dst_width(&self) -> u32 { self.dst_width }
    pub fn dst_height(&self) -> u32 { self.dst_height }
}
