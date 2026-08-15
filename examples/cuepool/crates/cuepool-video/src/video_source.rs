use crate::FramePool;
use crate::ZeroCopyAvailability;
use crate::frame::{BitDepth, ChromaSubsample, VideoFrame, YuvPlane};
use ffmpeg_next::Packet;
use ffmpeg_next::{codec, color, ffi, format, frame, media::Type, software::scaling, threading};
use hap_parser::{HapFrame, TextureFormat as HapFormat};
use std::ffi::{CString, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[cfg(windows)]
use crate::d3d12_zero_copy::DirectPoolRequest;

#[cfg(windows)]
type DirectPoolOption = Option<DirectPoolRequest>;
#[cfg(not(windows))]
#[derive(Default)]
enum DirectPoolOption {
    #[default]
    None,
}

fn no_direct_pool() -> DirectPoolOption {
    Default::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenRoute {
    HardwareCandidates,
    SoftwareOnly,
}

struct OpenOptions {
    route: OpenRoute,
    availability: Option<ZeroCopyAvailability>,
    fallback_reason: Option<String>,
}

impl OpenOptions {
    fn hardware(availability: Option<ZeroCopyAvailability>) -> Self {
        Self {
            route: OpenRoute::HardwareCandidates,
            availability,
            fallback_reason: None,
        }
    }

    #[cfg(any(windows, test))]
    fn after_zero_copy_decline(reason: String) -> Self {
        Self {
            route: OpenRoute::HardwareCandidates,
            availability: None,
            fallback_reason: Some(format!("shareable D3D12VA pool rejected: {reason}")),
        }
    }

    fn software(fallback_reason: Option<String>) -> Self {
        Self {
            route: OpenRoute::SoftwareOnly,
            availability: None,
            fallback_reason,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroCopyPreference {
    Disabled,
    Enabled,
}

impl ZeroCopyPreference {
    pub fn from_value(value: Option<&str>) -> Self {
        if value == Some("1") {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }

    pub fn from_env() -> Self {
        Self::from_value(std::env::var("QPLAYER_ZEROCOPY").ok().as_deref())
    }

    pub fn enabled(self) -> bool {
        self == Self::Enabled
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct VideoFrameTimings {
    pub decode: Duration,
    pub hw_transfer: Duration,
    pub plane_copy: Duration,
}

/// GPU capability passed by the renderer when native HAP uploads are safe.
#[derive(Clone, Debug)]
pub struct HapAcceleration {
    max_texture_dimension_2d: Option<u32>,
    fallback_reason: Option<String>,
}

/// Cue-lifetime memory of a native-HAP fallback decision.
#[derive(Clone, Default)]
pub struct HapFallbackSession(Arc<OnceLock<String>>);

impl HapFallbackSession {
    fn reason(&self) -> Option<&str> {
        self.0.get().map(String::as_str)
    }

    fn record(&self, reason: String) {
        let _ = self.0.set(reason);
    }
}

impl HapAcceleration {
    pub fn available(max_texture_dimension_2d: u32) -> Self {
        Self {
            max_texture_dimension_2d: Some(max_texture_dimension_2d),
            fallback_reason: None,
        }
    }

    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            max_texture_dimension_2d: None,
            fallback_reason: Some(reason.into()),
        }
    }
}

/// A hardware decode candidate: device type, the hw pixel format its frames
/// arrive in, and a log label.
type HwKind = (ffi::AVHWDeviceType, ffi::AVPixelFormat, &'static str);

/// Hardware decode candidates, tried in order at open. Linux is skipped:
/// cuepool isn't shipped there.
#[cfg(target_os = "windows")]
const HW_CANDIDATES: &[HwKind] = &[
    (
        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D11VA,
        ffi::AVPixelFormat::AV_PIX_FMT_D3D11,
        "d3d11va readback",
    ),
    (
        ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_DXVA2,
        ffi::AVPixelFormat::AV_PIX_FMT_DXVA2_VLD,
        "hardware (dxva2)",
    ),
];
/// The zero-copy candidate, tried before `HW_CANDIDATES` when a wgpu DX12
/// interop device is available. Not part of the readback list: without a
/// direct pool a plain D3D12VA open would just be a slower readback.
#[cfg(windows)]
const DIRECT_HW: HwKind = (
    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D12VA,
    ffi::AVPixelFormat::AV_PIX_FMT_D3D12,
    "d3d12va zero-copy",
);

#[cfg(target_os = "macos")]
const HW_CANDIDATES: &[HwKind] = &[(
    ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_VIDEOTOOLBOX,
    ffi::AVPixelFormat::AV_PIX_FMT_VIDEOTOOLBOX,
    "hardware (videotoolbox)",
)];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const HW_CANDIDATES: &[HwKind] = &[];

struct HwFormatState {
    want: ffi::AVPixelFormat,
    #[cfg(windows)]
    direct_pool: Option<DirectPoolRequest>,
}

/// `get_format` callback: picks the hw pixel format out of the decoder's
/// offered list. The format to match travels in the codec context's `opaque`
/// (set before open), so there's no global state and no concurrent-open race.
unsafe extern "C" fn hw_get_format(
    ctx: *mut ffi::AVCodecContext,
    fmts: *const ffi::AVPixelFormat,
) -> ffi::AVPixelFormat {
    unsafe {
        let state = &*((*ctx).opaque.cast::<HwFormatState>());
        let mut f = fmts;
        while !f.is_null() && *f != ffi::AVPixelFormat::AV_PIX_FMT_NONE {
            if *f == state.want {
                #[cfg(windows)]
                if let Some(request) = state.direct_pool.as_ref() {
                    return crate::d3d12_zero_copy::configure_pool(ctx, request)
                        .unwrap_or(ffi::AVPixelFormat::AV_PIX_FMT_NONE);
                }
                return *f;
            }
            f = f.add(1);
        }
        ffi::AVPixelFormat::AV_PIX_FMT_NONE
    }
}

enum VideoBackend {
    Ffmpeg(FfmpegVideoSource),
    Hap(HapVideoSource),
}

/// Chooses GPU-native HAP packet decoding when available, otherwise delegates
/// to CuePool's existing FFmpeg decoder.
pub struct VideoSource {
    path: String,
    frame_pool: Arc<FramePool>,
    hap_fallback_session: HapFallbackSession,
    interrupt: Option<Arc<std::sync::atomic::AtomicBool>>,
    fallback_decode_time: Duration,
    recovering_fallback_reason: Option<String>,
    terminal_error: Option<String>,
    backend: VideoBackend,
}

enum HapProbe {
    NotHap(format::context::Input),
    Native(Box<HapVideoSource>),
    Fallback(String),
}

enum HapRead {
    Frame(VideoFrame),
    Eof,
    Fallback {
        reason: String,
        pts: f64,
        decode_time: Duration,
    },
}

#[derive(Debug)]
enum HapPacketError {
    Unsupported(String),
    Corrupt(String),
}

impl std::fmt::Display for HapPacketError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported(reason) | Self::Corrupt(reason) => formatter.write_str(reason),
        }
    }
}

struct HapVideoSource {
    ictx: format::context::Input,
    // Must outlive `ictx`: FFmpeg's interrupt callback points into this Arc.
    _interrupt: Option<Arc<AtomicBool>>,
    stream_index: usize,
    time_base: f64,
    width: u32,
    height: u32,
    frame_duration: f64,
    format: HapFormat,
    max_texture_dimension_2d: u32,
    next_pts: f64,
    pending: Option<(Packet, Duration)>,
    last_timings: VideoFrameTimings,
    corrupt_warned: bool,
    corrupt_streak: u32,
}

impl VideoSource {
    pub fn open(path: &str) -> anyhow::Result<Self> {
        Self::open_with_pool(path, Arc::new(FramePool::new(0)))
    }

    pub fn open_with_pool(path: &str, frame_pool: Arc<FramePool>) -> anyhow::Result<Self> {
        Self::open_backend(
            path,
            frame_pool,
            None,
            HapAcceleration::unavailable(
                "GPU-native HAP unavailable: caller did not provide GPU BC capability",
            ),
            HapFallbackSession::default(),
            None,
        )
    }

    pub fn open_with_pool_and_hap(
        path: &str,
        frame_pool: Arc<FramePool>,
        native_hap_max_dimension: Option<u32>,
    ) -> anyhow::Result<Self> {
        let hap = native_hap_max_dimension.map_or_else(
            || {
                HapAcceleration::unavailable(
                    "GPU-native HAP unavailable: GPU device lacks BC texture compression",
                )
            },
            HapAcceleration::available,
        );
        Self::open_backend(
            path,
            frame_pool,
            None,
            hap,
            HapFallbackSession::default(),
            None,
        )
    }

    pub fn open_with_hap_acceleration(
        path: &str,
        frame_pool: Arc<FramePool>,
        hap_acceleration: HapAcceleration,
    ) -> anyhow::Result<Self> {
        Self::open_with_hap_acceleration_and_session(
            path,
            frame_pool,
            hap_acceleration,
            HapFallbackSession::default(),
        )
    }

    pub fn open_with_hap_acceleration_and_session(
        path: &str,
        frame_pool: Arc<FramePool>,
        hap_acceleration: HapAcceleration,
        hap_fallback_session: HapFallbackSession,
    ) -> anyhow::Result<Self> {
        Self::open_backend(
            path,
            frame_pool,
            None,
            hap_acceleration,
            hap_fallback_session,
            None,
        )
    }

    pub fn open_with_zero_copy(
        path: &str,
        frame_pool: Arc<FramePool>,
        availability: ZeroCopyAvailability,
    ) -> anyhow::Result<Self> {
        Self::open_with_acceleration(
            path,
            frame_pool,
            availability,
            HapAcceleration::unavailable(
                "GPU-native HAP unavailable: caller did not provide GPU BC capability",
            ),
        )
    }

    pub fn open_with_acceleration(
        path: &str,
        frame_pool: Arc<FramePool>,
        availability: ZeroCopyAvailability,
        hap_acceleration: HapAcceleration,
    ) -> anyhow::Result<Self> {
        Self::open_with_acceleration_and_session(
            path,
            frame_pool,
            availability,
            hap_acceleration,
            HapFallbackSession::default(),
        )
    }

    pub fn open_with_acceleration_and_session(
        path: &str,
        frame_pool: Arc<FramePool>,
        availability: ZeroCopyAvailability,
        hap_acceleration: HapAcceleration,
        hap_fallback_session: HapFallbackSession,
    ) -> anyhow::Result<Self> {
        Self::open_backend(
            path,
            frame_pool,
            Some(availability),
            hap_acceleration,
            hap_fallback_session,
            None,
        )
    }

    pub fn open_with_acceleration_and_session_cancellable(
        path: &str,
        frame_pool: Arc<FramePool>,
        availability: ZeroCopyAvailability,
        hap_acceleration: HapAcceleration,
        hap_fallback_session: HapFallbackSession,
        stop_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> anyhow::Result<Self> {
        Self::open_backend(
            path,
            frame_pool,
            Some(availability),
            hap_acceleration,
            hap_fallback_session,
            Some(stop_flag),
        )
    }

    pub fn open_with_hap_acceleration_and_session_cancellable(
        path: &str,
        frame_pool: Arc<FramePool>,
        hap_acceleration: HapAcceleration,
        hap_fallback_session: HapFallbackSession,
        stop_flag: Arc<std::sync::atomic::AtomicBool>,
    ) -> anyhow::Result<Self> {
        Self::open_backend(
            path,
            frame_pool,
            None,
            hap_acceleration,
            hap_fallback_session,
            Some(stop_flag),
        )
    }

    fn open_backend(
        path: &str,
        frame_pool: Arc<FramePool>,
        availability: Option<ZeroCopyAvailability>,
        hap_acceleration: HapAcceleration,
        hap_fallback_session: HapFallbackSession,
        stop_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> anyhow::Result<Self> {
        if stop_flag
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            anyhow::bail!("video open cancelled");
        }
        if let Some(reason) = hap_fallback_session.reason() {
            let reason =
                format!("GPU-native HAP disabled for this cue after prior fallback: {reason}");
            let source = FfmpegVideoSource::open_with_options(
                path,
                Arc::clone(&frame_pool),
                OpenOptions::software(Some(reason.clone())),
                None,
                stop_flag.clone(),
            )?;
            return Ok(Self {
                path: path.to_owned(),
                frame_pool,
                hap_fallback_session,
                interrupt: stop_flag,
                fallback_decode_time: Duration::ZERO,
                recovering_fallback_reason: Some(reason),
                terminal_error: None,
                backend: VideoBackend::Ffmpeg(source),
            });
        }
        let unavailable = hap_unavailable_reason(
            std::env::var("QPLAYER_NO_HWACCEL").as_deref() == Ok("1"),
            hap_acceleration.fallback_reason.as_deref(),
        );
        match HapVideoSource::probe(
            path,
            unavailable,
            hap_acceleration
                .max_texture_dimension_2d
                .unwrap_or_default(),
            stop_flag.clone(),
        )? {
            HapProbe::Native(source) => Ok(Self {
                path: path.to_owned(),
                frame_pool,
                hap_fallback_session,
                interrupt: stop_flag,
                fallback_decode_time: Duration::ZERO,
                recovering_fallback_reason: None,
                terminal_error: None,
                backend: VideoBackend::Hap(*source),
            }),
            HapProbe::Fallback(reason) => {
                if stop_flag
                    .as_ref()
                    .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
                {
                    anyhow::bail!("video open cancelled");
                }
                hap_fallback_session.record(reason.clone());
                log::warn!("Video decode: {reason}; using FFmpeg software fallback");
                let source = FfmpegVideoSource::open_with_options(
                    path,
                    Arc::clone(&frame_pool),
                    OpenOptions::software(Some(reason.clone())),
                    None,
                    stop_flag.clone(),
                )?;
                Ok(Self {
                    path: path.to_owned(),
                    frame_pool,
                    hap_fallback_session,
                    interrupt: stop_flag,
                    fallback_decode_time: Duration::ZERO,
                    recovering_fallback_reason: Some(reason),
                    terminal_error: None,
                    backend: VideoBackend::Ffmpeg(source),
                })
            }
            HapProbe::NotHap(input) => {
                let source = FfmpegVideoSource::open_with_options(
                    path,
                    Arc::clone(&frame_pool),
                    OpenOptions::hardware(availability),
                    Some(input),
                    stop_flag.clone(),
                )?;
                Ok(Self {
                    path: path.to_owned(),
                    frame_pool,
                    hap_fallback_session,
                    interrupt: stop_flag,
                    fallback_decode_time: Duration::ZERO,
                    recovering_fallback_reason: None,
                    terminal_error: None,
                    backend: VideoBackend::Ffmpeg(source),
                })
            }
        }
    }

    pub fn read_frame(&mut self) -> Option<VideoFrame> {
        self.read_frame_with_cancel(None)
    }

    pub fn read_frame_cancellable(
        &mut self,
        stop_flag: &std::sync::atomic::AtomicBool,
    ) -> Option<VideoFrame> {
        self.read_frame_with_cancel(Some(stop_flag))
    }

    fn read_frame_with_cancel(
        &mut self,
        stop_flag: Option<&std::sync::atomic::AtomicBool>,
    ) -> Option<VideoFrame> {
        loop {
            if stop_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
                return None;
            }
            let read = match &mut self.backend {
                VideoBackend::Ffmpeg(source) => {
                    let frame = source.read_frame();
                    source.last_timings.decode += std::mem::take(&mut self.fallback_decode_time);
                    if frame.is_some() {
                        self.recovering_fallback_reason = None;
                    } else if let Some(reason) = self.recovering_fallback_reason.take() {
                        let message = format!(
                            "software fallback produced no recoverable frame after {reason}"
                        );
                        log::error!("Video decode: {message}");
                        self.terminal_error = Some(message);
                    }
                    return frame;
                }
                VideoBackend::Hap(source) => source.read_frame(stop_flag),
            };
            match read {
                HapRead::Frame(frame) => return Some(frame),
                HapRead::Eof => return None,
                HapRead::Fallback {
                    reason,
                    pts,
                    decode_time,
                } => {
                    if stop_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
                    {
                        return None;
                    }
                    self.hap_fallback_session.record(reason.clone());
                    log::warn!("Video decode: {reason}; reopening in FFmpeg software at {pts:.3}s");
                    let recovery_started = Instant::now();
                    let mut source = match FfmpegVideoSource::open_with_options(
                        &self.path,
                        Arc::clone(&self.frame_pool),
                        OpenOptions::software(Some(reason.clone())),
                        None,
                        self.interrupt.clone(),
                    ) {
                        Ok(source) => source,
                        Err(error) => {
                            let message = format!(
                                "mid-stream software reopen failed after {reason}: {error}"
                            );
                            log::error!("Video decode: {message}");
                            self.fallback_decode_time += recovery_started.elapsed();
                            self.terminal_error = Some(message);
                            return None;
                        }
                    };
                    if stop_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
                    {
                        return None;
                    }
                    if pts > 0.0
                        && let Err(error) = source.seek_before(pts)
                    {
                        let message =
                            format!("mid-stream software seek failed after {reason}: {error}");
                        log::error!("Video decode: {message}");
                        self.fallback_decode_time += recovery_started.elapsed();
                        self.terminal_error = Some(message);
                        return None;
                    }
                    self.fallback_decode_time += decode_time + recovery_started.elapsed();
                    self.recovering_fallback_reason = Some(reason);
                    self.backend = VideoBackend::Ffmpeg(source);
                }
            }
        }
    }

    pub fn seek_before(&mut self, secs: f64) -> anyhow::Result<()> {
        match &mut self.backend {
            VideoBackend::Ffmpeg(source) => source.seek_before(secs),
            VideoBackend::Hap(source) => source.seek_before(secs),
        }
    }

    pub fn duration_secs(&self) -> Option<f64> {
        match &self.backend {
            VideoBackend::Ffmpeg(source) => source.duration_secs(),
            VideoBackend::Hap(source) => source.duration_secs(),
        }
    }

    pub fn decode_path(&self) -> &str {
        match &self.backend {
            VideoBackend::Ffmpeg(source) => source.decode_path(),
            VideoBackend::Hap(_) => "hap gpu-native",
        }
    }

    pub fn width(&self) -> u32 {
        match &self.backend {
            VideoBackend::Ffmpeg(source) => source.width(),
            VideoBackend::Hap(source) => source.width,
        }
    }

    pub fn height(&self) -> u32 {
        match &self.backend {
            VideoBackend::Ffmpeg(source) => source.height(),
            VideoBackend::Hap(source) => source.height,
        }
    }

    pub fn dst_width(&self) -> u32 {
        self.width()
    }
    pub fn dst_height(&self) -> u32 {
        self.height()
    }

    pub fn last_timings(&self) -> VideoFrameTimings {
        let mut timings = match &self.backend {
            VideoBackend::Ffmpeg(source) => source.last_timings(),
            VideoBackend::Hap(source) => source.last_timings,
        };
        if matches!(&self.backend, VideoBackend::Hap(_)) {
            timings.decode += self.fallback_decode_time;
        }
        timings
    }

    pub fn fallback_reason(&self) -> Option<&str> {
        if let Some(error) = self.terminal_error.as_deref() {
            return Some(error);
        }
        match &self.backend {
            VideoBackend::Ffmpeg(source) => source.fallback_reason(),
            VideoBackend::Hap(_) => None,
        }
    }

    pub fn failed(&self) -> bool {
        self.terminal_error.is_some()
    }

    #[cfg(windows)]
    pub fn fallback_zero_copy(&mut self, reason: String) -> bool {
        match &mut self.backend {
            VideoBackend::Ffmpeg(source) => source.fallback_zero_copy(reason),
            VideoBackend::Hap(_) => false,
        }
    }

    #[cfg(windows)]
    pub fn mark_zero_copy_engaged(&mut self) {
        if let VideoBackend::Ffmpeg(source) = &mut self.backend {
            source.mark_zero_copy_engaged();
        }
    }
}

fn hap_unavailable_reason(no_hw_accel: bool, fallback_reason: Option<&str>) -> Option<&str> {
    if no_hw_accel {
        Some("GPU-native HAP disabled by QPLAYER_NO_HWACCEL=1")
    } else {
        fallback_reason
    }
}

const HAP_COMPRESSOR_NONE: u8 = 0xA0;
const HAP_COMPRESSOR_SNAPPY: u8 = 0xB0;
const HAP_COMPRESSOR_COMPLEX: u8 = 0xC0;
const HAP_MULTI_IMAGE: u8 = 0x0D;
const HAP_DECODE_INSTRUCTIONS: u8 = 0x01;
const HAP_CHUNK_COMPRESSORS: u8 = 0x02;
const HAP_CHUNK_SIZES: u8 = 0x03;
const HAP_CHUNK_OFFSETS: u8 = 0x04;
const HAP_CHUNK_NONE: u8 = 0x0A;
const HAP_CHUNK_SNAPPY: u8 = 0x0B;
const MAX_CONSECUTIVE_CORRUPT_HAP_PACKETS: u32 = 120;

fn hap_section(data: &[u8]) -> Result<(u8, &[u8], usize), String> {
    if data.len() < 4 {
        return Err(format!(
            "truncated HAP section header: have {} bytes",
            data.len()
        ));
    }
    let (body_len, header_len) = if data[..3] == [0, 0, 0] {
        if data.len() < 8 {
            return Err(format!(
                "truncated extended HAP section header: have {} bytes",
                data.len()
            ));
        }
        (
            u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize,
            8usize,
        )
    } else {
        (
            usize::from(data[0]) | (usize::from(data[1]) << 8) | (usize::from(data[2]) << 16),
            4usize,
        )
    };
    let end = header_len
        .checked_add(body_len)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| {
            format!(
                "truncated HAP section body: need {} bytes, have {}",
                header_len.saturating_add(body_len),
                data.len()
            )
        })?;
    Ok((data[3], &data[header_len..end], end))
}

/// Read Snappy's leading uncompressed-length varint without allocating its
/// declared output. `hap-parser` allocates from this value, so the HAP frame's
/// exact BC byte count must be checked first.
fn snappy_declared_len(data: &[u8]) -> Result<usize, String> {
    let mut value = 0u32;
    for shift in (0..=28).step_by(7) {
        let byte = *data
            .get(shift / 7)
            .ok_or_else(|| "truncated Snappy length".to_string())?;
        if shift == 28 && byte > 0x0F {
            return Err("invalid Snappy length".into());
        }
        value |= u32::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok(value as usize);
        }
    }
    Err("invalid Snappy length".into())
}

fn table_u32(table: &[u8], index: usize, name: &str) -> Result<usize, String> {
    let start = index
        .checked_mul(4)
        .ok_or_else(|| format!("HAP {name} table index overflow"))?;
    let bytes = table
        .get(start..start + 4)
        .ok_or_else(|| format!("truncated HAP {name} table"))?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()) as usize)
}

