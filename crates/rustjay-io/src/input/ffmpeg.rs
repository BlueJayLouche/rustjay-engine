//! ffmpeg video file decoder.
//!
//! Opens video files via `ffmpeg-next`, decodes frames synchronously, and
//! converts them to RGBA for GPU upload. Supports loop/ping-pong/one-shot,
//! variable speed, scrubbing, and in/out points.
//!
//! # Known limitations
//! - Synchronous software decode on the caller thread. High-resolution files
//!   may drop frames; hardware acceleration is a future optimization.
//! - Seeking lands on the nearest keyframe before the target, then decodes
//!   forward. Random-access scrub is usable but not instant.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use ffmpeg::format::{input, Pixel};
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;
use ffmpeg_next as ffmpeg;

/// One decoded RGBA frame.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// RGBA pixel data, row-major.
    pub data: Vec<u8>,
}

/// How playback behaves when it reaches the out point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    /// Stop at the out point.
    None,
    /// Jump back to the in point.
    Loop,
    /// Reverse direction at boundaries.
    PingPong,
}

/// Playback direction for ping-pong mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PingPongDir {
    /// Playing forward.
    Forward,
    /// Playing backward.
    Backward,
}

/// ffmpeg-backed video file decoder.
pub struct FfmpegDecoder {
    path: PathBuf,
    width: u32,
    height: u32,
    fps: f32,
    duration: f64,
    frame_count: u32,

    // Playback state
    playing: bool,
    speed: f32,
    loop_mode: LoopMode,
    position: f64,  // seconds
    in_point: f64,  // seconds
    out_point: f64, // seconds
    ping_pong_dir: PingPongDir,

    // Active decode context (lazy-initialized)
    context: Option<DecodeContext>,

    // Cached last frame + pts
    last_frame: Option<VideoFrame>,
    last_pts: i64,
    last_decode_time: Option<Instant>,
}

struct DecodeContext {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::codec::decoder::Video,
    scaler: ffmpeg::software::scaling::Context,
    stream_index: usize,
    time_base: f64,
}

// ffmpeg-next's internal raw pointers are not Send by default, but the decoder
// is only ever accessed from a single thread (the render thread). Marking Send
// is required to satisfy `EffectInstance: Send` in the engine.
unsafe impl Send for DecodeContext {}
unsafe impl Send for FfmpegDecoder {}

