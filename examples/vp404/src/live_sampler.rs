//! Live sampler: capture frames from the engine's active input via async
//! GPU→CPU readback, then encode to HAP5 and assign to a pad.
//!
//! Previous design opened its own InputManager (second connection to the same
//! source), which failed for webcam (exclusive device access) and Syphon (GPU
//! texture — `take_frame()` always returned None). This version instead reads
//! `ctx.input.texture` from the render hook — the engine already decodes and
//! uploads the frame for us, regardless of source type.
//!
//! Flow (one frame at a time):
//!   render()  → submit_readback(texture, device, queue)
//!                → copy_texture_to_buffer + map_async
//!   prepare() → poll_readback()
//!                → if MAP_READY: strip row padding, push BGRA→RGBA frame
//!                → if frames_remaining == 0: spawn encoding thread

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use hap_wgpu::{EncodeConfig, EncodeQuality, HapFormat, HapVideoEncoder};

const MAP_PENDING: u8 = 0;
const MAP_READY: u8 = 1;
const MAP_FAILED: u8 = 2;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SamplerState {
    #[default]
    Idle,
    Recording,
    Encoding,
    Error,
}

struct EncodeResult {
    target_pad: usize,
    path: PathBuf,
}

struct Recording {
    target_pad: usize,
    frames_remaining: u32,
    width: u32,
    height: u32,
    fps: f32,
    frames: Vec<Vec<u8>>, // RGBA
}

pub struct LiveSampler {
    state: SamplerState,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    recording: Option<Recording>,
    encoding_handle: Option<JoinHandle<Result<EncodeResult, String>>>,
    assigned: Option<EncodeResult>,
    // Async GPU→CPU readback
    readback_buf: Option<wgpu::Buffer>,
    readback_size: (u32, u32),
    map_state: Arc<AtomicU8>,
    readback_in_flight: bool,
}

impl LiveSampler {
    pub fn new(device: Arc<wgpu::Device>, queue: Arc<wgpu::Queue>) -> Self {
        Self {
            state: SamplerState::Idle,
            device,
            queue,
            recording: None,
            encoding_handle: None,
            assigned: None,
            readback_buf: None,
            readback_size: (0, 0),
            map_state: Arc::new(AtomicU8::new(MAP_PENDING)),
            readback_in_flight: false,
        }
    }

    pub fn state(&self) -> SamplerState {
        self.state
    }

