use crate::frame::{BitDepth, ChromaSubsample, VideoFrame, YuvPlane};
use crate::FramePool;
use ffmpeg_next::{codec, color, ffi, format, frame, media::Type, software::scaling, threading};
use std::sync::Arc;

/// A hardware decode candidate: device type, the hw pixel format its frames
/// arrive in, and a log label.
type HwKind = (ffi::AVHWDeviceType, ffi::AVPixelFormat, &'static str);

/// Hardware decode candidates, tried in order at open. Linux is skipped:
/// cuepool isn't shipped there.
#[cfg(target_os = "windows")]
const HW_CANDIDATES: &[HwKind] = &[
    (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA, ffi::AVPixelFormat::AV_PIX_FMT_D3D11, "hardware (d3d11va)"),
    (ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2, ffi::AVPixelFormat::AV_PIX_FMT_DXVA2_VLD, "hardware (dxva2)"),
];
#[cfg(target_os = "macos")]
const HW_CANDIDATES: &[HwKind] =
    &[(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX, ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX, "hardware (videotoolbox)")];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const HW_CANDIDATES: &[HwKind] = &[];

/// `get_format` callback: picks the hw pixel format out of the decoder's
/// offered list. The format to match travels in the codec context's `opaque`
/// (set before open), so there's no global state and no concurrent-open race.
unsafe extern "C" fn hw_get_format(
    ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    unsafe {
        let want = (*ctx).opaque as isize as i32;
        let mut f = fmts;
        while !f.is_null() && *f != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *f as i32 == want {
                return *f;
            }
            f = f.add(1);
        }
        ffi::AVPixelFormat::AV_PIX_FMT_NONE
    }
}

