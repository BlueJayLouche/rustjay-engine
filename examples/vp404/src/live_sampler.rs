//! Live sampler: capture frames from the engine's active input via async
//! GPU→CPU readback, then encode to HAP5 and assign to a pad.
//!
//! Previous design opened its own InputManager (second connection to the same
//! source), which failed for webcam (exclusive device access) and Syphon (GPU
//! texture — `take_frame()` always returned None). This version instead reads
//! `ctx.input.texture` from the render hook — the engine already decodes and
//! uploads the frame for us, regardless of source type.
//!
//! Capture cadence is gated on `InputState::frame_seq` (bumped by the engine
//! once per uploaded source frame), so "record N frames" means N *source*
//! frames at any source rate. The encoded clip carries the *measured* capture
//! rate, so playback is 1:1 wall-clock by construction. `input.fps` is never
//! consulted — the engine does not populate it.
//!
//! Flow (per frame):
//!   render()  → submit_readback(texture, frame_seq)
//!                → copy_texture_to_buffer + map_async into a free slot
//!   prepare() → poll_readback()
//!                → pop ready slots FIFO: strip row padding, push BGRA→RGBA
//!                → if frames_remaining == 0: spawn encoding thread

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use hap_wgpu::{DxtQuality, EncodeConfig, HapFormat, HapVideoEncoder};

const MAP_PENDING: u8 = 0;
const MAP_READY: u8 = 1;
const MAP_FAILED: u8 = 2;

/// Staging buffers rotating for readback, so one slow map doesn't force
/// dropping the next source frame.
const READBACK_SLOTS: usize = 2;

/// Fixed safety cap on capture rate, guarding against a pathological input
/// path that re-uploads (and re-bumps `frame_seq`) every tick. Deliberately
/// far above any real source; never derived from `input.fps` (unpopulated).
const CAPTURE_CAP: Duration = Duration::from_micros(1_000_000 / 240);

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
    /// Set from the first captured frame's actual dimensions.
    width: u32,
    height: u32,
    /// Submit timestamps of the first/last *counted* frame, for measured fps.
    first_ts: Option<Instant>,
    last_ts: Option<Instant>,
    frames: Vec<Vec<u8>>, // RGBA
}

/// One staging buffer for async GPU→CPU readback.
struct ReadbackSlot {
    buf: Option<wgpu::Buffer>,
    size: (u32, u32),
    map_state: Arc<AtomicU8>,
    submitted_at: Instant,
}

impl ReadbackSlot {
    fn new() -> Self {
        Self {
            buf: None,
            size: (0, 0),
            map_state: Arc::new(AtomicU8::new(MAP_PENDING)),
            submitted_at: Instant::now(),
        }
    }
}

