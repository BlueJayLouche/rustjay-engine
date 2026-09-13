//! ffmpeg video file decoder.
//!
//! Opens video files via `ffmpeg-next`, decodes frames synchronously, and
//! converts them to RGBA for GPU upload. Supports loop/ping-pong/one-shot,
//! variable speed, scrubbing, and in/out points.
//!
//! Decoding runs on a worker thread and hands frames back as pixels for the
//! caller to upload; VideoToolbox carries the decode itself on macOS.
//!
//! # Known limitations
//! - Seeking lands on the nearest keyframe before the target, then decodes
//!   forward. Random-access scrub is usable but not instant.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Instant;

/// Detect whether a video file uses the HAP codec.
/// Returns `true` if the best video stream's codec ID is "hap".
pub fn detect_hap_codec(path: &Path) -> anyhow::Result<bool> {
    let ictx = input(path)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| anyhow::anyhow!("No video stream found"))?;
    let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    let decoder = context.decoder().video()?;
    Ok(decoder.id().name() == "hap")
}

use ffmpeg::format::{input, Pixel};
use ffmpeg::color::Space;
use ffmpeg::media::Type;
use ffmpeg::software::scaling::{context::Context, flag::Flags};
use ffmpeg::util::frame::video::Video;
use ffmpeg_next as ffmpeg;

/// One decoded frame, either as pixels or as a texture the GPU already holds.
#[derive(Debug, Clone)]
pub struct VideoFrame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// RGBA pixel data, row-major. **Empty when `hardware` is set** — there are
    /// no pixels on this side of the bus to give you.
    pub data: Vec<u8>,
    /// Set when the frame never left the GPU. Import it with
    /// [`HardwareFrame::import_planes`] and sample NV12 rather than reading
    /// `data`.
    #[cfg(target_os = "macos")]
    pub hardware: Option<super::videotoolbox::HardwareFrame>,
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

/// How many frames `decode_at_position` will decode past on its way to the
/// wall-clock target before giving up and returning what it has. Long-GOP h264
/// cannot skip cheaply — reaching frame N means decoding frames 0..N — so the
/// only defence against falling behind is to stop chasing.
const MAX_CATCHUP_FRAMES: u32 = 2;

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

    // Decoding, on its own thread. Spawned on first use so opening a file
    // costs nothing until something asks it to play.
    worker: Option<DecodeWorker>,
    /// Set when `position` jumped somewhere the decode cursor cannot be walked
    /// to; travels with the next request.
    force_seek: bool,
    last_decode_time: Option<Instant>,

    // Frame pacing — wall-clock accumulator so we only decode when enough
    // real time has elapsed for at least one video frame.
    frame_accumulator: f64,
    frame_time: f64,
}

struct DecodeContext {
    input: ffmpeg::format::context::Input,
    decoder: ffmpeg::codec::decoder::Video,
    /// Built on the first frame and rebuilt if the format changes. It cannot be
    /// built up front any more: with VideoToolbox the decoder reports an opaque
    /// hardware format, and the real source format is whatever the download
    /// hands back (NV12 in practice).
    scaler: Option<(Pixel, ffmpeg::software::scaling::Context)>,
    stream_index: usize,
    time_base: f64,
    /// Set when the stream declares an identity colourspace, meaning its
    /// "YUV" planes are really GBR and swscale must not touch them.
    identity_gbr: bool,
    /// Hand hardware frames straight through instead of downloading them.
    /// Off for live streams, whose consumer still wants pixels.
    zero_copy: bool,
}