fn chunked_declared_len(data: &[u8], expected: usize) -> Result<usize, String> {
    let (section_type, instructions, frame_data_start) = hap_section(data)?;
    if section_type != HAP_DECODE_INSTRUCTIONS {
        return Err(format!(
            "expected HAP decode instructions, got section 0x{section_type:02X}"
        ));
    }
    let frame_data = &data[frame_data_start..];
    let (mut compressors, mut sizes, mut offsets) = (&[][..], &[][..], None);
    let mut position = 0;
    let mut instruction_sections = 0usize;
    while position < instructions.len() {
        instruction_sections += 1;
        if instruction_sections > 16 {
            return Err("HAP decode instructions contain too many sections".into());
        }
        let (section_type, body, consumed) = hap_section(&instructions[position..])?;
        match section_type {
            HAP_CHUNK_COMPRESSORS => compressors = body,
            HAP_CHUNK_SIZES => sizes = body,
            HAP_CHUNK_OFFSETS => offsets = Some(body),
            _ => {}
        }
        position += consumed;
    }
    if compressors.len() > 4_096 {
        return Err(format!(
            "HAP frame declares too many chunks: {} (limit 4096)",
            compressors.len()
        ));
    }

    let mut total = 0usize;
    let mut running_offset = 0usize;
    for (index, compressor) in compressors.iter().copied().enumerate() {
        let size = table_u32(sizes, index, "chunk-size")?;
        let offset = match offsets {
            Some(table) => table_u32(table, index, "chunk-offset")?,
            None => running_offset,
        };
        let end = offset
            .checked_add(size)
            .filter(|end| *end <= frame_data.len())
            .ok_or_else(|| "HAP chunk runs past the packet".to_string())?;
        let chunk = &frame_data[offset..end];
        let decoded = match compressor {
            HAP_CHUNK_NONE => chunk.len(),
            HAP_CHUNK_SNAPPY => snappy_declared_len(chunk)?,
            other => return Err(format!("unsupported HAP chunk compressor 0x{other:02X}")),
        };
        total = total
            .checked_add(decoded)
            .filter(|total| *total <= expected)
            .ok_or_else(|| format!("HAP chunks declare more than the expected {expected} bytes"))?;
        running_offset = running_offset
            .checked_add(size)
            .ok_or_else(|| "HAP chunk offset overflow".to_string())?;
    }
    Ok(total)
}