pub struct LiveSampler {
    state: SamplerState,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    recording: Option<Recording>,
    encoding_handle: Option<JoinHandle<Result<EncodeResult, String>>>,
    assigned: Option<EncodeResult>,
    // Double-buffered readback; `pending` holds in-flight slot indices in
    // submit order so frames are pushed FIFO.
    slots: [ReadbackSlot; READBACK_SLOTS],
    pending: VecDeque<usize>,
    // Capture gating: one readback per engine input frame.
    last_seq: Option<u64>,
    next_capture: Option<Instant>,
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
            slots: [ReadbackSlot::new(), ReadbackSlot::new()],
            pending: VecDeque::new(),
            last_seq: None,
            next_capture: None,
        }
    }

    pub fn state(&self) -> SamplerState {
        self.state
    }

    /// Begin recording `frame_count` source frames into pad `target_pad`.
    pub fn start_recording(&mut self, target_pad: usize, frame_count: u32) -> anyhow::Result<()> {
        if self.state != SamplerState::Idle {
            anyhow::bail!("Sampler is not idle");
        }
        self.state = SamplerState::Recording;
        self.last_seq = None;
        self.next_capture = None;
        self.recording = Some(Recording {
            target_pad,
            frames_remaining: frame_count,
            width: 0,
            height: 0,
            first_ts: None,
            last_ts: None,
            frames: Vec::with_capacity(frame_count as usize),
        });
        log::info!("VP-404 live sampler: recording {frame_count} frames to pad {target_pad}");
        Ok(())
    }

    /// Cancel an in-progress recording or encoding.
    /// Stop a free-length recording now and encode what's been captured so
    /// far (rec-button workflow). Nothing captured yet → plain cancel.
    /// In-flight readbacks are dropped (≤2 frames).
    pub fn finish(&mut self) {
        if self.state != SamplerState::Recording {
            return;
        }
        if self.recording.as_ref().is_none_or(|r| r.frames.is_empty()) {
            self.cancel();
        } else {
            self.finish_recording();
        }
    }

    pub fn cancel(&mut self) {
        self.recording = None;
        self.encoding_handle = None;
        self.assigned = None;
        self.reset_readbacks();
        self.state = SamplerState::Idle;
        log::info!("VP-404 live sampler: cancelled");
    }

    /// Drop all staging buffers (including any with an outstanding map) and
    /// clear gating state.
    fn reset_readbacks(&mut self) {
        self.pending.clear();
        for slot in &mut self.slots {
            slot.buf = None;
            slot.size = (0, 0);
            slot.map_state = Arc::new(AtomicU8::new(MAP_PENDING));
        }
        self.last_seq = None;
        self.next_capture = None;
    }

    /// Submit an async GPU→CPU readback of `texture` if `frame_seq` shows a
    /// new source frame since the last capture.
    ///
    /// Called once per frame from `Vp404::render()` when recording is active.
    /// Creates its own encoder so `map_async` can be called right after submit
    /// (the main render encoder must not yet be submitted at this point).
    pub fn submit_readback(&mut self, texture: &wgpu::Texture, frame_seq: u64) {
        if self.state != SamplerState::Recording {
            return;
        }
        // One capture per source frame.
        if self.last_seq == Some(frame_seq) {
            return;
        }
        let now = Instant::now();
        if self.next_capture.is_some_and(|next| now < next) {
            return;
        }
        // Both slots in flight → drop this frame; measured-fps encode keeps
        // playback timing true regardless.
        let Some(slot_idx) = (0..READBACK_SLOTS).find(|i| !self.pending.contains(i)) else {
            return;
        };

        let width = texture.width();
        let height = texture.height();
        let bytes_per_row = (width * 4).div_ceil(256) * 256;

        let slot = &mut self.slots[slot_idx];
        if slot.buf.is_none() || slot.size != (width, height) {
            slot.buf = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("VP404 Sampler Readback"),
                size: bytes_per_row as u64 * height as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }));
            slot.size = (width, height);
        }
        let buf = slot.buf.as_ref().unwrap();

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
        slot.map_state = Arc::clone(&state);
        buf.slice(..).map_async(wgpu::MapMode::Read, move |res| {
            state.store(if res.is_ok() { MAP_READY } else { MAP_FAILED }, Ordering::SeqCst);
        });
        slot.submitted_at = now;
        self.pending.push_back(slot_idx);
        self.last_seq = Some(frame_seq);
        self.next_capture = Some(match self.next_capture {
            Some(next) => (next + CAPTURE_CAP).max(now),
            None => now + CAPTURE_CAP,
        });
    }

    /// Poll in-flight readbacks in submit order and push ready frames into
    /// the recording buffer. Calls `finish_recording()` when all frames are
    /// captured.
    ///
    /// Call from `Vp404::prepare()` each frame.
    pub fn poll_readback(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        self.device.poll(wgpu::PollType::Poll).ok();

        while let Some(&slot_idx) = self.pending.front() {
            let slot = &mut self.slots[slot_idx];
            match slot.map_state.load(Ordering::SeqCst) {
                MAP_READY => {
                    let (width, height) = slot.size;
                    let bytes_per_row = (width * 4).div_ceil(256) * 256;
                    let Some(buf) = &slot.buf else {
                        self.pending.pop_front();
                        continue;
                    };
                    let slice = buf.slice(..);
                    let data = slice.get_mapped_range().expect("buffer mapped by map_async");
                    // Strip row padding and swap BGRA→RGBA.
                    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
                    for row in 0..height {
                        let start = (row * bytes_per_row) as usize;
                        let row_data = &data[start..start + (width * 4) as usize];
                        for px in row_data.as_chunks::<4>().0 {
                            rgba.push(px[2]); // R ← B
                            rgba.push(px[1]); // G
                            rgba.push(px[0]); // B ← R
                            rgba.push(px[3]); // A
                        }
                    }
                    drop(data);
                    buf.unmap();
                    let ts = slot.submitted_at;
                    self.pending.pop_front();

                    if let Some(rec) = self.recording.as_mut() {
                        if rec.frames.is_empty() {
                            rec.width = width;
                            rec.height = height;
                        } else if (rec.width, rec.height) != (width, height) {
                            // Resolution changed mid-recording — skip the
                            // mismatched frame rather than corrupt the encode.
                            continue;
                        }
                        rec.first_ts.get_or_insert(ts);
                        rec.last_ts = Some(ts);
                        rec.frames.push(rgba);
                        rec.frames_remaining = rec.frames_remaining.saturating_sub(1);
                        if rec.frames_remaining == 0 {
                            self.finish_recording();
                            return;
                        }
                    }
                }
                MAP_FAILED => {
                    log::error!("VP-404 live sampler: readback map failed");
                    self.reset_readbacks();
                    self.state = SamplerState::Error;
                    return;
                }
                _ => break, // MAP_PENDING — preserve FIFO order
            }
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
        self.reset_readbacks();
        self.state = SamplerState::Encoding;
        // Encode at the measured capture rate so playback is 1:1 wall-clock
        // even if occasional source frames were dropped.
        let fps = measured_fps(rec.frames.len(), rec.first_ts, rec.last_ts);
        let path = sample_path(rec.target_pad);
        let device = Arc::clone(&self.device);
        let queue = Arc::clone(&self.queue);
        let frame_count = rec.frames.len();
        let path_display = path.display().to_string();
        let frames = rec.frames;
        let width = rec.width;
        let height = rec.height;
        let target_pad = rec.target_pad;

        let handle = std::thread::spawn(move || {
            encode_frames(device, queue, &frames, width, height, fps, &path)
                .map(|_| EncodeResult { target_pad, path })
                .map_err(|e| e.to_string())
        });
        self.encoding_handle = Some(handle);
        log::info!(
            "VP-404 live sampler: encoding {frame_count} frames @ {fps:.2} fps \
             for pad {target_pad} → {path_display}"
        );
    }
}

/// Measured capture rate: `(n − 1)` intervals over first→last submit time.
fn measured_fps(frame_count: usize, first_ts: Option<Instant>, last_ts: Option<Instant>) -> f32 {
    match (first_ts, last_ts) {
        (Some(a), Some(b)) if frame_count >= 2 && b > a => {
            (frame_count - 1) as f32 / (b - a).as_secs_f32()
        }
        _ => 30.0,
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
        .with_quality(DxtQuality::Fast)
        .with_snappy(true);
    encoder.encode_from_frames(path, config, frames.iter().cloned())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_fps_is_intervals_over_elapsed() {
        let t0 = Instant::now();
        // 300 frames spanning 299 intervals over ~4.983 s → 60 fps.
        let t1 = t0 + Duration::from_secs_f64(299.0 / 60.0);
        assert!((measured_fps(300, Some(t0), Some(t1)) - 60.0).abs() < 0.01);
        // Degenerate cases fall back to 30.
        assert_eq!(measured_fps(1, Some(t0), Some(t0)), 30.0);
        assert_eq!(measured_fps(0, None, None), 30.0);
    }
}