/// Wraps an FFmpeg video stream decoder and produces `VideoFrame`s.
pub struct VideoSource {
    /// Kept so a broken hwaccel can reopen the whole source in software.
    path: String,
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
    /// Reusable CPU frame that hw-decoded frames are downloaded into.
    sw_frame: frame::Video,
    rgb_frame: frame::Video,
    /// The hw pixel format in use, or `None` for pure software decode.
    hw_pix_fmt: Option<ffi::AVPixelFormat>,
    /// First hw download succeeded — distinguishes "hwaccel never worked"
    /// (reopen in software) from a one-off mid-stream failure (skip frame).
    hw_checked: bool,
    /// The hw label chosen at open ("hardware (d3d11va)" etc.), or
    /// "software". Only truthful via `decode_path()`, which gates on
    /// `hw_checked` — a created device proves nothing until a hw frame lands.
    hw_label: &'static str,
    eof: bool,
    /// `send_eof` has been issued; remaining calls just drain delayed frames.
    eof_sent: bool,
    frame_pool: Arc<FramePool>,
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

fn plane(frame: &frame::Video, i: usize, frame_pool: &FramePool) -> YuvPlane {
    YuvPlane {
        data: frame_pool.copy_from_slice(frame.data(i)),
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

/// Build a `VideoFrame` from a decoded, CPU-readable frame. GPU-native YUV is
/// handed over plane-by-plane; anything else is converted to RGBA by swscale.
fn convert_frame(
    frame: &frame::Video,
    scaler: &mut Option<scaling::Context>,
    rgb_frame: &mut frame::Video,
    dst_width: u32,
    dst_height: u32,
    time_base: f64,
    frame_pool: &FramePool,
) -> Option<VideoFrame> {
    let pts = frame.timestamp().unwrap_or(0) as f64 * time_base;

    if let Some(fmt) = gpu_format_class(frame.format()) {
        let full_range = is_full_range(frame);
        let bt709 = is_bt709(frame, frame.height());
        return Some(match fmt {
            GpuYuvFormat::Nv12 => VideoFrame::nv12(
                dst_width,
                dst_height,
                pts,
                plane(frame, 0, frame_pool),
                plane(frame, 1, frame_pool),
                full_range,
                bt709,
            ),
            GpuYuvFormat::Yuv420P10 => VideoFrame::yuv_planar(
                dst_width,
                dst_height,
                pts,
                ChromaSubsample::Cs420,
                BitDepth::B10,
                plane(frame, 0, frame_pool),
                plane(frame, 1, frame_pool),
                plane(frame, 2, frame_pool),
                full_range,
                bt709,
            ),
            GpuYuvFormat::Yuv420 => VideoFrame::yuv_planar(
                dst_width,
                dst_height,
                pts,
                ChromaSubsample::Cs420,
                BitDepth::B8,
                plane(frame, 0, frame_pool),
                plane(frame, 1, frame_pool),
                plane(frame, 2, frame_pool),
                full_range,
                bt709,
            ),
            GpuYuvFormat::Yuv422 => VideoFrame::yuv_planar(
                dst_width,
                dst_height,
                pts,
                ChromaSubsample::Cs422,
                BitDepth::B8,
                plane(frame, 0, frame_pool),
                plane(frame, 1, frame_pool),
                plane(frame, 2, frame_pool),
                full_range,
                bt709,
            ),
            GpuYuvFormat::Yuv444 => VideoFrame::yuv_planar(
                dst_width,
                dst_height,
                pts,
                ChromaSubsample::Cs444,
                BitDepth::B8,
                plane(frame, 0, frame_pool),
                plane(frame, 1, frame_pool),
                plane(frame, 2, frame_pool),
                full_range,
                bt709,
            ),
        });
    }

    // Long-tail format: swscale to RGBA. Created lazily (rather than only at
    // open) because a hw download's format is unknown until the first frame.
    if scaler.is_none() {
        *scaler = scaling::Context::get(
            frame.format(),
            frame.width(),
            frame.height(),
            format::Pixel::RGBA,
            dst_width,
            dst_height,
            scaling::Flags::BILINEAR,
        )
        .ok();
    }
    scaler.as_mut()?.run(frame, rgb_frame).ok()?;
    let data = frame_pool.copy_from_slice(rgb_frame.data(0));
    Some(VideoFrame::new(dst_width, dst_height, data, pts))
}

/// What to do after handling one freshly decoded frame.
enum Decoded {
    Frame(VideoFrame),
    /// Unconvertible frame / failed download: skip it, pull the next one.
    Skip,
    /// Software-path convert failure (previous behaviour: treat as EOF).
    End,
    /// First hw download failed — hwaccel is broken, reopen in software.
    ReopenSoftware,
}

impl VideoSource {
    /// Open a video file and initialise the decoder (+ a CPU scaler only if the
    /// pixel format is not handled natively by the GPU converter).
    ///
    /// Frames are produced at the source's **native resolution**; aspect-ratio
    /// fitting is the canvas's job, so forcing a fixed size here would pre-stretch
    /// non-matching sources.
    pub fn open(path: &str) -> anyhow::Result<Self> {
        Self::open_with_pool(path, Arc::new(FramePool::new(0)))
    }

    pub fn open_with_pool(path: &str, frame_pool: Arc<FramePool>) -> anyhow::Result<Self> {
        // Escape hatch for A/B diagnosis on production machines.
        if std::env::var("QPLAYER_NO_HWACCEL").as_deref() == Ok("1") {
            return Self::open_with(path, None, frame_pool);
        }
        for &hw in HW_CANDIDATES {
            match Self::open_with(path, Some(hw), Arc::clone(&frame_pool)) {
                Ok(src) => return Ok(src),
                Err(e) => log::warn!("Video decode: {} unavailable ({e})", hw.2),
            }
        }
        Self::open_with(path, None, frame_pool)
    }

    fn open_with(
        path: &str,
        hw: Option<HwKind>,
        frame_pool: Arc<FramePool>,
    ) -> anyhow::Result<Self> {
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
        let mut decoder = context.decoder();

        // Hardware decode: create the device and hand it to the codec context
        // before open; the `get_format` callback then picks the hw format.
        let hw_pix_fmt = if let Some((device_type, pix_fmt, _)) = hw {
            unsafe {
                let ctx = decoder.as_mut_ptr();
                let mut device = std::ptr::null_mut();
                if ffi::av_hwdevice_ctx_create(
                    &mut device,
                    device_type,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    0,
                ) < 0
                {
                    anyhow::bail!("av_hwdevice_ctx_create failed");
                }
                // Ownership of the device ref passes to the codec context
                // (avcodec_free_context unrefs it) — never unref it ourselves.
                (*ctx).hw_device_ctx = device;
                (*ctx).get_format = Some(hw_get_format);
                (*ctx).opaque = pix_fmt as i32 as isize as *mut std::ffi::c_void;
            }
            Some(pix_fmt)
        } else {
            None
        };

        let mut decoder = decoder.video()?;
        decoder.set_parameters(input.parameters())?;

        let width = decoder.width();
        let height = decoder.height();
        let (dst_width, dst_height) = (width, height);

        // GPU-native YUV → no scaler. Everything else → RGBA via swscale fallback.
        // In hw mode the decoder reports the opaque hw format and the real
        // (downloaded) format is only known per-frame, so a scaler — if one is
        // needed at all — is created lazily in `convert_frame`.
        let scaler = if hw_pix_fmt.is_some() || gpu_format_class(decoder.format()).is_some() {
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
        let sw_frame = frame::Video::empty();
        let mut rgb_frame = frame::Video::empty();
        rgb_frame.set_format(format::Pixel::RGBA);
        rgb_frame.set_width(dst_width);
        rgb_frame.set_height(dst_height);

        let hw_label = hw.map_or("software", |(_, _, label)| label);
        if hw.is_some() {
            // "attempting": a created device proves nothing until a hw frame
            // lands (codecs without hwaccel, e.g. Hap, never use it).
            log::info!("Video decode: attempting {hw_label}");
        } else {
            log::info!("Video decode: software");
        }

        Ok(Self {
            path: path.to_string(),
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
            sw_frame,
            rgb_frame,
            hw_pix_fmt,
            hw_checked: false,
            hw_label,
            eof: false,
            eof_sent: false,
            frame_pool,
        })
    }

    /// Handle the frame sitting in `self.decoded_frame`: download it from the
    /// GPU if it's a hw frame, then convert it to a `VideoFrame`.
    fn handle_decoded(&mut self) -> Decoded {
        if let Some(hw_fmt) = self.hw_pix_fmt
            && self.decoded_frame.format() == format::Pixel::from(hw_fmt) {
                // hw frames live in GPU memory and aren't readable via
                // `plane()`; download into the reusable CPU frame first.
                // `copy_props` carries PTS / color range / color space over,
                // which `is_full_range` / `is_bt709` read off the frame.
                unsafe {
                    ffi::av_frame_unref(self.sw_frame.as_mut_ptr());
                    if ffi::av_hwframe_transfer_data(
                        self.sw_frame.as_mut_ptr(),
                        self.decoded_frame.as_ptr(),
                        0,
                    ) < 0
                    {
                        if self.hw_checked {
                            log::warn!("Video decode: hw download failed, skipping frame");
                            return Decoded::Skip;
                        }
                        return Decoded::ReopenSoftware;
                    }
                    ffi::av_frame_copy_props(
                        self.sw_frame.as_mut_ptr(),
                        self.decoded_frame.as_ptr(),
                    );
                }
                if !self.hw_checked {
                    log::info!("Video decode: {} engaged", self.hw_label);
                }
                self.hw_checked = true;
                return convert_frame(
                    &self.sw_frame,
                    &mut self.scaler,
                    &mut self.rgb_frame,
                    self.dst_width,
                    self.dst_height,
                    self.time_base,
                    &self.frame_pool,
                )
                .map_or(Decoded::Skip, Decoded::Frame);
            }
        convert_frame(
            &self.decoded_frame,
            &mut self.scaler,
            &mut self.rgb_frame,
            self.dst_width,
            self.dst_height,
            self.time_base,
            &self.frame_pool,
        )
        .map_or(Decoded::End, Decoded::Frame)
    }

    /// Pull the next decoded frame into `self.decoded_frame`. `false` at EOF.
    fn next_raw_frame(&mut self) -> bool {
        if self.eof {
            return false;
        }

        // Try draining already-decoded frames first.
        if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            return true;
        }

        for (stream, packet) in self.ictx.packets() {
            if stream.index() == self.stream_index {
                if self.decoder.send_packet(&packet).is_err() {
                    continue;
                }
                if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                    return true;
                }
            }
        }

        // Flush: send EOF once, then drain the delayed frames the threaded
        // decoder still holds — one per call — until it runs dry. (Sending
        // EOF and converting a single frame would silently drop the tail of
        // every clip: the decoder queues ~thread-count frames.)
        if !self.eof_sent {
            let _ = self.decoder.send_eof();
            self.eof_sent = true;
        }
        if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            return true;
        }
        self.eof = true;
        false
    }

    /// Reopen the whole source in pure software mode (the broken-hwaccel
    /// escape hatch; playback position is lost, this is not a routine path).
    fn reopen_software(&mut self) -> bool {
        let path = self.path.clone();
        match Self::open_with(&path, None, Arc::clone(&self.frame_pool)) {
            Ok(src) => {
                log::warn!("Video decode: hardware broke on first frame, reopened in software");
                *self = src;
                true
            }
            Err(e) => {
                log::error!("Video decode: software reopen failed: {e}");
                self.eof = true;
                false
            }
        }
    }

    /// Read the next frame and return it with PTS in seconds. `None` at EOF.
    pub fn read_frame(&mut self) -> Option<VideoFrame> {
        loop {
            if !self.next_raw_frame() {
                return None;
            }
            match self.handle_decoded() {
                Decoded::Frame(f) => return Some(f),
                Decoded::Skip => continue,
                Decoded::End => return None,
                Decoded::ReopenSoftware => {
                    if !self.reopen_software() {
                        return None;
                    }
                }
            }
        }
    }

    /// Seek to the keyframe at or before `secs` and reset decoder state, so the
    /// next `read_frame` calls decode forward from there. Used for frame-step-back.
    pub fn seek_before(&mut self, secs: f64) -> anyhow::Result<()> {
        let ts = (secs.max(0.0) * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
        self.ictx.seek(ts, ..ts)?;
        self.decoder.flush();
        self.eof = false;
        self.eof_sent = false;
        Ok(())
    }

    /// Which decode path was chosen at open: `hardware (<api>)` or `software`.
    /// The active decode path. Truthful once frames have flowed: a
    /// created-but-unused hw device (e.g. Hap has no hwaccel) reports
    /// "software" until the first hw frame actually downloads.
    pub fn decode_path(&self) -> &'static str {
        if self.hw_checked { self.hw_label } else { "software" }
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