fn validate_hap_packet(
    data: &[u8],
    width: u32,
    height: u32,
    max_texture_dimension_2d: u32,
) -> Result<HapFormat, HapPacketError> {
    if width == 0 || height == 0 {
        return Err(HapPacketError::Corrupt("zero stream dimensions".into()));
    }
    if width > max_texture_dimension_2d || height > max_texture_dimension_2d {
        return Err(HapPacketError::Unsupported(format!(
            "dimensions {width}x{height} exceed the device texture limit {max_texture_dimension_2d}"
        )));
    }
    let (section_type, body, _) = hap_section(data).map_err(HapPacketError::Corrupt)?;
    if section_type == HAP_MULTI_IMAGE {
        return Err(HapPacketError::Unsupported(
            "HAP Q Alpha is not supported by the GPU-native path".into(),
        ));
    }
    let format = hap_parser::detect_format(data)
        .map_err(|error| HapPacketError::Unsupported(error.to_string()))?;
    if !matches!(
        format,
        HapFormat::RgbDxt1 | HapFormat::RgbaDxt5 | HapFormat::YcoCgDxt5
    ) {
        return Err(HapPacketError::Unsupported(format!(
            "unsupported HAP texture format: {format:?}"
        )));
    }
    let expected = format.frame_size(width, height);
    let declared = match section_type & 0xF0 {
        HAP_COMPRESSOR_NONE => body.len(),
        HAP_COMPRESSOR_SNAPPY => snappy_declared_len(body).map_err(HapPacketError::Corrupt)?,
        HAP_COMPRESSOR_COMPLEX => {
            chunked_declared_len(body, expected).map_err(HapPacketError::Corrupt)?
        }
        other => {
            return Err(HapPacketError::Unsupported(format!(
                "unsupported HAP compressor 0x{other:02X}"
            )));
        }
    };
    if declared != expected {
        return Err(HapPacketError::Corrupt(format!(
            "compressed size mismatch for {width}x{height} {format:?}: declared {declared}, expected {expected}"
        )));
    }
    Ok(format)
}