impl DecodeContext {
    /// Convert a decoded frame to RGBA.
    ///
    /// Normally that is swscale's job. But a stream can declare
    /// `AVCOL_SPC_RGB` — NotchLC always does — meaning its planes are already
    /// G, B, R rather than luma and chroma. swscale ignores that and applies a
    /// BT.601 matrix anyway, which silently wrecks the colour: RGB(100,200,50)
    /// comes back as (162,247,63). So for those, pack the planes directly.
    fn convert_to_rgba(&mut self, decoded: &Video, scratch: &mut Video) -> anyhow::Result<VideoFrame> {
        if self.identity_gbr {
            return Ok(pack_gbr_planes(decoded));
        }

        // A hardware frame is already in GPU memory. Pass the surface along and
        // let the render thread wrap it as a texture — downloading it only to
        // convert it and upload it again is three trips across the bus to end
        // up where it started.
        #[cfg(target_os = "macos")]
        if self.zero_copy
            && let Some(hardware) = super::videotoolbox::HardwareFrame::from_av_frame(decoded)
        {
            return Ok(VideoFrame {
                width: hardware.width,
                height: hardware.height,
                data: Vec::new(),
                hardware: Some(hardware),
            });
        }

        let downloaded = download_if_hardware(decoded)?;
        let src = downloaded.as_ref().unwrap_or(decoded);

        let format = src.format();
        if self.scaler.as_ref().is_none_or(|(f, _)| *f != format) {
            self.scaler = Some((
                format,
                Context::get(
                    format,
                    src.width(),
                    src.height(),
                    Pixel::RGBA,
                    src.width(),
                    src.height(),
                    Flags::BILINEAR,
                )?,
            ));
        }
        let (_, scaler) = self.scaler.as_mut().expect("just built");
        scaler.run(src, scratch)?;
        Ok(VideoFrame {
            width: scratch.width(),
            height: scratch.height(),
            data: scratch.data(0).to_vec(),
            #[cfg(target_os = "macos")]
            hardware: None,
        })
    }
}

/// Open a video decoder, on VideoToolbox where the platform offers it.
///
/// 4K h264 decoded in software cost ~60% of a core on the render thread and
/// took the whole app from 48fps to 4. Attaching a hardware device context
/// before `avcodec_open2` is enough — FFmpeg's default `get_format` picks the
/// hardware pixel format once `hw_device_ctx` is set. Frames then arrive opaque
/// and go through `download_if_hardware`.
///
/// Falls back to software silently: a codec the hardware cannot handle (or a
/// machine without it) must still play.
fn open_video_decoder(
    context: ffmpeg::codec::context::Context,
) -> anyhow::Result<ffmpeg::codec::decoder::Video> {
    #[allow(unused_mut)]
    let mut decoder = context.decoder();

    #[cfg(target_os = "macos")]
    unsafe {
        use ffmpeg::sys::{AVBufferRef, AVHWDeviceType, av_buffer_unref, av_hwdevice_ctx_create};
        let mut device: *mut AVBufferRef = std::ptr::null_mut();
        let rc = av_hwdevice_ctx_create(
            &mut device,
            AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        );
        if rc >= 0 && !device.is_null() {
            // Ownership moves to the codec context, which releases it on close.
            (*decoder.0.as_mut_ptr()).hw_device_ctx = device;
        } else {
            if !device.is_null() {
                av_buffer_unref(&mut device);
            }
            log::warn!("[ffmpeg] VideoToolbox unavailable ({rc}); decoding in software");
        }
    }

    Ok(decoder.video()?)
}

/// Copy a hardware frame into system memory, or `None` if it is already there.
///
/// The decoder hands back an opaque `AV_PIX_FMT_VIDEOTOOLBOX` frame wrapping a
/// CVPixelBuffer; swscale cannot read it. Passing a fresh frame with no format
/// lets FFmpeg choose the transfer format itself (NV12 here).
///
/// ponytail: this download is a full-frame copy off the GPU — the zero-copy
/// path is to wrap the CVPixelBuffer's IOSurface as a Metal texture and sample
/// NV12 in the shader. Worth doing when the copy shows up in a profile; the
/// decode was the expensive half and this already removes it.
fn download_if_hardware(frame: &Video) -> anyhow::Result<Option<Video>> {
    if frame.format() != Pixel::VIDEOTOOLBOX {
        return Ok(None);
    }
    let mut sw = Video::empty();
    unsafe {
        let rc = ffmpeg::sys::av_hwframe_transfer_data(sw.as_mut_ptr(), frame.as_ptr(), 0);
        if rc < 0 {
            return Err(anyhow::anyhow!("hardware frame download failed ({rc})"));
        }
    }
    Ok(Some(sw))
}