    /// Begin recording `frame_count` frames into pad `target_pad`.
    ///
    /// `width`/`height`/`fps` should come from `engine.input` so we know the
    /// expected frame dimensions before the first readback arrives.
    pub fn start_recording(
        &mut self,
        target_pad: usize,
        frame_count: u32,
        width: u32,
        height: u32,
        fps: f32,
    ) -> anyhow::Result<()> {
        if self.state != SamplerState::Idle {
            anyhow::bail!("Sampler is not idle");
        }
        // Fall back to a safe default if the engine hasn't received a frame yet.
        let width = width.max(1280);
        let height = height.max(720);
        let fps = if fps > 0.0 { fps } else { 30.0 };

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
            "VP-404 live sampler: recording {frame_count} frames to pad {target_pad} \
             ({width}x{height} @ {fps:.1} fps)"
        );
        Ok(())
    }

    /// Cancel an in-progress recording or encoding.
    pub fn cancel(&mut self) {
        self.recording = None;
        self.encoding_handle = None;
        self.assigned = None;
        self.readback_in_flight = false;
        self.map_state = Arc::new(AtomicU8::new(MAP_PENDING));
        self.state = SamplerState::Idle;
        log::info!("VP-404 live sampler: cancelled");
    }

    /// Submit an async GPU→CPU readback of `texture`.
    ///
    /// Called once per frame from `Vp404::render()` when recording is active.
    /// Creates its own encoder so `map_async` can be called right after submit
    /// (the main render encoder must not yet be submitted at this point).
    /// Skips submission if a previous readback is still in flight.
    pub fn submit_readback(&mut self, texture: &wgpu::Texture) {
        if self.state != SamplerState::Recording || self.readback_in_flight {
            return;
        }
        let width = texture.width();
        let height = texture.height();

        // Reallocate the staging buffer if the texture size changed.
        let bytes_per_row = (width * 4).div_ceil(256) * 256;
        if self.readback_buf.is_none() || self.readback_size != (width, height) {
            self.readback_buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VP404 Sampler Readback"),
                size: bytes_per_row as u64 * height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
            self.readback_size = (width, height);
        }
        let buf = self.readback_buf.as_ref().unwrap();

        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("VP404 Sampler Copy"),
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        );
        self.queue.submit(std::iter::once(enc.finish()));

        let state = Arc::new(AtomicU8::new(MAP_PENDING));
        self.map_state = Arc::clone(&state);
        buf.slice(..).map_async(wgpu::MapMode::Read, move |res| {
            state.store(if res.is_ok() { MAP_READY } else { MAP_FAILED }, Ordering::SeqCst);
        });
        self.readback_in_flight = true;
    }

    /// Poll the in-flight readback and, when ready, push the frame into the
    /// recording buffer. Calls `finish_recording()` when all frames are captured.
    ///
    /// Call from `Vp404::prepare()` each frame.
    pub fn poll_readback(&mut self) {
        if !self.readback_in_flight {
            return;
        }
        self.device.poll(wgpu::PollType::Poll).ok();

        match self.map_state.load(Ordering::SeqCst) {
            MAP_READY => {
                let (width, height) = self.readback_size;
                let bytes_per_row = (width * 4).div_ceil(256) * 256;
                if let Some(buf) = &self.readback_buf {
                    let slice = buf.slice(..);
                    let data = slice.get_mapped_range();
                    // Strip row padding and swap BGRA→RGBA.
                    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                    for row in 0..height {
                        let start = (row * bytes_per_row) as usize;
                        let row_data = &data[start..start + (width * 4) as usize];
                        for px in row_data.chunks_exact(4) {
                            rgba.push(px[2]); // R ← B
                            rgba.push(px[1]); // G
                            rgba.push(px[0]); // B ← R
                            rgba.push(px[3]); // A
                        }
                    }
                    drop(data);
                    buf.unmap();
                    self.readback_in_flight = false;

                    if let Some(rec) = self.recording.as_mut() {
                        rec.frames.push(rgba);
                        rec.frames_remaining = rec.frames_remaining.saturating_sub(1);
                        if rec.frames_remaining == 0 {
                            self.finish_recording();
                        }
                    }
                }
            }
            MAP_FAILED => {
                log::error!("VP-404 live sampler: readback map failed");
                self.readback_in_flight = false;
                self.state = SamplerState::Error;
            }
            _ => {} // MAP_PENDING
        }
    }

    /// Poll the encoding thread. Returns `(pad_index, path)` when encoding finishes.
    pub fn update(&mut self) -> Option<(usize, PathBuf)> {
        if let Some(handle) = self.encoding_handle.as_ref() {
            if handle.is_finished() {
                let handle = self.encoding_handle.take().unwrap();
                match handle.join() {
                    Ok(Ok(r)) => {
                        self.state = SamplerState::Idle;
                        self.assigned = Some(r);
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
        self.assigned.take().map(|r| (r.target_pad, r.path))
    }

    fn finish_recording(&mut self) {
        let Some(rec) = self.recording.take() else { return };
        self.state = SamplerState::Encoding;
        let path = sample_path(rec.target_pad);
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let frame_count = rec.frames.len();
        let path_display = path.display().to_string();
        let frames = rec.frames;
        let width = rec.width;
        let height = rec.height;
        let fps = rec.fps;
        let target_pad = rec.target_pad;

        let handle = std::thread::spawn(move || {
            encode_frames(device, queue, &frames, width, height, fps, &path)
                .map(|_| EncodeResult { target_pad, path })
                .map_err(|e| e.to_string())
        });
        self.encoding_handle = Some(handle);
        log::info!("VP-404 live sampler: encoding {frame_count} frames for pad {target_pad} → {path_display}");
    }
}

fn sample_path(pad_index: usize) -> PathBuf {
    let dir = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("samples")
        .join("recorded");
    let _ = std::fs::create_dir_all(&dir);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    dir.join(format!("pad{pad_index}_rec_{ts}.mov"))
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