extern "C" fn ffmpeg_interrupt_callback(opaque: *mut c_void) -> c_int {
    if opaque.is_null() {
        return 0;
    }
    // SAFETY: `open_input` points this at an AtomicBool owned by an Arc that is
    // retained until after the corresponding AVFormatContext is closed.
    unsafe { (&*(opaque.cast::<AtomicBool>())).load(Ordering::Relaxed) as c_int }
}

fn open_input(
    path: &str,
    interrupt: Option<&Arc<AtomicBool>>,
) -> Result<format::context::Input, ffmpeg_next::Error> {
    let Some(stop) = interrupt else {
        return format::input(path);
    };
    let path = CString::new(path).map_err(|_| ffmpeg_next::Error::InvalidData)?;
    // ffmpeg-next 8.1's input_with_interrupt leaks its boxed callback. Point
    // directly into our retained Arc instead, so there is no callback allocation
    // to reclaim after repeated loop/fallback reopens.
    unsafe {
        let mut context = ffi::avformat_alloc_context();
        if context.is_null() {
            return Err(ffmpeg_next::Error::Unknown);
        }
        (*context).interrupt_callback = ffi::AVIOInterruptCB {
            callback: Some(ffmpeg_interrupt_callback),
            opaque: Arc::as_ptr(stop).cast_mut().cast(),
        };
        let opened = ffi::avformat_open_input(
            &mut context,
            path.as_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if opened < 0 {
            if !context.is_null() {
                ffi::avformat_close_input(&mut context);
            }
            return Err(ffmpeg_next::Error::from(opened));
        }
        let streams = ffi::avformat_find_stream_info(context, ptr::null_mut());
        if streams < 0 {
            ffi::avformat_close_input(&mut context);
            return Err(ffmpeg_next::Error::from(streams));
        }
        Ok(format::context::Input::wrap(context))
    }
}

impl HapVideoSource {
    fn probe(
        path: &str,
        unavailable: Option<&str>,
        max_texture_dimension_2d: u32,
        stop_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> anyhow::Result<HapProbe> {
        ffmpeg_next::init()?;
        let mut ictx = open_input(path, stop_flag.as_ref())?;
        if stop_flag
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
        {
            anyhow::bail!("video open cancelled");
        }
        let (stream_index, time_base, width, height, frame_duration, is_hap) = {
            let input = ictx
                .streams()
                .best(Type::Video)
                .ok_or_else(|| anyhow::anyhow!("no video stream found"))?;
            let parameters = input.parameters();
            let is_hap = parameters.id() == codec::Id::HAP;
            let (width, height) =
                unsafe { ((*parameters.as_ptr()).width, (*parameters.as_ptr()).height) };
            let rate = f64::from(input.avg_frame_rate());
            (
                input.index(),
                f64::from(input.time_base()),
                u32::try_from(width).unwrap_or(0),
                u32::try_from(height).unwrap_or(0),
                if rate > 0.0 { 1.0 / rate } else { 0.0 },
                is_hap,
            )
        };
        if !is_hap {
            return Ok(HapProbe::NotHap(ictx));
        }
        if let Some(reason) = unavailable {
            return Ok(HapProbe::Fallback(reason.to_owned()));
        }
        if width == 0 || height == 0 {
            return Ok(HapProbe::Fallback(
                "GPU-native HAP first packet invalid: zero stream dimensions".into(),
            ));
        }
        if width > max_texture_dimension_2d || height > max_texture_dimension_2d {
            return Ok(HapProbe::Fallback(format!(
                "GPU-native HAP dimensions {width}x{height} exceed the device texture limit {max_texture_dimension_2d}"
            )));
        }
        let started = Instant::now();
        let mut first_packet = None;
        for (stream, packet) in ictx.packets() {
            if stop_flag
                .as_ref()
                .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed))
            {
                anyhow::bail!("video open cancelled");
            }
            if stream.index() == stream_index {
                first_packet = Some(packet);
                break;
            }
        }
        let Some(packet) = first_packet else {
            return Ok(HapProbe::Fallback(
                "GPU-native HAP first packet unavailable".into(),
            ));
        };
        let data = packet
            .data()
            .ok_or_else(|| anyhow::anyhow!("GPU-native HAP first packet unavailable"))?;
        let format = match validate_hap_packet(data, width, height, max_texture_dimension_2d) {
            Ok(format) => format,
            Err(error) => {
                return Ok(HapProbe::Fallback(format!(
                    "GPU-native HAP first packet invalid: {error}"
                )));
            }
        };
        log::info!("Video decode: HAP GPU-native path selected ({:?})", format);
        Ok(HapProbe::Native(Box::new(Self {
            ictx,
            _interrupt: stop_flag,
            stream_index,
            time_base,
            width,
            height,
            frame_duration,
            format,
            max_texture_dimension_2d,
            next_pts: 0.0,
            pending: Some((packet, started.elapsed())),
            last_timings: VideoFrameTimings::default(),
            corrupt_warned: false,
            corrupt_streak: 0,
        })))
    }

    fn parse_packet(
        packet: &Packet,
        width: u32,
        height: u32,
        max_texture_dimension_2d: u32,
    ) -> Result<HapFrame, HapPacketError> {
        let data = packet
            .data()
            .ok_or_else(|| HapPacketError::Corrupt("empty packet".into()))?;
        let expected_format = validate_hap_packet(data, width, height, max_texture_dimension_2d)?;
        let frame = hap_parser::parse_frame(data)
            .map_err(|error| HapPacketError::Corrupt(error.to_string()))?;
        if frame.format != expected_format {
            return Err(HapPacketError::Corrupt(format!(
                "HAP texture format changed while parsing: expected {expected_format:?}, got {:?}",
                frame.format
            )));
        }
        Ok(frame)
    }

    fn frame_from_packet(&mut self, packet: Packet, parsed: HapFrame) -> VideoFrame {
        let packet_step = packet.duration() as f64 * self.time_base;
        let step = if packet_step > 0.0 {
            packet_step
        } else {
            self.frame_duration
        };
        let pts = packet
            .pts()
            .or(packet.dts())
            .map(|value| value as f64 * self.time_base)
            .unwrap_or(self.next_pts);
        self.next_pts = pts + step.max(0.0);
        VideoFrame::hap(self.width, self.height, pts, parsed.format, parsed.data)
    }

    fn read_frame(&mut self, stop_flag: Option<&std::sync::atomic::AtomicBool>) -> HapRead {
        self.last_timings = VideoFrameTimings::default();
        loop {
            if stop_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
                return HapRead::Eof;
            }
            if let Some((packet, elapsed)) = self.pending.take() {
                let started = Instant::now();
                match Self::parse_packet(
                    &packet,
                    self.width,
                    self.height,
                    self.max_texture_dimension_2d,
                ) {
                    Ok(parsed) => {
                        self.last_timings.decode = elapsed + started.elapsed();
                        return HapRead::Frame(self.frame_from_packet(packet, parsed));
                    }
                    Err(error) => {
                        self.last_timings.decode = elapsed + started.elapsed();
                        let pts = packet
                            .pts()
                            .or(packet.dts())
                            .map(|value| value as f64 * self.time_base)
                            .unwrap_or(self.next_pts)
                            .max(0.0);
                        return HapRead::Fallback {
                            reason: format!("GPU-native HAP first packet invalid: {error}"),
                            pts,
                            decode_time: self.last_timings.decode,
                        };
                    }
                }
            }
            let started = Instant::now();
            let mut next_packet = None;
            for (stream, packet) in self.ictx.packets() {
                if stop_flag.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed)) {
                    return HapRead::Eof;
                }
                if stream.index() == self.stream_index {
                    next_packet = Some(packet);
                    break;
                }
            }
            let Some(packet) = next_packet else {
                return HapRead::Eof;
            };
            match Self::parse_packet(
                &packet,
                self.width,
                self.height,
                self.max_texture_dimension_2d,
            ) {
                Ok(parsed) => {
                    self.last_timings.decode += started.elapsed();
                    self.corrupt_streak = 0;
                    if parsed.format != self.format {
                        let pts = packet
                            .pts()
                            .or(packet.dts())
                            .map(|value| value as f64 * self.time_base)
                            .unwrap_or(self.next_pts)
                            .max(0.0);
                        return HapRead::Fallback {
                            reason: format!(
                                "GPU-native HAP variant changed mid-stream: expected {:?}, got {:?}",
                                self.format, parsed.format
                            ),
                            pts,
                            decode_time: self.last_timings.decode,
                        };
                    }
                    return HapRead::Frame(self.frame_from_packet(packet, parsed));
                }
                Err(HapPacketError::Unsupported(reason)) => {
                    self.last_timings.decode += started.elapsed();
                    let pts = packet
                        .pts()
                        .or(packet.dts())
                        .map(|value| value as f64 * self.time_base)
                        .unwrap_or(self.next_pts)
                        .max(0.0);
                    return HapRead::Fallback {
                        reason: format!("GPU-native HAP variant changed mid-stream: {reason}"),
                        pts,
                        decode_time: self.last_timings.decode,
                    };
                }
                Err(HapPacketError::Corrupt(error)) => {
                    self.last_timings.decode += started.elapsed();
                    self.corrupt_streak += 1;
                    if !std::mem::replace(&mut self.corrupt_warned, true) {
                        log::warn!("Video decode: skipping corrupt HAP packet: {error}");
                    }
                    if self.corrupt_streak >= MAX_CONSECUTIVE_CORRUPT_HAP_PACKETS {
                        let pts = packet
                            .pts()
                            .or(packet.dts())
                            .map(|value| value as f64 * self.time_base)
                            .unwrap_or(self.next_pts)
                            .max(0.0);
                        return HapRead::Fallback {
                            reason: format!(
                                "GPU-native HAP encountered {} consecutive corrupt packets",
                                self.corrupt_streak
                            ),
                            pts,
                            decode_time: self.last_timings.decode,
                        };
                    }
                }
            }
        }
    }

    fn seek_before(&mut self, secs: f64) -> anyhow::Result<()> {
        let secs = secs.max(0.0);
        let ts = (secs * f64::from(ffi::AV_TIME_BASE)) as i64;
        self.ictx.seek(ts, ..ts)?;
        self.pending = None;
        self.next_pts = secs;
        self.corrupt_warned = false;
        self.corrupt_streak = 0;
        Ok(())
    }

    fn duration_secs(&self) -> Option<f64> {
        let duration = self.ictx.duration();
        (duration > 0).then(|| duration as f64 / f64::from(ffi::AV_TIME_BASE))
    }
}