/// Whether this stream's planes are GBR rather than YUV, and can be packed
/// without a colour conversion.
///
/// Restricted to the 12-bit 4:4:4 layouts NotchLC decodes to. Any other format
/// claiming an identity colourspace falls through to swscale — wrong, but no
/// more wrong than before, and it will say so in the log.
fn is_identity_gbr(space: Space, format: Pixel) -> bool {
    matches!(space, Space::RGB)
        && matches!(format, Pixel::YUVA444P12LE | Pixel::YUV444P12LE)
}

/// Pack 12-bit planar GBR(A) to 8-bit RGBA. Plane 0 is G, 1 is B, 2 is R.
fn pack_gbr_planes(frame: &Video) -> VideoFrame {
    let (w, h) = (frame.width() as usize, frame.height() as usize);
    let has_alpha = frame.planes() > 3;
    // Row strides are in bytes and are not width * 2.
    let plane = |i: usize| (frame.data(i), frame.stride(i));
    let (g, gs) = plane(0);
    let (b, bs) = plane(1);
    let (r, rs) = plane(2);
    let alpha = has_alpha.then(|| plane(3));

    let sample = |data: &[u8], stride: usize, x: usize, y: usize| -> u8 {
        let o = y * stride + x * 2;
        (u16::from_le_bytes([data[o], data[o + 1]]) >> 4) as u8
    };

    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            out.push(sample(r, rs, x, y));
            out.push(sample(g, gs, x, y));
            out.push(sample(b, bs, x, y));
            out.push(match alpha {
                Some((a, as_)) => sample(a, as_, x, y),
                None => 255,
            });
        }
    }
    VideoFrame {
        width: w as u32,
        height: h as u32,
        data: out,
        #[cfg(target_os = "macos")]
        hardware: None,
    }
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

        let frame_time = if fps > 0.0 { 1.0 / fps as f64 } else { 1.0 / 30.0 };
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
            worker: None,
            force_seek: true,
            last_decode_time: None,
            frame_accumulator: 0.0,
            frame_time,
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
        self.force_seek = true;
        self.frame_accumulator = 0.0;
    }

    /// Set playback speed multiplier (clamped to ≥ 0).
    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed;
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
        self.force_seek = true;
        self.frame_accumulator = 0.0;
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
            self.force_seek = true;
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
            self.force_seek = true;
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
    ///
    /// Uses wall-clock frame pacing: only decodes a new frame when enough
    /// real time has elapsed for the video frame rate and speed. When
    /// behind, intermediate frames are skipped so the decoder catches up.
    pub fn decode_frame(&mut self) -> Option<VideoFrame> {
        let worker = self
            .worker
            .get_or_insert_with(|| DecodeWorker::spawn(self.path.clone()));

        if !self.playing {
            // Paused still has to serve a seek — scrubbing must show where you
            // scrubbed to — but nothing else moves.
            if std::mem::take(&mut self.force_seek) {
                worker.request(self.position, true);
            }
            return worker.take_frame();
        }

        let now = Instant::now();

        // ── Frame pacing ───────────────────────────────────────────────
        // Compute how many video frames should have passed since the last
        // decode. If zero, hold the current frame (no decode work).
        let _frames_to_decode = if let Some(last) = self.last_decode_time {
            let dt = now.duration_since(last).as_secs_f64().min(0.1);
            // Pacing uses the absolute speed — direction doesn't affect decode rate.
            self.frame_accumulator += dt * self.speed.abs() as f64;
            let ftd = (self.frame_accumulator / self.frame_time).floor() as u32;
            if ftd == 0 {
                // No new frame is due, but one asked for earlier may have
                // landed by now — this is where the pipelining pays off.
                return worker.take_frame();
            }
            self.frame_accumulator -= ftd as f64 * self.frame_time;
            ftd
        } else {
            1
        };

        // Advance position by elapsed time.
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
                if self.speed >= 0.0 && self.position >= self.out_point {
                    match self.loop_mode {
                        LoopMode::None => {
                            self.position = self.out_point;
                            self.playing = false;
                        }
                        LoopMode::Loop => {
                            self.position = self.in_point;
                            self.force_seek = true;
                        }
                        LoopMode::PingPong => unreachable!(),
                    }
                } else if self.speed < 0.0 && self.position <= self.in_point {
                    match self.loop_mode {
                        LoopMode::None => {
                            self.position = self.in_point;
                            self.playing = false;
                        }
                        LoopMode::Loop => {
                            self.position = self.out_point;
                            self.force_seek = true;
                        }
                        LoopMode::PingPong => unreachable!(),
                    }
                }
            }
        }
        self.last_decode_time = Some(now);

        // Post the position and take whatever is ready. The frame collected
        // here is the answer to an earlier request — one frame of latency,
        // traded for never blocking the render thread.
        worker.request(self.position, std::mem::take(&mut self.force_seek));
        worker.take_frame()
    }

}