impl FfmpegDecoder {
    /// Open a video file and read its metadata.
    ///
    /// Does not allocate the decoder until the first `decode_frame()` call.
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        // Probe the file for metadata.
        let ictx = input(path)?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("No video stream found in {}", path.display()))?;

        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context.decoder().video()?;

        let width = decoder.width();
        let height = decoder.height();

        let fps = stream.rate();
        let fps = fps.numerator() as f32 / fps.denominator().max(1) as f32;

        let duration_secs = ictx.duration() as f64 / 1_000_000.0;
        let duration = if duration_secs > 0.0 {
            duration_secs
        } else {
            // Fallback: estimate from stream duration.
            stream.duration() as f64 * stream.time_base().numerator() as f64
                / stream.time_base().denominator().max(1) as f64
        };

        let frame_count = if fps > 0.0 && duration > 0.0 {
            (fps * duration as f32) as u32
        } else {
            0
        };

        drop(decoder);
        drop(ictx);

        Ok(Self {
            path: path.to_path_buf(),
            width,
            height,
            fps,
            duration,
            frame_count,
            playing: true,
            speed: 1.0,
            loop_mode: LoopMode::Loop,
            position: 0.0,
            in_point: 0.0,
            out_point: duration,
            ping_pong_dir: PingPongDir::Forward,
            context: None,
            last_frame: None,
            last_pts: -1,
            last_decode_time: None,
        })
    }

    // ------------------------------------------------------------------
    // Playback controls
    // ------------------------------------------------------------------

    /// Start or resume playback.
    pub fn play(&mut self) {
        self.playing = true;
        self.last_decode_time = Some(Instant::now());
    }

    /// Pause playback at the current position.
    pub fn pause(&mut self) {
        self.playing = false;
        self.last_decode_time = None;
    }

    /// Stop playback and reset to the in point.
    pub fn stop(&mut self) {
        self.pause();
        self.position = self.in_point;
        self.ping_pong_dir = PingPongDir::Forward;
        self.last_pts = -1;
        self.context = None;
        self.last_frame = None;
    }

    /// Set playback speed multiplier (clamped to ≥ 0).
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.max(0.0);
    }

    /// Set loop mode.
    pub fn set_loop_mode(&mut self, mode: LoopMode) {
        self.loop_mode = mode;
        if mode != LoopMode::PingPong {
            self.ping_pong_dir = PingPongDir::Forward;
        }
    }

    /// Seek to a normalized position 0.0–1.0 (relative to in/out points).
    pub fn seek_to(&mut self, position: f64) {
        let t = position.clamp(0.0, 1.0);
        let range = self.out_point - self.in_point;
        self.position = self.in_point + t * range;
        self.last_pts = -1;
        // Context will seek on next decode.
    }

    /// Set the in point in seconds (clamped to the file duration).
    pub fn set_in_point(&mut self, t: f64) {
        self.in_point = t.clamp(0.0, self.duration);
        if self.in_point > self.out_point {
            self.out_point = self.in_point;
        }
        if self.position < self.in_point {
            self.position = self.in_point;
            self.last_pts = -1;
        }
    }

    /// Set the out point in seconds (clamped to the file duration).
    pub fn set_out_point(&mut self, t: f64) {
        self.out_point = t.clamp(0.0, self.duration);
        if self.out_point < self.in_point {
            self.in_point = self.out_point;
        }
        if self.position > self.out_point {
            self.position = self.out_point;
            self.last_pts = -1;
        }
    }

    // ------------------------------------------------------------------
    // Getters
    // ------------------------------------------------------------------

    /// Video width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }
    /// Video height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }
    /// Frame rate in frames per second.
    pub fn fps(&self) -> f32 {
        self.fps
    }
    /// Total duration in seconds.
    pub fn duration(&self) -> f64 {
        self.duration
    }
    /// Estimated total frame count.
    pub fn frame_count(&self) -> u32 {
        self.frame_count
    }
    /// Whether playback is currently active.
    pub fn is_playing(&self) -> bool {
        self.playing
    }
    /// Current playback position in seconds.
    pub fn position(&self) -> f64 {
        self.position
    }
    /// Current loop mode.
    pub fn loop_mode(&self) -> LoopMode {
        self.loop_mode
    }
    /// In point of the playback region in seconds.
    pub fn in_point(&self) -> f64 {
        self.in_point
    }
    /// Out point of the playback region in seconds.
    pub fn out_point(&self) -> f64 {
        self.out_point
    }

    // ------------------------------------------------------------------
    // Decode
    // ------------------------------------------------------------------

    /// Decode and return the frame for the current playback position.
    ///
    /// Advances playback time when `playing == true`. Returns the last
    /// decoded frame when paused.
    pub fn decode_frame(&mut self) -> Option<VideoFrame> {
        if self.context.is_none() {
            if let Err(e) = self.init_context() {
                log::warn!(
                    "FfmpegDecoder failed to init context for {}: {}",
                    self.path.display(),
                    e
                );
                return self.last_frame.clone();
            }
        }

        if !self.playing {
            return self.last_frame.clone();
        }

        // Advance position by elapsed time.
        let now = Instant::now();
        if let Some(last) = self.last_decode_time {
            let elapsed = last.elapsed().as_secs_f64();
            if self.loop_mode == LoopMode::PingPong {
                let dir = match self.ping_pong_dir {
                    PingPongDir::Forward => 1.0,
                    PingPongDir::Backward => -1.0,
                };
                self.position += elapsed * self.speed as f64 * dir;
                if self.position >= self.out_point {
                    self.position = self.out_point;
                    self.ping_pong_dir = PingPongDir::Backward;
                } else if self.position <= self.in_point {
                    self.position = self.in_point;
                    self.ping_pong_dir = PingPongDir::Forward;
                }
            } else {
                self.position += elapsed * self.speed as f64;
                if self.position >= self.out_point {
                    match self.loop_mode {
                        LoopMode::None => {
                            self.position = self.out_point;
                            self.playing = false;
                        }
                        LoopMode::Loop => {
                            self.position = self.in_point;
                            self.last_pts = -1;
                        }
                        LoopMode::PingPong => unreachable!(),
                    }
                }
            }
        }
        self.last_decode_time = Some(now);

        // Decode the frame at the current position.
        match self.decode_at_position(self.position) {
            Ok(frame) => {
                self.last_frame = Some(frame.clone());
                Some(frame)
            }
            Err(e) => {
                log::warn!("FfmpegDecoder decode error: {}", e);
                self.last_frame.clone()
            }
        }
    }

    fn init_context(&mut self) -> anyhow::Result<()> {
        let ictx = input(&self.path)?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("No video stream"))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let time_base = time_base.numerator() as f64 / time_base.denominator().max(1) as f64;

        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context.decoder().video()?;

        let scaler = Context::get(
            decoder.format(),
            decoder.width(),
            decoder.height(),
            Pixel::RGBA,
            decoder.width(),
            decoder.height(),
            Flags::BILINEAR,
        )?;

        self.context = Some(DecodeContext {
            input: ictx,
            decoder,
            scaler,
            stream_index,
            time_base,
        });
        Ok(())
    }

    fn decode_at_position(&mut self, position_secs: f64) -> anyhow::Result<VideoFrame> {
        let ctx = self
            .context
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("Decoder not initialized"))?;

        // Convert target position to stream timestamp units.
        let target_ts = (position_secs / ctx.time_base) as i64;

        // Seek if we're before the last decoded frame or far ahead.
        let needs_seek = self.last_pts < 0
            || target_ts < self.last_pts
            || target_ts > self.last_pts + (1.0 / ctx.time_base) as i64 * 2;

        if needs_seek {
            // Seek to a keyframe at or before the target.
            let seek_ts = (target_ts as f64 * ctx.time_base / 1.0) as i64;
            if let Err(e) = ctx.input.seek(seek_ts, ..) {
                log::warn!("FfmpegDecoder seek failed: {}", e);
            }
            ctx.decoder.flush();
            self.last_pts = -1;
        }

        let mut decoded = Video::empty();
        let mut rgba_frame = Video::empty();

        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut ctx.input) {
                Ok(()) => {
                    if packet.stream() != ctx.stream_index {
                        continue;
                    }
                    ctx.decoder.send_packet(&packet)?;
                    while ctx.decoder.receive_frame(&mut decoded).is_ok() {
                        let pts = decoded.timestamp().unwrap_or(-1);
                        if pts >= target_ts || self.last_pts < 0 {
                            ctx.scaler.run(&decoded, &mut rgba_frame)?;
                            self.last_pts = pts;
                            return Ok(VideoFrame {
                                width: rgba_frame.width(),
                                height: rgba_frame.height(),
                                data: rgba_frame.data(0).to_vec(),
                            });
                        }
                        self.last_pts = pts;
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    // End of file: drain decoder.
                    ctx.decoder.send_eof()?;
                    if ctx.decoder.receive_frame(&mut decoded).is_ok() {
                        ctx.scaler.run(&decoded, &mut rgba_frame)?;
                        return Ok(VideoFrame {
                            width: rgba_frame.width(),
                            height: rgba_frame.height(),
                            data: rgba_frame.data(0).to_vec(),
                        });
                    }
                    return Err(anyhow::anyhow!("EOF reached without finding target frame"));
                }
                Err(e) => {
                    // Skip corrupt packets.
                    log::debug!("FfmpegDecoder packet read error: {}", e);
                    continue;
                }
            }
        }
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        // Context drops automatically.
    }
}