/// The existing FFmpeg pixel decoder, kept as the universal fallback.
struct FfmpegVideoSource {
    /// Kept so a broken hwaccel can reopen the whole source in software.
    path: String,
    ictx: format::context::Input,
    // Must outlive `ictx`: FFmpeg's interrupt callback points into this Arc.
    interrupt: Option<Arc<AtomicBool>>,
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
    last_timings: VideoFrameTimings,
    _hw_format_state: Option<Box<HwFormatState>>,
    fallback_reason: Option<String>,
    #[cfg(windows)]
    direct_engaged: bool,
    /// "d3d12va zero-copy (<adapter>)" when this source opened with a direct
    /// pool; what `decode_path` reports once the canary engages.
    #[cfg(windows)]
    direct_label: Option<String>,
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
    #[cfg(windows)]
    ReopenReadback(String),
}

impl FfmpegVideoSource {
    fn open_with_options(
        path: &str,
        frame_pool: Arc<FramePool>,
        options: OpenOptions,
        mut input_context: Option<format::context::Input>,
        interrupt: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> anyhow::Result<Self> {
        let OpenOptions {
            route,
            availability,
            fallback_reason,
        } = options;
        let fallback_reason = fallback_reason.or_else(|| {
            availability
                .as_ref()
                .and_then(ZeroCopyAvailability::fallback_reason)
                .map(str::to_owned)
        });
        #[cfg(windows)]
        let mut fallback_reason = fallback_reason;
        // Escape hatch for A/B diagnosis on production machines.
        if route == OpenRoute::SoftwareOnly
            || std::env::var("QPLAYER_NO_HWACCEL").as_deref() == Ok("1")
        {
            return Self::open_with(
                path,
                None,
                frame_pool,
                no_direct_pool(),
                fallback_reason,
                input_context.take(),
                interrupt,
            );
        }
        #[cfg(windows)]
        if let Some(device) = availability
            .as_ref()
            .and_then(|value| value.device.as_ref())
        {
            let request = DirectPoolRequest::new(Arc::clone(device));
            match Self::open_with(
                path,
                Some(DIRECT_HW),
                Arc::clone(&frame_pool),
                Some(request.clone()),
                None,
                input_context.take(),
                interrupt.clone(),
            ) {
                Ok(source) => return Ok(source),
                Err(error) => {
                    let reason = request.failure().unwrap_or_else(|| error.to_string());
                    fallback_reason = Some(format!("shareable D3D12VA open failed: {reason}"));
                    log::warn!(
                        "Video zero-copy fallback: {}; retrying readback",
                        fallback_reason.as_deref().unwrap_or_default()
                    );
                }
            }
        }
        for &hw in HW_CANDIDATES {
            match Self::open_with(
                path,
                Some(hw),
                Arc::clone(&frame_pool),
                no_direct_pool(),
                fallback_reason.clone(),
                input_context.take(),
                interrupt.clone(),
            ) {
                Ok(src) => return Ok(src),
                Err(e) => log::warn!("Video decode: {} unavailable ({e})", hw.2),
            }
        }
        Self::open_with(
            path,
            None,
            frame_pool,
            no_direct_pool(),
            fallback_reason,
            input_context.take(),
            interrupt,
        )
    }

    fn open_with(
        path: &str,
        hw: Option<HwKind>,
        frame_pool: Arc<FramePool>,
        _direct_pool: DirectPoolOption,
        fallback_reason: Option<String>,
        input_context: Option<format::context::Input>,
        interrupt: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> anyhow::Result<Self> {
        ffmpeg_next::init()?;

        let ictx = match input_context {
            Some(input) => input,
            None => open_input(path, interrupt.as_ref())?,
        };
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
        let mut hw_format_state = hw.map(|(_, want, _)| {
            Box::new(HwFormatState {
                want,
                #[cfg(windows)]
                direct_pool: _direct_pool,
            })
        });
        let hw_pix_fmt = if let Some((device_type, pix_fmt, _)) = hw {
            unsafe {
                let ctx = decoder.as_mut_ptr();
                let mut device: *mut ffi::AVBufferRef = std::ptr::null_mut();
                // The zero-copy candidate adopts wgpu's ID3D12Device so decoded
                // resources are usable by the renderer without sharing; every
                // readback candidate creates its own device as before.
                #[cfg(windows)]
                if let Some(request) = hw_format_state
                    .as_ref()
                    .and_then(|state| state.direct_pool.as_ref())
                {
                    device = request
                        .interop_device()
                        .create_hw_device_ctx()
                        .map_err(|reason| {
                            anyhow::anyhow!("D3D12VA device adoption failed: {reason}")
                        })?;
                }
                if device.is_null()
                    && ffi::av_hwdevice_ctx_create(
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
                (*ctx).opaque = hw_format_state
                    .as_deref_mut()
                    .map_or(std::ptr::null_mut(), |state| {
                        std::ptr::from_mut(state).cast()
                    });
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
        #[cfg(windows)]
        let direct_label = hw_format_state
            .as_ref()
            .and_then(|state| state.direct_pool.as_ref())
            .map(|request| {
                format!(
                    "d3d12va zero-copy ({})",
                    request.interop_device().adapter_name()
                )
            });
        if hw.is_some() {
            // "attempting": a created device proves nothing until a hw frame
            // lands (codecs without hwaccel, e.g. Hap, never use it).
            log::info!("Video decode: attempting {hw_label}");
        } else {
            log::info!("Video decode: software");
        }

        Ok(Self {
            path: path.to_string(),
            interrupt,
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
            last_timings: VideoFrameTimings::default(),
            _hw_format_state: hw_format_state,
            fallback_reason,
            #[cfg(windows)]
            direct_engaged: false,
            #[cfg(windows)]
            direct_label,
        })
    }

    /// Handle the frame sitting in `self.decoded_frame`: download it from the
    /// GPU if it's a hw frame, then convert it to a `VideoFrame`.
    fn handle_decoded(&mut self) -> Decoded {
        if let Some(hw_fmt) = self.hw_pix_fmt
            && self.decoded_frame.format() == format::Pixel::from(hw_fmt)
        {
            #[cfg(windows)]
            if hw_fmt == ffi::AVPixelFormat::AV_PIX_FMT_D3D12
                && let Some(request) = self
                    ._hw_format_state
                    .as_ref()
                    .and_then(|state| state.direct_pool.clone())
            {
                return self.handle_direct_d3d12(&request);
            }
            // hw frames live in GPU memory and aren't readable via
            // `plane()`; download into the reusable CPU frame first.
            // `copy_props` carries PTS / color range / color space over,
            // which `is_full_range` / `is_bt709` read off the frame.
            unsafe {
                ffi::av_frame_unref(self.sw_frame.as_mut_ptr());
                let transfer_started = Instant::now();
                let transfer_result = ffi::av_hwframe_transfer_data(
                    self.sw_frame.as_mut_ptr(),
                    self.decoded_frame.as_ptr(),
                    0,
                );
                self.last_timings.hw_transfer += transfer_started.elapsed();
                if transfer_result < 0 {
                    if self.hw_checked {
                        log::warn!("Video decode: hw download failed, skipping frame");
                        return Decoded::Skip;
                    }
                    return Decoded::ReopenSoftware;
                }
                ffi::av_frame_copy_props(self.sw_frame.as_mut_ptr(), self.decoded_frame.as_ptr());
            }
            if !self.hw_checked {
                log::info!("Video decode: {} engaged", self.hw_label);
            }
            self.hw_checked = true;
            let plane_copy_started = Instant::now();
            let converted = convert_frame(
                &self.sw_frame,
                &mut self.scaler,
                &mut self.rgb_frame,
                self.dst_width,
                self.dst_height,
                self.time_base,
                &self.frame_pool,
            );
            self.last_timings.plane_copy += plane_copy_started.elapsed();
            return converted.map_or(Decoded::Skip, Decoded::Frame);
        }
        let plane_copy_started = Instant::now();
        let converted = convert_frame(
            &self.decoded_frame,
            &mut self.scaler,
            &mut self.rgb_frame,
            self.dst_width,
            self.dst_height,
            self.time_base,
            &self.frame_pool,
        );
        self.last_timings.plane_copy += plane_copy_started.elapsed();
        converted.map_or(Decoded::End, Decoded::Frame)
    }

    #[cfg(windows)]
    fn handle_direct_d3d12(&mut self, request: &DirectPoolRequest) -> Decoded {
        let canary_readback = if request.take_canary() {
            let transfer_started = Instant::now();
            let transfer_result = unsafe {
                ffi::av_frame_unref(self.sw_frame.as_mut_ptr());
                ffi::av_hwframe_transfer_data(
                    self.sw_frame.as_mut_ptr(),
                    self.decoded_frame.as_ptr(),
                    0,
                )
            };
            self.last_timings.hw_transfer += transfer_started.elapsed();
            if transfer_result < 0 {
                return Decoded::ReopenReadback("first-frame canary readback failed".into());
            }
            unsafe {
                ffi::av_frame_copy_props(self.sw_frame.as_mut_ptr(), self.decoded_frame.as_ptr());
            }
            let copy_started = Instant::now();
            let frame = convert_frame(
                &self.sw_frame,
                &mut self.scaler,
                &mut self.rgb_frame,
                self.dst_width,
                self.dst_height,
                self.time_base,
                &self.frame_pool,
            );
            self.last_timings.plane_copy += copy_started.elapsed();
            match frame {
                Some(frame) => Some(frame),
                None => {
                    return Decoded::ReopenReadback(
                        "first-frame canary CPU conversion failed".into(),
                    );
                }
            }
        } else {
            None
        };

        let pts = self.decoded_frame.timestamp().unwrap_or(0) as f64 * self.time_base;
        let full_range = is_full_range(&self.decoded_frame);
        let bt709 = is_bt709(&self.decoded_frame, self.decoded_frame.height());
        match unsafe {
            request.frame(
                self.decoded_frame.as_ptr(),
                self.dst_width,
                self.dst_height,
                pts,
                full_range,
                bt709,
                canary_readback,
            )
        } {
            Ok(frame) => {
                self.hw_checked = true;
                Decoded::Frame(frame)
            }
            Err(reason) => Decoded::ReopenReadback(reason),
        }
    }

    /// Pull the next decoded frame into `self.decoded_frame`. `false` at EOF.
    fn next_raw_frame(&mut self) -> bool {
        if self.eof {
            return false;
        }

        #[cfg(windows)]
        let direct_request = self
            ._hw_format_state
            .as_ref()
            .and_then(|state| state.direct_pool.clone());

        // Try draining already-decoded frames first.
        if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
            return true;
        }
        #[cfg(windows)]
        if direct_request
            .as_ref()
            .and_then(DirectPoolRequest::failure)
            .is_some()
        {
            return false;
        }

        for (stream, packet) in self.ictx.packets() {
            if stream.index() == self.stream_index {
                if self.decoder.send_packet(&packet).is_err() {
                    #[cfg(windows)]
                    if direct_request
                        .as_ref()
                        .and_then(DirectPoolRequest::failure)
                        .is_some()
                    {
                        return false;
                    }
                    continue;
                }
                if self.decoder.receive_frame(&mut self.decoded_frame).is_ok() {
                    return true;
                }
                #[cfg(windows)]
                if direct_request
                    .as_ref()
                    .and_then(DirectPoolRequest::failure)
                    .is_some()
                {
                    return false;
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

    #[cfg(windows)]
    fn direct_pool_failure(&self) -> Option<String> {
        self._hw_format_state
            .as_ref()
            .and_then(|state| state.direct_pool.as_ref())
            .and_then(DirectPoolRequest::failure)
    }

    #[cfg(windows)]
    fn reopen_d3d11_readback(&mut self, reason: String) -> bool {
        let path = self.path.clone();
        let options = OpenOptions::after_zero_copy_decline(reason);
        log::warn!(
            "Video zero-copy fallback: {}; retrying hardware readback",
            options.fallback_reason.as_deref().unwrap_or_default()
        );
        match Self::open_with_options(
            &path,
            Arc::clone(&self.frame_pool),
            options,
            None,
            self.interrupt.clone(),
        ) {
            Ok(source) => {
                *self = source;
                true
            }
            Err(error) => {
                log::error!("Video zero-copy fallback: readback reopen failed: {error}");
                self.eof = true;
                false
            }
        }
    }

    #[cfg(windows)]
    pub fn fallback_zero_copy(&mut self, reason: String) -> bool {
        self.reopen_d3d11_readback(reason)
    }

    #[cfg(windows)]
    pub fn mark_zero_copy_engaged(&mut self) {
        if !self.direct_engaged {
            self.direct_engaged = true;
            log::info!(
                "Video decode: {} engaged after canary",
                self.direct_label.as_deref().unwrap_or("d3d12va zero-copy")
            );
        }
    }

    /// Reopen the whole source in pure software mode (the broken-hwaccel
    /// escape hatch; playback position is lost, this is not a routine path).
    fn reopen_software(&mut self) -> bool {
        let path = self.path.clone();
        match Self::open_with_options(
            &path,
            Arc::clone(&self.frame_pool),
            OpenOptions::software(self.fallback_reason.clone()),
            None,
            self.interrupt.clone(),
        ) {
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
        self.last_timings = VideoFrameTimings::default();
        loop {
            let decode_started = Instant::now();
            let decoded = self.next_raw_frame();
            self.last_timings.decode += decode_started.elapsed();
            if !decoded {
                #[cfg(windows)]
                if let Some(reason) = self.direct_pool_failure()
                    && self.reopen_d3d11_readback(reason)
                {
                    continue;
                }
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
                #[cfg(windows)]
                Decoded::ReopenReadback(reason) => {
                    if !self.reopen_d3d11_readback(reason) {
                        return None;
                    }
                }
            }
        }
    }

    /// Seek to the keyframe at or before `secs` and reset decoder state, so the
    /// next `read_frame` calls decode forward from there. Used for cue seeking
    /// and frame-step-back.
    pub fn seek_before(&mut self, secs: f64) -> anyhow::Result<()> {
        let ts = (secs.max(0.0) * f64::from(ffmpeg_next::ffi::AV_TIME_BASE)) as i64;
        self.ictx.seek(ts, ..ts)?;
        self.decoder.flush();
        self.eof = false;
        self.eof_sent = false;
        Ok(())
    }

    /// Container duration in seconds, when FFmpeg reports one.
    pub fn duration_secs(&self) -> Option<f64> {
        let duration = self.ictx.duration();
        (duration > 0).then(|| duration as f64 / f64::from(ffi::AV_TIME_BASE))
    }

    /// Which decode path was chosen at open: `hardware (<api>)` or `software`.
    /// The active decode path. Truthful once frames have flowed: a
    /// created-but-unused hw device (e.g. Hap has no hwaccel) reports
    /// "software" until the first hw frame actually downloads.
    pub fn decode_path(&self) -> &str {
        #[cfg(windows)]
        if self.direct_engaged {
            return self.direct_label.as_deref().unwrap_or("d3d12va zero-copy");
        }
        if self.hw_checked {
            self.hw_label
        } else {
            "software"
        }
    }
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn last_timings(&self) -> VideoFrameTimings {
        self.last_timings
    }
    pub fn fallback_reason(&self) -> Option<&str> {
        self.fallback_reason.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FramePixels;
    use hap_qt::{HapFormat as QtHapFormat, HapFrameEncoder, QtHapWriter, VideoConfig};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_MEDIA_ID: AtomicU64 = AtomicU64::new(0);

    struct TempHap {
        dir: PathBuf,
        path: PathBuf,
    }

    impl TempHap {
        fn new(format: QtHapFormat, width: u32, height: u32, frames: u32) -> Self {
            let id = NEXT_MEDIA_ID.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("cuepool-hap-test-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("fixture.mov");
            let encoder = (!matches!(format, QtHapFormat::HapA | QtHapFormat::Hap7))
                .then(|| HapFrameEncoder::new(format, width, height).unwrap());
            let mut writer =
                QtHapWriter::create(&path, VideoConfig::new(width, height, 50.0, format)).unwrap();
            for index in 0..frames {
                let mut rgba = vec![0; (width * height * 4) as usize];
                for pixel in rgba.chunks_exact_mut(4) {
                    pixel.copy_from_slice(&[
                        32 + index as u8,
                        96,
                        160,
                        if format == QtHapFormat::Hap5 {
                            128
                        } else {
                            255
                        },
                    ]);
                }
                let encoded = match format {
                    // hap-qt advertises these variants but does not yet encode
                    // them. A single uncompressed BC block is enough to verify
                    // CuePool's explicit software-fallback routing.
                    QtHapFormat::HapA => [vec![8, 0, 0, 0xA1], vec![0; 8]].concat(),
                    QtHapFormat::Hap7 => [vec![16, 0, 0, 0xAC], vec![0; 16]].concat(),
                    _ => encoder.as_ref().unwrap().encode(&rgba).unwrap(),
                };
                writer.write_frame(&encoded).unwrap();
            }
            writer.finalize().unwrap();
            Self { dir, path }
        }

        fn with_midstream_alpha() -> Self {
            let id = NEXT_MEDIA_ID.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("cuepool-hap-test-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("fixture.mov");
            let encoder = HapFrameEncoder::new(QtHapFormat::Hap1, 8, 8).unwrap();
            let mut writer =
                QtHapWriter::create(&path, VideoConfig::new(8, 8, 50.0, QtHapFormat::Hap1))
                    .unwrap();
            writer
                .write_frame(&encoder.encode(&[64; 8 * 8 * 4]).unwrap())
                .unwrap();
            writer
                .write_frame(&[vec![8, 0, 0, 0xA1], vec![0; 8]].concat())
                .unwrap();
            writer.finalize().unwrap();
            Self { dir, path }
        }

        fn with_midstream_supported_format_change() -> Self {
            let id = NEXT_MEDIA_ID.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("cuepool-hap-test-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("fixture.mov");
            let first = HapFrameEncoder::new(QtHapFormat::Hap1, 8, 8).unwrap();
            let second = HapFrameEncoder::new(QtHapFormat::Hap5, 8, 8).unwrap();
            let mut writer =
                QtHapWriter::create(&path, VideoConfig::new(8, 8, 50.0, QtHapFormat::Hap1))
                    .unwrap();
            writer
                .write_frame(&first.encode(&[64; 8 * 8 * 4]).unwrap())
                .unwrap();
            writer
                .write_frame(&second.encode(&[128; 8 * 8 * 4]).unwrap())
                .unwrap();
            writer.finalize().unwrap();
            Self { dir, path }
        }

        fn with_malformed_first_packet() -> Self {
            let id = NEXT_MEDIA_ID.fetch_add(1, Ordering::Relaxed);
            let dir =
                std::env::temp_dir().join(format!("cuepool-hap-test-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("fixture.mov");
            let mut writer =
                QtHapWriter::create(&path, VideoConfig::new(8, 8, 50.0, QtHapFormat::Hap1))
                    .unwrap();
            writer.write_frame(&[1, 2, 3]).unwrap();
            writer.finalize().unwrap();
            Self { dir, path }
        }
    }

    impl Drop for TempHap {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn interruptible_input_does_not_retain_the_stop_flag_after_drop() {
        let media = TempHap::new(QtHapFormat::Hap1, 4, 4, 1);
        let stop = Arc::new(AtomicBool::new(false));

        for _ in 0..8 {
            drop(open_input(media.path.to_str().unwrap(), Some(&stop)).unwrap());
            assert_eq!(Arc::strong_count(&stop), 1);
        }
    }

    #[test]
    fn zero_copy_is_enabled_only_by_exact_opt_in() {
        assert_eq!(
            ZeroCopyPreference::from_value(Some("1")),
            ZeroCopyPreference::Enabled
        );
        for value in [
            None,
            Some(""),
            Some("0"),
            Some("true"),
            Some("yes"),
            Some(" 1"),
        ] {
            assert_eq!(
                ZeroCopyPreference::from_value(value),
                ZeroCopyPreference::Disabled
            );
        }
    }

    #[test]
    fn hap_disable_reasons_are_explicit_and_prioritise_the_escape_hatch() {
        assert_eq!(hap_unavailable_reason(false, None), None);
        assert_eq!(
            hap_unavailable_reason(
                false,
                Some("GPU-native HAP unavailable: GPU device lacks BC texture compression")
            ),
            Some("GPU-native HAP unavailable: GPU device lacks BC texture compression")
        );
        assert_eq!(
            hap_unavailable_reason(
                true,
                Some("GPU-native HAP unavailable: GPU device lacks BC texture compression")
            ),
            Some("GPU-native HAP disabled by QPLAYER_NO_HWACCEL=1")
        );
    }

    #[test]
    fn common_hap_variants_use_packet_native_frames_with_pts_and_seek() {
        for (qt_format, expected) in [
            (QtHapFormat::Hap1, HapFormat::RgbDxt1),
            (QtHapFormat::Hap5, HapFormat::RgbaDxt5),
            (QtHapFormat::HapY, HapFormat::YcoCgDxt5),
        ] {
            let media = TempHap::new(qt_format, 8, 8, 5);
            let mut source = VideoSource::open_with_pool_and_hap(
                media.path.to_str().unwrap(),
                Arc::new(FramePool::new(0)),
                Some(wgpu::Limits::default().max_texture_dimension_2d),
            )
            .unwrap();

            assert_eq!(source.decode_path(), "hap gpu-native");
            assert_eq!((source.width(), source.height()), (8, 8));
            assert!(source.fallback_reason().is_none());
            let first = source.read_frame().unwrap();
            assert_eq!(first.pts, 0.0);
            let FramePixels::Hap {
                format,
                data,
                padded_width,
                padded_height,
            } = first.pixels
            else {
                panic!("expected HAP pixels");
            };
            assert_eq!(format, expected);
            assert_eq!(data.len(), expected.frame_size(8, 8));
            assert_eq!((padded_width, padded_height), (8, 8));
            assert!((source.read_frame().unwrap().pts - 0.02).abs() < 1e-6);

            source.seek_before(0.04).unwrap();
            let sought = source.read_frame().unwrap();
            assert!(sought.pts <= 0.04 + 1e-6, "PTS was {}", sought.pts);
        }
    }

    #[test]
    fn unavailable_or_unsupported_hap_uses_software_with_a_reason() {
        let common = TempHap::new(QtHapFormat::Hap1, 8, 8, 1);
        let source = VideoSource::open_with_pool_and_hap(
            common.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            None,
        )
        .unwrap();
        assert_eq!(source.decode_path(), "software");
        assert_eq!(
            source.fallback_reason(),
            Some("GPU-native HAP unavailable: GPU device lacks BC texture compression")
        );

        let unsupported = TempHap::new(QtHapFormat::HapA, 4, 4, 1);
        let source = VideoSource::open_with_pool_and_hap(
            unsupported.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            Some(wgpu::Limits::default().max_texture_dimension_2d),
        )
        .unwrap();
        assert_eq!(source.decode_path(), "software");
        assert!(source.fallback_reason().unwrap().contains("AlphaRgtc1"));
    }

    #[test]
    fn zero_frame_initial_fallback_is_a_failure_not_eof() {
        let media = TempHap::with_malformed_first_packet();
        let mut source = VideoSource::open_with_pool_and_hap(
            media.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            Some(8_192),
        )
        .unwrap();

        assert_eq!(source.decode_path(), "software");
        assert!(source.read_frame().is_none());
        assert!(source.failed());
        assert!(
            source
                .fallback_reason()
                .unwrap()
                .contains("no recoverable frame")
        );
    }

    #[test]
    fn malformed_hap_packet_is_rejected_before_gpu_upload() {
        let packet = Packet::copy(&[1, 2, 3]);
        assert!(HapVideoSource::parse_packet(&packet, 8, 8, 8_192).is_err());
    }

    #[test]
    fn oversized_snappy_output_is_rejected_before_decompression() {
        let packet = Packet::copy(&[
            5, 0, 0, 0xBB, // five-byte Snappy BC1 section
            0xFF, 0xFF, 0xFF, 0xFF, 0x0F, // declares u32::MAX output bytes
        ]);
        let error = HapVideoSource::parse_packet(&packet, 8, 8, 8_192).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("declared 4294967295, expected 32")
        );
    }

    #[test]
    fn bounded_preflight_accepts_complex_chunked_hap() {
        fn section(section_type: u8, body: &[u8]) -> Vec<u8> {
            let len = body.len();
            let mut result = vec![len as u8, (len >> 8) as u8, (len >> 16) as u8, section_type];
            result.extend_from_slice(body);
            result
        }

        let compressors = section(HAP_CHUNK_COMPRESSORS, &[HAP_CHUNK_NONE]);
        let sizes = section(HAP_CHUNK_SIZES, &32u32.to_le_bytes());
        let instructions = section(HAP_DECODE_INSTRUCTIONS, &[compressors, sizes].concat());
        let packet = Packet::copy(&section(
            HAP_COMPRESSOR_COMPLEX | 0x0B,
            &[instructions, vec![0x55; 32]].concat(),
        ));
        let parsed = HapVideoSource::parse_packet(&packet, 8, 8, 8_192).unwrap();
        assert_eq!(parsed.format, HapFormat::RgbDxt1);
        assert_eq!(parsed.data, vec![0x55; 32]);
    }

    #[test]
    fn complex_hap_rejects_excessive_chunk_counts() {
        fn section(section_type: u8, body: &[u8]) -> Vec<u8> {
            let len = body.len();
            let mut result = vec![len as u8, (len >> 8) as u8, (len >> 16) as u8, section_type];
            result.extend_from_slice(body);
            result
        }

        let compressors = section(HAP_CHUNK_COMPRESSORS, &vec![HAP_CHUNK_NONE; 4_097]);
        let sizes = section(HAP_CHUNK_SIZES, &vec![0; 4_097 * 4]);
        let body = section(HAP_DECODE_INSTRUCTIONS, &[compressors, sizes].concat());
        assert!(
            chunked_declared_len(&body, 32)
                .unwrap_err()
                .contains("too many chunks")
        );
    }

    #[test]
    fn cancellation_stops_hap_scanning_without_consuming_a_pending_frame() {
        let media = TempHap::new(QtHapFormat::Hap1, 8, 8, 1);
        let mut source = VideoSource::open_with_pool_and_hap(
            media.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            Some(8_192),
        )
        .unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(true));
        assert!(source.read_frame_cancellable(&stop).is_none());
        stop.store(false, std::sync::atomic::Ordering::Relaxed);
        assert!(source.read_frame_cancellable(&stop).is_some());
    }

    #[test]
    fn cancellation_stops_hap_open_before_packet_preflight() {
        let media = TempHap::new(QtHapFormat::Hap1, 8, 8, 1);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let result = VideoSource::open_with_acceleration_and_session_cancellable(
            media.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            ZeroCopyAvailability::declined("test"),
            HapAcceleration::available(8_192),
            HapFallbackSession::default(),
            stop,
        );

        assert!(result.err().unwrap().to_string().contains("cancelled"));
    }

    #[test]
    fn unsupported_midstream_variant_reopens_the_software_decoder() {
        let media = TempHap::with_midstream_alpha();
        let mut source = VideoSource::open_with_pool_and_hap(
            media.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            Some(8_192),
        )
        .unwrap();
        assert!(matches!(
            source.read_frame().unwrap().pixels,
            FramePixels::Hap { .. }
        ));
        assert!(source.read_frame().is_none());
        assert_eq!(source.decode_path(), "software");
        assert!(source.failed());
        assert!(source.last_timings().decode > Duration::ZERO);
        assert!(
            source
                .fallback_reason()
                .unwrap()
                .contains("variant changed mid-stream")
        );
    }

    #[test]
    fn supported_midstream_format_change_reopens_the_software_decoder() {
        let media = TempHap::with_midstream_supported_format_change();
        let mut source = VideoSource::open_with_pool_and_hap(
            media.path.to_str().unwrap(),
            Arc::new(FramePool::new(0)),
            Some(8_192),
        )
        .unwrap();
        assert!(matches!(
            source.read_frame().unwrap().pixels,
            FramePixels::Hap { .. }
        ));

        assert!(source.read_frame().is_none());

        assert_eq!(source.decode_path(), "software");
        assert!(source.failed());
        assert!(
            source
                .fallback_reason()
                .unwrap()
                .contains("variant changed mid-stream")
        );
    }

    #[test]
    fn cue_session_keeps_software_fallback_across_reopens() {
        let media = TempHap::with_midstream_supported_format_change();
        let pool = Arc::new(FramePool::new(0));
        let acceleration = HapAcceleration::available(8_192);
        let session = HapFallbackSession::default();
        let mut first = VideoSource::open_with_hap_acceleration_and_session(
            media.path.to_str().unwrap(),
            Arc::clone(&pool),
            acceleration.clone(),
            session.clone(),
        )
        .unwrap();
        assert_eq!(first.decode_path(), "hap gpu-native");
        assert!(first.read_frame().is_some());
        assert!(first.read_frame().is_none());

        let same_session = VideoSource::open_with_hap_acceleration_and_session(
            media.path.to_str().unwrap(),
            Arc::clone(&pool),
            acceleration.clone(),
            session,
        )
        .unwrap();
        assert_eq!(same_session.decode_path(), "software");
        assert!(
            same_session
                .fallback_reason()
                .unwrap()
                .contains("after prior fallback")
        );

        let fresh_session = VideoSource::open_with_hap_acceleration_and_session(
            media.path.to_str().unwrap(),
            pool,
            acceleration,
            HapFallbackSession::default(),
        )
        .unwrap();
        assert_eq!(fresh_session.decode_path(), "hap gpu-native");
    }

    #[test]
    fn native_hap_rejects_dimensions_above_the_device_limit() {
        let encoded = HapFrameEncoder::new(QtHapFormat::Hap1, 8, 8)
            .unwrap()
            .encode(&[0; 8 * 8 * 4])
            .unwrap();
        let packet = Packet::copy(&encoded);
        let error = HapVideoSource::parse_packet(&packet, 8, 8, 4).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("exceed the device texture limit 4")
        );
    }

    #[test]
    fn declined_zero_copy_retries_hardware_without_a_direct_pool() {
        let options = OpenOptions::after_zero_copy_decline("canary mismatch".into());

        assert_eq!(options.route, OpenRoute::HardwareCandidates);
        assert!(options.availability.is_none());
    }

    #[test]
    fn declined_zero_copy_preserves_the_reason() {
        let options = OpenOptions::after_zero_copy_decline("canary mismatch".into());

        assert_eq!(
            options.fallback_reason.as_deref(),
            Some("shareable D3D12VA pool rejected: canary mismatch")
        );
    }

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