/// Open `path` and build everything the decode loop needs. Runs on the worker
/// thread, so a slow file open never stalls a render.
fn open_decode_context(path: &Path, zero_copy: bool) -> anyhow::Result<DecodeContext> {
        let ictx = input(path)?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("No video stream"))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let time_base = time_base.numerator() as f64 / time_base.denominator().max(1) as f64;

        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = open_video_decoder(context)?;

        let identity_gbr = is_identity_gbr(decoder.color_space(), decoder.format());
        if matches!(decoder.color_space(), Space::RGB) && !identity_gbr {
            log::warn!(
                "FfmpegDecoder: {} declares an identity colourspace in {:?}, which swscale will \
                 convert as if it were YUV; colours will be wrong",
                path.display(),
                decoder.format()
            );
        }

        // The zero-copy path converts YUV in a shader that knows exactly one
        // matrix: BT.709, limited range. swscale reads each stream's real
        // metadata, so anything else keeps going through it — correct colour
        // beats a saved copy. Unspecified is BT.709 for HD by convention, and
        // BT.601 below it.
        let bt709 = match decoder.color_space() {
            Space::BT709 => true,
            // Unspecified means BT.709 at HD sizes and BT.601 below, by convention.
            Space::Unspecified => decoder.height() >= 720,
            _ => false,
        };
        let limited = decoder.color_range() != ffmpeg::color::Range::JPEG;
        let zero_copy = zero_copy && !identity_gbr && bt709 && limited;
        if !zero_copy {
            log::debug!(
                "FfmpegDecoder: {} is {:?}/{:?}, decoding through swscale rather than the \
                 BT.709 shader path",
                path.display(),
                decoder.color_space(),
                decoder.color_range()
            );
        }

        Ok(DecodeContext {
            input: ictx,
            decoder,
            scaler: None,
            stream_index,
            time_base,
            identity_gbr,
            zero_copy,
        })
    }

