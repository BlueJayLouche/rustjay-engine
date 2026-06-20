//! Live sampler: capture from a `rustjay-io` input, encode to HAP5, assign to a pad.
//!
//! Compiles only when the `capture` feature is enabled. The default build leaves
//! this out so the example stays lean on headless/Pi targets.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hap_wgpu::{EncodeConfig, EncodeQuality, HapFormat, HapVideoEncoder};
use rustjay_io::InputManager;

/// Current state of the live sampler.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SamplerState {
    #[default]
    Idle,
    Recording,
    Encoding,
    Error,
}

/// A frame captured from the input source.
struct CapturedFrame {
    rgba: Vec<u8>,
}

/// Result returned by the background encoding thread.
struct EncodeResult {
    target_pad: usize,
    path: PathBuf,
}

/// Live-sampling FSM: Idle → Recording N frames → Encoding (thread) → assigned.
pub struct LiveSampler {
    state: SamplerState,
    input: InputManager,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    recording: Option<Recording>,
    encoding_handle: Option<std::thread::JoinHandle<Result<EncodeResult, String>>>,
    assigned: Option<EncodeResult>,
    default_resolution: (u32, u32),
    default_fps: f32,
}

struct Recording {
    target_pad: usize,
    frames_remaining: u32,
    width: u32,
    height: u32,
    fps: f32,
    frames: Vec<CapturedFrame>,
}

impl LiveSampler {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        let mut input = InputManager::new();
        input.initialize(&device, &queue);
        input.begin_refresh_devices();

        Self {
            state: SamplerState::Idle,
            input,
            device: Arc::clone(&device),
            queue: Arc::clone(&queue),
            recording: None,
            encoding_handle: None,
            assigned: None,
            default_resolution: (1280, 720),
            default_fps: 30.0,
        }
    }

    pub fn state(&self) -> SamplerState {
        self.state
    }

    /// Poll the input manager for device discovery. Call once per `prepare`.
    pub fn poll_devices(&mut self) {
        let _ = self.input.poll_discovery();
    }

    /// Start recording a fixed number of frames to the given pad.
    ///
    /// Uses the first available webcam source. Syphon/NDI can be added later;
    /// for now the engine's own Input tab is the recommended way to preview
    /// those sources, while this sampler captures from a local camera.
    pub fn start_recording(&mut self, target_pad: usize, frame_count: u32) -> anyhow::Result<()> {
        if self.state != SamplerState::Idle {
            anyhow::bail!("Sampler is not idle");
        }

        let (width, height) = self.default_resolution;
        let fps = self.default_fps;

        // Start the default webcam (device 0). Fall back to the platform's first
        // discovered camera if device 0 is not in the list.
        let device_index = self
            .input
            .webcam_devices()
            .iter()
            .enumerate()
            .next()
            .map(|(i, _)| i)
            .unwrap_or(0);

        if let Err(e) = self
            .input
            .start_webcam(device_index, width, height, fps as u32)
        {
            anyhow::bail!("Failed to start webcam {device_index}: {e}");
        }

        self.state = SamplerState::Recording;
        self.recording = Some(Recording {
            target_pad,
            frames_remaining: frame_count,
            width,
            height,
            fps,
            frames: Vec::with_capacity(frame_count as usize),
        });

        log::info!(
            "VP-404 live sampler: recording {frame_count} frames to pad {target_pad} ({width}x{height} @ {fps} fps)"
        );
        Ok(())
    }

    /// Stop/cancel the current recording.
    pub fn cancel(&mut self) {
        self.input.stop();
        self.recording = None;
        self.encoding_handle = None;
        self.assigned = None;
        self.state = SamplerState::Idle;
        log::info!("VP-404 live sampler: cancelled");
    }

    /// Poll the input source and capture frames while recording.
    ///
    /// Call from `prepare` every frame. Returns `(pad_index, path)` if a new
    /// sample was just assigned (the caller should load it into the bank).
    pub fn update(&mut self) -> Option<(usize, PathBuf)> {
        self.poll_devices();
        self.input.update();

        // If encoding just finished, pick up the result.
        if let Some(handle) = self.encoding_handle.as_ref() {
            if handle.is_finished() {
                let handle = self.encoding_handle.take().unwrap();
                match handle.join() {
                    Ok(Ok(result)) => {
                        self.state = SamplerState::Idle;
                        self.assigned = Some(result);
                    }
                    Ok(Err(e)) => {
                        log::error!("VP-404 live sampler encode failed: {e}");
                        self.state = SamplerState::Error;
                    }
                    Err(_) => {
                        log::error!("VP-404 live sampler encode thread panicked");
                        self.state = SamplerState::Error;
                    }
                }
            }
        }

        if let Some(result) = self.assigned.take() {
            return Some((result.target_pad, result.path));
        }

        // Capture frames when recording.
        if self.state == SamplerState::Recording {
            if let Some(frame) = self.take_frame() {
                if let Some(rec) = self.recording.as_mut() {
                    rec.frames.push(frame);
                    rec.frames_remaining = rec.frames_remaining.saturating_sub(1);

                    if rec.frames_remaining == 0 {
                        self.finish_recording();
                    }
                }
            }
        }

        None
    }

    fn take_frame(&mut self) -> Option<CapturedFrame> {
        if !self.input.has_frame() {
            return None;
        }
        let (width, height) = self.input.resolution();
        let bgra = self.input.take_frame()?;
        if bgra.len() < (width * height * 4) as usize {
            return None;
        }

        // rustjay-io CPU sources are BGRA; hap-wgpu expects RGBA.
        let rgba = bgra_to_rgba(&bgra[..(width * height * 4) as usize]);
        Some(CapturedFrame { rgba })
    }

    fn finish_recording(&mut self) {
        let Some(rec) = self.recording.take() else {
            return;
        };
        self.input.stop();
        self.state = SamplerState::Encoding;

        let path = sample_path(rec.target_pad);
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let width = rec.width;
        let height = rec.height;
        let fps = rec.fps;
        let target_pad = rec.target_pad;
        let frames: Vec<Vec<u8>> = rec.frames.into_iter().map(|f| f.rgba).collect();
        let frame_count = frames.len();
        let path_display = path.display().to_string();

        let handle = std::thread::spawn(move || {
            encode_frames(device, queue, &frames, width, height, fps, &path)
                .map(|_| EncodeResult { target_pad, path })
                .map_err(|e| e.to_string())
        });
        self.encoding_handle = Some(handle);

        log::info!(
            "VP-404 live sampler: encoding {frame_count} frames for pad {target_pad} -> {path_display}"
        );
    }
}

fn sample_path(pad_index: usize) -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("samples")
        .join("recorded");
    let _ = std::fs::create_dir_all(&dir);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    dir.join(format!("pad{pad_index}_rec_{timestamp}.mov"))
}

fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut rgba = vec![0u8; bgra.len()];
    for (src, dst) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    rgba
}

fn encode_frames(
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    frames: &[Vec<u8>],
    width: u32,
    height: u32,
    fps: f32,
    path: &Path,
) -> anyhow::Result<()> {
    if frames.is_empty() {
        anyhow::bail!("no frames to encode");
    }

    let mut encoder = HapVideoEncoder::new(device, queue);
    encoder.init_gpu(width, height);

    let config = EncodeConfig::new(width, height, fps, frames.len() as u32)
        .with_format(HapFormat::Hap5)
        .with_quality(EncodeQuality::Fast)
        .with_snappy(true);

    encoder.encode_from_frames(path, config, frames.iter().cloned())?;
    Ok(())
}