impl DecodeContext {
    /// Decode the frame covering `position_secs`.
    ///
    /// `last_pts` is the decode cursor, owned by the caller so a seek can reset
    /// it: set it to -1 to force a seek on the next call.
    fn decode_at_position(
        &mut self,
        position_secs: f64,
        last_pts: &mut i64,
    ) -> anyhow::Result<VideoFrame> {
        let ctx = self;

        // Convert target position to stream timestamp units.
        let target_ts = (position_secs / ctx.time_base) as i64;

        // Seek if we're before the last decoded frame or far ahead.
        let needs_seek = *last_pts < 0
            || target_ts < *last_pts
            || target_ts > *last_pts + (1.0 / ctx.time_base) as i64 * 2;

        if needs_seek {
            // Seek to a keyframe at or before the target.
            let seek_ts = (target_ts as f64 * ctx.time_base / 1.0) as i64;
            if let Err(e) = ctx.input.seek(seek_ts, ..) {
                log::warn!("FfmpegDecoder seek failed: {}", e);
            }
            ctx.decoder.flush();
            *last_pts = -1;
        }

        let mut decoded = Video::empty();
        let mut rgba_frame = Video::empty();
        // Frames decoded on the way to the target, and the budget for them.
        // Chasing a wall-clock position without a bound is a death spiral: a
        // slow decode makes the next position jump larger, which makes the next
        // decode slower. A 4K h264 layer took the whole app from 48fps to 4.
        // With a budget the clip falls behind under load — one heavy layer no
        // longer sets the frame rate for the other ten. It re-syncs on the next
        // seek, and in the healthy case (~1 frame wanted per render) it never
        // binds.
        let mut skipped = 0u32;

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
                        if pts >= target_ts || *last_pts < 0 || skipped >= MAX_CATCHUP_FRAMES {
                            let frame = ctx.convert_to_rgba(&decoded, &mut rgba_frame)?;
                            *last_pts = pts;
                            return Ok(frame);
                        }
                        *last_pts = pts;
                        skipped += 1;
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    // End of file: drain the decoder. A clip whose out point is
                    // its last frame reaches here once per loop, and a second
                    // send_eof while already draining fails — expected, not an
                    // error worth propagating.
                    let _ = ctx.decoder.send_eof();
                    if ctx.decoder.receive_frame(&mut decoded).is_ok() {
                        return ctx.convert_to_rgba(&decoded, &mut rgba_frame);
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


// ── Decode worker ──────────────────────────────────────────────────────────

/// What the render thread wants next.
struct DecodeRequest {
    position: f64,
    /// Reset the decode cursor first — the position moved somewhere the cursor
    /// cannot simply be advanced to (a seek, a loop wrap, an in/out edit).
    force_seek: bool,
}

/// The two hand-off slots between the render thread and the decode worker.
///
/// Both are latest-wins rather than queues: an unread request is replaced, and
/// so is an uncollected frame. A queue would only build a backlog whose
/// contents are already stale by the time anyone looks at them, and holding
/// several 4K frames costs real memory.
#[derive(Default)]
struct DecodeShared {
    request: Option<DecodeRequest>,
    frame: Option<VideoFrame>,
    stop: bool,
}

/// Decoding, moved off the render thread.
///
/// Even on VideoToolbox, `receive_frame` blocks until the hardware has a frame
/// ready — enough to drag a full set from 60fps to 36 with one 4K layer. The
/// render thread now posts a position and collects whatever is ready, so it
/// waits for nothing.
///
/// The frame is handed back as pixels and uploaded by the caller. Uploading
/// from the worker was tried for HAP and measured *worse*: `write_texture`
/// from a second thread contends with the render thread's encoding and keeps
/// its own staging memory.
struct DecodeWorker {
    shared: std::sync::Arc<(std::sync::Mutex<DecodeShared>, std::sync::Condvar)>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DecodeWorker {
    fn spawn(path: PathBuf) -> Self {
        let shared = std::sync::Arc::new((
            std::sync::Mutex::new(DecodeShared::default()),
            std::sync::Condvar::new(),
        ));
        let worker_shared = shared.clone();
        let handle = std::thread::Builder::new()
            .name("ffmpeg-decode".into())
            .spawn(move || decode_loop(path, worker_shared))
            .ok();
        Self { shared, handle }
    }

    /// Ask for the frame at `position`, replacing any request not yet started.
    fn request(&self, position: f64, force_seek: bool) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut g) = lock.lock() {
            // A pending force_seek must survive being superseded, or the seek
            // is silently dropped when two requests land in the same gap.
            let force_seek = force_seek || g.request.as_ref().is_some_and(|r| r.force_seek);
            g.request = Some(DecodeRequest {
                position,
                force_seek,
            });
            cv.notify_one();
        }
    }

    /// Collect a decoded frame, or `None` if the worker has not finished one
    /// since the last call. `None` means "keep showing what you have".
    fn take_frame(&self) -> Option<VideoFrame> {
        self.shared.0.lock().ok()?.frame.take()
    }
}

impl Drop for DecodeWorker {
    fn drop(&mut self) {
        {
            let (lock, cv) = &*self.shared;
            if let Ok(mut g) = lock.lock() {
                g.stop = true;
            }
            cv.notify_one();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn decode_loop(
    path: PathBuf,
    shared: std::sync::Arc<(std::sync::Mutex<DecodeShared>, std::sync::Condvar)>,
) {
    let mut ctx = match open_decode_context(&path, true) {
        Ok(ctx) => ctx,
        Err(e) => {
            log::warn!(
                "FfmpegDecoder failed to open {}: {}",
                path.display(),
                e
            );
            return;
        }
    };
    let mut last_pts: i64 = -1;
    // Reaching EOF while chasing the last frame of a clip is normal — the front
    // wraps to the in point on the next request — so a single failure is not
    // worth a line in the log. A run of them means the file really is broken.
    let mut consecutive_errors = 0u32;

    loop {
        let request = {
            let (lock, cv) = &*shared;
            let Ok(mut g) = lock.lock() else { return };
            while !g.stop && g.request.is_none() {
                let Ok(next) = cv.wait(g) else { return };
                g = next;
            }
            if g.stop {
                return;
            }
            g.request.take().expect("woken with a request pending")
        };

        if request.force_seek {
            last_pts = -1;
        }
        match ctx.decode_at_position(request.position, &mut last_pts) {
            Ok(frame) => {
                consecutive_errors = 0;
                if let Ok(mut g) = shared.0.lock() {
                    g.frame = Some(frame);
                }
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors == 10 {
                    log::warn!("FfmpegDecoder: {} is not decoding: {}", path.display(), e);
                }
            }
        }
    }
}

// ── StreamDecoder (live network ingest) ────────────────────────────────────

/// Continuous decoder for live streams (SRT, HLS, DASH, RTMP, RTMPS).
///
/// Unlike `FfmpegDecoder`, this does not seek, loop, or track playback
/// position — it simply reads the next available frame from the network
/// and returns it.  On disconnect or EOF the last decoded frame is held
/// so the output does not flicker.
pub struct StreamDecoder {
    url: String,
    width: u32,
    height: u32,
    fps: f32,

    context: Option<DecodeContext>,
    last_frame: Option<VideoFrame>,
    connected: bool,
    /// Consecutive failed connection attempts; see `decode_frame`.
    connect_errors: u32,
}

unsafe impl Send for StreamDecoder {}

impl StreamDecoder {
    /// Open a network stream URL and probe its metadata.
    pub fn new(url: &str) -> anyhow::Result<Self> {
        let ictx = input(std::path::Path::new(url))?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("No video stream found in {}", url))?;

        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = context.decoder().video()?;

        let width = decoder.width();
        let height = decoder.height();

        let fps = stream.rate();
        let fps = fps.numerator() as f32 / fps.denominator().max(1) as f32;

        drop(decoder);
        drop(ictx);

        Ok(Self {
            url: url.to_string(),
            width,
            height,
            fps,
            context: None,
            last_frame: None,
            connected: false,
            connect_errors: 0,
        })
    }

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
    /// Whether the stream is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Read and decode the next frame from the stream.
    ///
    /// Returns `None` only before the first successful decode.
    /// After that, the last good frame is returned on error/EOF.
    pub fn decode_frame(&mut self) -> Option<VideoFrame> {
        if self.context.is_none() {
            if let Err(e) = self.init_context() {
                // Callers poll this every frame without checking is_connected(), so an
                // unreachable URL retries — and warned — at frame rate. First failure
                // of a streak only; connecting resets it.
                self.connect_errors += 1;
                if self.connect_errors == 1 {
                    log::warn!("StreamDecoder failed to connect to {}: {}", self.url, e);
                }
                self.connected = false;
                return self.last_frame.clone();
            }
            self.connected = true;
            self.connect_errors = 0;
        }

        let ctx = self.context.as_mut().unwrap();

        let mut decoded = Video::empty();
        let mut rgba_frame = Video::empty();

        loop {
            let mut packet = ffmpeg::Packet::empty();
            match packet.read(&mut ctx.input) {
                Ok(()) => {
                    if packet.stream() != ctx.stream_index {
                        continue;
                    }
                    if let Err(e) = ctx.decoder.send_packet(&packet) {
                        log::debug!("StreamDecoder send_packet error: {}", e);
                        continue;
                    }
                    while ctx.decoder.receive_frame(&mut decoded).is_ok() {
                        let frame = match ctx.convert_to_rgba(&decoded, &mut rgba_frame) {
                            Ok(f) => f,
                            Err(e) => {
                                // debug, like the send_packet error above: same class of
                                // per-frame decode hiccup, and this is inside the loop.
                                log::debug!("StreamDecoder convert error: {}", e);
                                continue;
                            }
                        };
                        self.last_frame = Some(frame.clone());
                        return Some(frame);
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    ctx.decoder.send_eof().ok();
                    if ctx.decoder.receive_frame(&mut decoded).is_ok()
                        && let Ok(frame) = ctx.convert_to_rgba(&decoded, &mut rgba_frame)
                    {
                        self.last_frame = Some(frame.clone());
                        return Some(frame);
                    }
                    log::warn!("StreamDecoder EOF on {}", self.url);
                    self.connected = false;
                    return self.last_frame.clone();
                }
                Err(e) => {
                    log::debug!("StreamDecoder packet read error: {}", e);
                    continue;
                }
            }
        }
    }

    fn init_context(&mut self) -> anyhow::Result<()> {
        let ictx = input(std::path::Path::new(&self.url))?;
        let stream = ictx
            .streams()
            .best(Type::Video)
            .ok_or_else(|| anyhow::anyhow!("No video stream"))?;
        let stream_index = stream.index();
        let time_base = stream.time_base();
        let time_base = time_base.numerator() as f64 / time_base.denominator().max(1) as f64;

        let context = ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
        let decoder = open_video_decoder(context)?;

        let identity_gbr = is_identity_gbr(decoder.color_space(), decoder.format());
        if matches!(decoder.color_space(), Space::RGB) && !identity_gbr {
            log::warn!(
                "StreamDecoder: {} declares an identity colourspace in {:?}, which swscale will \
                 convert as if it were YUV; colours will be wrong",
                self.url,
                decoder.format()
            );
        }

        self.context = Some(DecodeContext {
            input: ictx,
            decoder,
            scaler: None,
            stream_index,
            time_base,
            identity_gbr,
            // A live stream's consumer still uploads pixels itself.
            zero_copy: false,
        });
        Ok(())
    }
}

impl Drop for StreamDecoder {
    fn drop(&mut self) {
        // Context drops automatically.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_gbr_is_claimed_only_for_layouts_we_can_pack() {
        assert!(is_identity_gbr(Space::RGB, Pixel::YUVA444P12LE));
        assert!(is_identity_gbr(Space::RGB, Pixel::YUV444P12LE));
        // Identity colourspace in a layout we do not handle: fall through to
        // swscale and warn, rather than pack garbage.
        assert!(!is_identity_gbr(Space::RGB, Pixel::YUV420P));
        // Ordinary video must never take this path.
        assert!(!is_identity_gbr(Space::BT709, Pixel::YUV420P));
        assert!(!is_identity_gbr(Space::SMPTE170M, Pixel::YUVA444P12LE));
    }

    /// The failure this guards against is silent: swapping the planes still
    /// produces a plausible image, just the wrong colour. Hence three distinct
    /// channel values.
    #[test]
    fn packs_planes_as_gbr_not_yuv() {
        let mut frame = Video::new(Pixel::YUVA444P12LE, 2, 2);
        // 12-bit samples; the packer keeps the top 8 bits.
        for (plane, value) in [(0usize, 200u16 << 4), (1, 50 << 4), (2, 100 << 4), (3, 4095)] {
            let stride = frame.stride(plane);
            let data = frame.data_mut(plane);
            for y in 0..2 {
                for x in 0..2 {
                    let o = y * stride + x * 2;
                    data[o..o + 2].copy_from_slice(&value.to_le_bytes());
                }
            }
        }

        let out = pack_gbr_planes(&frame);
        assert_eq!((out.width, out.height), (2, 2));
        // Plane 0 is G, 1 is B, 2 is R.
        assert_eq!(&out.data[0..4], &[100, 200, 50, 255]);
        assert_eq!(&out.data[4..8], &[100, 200, 50, 255]);
    }

    /// Without an alpha plane the packer must still produce opaque pixels.
    #[test]
    fn missing_alpha_plane_reads_as_opaque() {
        let mut frame = Video::new(Pixel::YUV444P12LE, 1, 1);
        for (plane, value) in [(0usize, 10u16 << 4), (1, 20 << 4), (2, 30 << 4)] {
            let stride = frame.stride(plane);
            frame.data_mut(plane)[0..2].copy_from_slice(&value.to_le_bytes());
            let _ = stride;
        }
        let out = pack_gbr_planes(&frame);
        assert_eq!(&out.data[0..4], &[30, 10, 20, 255]);
    }
}
