use crate::output_window::unpack_size;
use crate::video_timing::fade_elapsed;
use crate::{AppEvent, IDENTIFY_COLORS};
use cuepool_core::{CanvasFit, LockExt};
use cuepool_gui::{SharedStateHandle, VideoDiagnostics, VideoTimings};
use cuepool_video::{
    FramePool, HapAcceleration, HapFallbackSession, VideoFrame, VideoSource, ZeroCopyAvailability,
};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};
use winit::window::WindowId;

/// Ordered decode output. EOF follows the final frame through the same bounded
/// channel so it cannot overtake buffered frames.
pub(crate) enum VideoMessage {
    Frame(VideoFrame),
    Eof,
    Failed,
}

/// Frame content published by the consume thread for the output render threads.
/// Everything needed to draw one output frame, snapped under one brief lock.
/// The canvas/overlay TEXTURES are shared GPU resources — new video frames
/// land in them via queue uploads, so a stable `generation` still shows fresh
/// content; `generation` only bumps when a field here changes.
pub(crate) struct OutputFrameState {
    pub(crate) canvas_view: Option<wgpu::TextureView>,
    /// Linear (non-sRGB) canvas view, read by the winit thread's pixel sampler
    /// (canvas-source lighting segments). Not used by the render threads.
    pub(crate) canvas_render_view: Option<wgpu::TextureView>,
    pub(crate) overlay_view: Option<wgpu::TextureView>,
    pub(crate) canvas_size: [u32; 2],
    /// Anything to show (video frame, still image, or text overlay).
    pub(crate) has_content: bool,
    /// Stop-cue picture fade (1.0 = full brightness).
    pub(crate) opacity: f32,
    pub(crate) identify: bool,
    /// Live projection outputs (source rect / edge blend edits apply live).
    pub(crate) outputs: Vec<cuepool_core::ProjectorOutput>,
    pub(crate) generation: u64,
}

impl Default for OutputFrameState {
    fn default() -> Self {
        Self {
            canvas_view: None,
            canvas_render_view: None,
            overlay_view: None,
            canvas_size: [0, 0],
            has_content: false,
            opacity: 1.0,
            identify: false,
            outputs: Vec::new(),
            generation: 0,
        }
    }
}

/// A paused-seek frame request: the media position whose first decoded frame
/// the consume thread should display; frame-step-back also snaps the show
/// clock to it.
#[derive(Clone, Copy)]
pub(crate) struct VideoSeekFrameRequest {
    pub(crate) position: f64,
    pub(crate) adjust_show_clock: bool,
}

/// Video playback control shared between the winit thread and the video
/// consume thread. The winit thread owns user-driven mutations (play, stop,
/// pause, seek, step, MTC nudges); the consume thread owns the decode-channel
/// drain, the canvas/overlay textures, and the frame-state publish.
///
/// Lock order: this is a LEAF lock — never held while taking the GUI state
/// lock or the frame bundle lock, and those two are never held while taking
/// this one. The 1 s step-back `recv_timeout` happens AFTER the request is
/// taken out and the guard dropped.
#[derive(Default)]
pub(crate) struct VideoControl {
    /// Playback identity. Every play/stop transition invalidates receiver and
    /// frame work captured under an older epoch.
    pub(crate) stream_epoch: u64,
    /// Current decode channel receiver, installed by `spawn_video_decode` on
    /// the winit thread and taken out by the consume thread. A new receiver
    /// means a new stream: the consume thread drops its peeked frame.
    pub(crate) frame_rx: Option<std::sync::mpsc::Receiver<VideoMessage>>,
    pub(crate) timings: VideoTimings,
    /// Wall-clock playback anchor (real time = A/V sync reference).
    pub(crate) clock: Option<Instant>,
    /// Set while paused, to freeze `clock` across the pause.
    pub(crate) pause_started: Option<Instant>,
    /// Mirror of `App::paused`.
    pub(crate) paused: bool,
    /// A frame-step was requested while paused: consume one video frame.
    pub(crate) step_pending: bool,
    /// PTS of the frame the consume thread has peeked but not yet shown
    /// (read by `frame_step` on the winit thread).
    pub(crate) peek_pts: Option<f64>,
    /// PTS of the most recently consumed frame (frame-step-back anchor).
    pub(crate) last_pts: Option<f64>,
    /// Paused seek request: display the first frame from the re-seeked decoder;
    /// frame-step-back additionally snaps the frozen clock to that frame.
    pub(crate) seek_frame: Option<VideoSeekFrameRequest>,
    /// Clock delta from a frame-step-back; the winit thread folds it into the
    /// show clock (`show_paused_offset`) on its next tick.
    pub(crate) seek_show_delta: Option<f64>,
    /// Stop-cue picture fade: (start, duration_secs). The winit thread stops
    /// playback when it completes; the consume thread only reads it for opacity.
    pub(crate) fade: Option<(Instant, f32)>,
    /// MTC-hold position mirror (the MTC master owns the position).
    pub(crate) hold_position: Option<f64>,
    /// Full media duration reported by the active decoder.
    pub(crate) media_length_secs: Option<f64>,
    /// Media timestamp corresponding to position zero in `ActiveCueInfo`.
    pub(crate) timeline_offset_secs: f64,
    /// Mirrors of `App::current_video_qid.is_some()` / `current_text_qid.is_some()`.
    pub(crate) video_active: bool,
    pub(crate) text_active: bool,
    /// A frame has been uploaded to the canvas (written by the consume thread,
    /// cleared by the winit thread on stop/start).
    pub(crate) canvas_has_frame: bool,
    /// Identify flash state, refreshed by the winit thread each tick.
    pub(crate) identify: bool,
    /// Canvas fit mode for frame uploads, pushed by the winit thread.
    pub(crate) fit: CanvasFit,
    /// Live projection outputs, pushed by the winit thread when they change.
    pub(crate) outputs: Vec<cuepool_core::ProjectorOutput>,
    pub(crate) outputs_gen: u64,
    /// Decode-starvation counter (consume → winit diagnostics; swapped out ~1 Hz).
    pub(crate) starved: u32,
    /// Video-frame upload counter (consume → winit diagnostics; swapped out ~1 Hz).
    pub(crate) uploads: u32,
    /// Due-frame drop counter (consume → winit diagnostics; swapped out ~1 Hz).
    pub(crate) dropped: u32,
    /// Consume-loop pacing stats (consume → winit diagnostics; swapped ~1 Hz).
    /// Splits "why did an upload miss its tick": loop iterations, vsync ticks
    /// observed, wakes that saw >1 tick at once (each coalesced tick is a
    /// frame that could not have been shown), and the longest single loop
    /// iteration — if iterations stay short while ticks coalesce, the pacer
    /// output's presents are irregular, not the consume thread.
    pub(crate) consume_iters: u32,
    pub(crate) consume_ticks: u32,
    pub(crate) consume_coalesced: u32,
    pub(crate) consume_max_iter_us: u32,
}

/// Cue-driven canvas/overlay work for the consume thread (rare; the video
/// frames themselves flow through the decode channel). Keeps every non-egui
/// GPU call off the winit thread: on Windows+NVIDIA Vulkan WSI, a main-thread
/// `write_texture`/submit stalls 20-60 ms behind the vsync-blocked render
/// threads, which dragged the whole event loop to ~10 Hz.
pub(crate) enum CanvasCommand {
    /// (Re)create the canvas at this size; the overlay follows its dims.
    /// `force` recreates even at matching dims (video start clears the last
    /// frame); otherwise matching dims keep the existing content.
    Resize { w: u32, h: u32, force: bool },
    /// Drop both textures (project reset).
    Drop,
    /// Blank the canvas to black (text cue with nothing underneath, EOF blank).
    BlankCanvas,
    /// Decode a still image file onto the canvas (resolved path).
    Image(String, CanvasFit),
    /// Rasterized text block onto the overlay; None clears it.
    Overlay(Option<(VideoFrame, CanvasFit)>),
}

/// Collapse a drained burst to the latest state setters. Survivors stay in
/// their original order; `Drop` resets every earlier canvas/overlay command.
fn coalesce_canvas_commands(
    commands: impl IntoIterator<Item = CanvasCommand>,
) -> Vec<CanvasCommand> {
    let mut pending = Vec::new();
    for command in commands {
        if matches!(command, CanvasCommand::Drop) {
            pending.clear();
        } else {
            pending.retain(|earlier| {
                !matches!(
                    (&command, earlier),
                    (CanvasCommand::Resize { .. }, CanvasCommand::Resize { .. })
                        | (CanvasCommand::Overlay(_), CanvasCommand::Overlay(_))
                        | (
                            CanvasCommand::BlankCanvas | CanvasCommand::Image(..),
                            CanvasCommand::BlankCanvas | CanvasCommand::Image(..)
                        )
                )
            });
        }
        pending.push(command);
    }
    pending
}

/// Raise the Windows timer resolution to 1 ms for the process lifetime.
/// winit does not do this itself: without it, `ControlFlow::WaitUntil` and
/// `thread::sleep` quantize to the 15.6 ms default, capping the main-loop tick
/// and the consume thread's frame pacing at ~64 Hz (and wrecking 50 Hz
/// cadences). Direct winmm FFI — no crate dependency.
#[cfg(windows)]
pub(crate) mod win_timer {
    const TIMERR_NOCANDO: u32 = 97;

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
        fn timeEndPeriod(period: u32) -> u32;
    }
    pub fn raise() {
        if unsafe { timeBeginPeriod(1) } == TIMERR_NOCANDO {
            log::warn!(
                "timeBeginPeriod(1) failed: TIMERR_NOCANDO; timer quantization may degrade playback"
            );
        }
    }
    pub fn release() {
        if unsafe { timeEndPeriod(1) } == TIMERR_NOCANDO {
            log::warn!(
                "timeEndPeriod(1) failed: TIMERR_NOCANDO; timer-resolution request may remain active"
            );
        }
    }
}

const VSYNC_INTERVAL_MIN: Duration = Duration::from_millis(4);
const VSYNC_INTERVAL_MAX: Duration = Duration::from_millis(40);
const VSYNC_STALE_MAX: Duration = Duration::from_millis(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FramePacingDecision {
    target: Option<Duration>,
    tick_paced: bool,
}

fn frame_pacing_decision(
    position: Option<Duration>,
    stepping: bool,
    woke_on_tick: bool,
    last_tick_age: Option<Duration>,
    tick_interval: Option<Duration>,
    peek_pts: Option<f64>,
) -> FramePacingDecision {
    let Some(position) = position else {
        return FramePacingDecision {
            target: None,
            tick_paced: false,
        };
    };
    let healthy_interval = tick_interval
        .map(|interval| interval.clamp(VSYNC_INTERVAL_MIN, VSYNC_INTERVAL_MAX))
        .filter(|interval| {
            last_tick_age.is_some_and(|age| age < interval.saturating_mul(3).min(VSYNC_STALE_MAX))
        });
    if !stepping && let Some(interval) = healthy_interval {
        let target = if woke_on_tick {
            let raw = position.saturating_add(interval);
            // Snap to the frame this tick is meant to show. `raw` is a clock read
            // taken at wake time and jitters by a few hundred µs; when the media
            // and display phases happen to line up on a frame boundary, that
            // jitter flips the drain between 0 and 2 due frames on alternate
            // ticks — one repeated frame then one dropped, i.e. the rare hitch.
            // Presentation timestamps are exact, so picking the frame rather than
            // a time removes the boundary entirely. Outside half an interval
            // (seek, genuine clock drift, source rate != tick rate) fall back to
            // `raw` and let the drain resync in one tick.
            match peek_pts {
                Some(pts) if (pts - raw.as_secs_f64()).abs() < interval.as_secs_f64() * 0.5 => {
                    Duration::from_secs_f64(pts.max(0.0))
                }
                _ => raw,
            }
        } else {
            position
        };
        return FramePacingDecision {
            target: Some(target),
            tick_paced: true,
        };
    }
    FramePacingDecision {
        target: Some(position),
        tick_paced: false,
    }
}

fn update_vsync_interval(
    interval: Option<Duration>,
    elapsed: Duration,
    tick_count: u64,
) -> Duration {
    let sample = Duration::from_secs_f64(elapsed.as_secs_f64() / tick_count as f64)
        .clamp(VSYNC_INTERVAL_MIN, VSYNC_INTERVAL_MAX);
    interval
        .map(|previous| previous.mul_f64(0.8) + sample.mul_f64(0.2))
        .unwrap_or(sample)
        .clamp(VSYNC_INTERVAL_MIN, VSYNC_INTERVAL_MAX)
}

/// Video consume thread: owns the canvas/overlay textures, the YUV converter,
/// the decode-channel drain and the frame-state publish — every non-egui GPU
/// call. Moving this off the winit thread is the Windows fix: NVIDIA's Vulkan
/// WSI serializes a thread's GPU calls behind the vsync-blocked render
/// threads (20-60 ms per call), which dragged the whole event loop to ~10 Hz.
///
/// While output 0 presents regularly, its ticks use a one-interval lookahead
/// to select what the next scanout should show. Timeout wakes may drain frames
/// already behind the wall clock so a serial decoder can catch up after startup.
/// Missing/stale ticks and stepping use wall-clock selection unchanged.
///
/// Lock order: `control` and `frame_state` are both leaf locks, never held
/// together across GPU calls and never held while sleeping or blocking on
/// `recv_timeout`.
// ponytail: Keep thread resources explicit; bundle them when this pipeline API next changes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn video_consume_thread(
    device: wgpu::Device,
    queue: Arc<cuepool_video::SharedQueue>,
    configure_gate: Arc<RwLock<()>>,
    control: Arc<Mutex<VideoControl>>,
    frame_state: Arc<Mutex<OutputFrameState>>,
    vsync_tick: Arc<(Mutex<u64>, Condvar)>,
    cmd_rx: std::sync::mpsc::Receiver<CanvasCommand>,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    stop: Arc<AtomicBool>,
    frame_pool: Arc<FramePool>,
) {
    let mut canvas: Option<cuepool_video::CanvasTexture> = None;
    let mut overlay: Option<cuepool_video::CanvasTexture> = None;
    let mut yuv_converter: Option<cuepool_video::YuvConverter> = None;
    let mut hap_converter: Option<cuepool_video::HapConverter> = None;
    // Decode channel taken out of the control struct; None when stopped/EOF.
    let mut rx: Option<std::sync::mpsc::Receiver<VideoMessage>> = None;
    // Epoch captured with `rx`; all frame work is discarded if control moves on.
    let mut rx_epoch: Option<u64> = None;
    // Next decoded frame, peeked but not yet due.
    let mut peek: Option<VideoFrame> = None;
    // Live outputs copy (re-cloned only when `outputs_gen` advances).
    let mut outputs: Vec<cuepool_core::ProjectorOutput> = Vec::new();
    let mut outputs_gen = 0u64;
    let mut timings = VideoTimings::default();
    let mut upload_timing = TimingWindow::default();
    let mut conversion_submit_timing = TimingWindow::default();
    #[cfg(windows)]
    let mut direct_retirement = cuepool_video::SubmissionRetirement::default();
    #[cfg(windows)]
    let (retirement_tx, retirement_rx) = std::sync::mpsc::channel::<()>();
    let mut last_vsync_tick = *vsync_tick.0.lock_unpoisoned();
    let mut woke_on_vsync = false;
    let mut last_vsync_at: Option<Instant> = None;
    let mut vsync_interval: Option<Duration> = None;

    // Consume-loop pacing stats, accumulated locally and flushed to
    // `VideoControl` about once a second (see the struct fields for meaning).
    let mut stat_iters = 0u32;
    let mut stat_ticks = 0u32;
    let mut stat_coalesced = 0u32;
    let mut stat_max_iter_us = 0u32;
    let mut stat_last_flush = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        let iter_started = Instant::now();
        #[cfg(windows)]
        {
            // Poll only while zero-copy submissions await retirement: on the
            // readback path this ran every iteration for nothing, and on
            // Windows/NVIDIA an idle vkGetSemaphoreCounterValue-class call can
            // still stall tens of ms behind the vsync-blocked render threads.
            if !direct_retirement.is_empty() {
                let _ = device.poll(wgpu::PollType::Poll);
                if retirement_rx.try_iter().next().is_some() {
                    direct_retirement.drain_completed();
                }
            }
        }
        // ── Cue-driven canvas/overlay commands (rare) ──
        let mut views_dirty = false;
        for cmd in coalesce_canvas_commands(cmd_rx.try_iter()) {
            match cmd {
                CanvasCommand::Resize { w, h, force } => {
                    if force
                        || canvas
                            .as_ref()
                            .is_none_or(|c| c.width != w || c.height != h)
                    {
                        canvas = Some(cuepool_video::CanvasTexture::new(&device, w, h));
                        views_dirty = true;
                    }
                    if overlay
                        .as_ref()
                        .is_none_or(|o| o.width != w || o.height != h)
                    {
                        overlay = Some(cuepool_video::CanvasTexture::new(&device, w, h));
                        views_dirty = true;
                    }
                }
                CanvasCommand::Drop => {
                    canvas = None;
                    overlay = None;
                    views_dirty = true;
                }
                CanvasCommand::BlankCanvas => {
                    if let Some(c) = canvas.as_ref() {
                        let blank = vec![0u8; (c.width * c.height * 4) as usize];
                        let _configure_guard =
                            configure_gate.read().unwrap_or_else(|e| e.into_inner());
                        c.upload_rgba(queue.queue(), &blank);
                    }
                }
                CanvasCommand::Image(path, fit) => {
                    if let Some(c) = canvas.as_ref() {
                        match image::open(&path) {
                            Ok(image) => {
                                let image = image.to_rgba8();
                                let frame = VideoFrame::new(
                                    image.width(),
                                    image.height(),
                                    image.into_raw(),
                                    0.0,
                                );
                                let _configure_guard =
                                    configure_gate.read().unwrap_or_else(|e| e.into_inner());
                                c.upload_frame(queue.queue(), &frame, fit);
                            }
                            Err(e) => log::error!("Image cue failed to load '{path}': {e}"),
                        }
                    }
                }
                CanvasCommand::Overlay(content) => {
                    if let Some(ov) = overlay.as_ref() {
                        match content {
                            Some((frame, fit)) => {
                                let _configure_guard =
                                    configure_gate.read().unwrap_or_else(|e| e.into_inner());
                                ov.upload_frame(queue.queue(), &frame, fit);
                            }
                            None => {
                                let blank = vec![0u8; (ov.width * ov.height * 4) as usize];
                                let _configure_guard =
                                    configure_gate.read().unwrap_or_else(|e| e.into_inner());
                                ov.upload_rgba(queue.queue(), &blank)
                            }
                        }
                    }
                }
            }
        }

        let mut terminal = None;

        // ── Control handshake: new stream, stop, paused seek ──
        {
            let mut ctl = control.lock_unpoisoned();
            if rx_epoch.is_some_and(|epoch| epoch != ctl.stream_epoch) {
                rx = None;
                rx_epoch = None;
                peek = None;
                ctl.peek_pts = None;
            }
            // A newly installed receiver = a new stream: take it and drop the
            // peeked frame (stale PTS).
            if let Some(new_rx) = ctl.frame_rx.take() {
                rx = Some(new_rx);
                rx_epoch = Some(ctl.stream_epoch);
                timings = ctl.timings.clone();
                upload_timing = TimingWindow::default();
                conversion_submit_timing = TimingWindow::default();
                peek = None;
                ctl.peek_pts = None;
            }
            // Clock cleared (stop / still-image cue): retire the channel so the
            // decode thread's next send fails and it exits.
            if ctl.clock.is_none() {
                rx = None;
                rx_epoch = None;
                peek = None;
                ctl.peek_pts = None;
            }
        }
        // A paused seek waits here, off the GUI thread, for the new decoder's
        // first frame. Frame-step-back also snaps both clocks to that frame.
        let seek_frame = {
            let mut ctl = control.lock_unpoisoned();
            (rx_epoch == Some(ctl.stream_epoch))
                .then(|| ctl.seek_frame.take())
                .flatten()
        };
        if let Some(request) = seek_frame {
            peek = None;
            let delivered = rx
                .as_ref()
                .map(|r| r.recv_timeout(Duration::from_millis(1000)));
            let mut ctl = control.lock_unpoisoned();
            if rx_epoch != Some(ctl.stream_epoch) {
                rx = None;
                rx_epoch = None;
            } else {
                match delivered {
                    Some(Ok(VideoMessage::Frame(f))) => {
                        let delta = request.position - f.pts;
                        if delta > 0.0 && request.adjust_show_clock {
                            if let Some(c) = ctl.clock {
                                // Moving the epoch forward rewinds the paused position.
                                ctl.clock = Some(c + Duration::from_secs_f64(delta));
                            }
                            ctl.seek_show_delta = Some(delta);
                        }
                        ctl.peek_pts = Some(f.pts);
                        peek = Some(f);
                        ctl.step_pending = true;
                    }
                    Some(Ok(VideoMessage::Eof)) => {
                        rx = None;
                        terminal = rx_epoch.map(|epoch| (epoch, false));
                        ctl.peek_pts = None;
                    }
                    Some(Ok(VideoMessage::Failed)) => {
                        rx = None;
                        terminal = rx_epoch.map(|epoch| (epoch, true));
                        ctl.peek_pts = None;
                    }
                    Some(Err(std::sync::mpsc::RecvTimeoutError::Disconnected)) => {
                        rx = None;
                        terminal = rx_epoch.map(|epoch| (epoch, true));
                        ctl.peek_pts = None;
                    }
                    Some(Err(std::sync::mpsc::RecvTimeoutError::Timeout)) => {
                        log::warn!("Video seek: no frame delivered after seek");
                    }
                    None => {}
                }
            }
        }

        // ── Due-frame selection against the video clock ──
        let (position, fit, stepping) = {
            let mut ctl = control.lock_unpoisoned();
            let stream_current = rx_epoch == Some(ctl.stream_epoch);
            let stepping = stream_current && std::mem::take(&mut ctl.step_pending);
            let position = if stream_current && (!ctl.paused || stepping) {
                // While paused-and-stepping, the target is the frozen position —
                // clock.elapsed() keeps growing through the pause and would drain
                // every buffered frame. +1µs absorbs the f64→Duration rounding of
                // the step's clock adjustment so the frame it made due isn't held.
                let pos = if let Some(h) = ctl.hold_position {
                    Some(Duration::from_secs_f64(h.max(0.0)))
                } else {
                    ctl.clock.map(|clock| match ctl.pause_started {
                        Some(paused_at) => paused_at.duration_since(clock),
                        None => clock.elapsed(),
                    })
                };
                pos.map(|t| {
                    if stepping {
                        t + Duration::from_micros(1)
                    } else {
                        t
                    }
                })
            } else {
                None
            };
            (position, ctl.fit, stepping)
        };
        // Keep one frame peeked while paused, and notice an immediately-following
        // EOF even without a running target clock. A future frame still holds EOF
        // behind it until playback resumes and presents that frame. Runs before
        // the pacing decision so that decision can snap to the peeked frame's pts.
        if peek.is_none() && terminal.is_none() {
            match rx.as_ref().map(|r| r.try_recv()) {
                Some(Ok(VideoMessage::Frame(f))) => peek = Some(f),
                Some(Ok(VideoMessage::Eof)) => {
                    rx = None;
                    terminal = rx_epoch.map(|epoch| (epoch, false));
                }
                Some(Ok(VideoMessage::Failed)) => {
                    rx = None;
                    terminal = rx_epoch.map(|epoch| (epoch, true));
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                    rx = None;
                    terminal = rx_epoch.map(|epoch| (epoch, true));
                }
                _ => {}
            }
        }

        let pacing = frame_pacing_decision(
            position,
            stepping,
            woke_on_vsync,
            last_vsync_at.map(|tick| tick.elapsed()),
            vsync_interval,
            peek.as_ref().map(|f| f.pts),
        );
        let target = pacing.target;
        woke_on_vsync = false;

        let mut consumed: Option<VideoFrame> = None;
        let mut dropped = 0u32;
        if let Some(target) = target {
            loop {
                if peek.is_none() {
                    match rx.as_ref().map(|r| r.try_recv()) {
                        Some(Ok(VideoMessage::Frame(f))) => peek = Some(f),
                        Some(Ok(VideoMessage::Eof)) => {
                            rx = None;
                            terminal = rx_epoch.map(|epoch| (epoch, false));
                        }
                        Some(Ok(VideoMessage::Failed)) => {
                            rx = None;
                            terminal = rx_epoch.map(|epoch| (epoch, true));
                        }
                        Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                            rx = None;
                            terminal = rx_epoch.map(|epoch| (epoch, true));
                        }
                        _ => {}
                    }
                }
                match peek.as_ref() {
                    Some(f) if Duration::from_secs_f64(f.pts.max(0.0)) <= target => {
                        if let Some(discarded) = consumed.replace(peek.take().unwrap()) {
                            dropped += 1;
                            frame_pool.recycle_frame(discarded);
                        }
                    }
                    _ => break, // next frame not due yet, or channel empty
                }
            }
        }
        if dropped != 0 {
            control.lock_unpoisoned().dropped += dropped;
        }

        // Check immediately before GPU work without holding the control lock
        // across uploads/submits. The write-back below checks again afterward.
        if consumed.is_some()
            && rx_epoch.is_none_or(|epoch| control.lock_unpoisoned().stream_epoch != epoch)
        {
            if let Some(f) = consumed.take() {
                frame_pool.recycle_frame(f);
            }
            if let Some(f) = peek.take() {
                frame_pool.recycle_frame(f);
            }
            rx = None;
            rx_epoch = None;
            terminal = None;
        }

        // ── Upload the newest due frame to the canvas (GPU work) ──
        if let Some(frame) = consumed {
            let mut frame_presented = true;
            #[cfg(windows)]
            let mut direct_submitted = false;
            let mut uploaded = false;
            match &frame.pixels {
                cuepool_video::FramePixels::Rgba(_) => {
                    if let Some(c) = canvas.as_ref() {
                        let _configure_guard =
                            configure_gate.read().unwrap_or_else(|e| e.into_inner());
                        let upload_started = Instant::now();
                        c.upload_frame(queue.queue(), &frame, fit);
                        timings
                            .upload
                            .set_ms(upload_timing.record(upload_started.elapsed()));
                        uploaded = true;
                    }
                }
                cuepool_video::FramePixels::Hap { .. } => {
                    if hap_converter.is_none() {
                        hap_converter = Some(cuepool_video::HapConverter::new(
                            &device,
                            wgpu::TextureFormat::Rgba8Unorm,
                        ));
                    }
                    if let (Some(c), Some(conv)) = (canvas.as_ref(), hap_converter.as_mut()) {
                        let _configure_guard =
                            configure_gate.read().unwrap_or_else(|e| e.into_inner());
                        let upload_started = Instant::now();
                        match conv.upload(&device, queue.queue(), &frame, [c.width, c.height], fit)
                        {
                            Ok(()) => {
                                timings
                                    .upload
                                    .set_ms(upload_timing.record(upload_started.elapsed()));
                                let conversion_started = Instant::now();
                                let mut encoder = device.create_command_encoder(
                                    &wgpu::CommandEncoderDescriptor {
                                        label: Some("canvas-hap-convert"),
                                    },
                                );
                                conv.encode(&mut encoder, &c.render_view());
                                queue.submit(std::iter::once(encoder.finish()));
                                timings.conversion_submit.set_ms(
                                    conversion_submit_timing.record(conversion_started.elapsed()),
                                );
                                uploaded = true;
                            }
                            Err(error) => {
                                log::warn!("Video HAP upload skipped: {error}");
                                frame_presented = false;
                            }
                        }
                    }
                }
                #[cfg(windows)]
                cuepool_video::FramePixels::D3d12Nv12(direct) => {
                    if let Err(reason) =
                        cuepool_video::ZeroCopyAvailability::catch_direct_path_panic(|| {
                            if yuv_converter.is_none() {
                                yuv_converter = Some(cuepool_video::YuvConverter::new(
                                    &device,
                                    wgpu::TextureFormat::Rgba8Unorm,
                                ));
                            }
                        })
                    {
                        direct.complete(Err(reason));
                        frame_presented = false;
                    }
                    if frame_presented
                        && let (Some(c), Some(conv)) = (canvas.as_ref(), yuv_converter.as_mut())
                    {
                        let _configure_guard =
                            configure_gate.read().unwrap_or_else(|e| e.into_inner());
                        if let Some(readback) = direct.take_canary_readback() {
                            let canary_result = cuepool_video::YuvConverter::run_d3d12_canary(
                                &device,
                                &queue,
                                &frame,
                                &readback,
                                [c.width, c.height],
                                fit,
                            );
                            frame_pool.recycle_frame(readback);
                            if let Err(reason) = canary_result {
                                let reason = format!("zero-copy canary failed: {reason}");
                                direct.complete(Err(reason));
                                frame_presented = false;
                            }
                        }

                        if frame_presented {
                            let prepared =
                                cuepool_video::ZeroCopyAvailability::catch_direct_path_panic(
                                    || {
                                        let upload_started = Instant::now();
                                        conv.upload(
                                            &device,
                                            queue.queue(),
                                            &frame,
                                            [c.width, c.height],
                                            fit,
                                        );
                                        timings
                                            .upload
                                            .set_ms(upload_timing.record(upload_started.elapsed()));
                                        let conversion_started = Instant::now();
                                        let mut convert_encoder = device.create_command_encoder(
                                            &wgpu::CommandEncoderDescriptor {
                                                label: Some("canvas-d3d12va-convert"),
                                            },
                                        );
                                        conv.encode(&mut convert_encoder, &c.render_view());
                                        (convert_encoder.finish(), conversion_started)
                                    },
                                );
                            let epoch = rx_epoch.unwrap_or_default();
                            let retired = direct_retirement.submit(epoch, direct.clone());
                            match (prepared, retired) {
                                (Ok((command_buffer, conversion_started)), Ok(completed)) => {
                                    let callback_completed = Arc::clone(&completed);
                                    let retirement_tx = retirement_tx.clone();
                                    let submitted = match cuepool_video::ZeroCopyAvailability::catch_direct_path_panic(
                                        || {
                                            // The decode fence is staged and drained by exactly
                                            // this submission inside the serialized queue, so the
                                            // conversion cannot sample the frame before the
                                            // decoder signals it.
                                            queue.submit_with_decode_wait(
                                                direct.decode_fence(),
                                                [command_buffer],
                                            )?;
                                            queue.queue().on_submitted_work_done(move || {
                                                callback_completed.store(true, Ordering::Release);
                                                let _ = retirement_tx.send(());
                                            });
                                            Ok(())
                                        },
                                    ) {
                                        Ok(submitted) => submitted,
                                        Err(reason) => Err(reason),
                                    };
                                    if let Err(reason) = submitted {
                                        completed.store(true, Ordering::Release);
                                        direct_retirement.drain_completed();
                                        direct.complete(Err(reason));
                                        frame_presented = false;
                                    } else {
                                        // The retirement queue keeps the AVFrame leased through
                                        // GPU completion; the decoder only needs to wait until
                                        // the conversion has been submitted so it can queue ahead.
                                        direct.complete(Ok(()));
                                        direct_submitted = true;
                                        uploaded = true;
                                        timings.conversion_submit.set_ms(
                                            conversion_submit_timing
                                                .record(conversion_started.elapsed()),
                                        );
                                    }
                                }
                                (Err(reason), Ok(completed)) => {
                                    completed.store(true, Ordering::Release);
                                    direct_retirement.drain_completed();
                                    direct.complete(Err(reason));
                                    frame_presented = false;
                                }
                                (Ok(_), Err(_)) => {
                                    direct.complete(Err(
                                        "zero-copy submission retirement budget exhausted".into(),
                                    ));
                                    frame_presented = false;
                                }
                                (Err(reason), Err(_)) => {
                                    direct.complete(Err(reason));
                                    frame_presented = false;
                                }
                            }
                        }
                    } else if frame_presented {
                        direct.complete(Err("zero-copy canvas is unavailable".into()));
                        frame_presented = false;
                    }
                }
                _ => {
                    // GPU path: upload decoded planes, then run the YUV→RGB
                    // convert as its own standalone submit (no CPU swscale, no
                    // 23 MB RGBA copy).
                    if yuv_converter.is_none() {
                        yuv_converter = Some(cuepool_video::YuvConverter::new(
                            &device,
                            wgpu::TextureFormat::Rgba8Unorm,
                        ));
                    }
                    if let (Some(c), Some(conv)) = (canvas.as_ref(), yuv_converter.as_mut()) {
                        let _configure_guard =
                            configure_gate.read().unwrap_or_else(|e| e.into_inner());
                        let upload_started = Instant::now();
                        conv.upload(&device, queue.queue(), &frame, [c.width, c.height], fit);
                        timings
                            .upload
                            .set_ms(upload_timing.record(upload_started.elapsed()));
                        let conversion_started = Instant::now();
                        let mut encoder =
                            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("canvas-yuv-convert"),
                            });
                        conv.encode(&mut encoder, &c.render_view());
                        queue.submit(std::iter::once(encoder.finish()));
                        timings
                            .conversion_submit
                            .set_ms(conversion_submit_timing.record(conversion_started.elapsed()));
                        uploaded = true;
                    }
                }
            }
            let mut ctl = control.lock_unpoisoned();
            if uploaded {
                ctl.uploads += 1;
            }
            if rx_epoch == Some(ctl.stream_epoch) {
                if frame_presented {
                    ctl.last_pts = Some(frame.pts);
                    ctl.canvas_has_frame = true;
                }
                ctl.peek_pts = peek.as_ref().map(|f| f.pts);
            } else {
                drop(ctl);
                rx = None;
                rx_epoch = None;
                peek = None;
                terminal = None;
            }
            #[cfg(windows)]
            if direct_submitted {
                drop(frame);
            } else {
                frame_pool.recycle_frame(frame);
            }
            #[cfg(not(windows))]
            frame_pool.recycle_frame(frame);
        } else if let Some(epoch) = rx_epoch {
            let mut ctl = control.lock_unpoisoned();
            if ctl.stream_epoch == epoch {
                ctl.peek_pts = peek.as_ref().map(|f| f.pts);
                // Starved: a frame was due (clock running) but the channel is empty.
                if target.is_some() && peek.is_none() && rx.is_some() {
                    ctl.starved += 1;
                }
            } else {
                drop(ctl);
                rx = None;
                rx_epoch = None;
                peek = None;
                terminal = None;
            }
        }

        // The marker is observed only after every preceding frame has left the
        // FIFO. Emit after the final due upload/write-back, tagged for winit.
        if let Some((epoch, failed)) = terminal {
            if rx_epoch == Some(epoch) {
                let event = if failed {
                    AppEvent::VideoFailed(epoch)
                } else {
                    AppEvent::VideoEof(epoch)
                };
                let _ = proxy.send_event(event);
            }
            rx = None;
            rx_epoch = None;
            peek = None;
        } else if rx.is_none() && peek.is_none() {
            rx_epoch = None;
        }

        // ── Publish the frame-state bundle (change-detect + generation bump) ──
        {
            let (opacity, has_content, identify, new_outputs) = {
                let ctl = control.lock_unpoisoned();
                // Stop-cue picture fade: ramp to black; the winit thread stops
                // playback when the ramp completes.
                let opacity = match ctl.fade {
                    Some((start, dur)) => {
                        1.0 - (fade_elapsed(start, ctl.pause_started).as_secs_f32() / dur)
                            .clamp(0.0, 1.0)
                    }
                    None => 1.0,
                };
                let has_content = ctl.video_active || ctl.canvas_has_frame || ctl.text_active;
                let new_outputs = if ctl.outputs_gen != outputs_gen {
                    outputs_gen = ctl.outputs_gen;
                    Some(ctl.outputs.clone())
                } else {
                    None
                };
                (opacity, has_content, ctl.identify, new_outputs)
            };
            if let Some(o) = new_outputs {
                outputs = o;
            }
            let mut frame = frame_state.lock_unpoisoned();
            let mut changed = false;
            if views_dirty {
                frame.canvas_view = canvas.as_ref().map(|c| c.view());
                frame.canvas_render_view = canvas.as_ref().map(|c| c.render_view());
                frame.overlay_view = overlay.as_ref().map(|o| o.view());
                frame.canvas_size = canvas
                    .as_ref()
                    .map(|c| [c.width, c.height])
                    .unwrap_or([0, 0]);
                changed = true;
            }
            if frame.outputs != outputs {
                frame.outputs = outputs.clone();
                changed = true;
            }
            if frame.opacity != opacity {
                frame.opacity = opacity;
                changed = true;
            }
            if frame.has_content != has_content {
                frame.has_content = has_content;
                changed = true;
            }
            if frame.identify != identify {
                frame.identify = identify;
                changed = true;
            }
            if changed {
                frame.generation += 1;
            }
        }

        // ── Pace: wake after reference present, with the old poll as fallback ──
        let sleep_for = match position {
            None => Duration::from_millis(2), // paused/idle: watch for step/stop
            Some(pos) => match peek.as_ref() {
                Some(f) => {
                    let due_in = f.pts.max(0.0) - pos.as_secs_f64();
                    if due_in <= 0.0 {
                        Duration::ZERO
                    } else {
                        Duration::from_secs_f64(due_in).min(Duration::from_millis(4))
                    }
                }
                // Decode behind (or channel retired): poll quickly / lazily.
                None => {
                    if rx.is_some() {
                        Duration::from_millis(1)
                    } else {
                        Duration::from_millis(4)
                    }
                }
            },
        };
        // A frame held for the next healthy tick would otherwise leave the old
        // due-now calculation at zero and spin instead of returning to the wait.
        let sleep_for = if pacing.tick_paced {
            sleep_for.max(Duration::from_millis(1))
        } else {
            sleep_for
        };
        stat_iters += 1;
        let iter_us = iter_started.elapsed().as_micros().min(u32::MAX as u128) as u32;
        stat_max_iter_us = stat_max_iter_us.max(iter_us);
        if stat_last_flush.elapsed() >= Duration::from_secs(1) {
            let mut ctl = control.lock_unpoisoned();
            ctl.consume_iters += stat_iters;
            ctl.consume_ticks += stat_ticks;
            ctl.consume_coalesced += stat_coalesced;
            ctl.consume_max_iter_us = ctl.consume_max_iter_us.max(stat_max_iter_us);
            drop(ctl);
            stat_iters = 0;
            stat_ticks = 0;
            stat_coalesced = 0;
            stat_max_iter_us = 0;
            stat_last_flush = Instant::now();
        }
        if !sleep_for.is_zero() {
            let tick = vsync_tick.0.lock_unpoisoned();
            let (tick, _) = vsync_tick
                .1
                .wait_timeout_while(tick, sleep_for, |tick| *tick == last_vsync_tick)
                .unwrap_or_else(|e| e.into_inner());
            let next_vsync_tick = *tick;
            drop(tick);
            let tick_count = next_vsync_tick.wrapping_sub(last_vsync_tick);
            stat_ticks = stat_ticks.saturating_add(tick_count.min(u32::MAX as u64) as u32);
            if tick_count > 1 {
                stat_coalesced =
                    stat_coalesced.saturating_add((tick_count - 1).min(u32::MAX as u64) as u32);
            }
            woke_on_vsync = tick_count != 0;
            if woke_on_vsync {
                let now = Instant::now();
                if let Some(previous) = last_vsync_at {
                    vsync_interval = Some(update_vsync_interval(
                        vsync_interval,
                        now.saturating_duration_since(previous),
                        tick_count,
                    ));
                }
                last_vsync_at = Some(now);
            }
            last_vsync_tick = next_vsync_tick;
        }
    }

    #[cfg(windows)]
    while !direct_retirement.is_empty() {
        if device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(Duration::from_secs(2)),
            })
            .is_err()
        {
            log::error!("Video zero-copy teardown timed out; waiting to preserve submitted leases");
            let _ = device.poll(wgpu::PollType::wait_indefinitely());
        }
        direct_retirement.drain_completed();
    }
}

/// Per-output render thread: owns the surface, its config and the projection
/// renderer, and loops acquire → encode → submit → present. With Fifo the
/// acquire blocks on THIS display's vsync, so every output runs at its own
/// refresh without serializing against the others (the ungenlocked-projector
/// fix). Frame content comes from the shared `OutputFrameState` bundle,
/// re-snapshotted only when its generation bumps; the video frames themselves
/// flow through the shared canvas texture via queue uploads.
#[allow(clippy::too_many_arguments)]
pub(crate) fn output_render_thread(
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    renderer: cuepool_video::ProjectionRenderer,
    device: wgpu::Device,
    queue: Arc<cuepool_video::SharedQueue>,
    configure_gate: Arc<RwLock<()>>,
    event_loop_proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    frame_state: Arc<Mutex<OutputFrameState>>,
    vsync_tick: Arc<(Mutex<u64>, Condvar)>,
    size: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    presented: Arc<AtomicU32>,
    window_id: WindowId,
    out_index: usize,
    paces_video: bool,
    fallback_output: cuepool_core::ProjectorOutput,
) {
    // Local snapshot of the bundle, refreshed when its generation advances.
    let mut generation = 0u64;
    let mut canvas_view: Option<wgpu::TextureView> = None;
    let mut overlay_view: Option<wgpu::TextureView> = None;
    let mut canvas_size = [0u32; 2];
    let mut has_content = false;
    let mut opacity = 1.0f32;
    let mut identify = false;
    let mut outputs: Vec<cuepool_core::ProjectorOutput> = Vec::new();

    while !stop.load(Ordering::Relaxed) {
        // Resizes (incl. fullscreen toggles) are forwarded from the winit thread.
        let (w, h) = unpack_size(size.load(Ordering::Relaxed));
        if w > 0 && h > 0 && (w != config.width || h != config.height) {
            config.width = w;
            config.height = h;
            let _configure_guard = configure_gate.write().unwrap_or_else(|e| e.into_inner());
            surface.configure(&device, &config);
        }
        if w == 0 || h == 0 {
            // Minimized to zero size — nothing to present; don't spin.
            std::thread::sleep(Duration::from_millis(8));
            continue;
        }

        use wgpu::CurrentSurfaceTexture::{Lost, Occluded, Outdated, Suboptimal, Success};
        let mut submit_guard = configure_gate.read().unwrap_or_else(|e| e.into_inner());
        let surface_texture = match surface.get_current_texture() {
            Success(o) | Suboptimal(o) => o,
            // Output window covered/minimized — skip quietly (decode + audio keep
            // running on their own threads, so playback is unaffected). Sleep so
            // the non-blocking Occluded return can't free-spin this thread.
            Occluded => {
                drop(submit_guard);
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
            Lost => {
                log::warn!("Output surface lost; requesting a winit-side rebuild");
                drop(submit_guard);
                if event_loop_proxy
                    .send_event(AppEvent::OutputSurfaceLost(window_id))
                    .is_err()
                {
                    log::error!("Cannot request output surface rebuild: event loop closed");
                    break;
                }
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(8));
                }
                break;
            }
            Outdated => {
                log::debug!("Output surface outdated, reconfiguring");
                drop(submit_guard);
                {
                    let _configure_guard =
                        configure_gate.write().unwrap_or_else(|e| e.into_inner());
                    surface.configure(&device, &config);
                }
                submit_guard = configure_gate.read().unwrap_or_else(|e| e.into_inner());
                match surface.get_current_texture() {
                    Success(o) | Suboptimal(o) => o,
                    err => {
                        log::warn!("Output surface acquire failed after reconfigure: {err:?}");
                        drop(submit_guard);
                        std::thread::sleep(Duration::from_millis(8));
                        continue;
                    }
                }
            }
            err => {
                log::warn!("Output surface acquire failed: {err:?}");
                drop(submit_guard);
                std::thread::sleep(Duration::from_millis(8));
                continue;
            }
        };

        // Re-snapshot the published frame state when it advanced.
        {
            let frame = frame_state.lock_unpoisoned();
            if frame.generation != generation {
                generation = frame.generation;
                canvas_view = frame.canvas_view.clone();
                overlay_view = frame.overlay_view.clone();
                canvas_size = frame.canvas_size;
                has_content = frame.has_content;
                opacity = frame.opacity;
                identify = frame.identify;
                outputs = frame.outputs.clone();
            }
        }

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("output-encoder"),
        });

        if identify {
            let color = IDENTIFY_COLORS[out_index % IDENTIFY_COLORS.len()];
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("identify-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(color),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
        } else if has_content {
            if let (Some(canvas_view), Some(overlay_view)) =
                (canvas_view.as_ref(), overlay_view.as_ref())
            {
                // Source rect + edge blend (incl. gamma) come from the published
                // live outputs, so projection-panel edits apply without a window
                // rebuild. The baked config is only a fallback for when the live
                // list has no entry for this window.
                let output = outputs.get(out_index).unwrap_or(&fallback_output);
                renderer.render(
                    &device,
                    queue.queue(),
                    &mut encoder,
                    canvas_view,
                    overlay_view,
                    &view,
                    output,
                    canvas_size,
                    opacity,
                );
            } else {
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("output-clear-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    timestamp_writes: None,
                    multiview_mask: None,
                });
            }
        } else {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("output-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None,
            });
        }

        queue.submit(std::iter::once(encoder.finish()));
        queue.queue().present(surface_texture);
        if paces_video {
            {
                let mut tick = vsync_tick.0.lock_unpoisoned();
                *tick = tick.wrapping_add(1);
            }
            vsync_tick.1.notify_all();
        }
        drop(submit_guard);
        presented.fetch_add(1, Ordering::Relaxed);
        if !matches!(
            config.present_mode,
            wgpu::PresentMode::Fifo | wgpu::PresentMode::FifoRelaxed
        ) {
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

/// Blocking send with backpressure: waits until the clock-paced consumer takes
/// a frame, polling so it stays responsive to the stop signal. The bounded
/// channel is what paces decode to what playback actually consumes.
/// Returns `false` when the thread should exit (stopped or disconnected).
fn send_video_message(
    frame_tx: &std::sync::mpsc::SyncSender<VideoMessage>,
    stop_flag: &AtomicBool,
    mut message: VideoMessage,
) -> bool {
    use std::sync::mpsc::TrySendError;
    loop {
        if stop_flag.load(Ordering::Relaxed) {
            return false;
        }
        match frame_tx.try_send(message) {
            Ok(()) => return true,
            Err(TrySendError::Full(m)) => {
                message = m;
                std::thread::sleep(Duration::from_millis(2));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

/// Video decode thread: sends frames and EOF through the bounded consumer channel.
/// `start_before`: deliver first the last frame with PTS strictly below this
/// timestamp (seeking and frame-step-back), then continue with the frames after it.
// ponytail: Keep the one-thread entry point flat; introduce a context struct if it grows again.
#[allow(clippy::too_many_arguments)]
pub(crate) fn video_decode_thread(
    path: &str,
    start_before: Option<f64>,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    frame_tx: std::sync::mpsc::SyncSender<VideoMessage>,
    diag_state: SharedStateHandle,
    video_control: Arc<Mutex<VideoControl>>,
    stream_epoch: u64,
    clamp_to_media: bool,
    frame_pool: Arc<FramePool>,
    timings: VideoTimings,
    zero_copy: ZeroCopyAvailability,
    hap_acceleration: HapAcceleration,
    hap_fallback_session: HapFallbackSession,
) {
    if stop_flag.load(Ordering::Acquire) {
        return;
    }
    let zero_copy = if start_before.is_some() {
        ZeroCopyAvailability::declined("seek/frame-step-back uses D3D11VA readback")
    } else {
        zero_copy
    };
    let mut source = match VideoSource::open_with_acceleration_and_session_cancellable(
        path,
        Arc::clone(&frame_pool),
        zero_copy,
        hap_acceleration,
        hap_fallback_session,
        Arc::clone(&stop_flag),
    ) {
        Ok(s) => s,
        Err(_) if stop_flag.load(Ordering::Acquire) => return,
        Err(e) => {
            log::error!("Failed to open video source {}: {e}", path);
            send_video_message(&frame_tx, &stop_flag, VideoMessage::Failed);
            return;
        }
    };
    if stop_flag.load(Ordering::Acquire) {
        return;
    }

    // Publish what's decoding to the Status window (Help → Status…).
    diag_state.lock_unpoisoned().diagnostics.video = Some(VideoDiagnostics {
        path: path.to_string(),
        width: source.width(),
        height: source.height(),
        decode_path: source.decode_path().to_string(),
        fallback_reason: source.fallback_reason().map(str::to_owned),
        timings: timings.clone(),
    });
    let media_length_secs = source.duration_secs();
    let seek_target = if clamp_to_media {
        start_before.map(|target| crate::clamp_video_seek_secs(target, media_length_secs))
    } else {
        start_before
    };
    {
        let mut ctl = video_control.lock_unpoisoned();
        if ctl.stream_epoch == stream_epoch {
            ctl.media_length_secs = media_length_secs;
            if let (Some(requested), Some(actual)) = (start_before, seek_target)
                && requested != actual
            {
                let now = Instant::now();
                if let Some((clock, pause_started)) =
                    crate::video_seek_clock(now, actual, ctl.paused)
                {
                    ctl.clock = Some(clock);
                    ctl.pause_started = pause_started;
                    if ctl.hold_position.is_some() {
                        ctl.hold_position =
                            Some(crate::video_timeline_secs(actual, ctl.timeline_offset_secs));
                    }
                }
            }
        }
    }

    let mut timing_windows = VideoTimingWindows::default();

    if let Some(t) = seek_target {
        // Seek to the keyframe at/before t, then scan forward for the frame
        // pair straddling t. On seek failure the scan decodes from the start —
        // slower, but still lands on the right frame.
        if let Err(e) = source.seek_before(t) {
            log::warn!("Video seek to {t:.3}s failed ({e}); scanning from start");
        }
        let mut prev: Option<VideoFrame> = None;
        loop {
            if stop_flag.load(Ordering::Relaxed) {
                return;
            }
            match timed_read_frame(&mut source, &mut timing_windows, &timings, &stop_flag) {
                Some(f) if f.pts + 1e-4 < t => {
                    if let Some(discarded) = prev.replace(f) {
                        frame_pool.recycle_frame(discarded);
                    }
                }
                Some(f) => {
                    if let Some(p) = prev.take()
                        && !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(p))
                    {
                        return;
                    }
                    if !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(f)) {
                        return;
                    }
                    break;
                }
                None => {
                    if source.failed() {
                        if let Some(video) = diag_state.lock_unpoisoned().diagnostics.video.as_mut()
                        {
                            video.decode_path = source.decode_path().to_string();
                            video.fallback_reason = source.fallback_reason().map(str::to_owned);
                        }
                        log::error!(
                            "Video decode stopped during seek recovery: {}",
                            source
                                .fallback_reason()
                                .unwrap_or("unknown decoder failure")
                        );
                        send_video_message(&frame_tx, &stop_flag, VideoMessage::Failed);
                        return;
                    }
                    // t is past the last frame: deliver that frame and end.
                    if let Some(p) = prev.take()
                        && !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(p))
                    {
                        return;
                    }
                    send_video_message(&frame_tx, &stop_flag, VideoMessage::Eof);
                    return;
                }
            }
        }
    }

    // Note: decode does NOT stop while paused — the bounded channel blocks it
    // after VIDEO_QUEUE_CAP frames anyway, and keeping it topped up (without
    // dropping frames) is what makes frame-stepping through a pause possible.
    let _ = &pause_flag;
    // Re-publish whenever the active backend changes: hardware engagement and
    // a mid-stream HAP variant fallback both happen after open.
    let mut reported_decode_path = source.decode_path().to_string();
    let mut reported_fallback_reason = source.fallback_reason().map(str::to_owned);
    if let Some(video) = diag_state.lock_unpoisoned().diagnostics.video.as_mut() {
        video.decode_path = reported_decode_path.clone();
        video.fallback_reason = reported_fallback_reason.clone();
    }
    while !stop_flag.load(Ordering::Relaxed) {
        let frame = timed_read_frame(&mut source, &mut timing_windows, &timings, &stop_flag);
        let decode_path = source.decode_path().to_string();
        let fallback_reason = source.fallback_reason().map(str::to_owned);
        if (decode_path != reported_decode_path || fallback_reason != reported_fallback_reason)
            && let Some(v) = diag_state.lock_unpoisoned().diagnostics.video.as_mut()
        {
            reported_decode_path = decode_path.clone();
            reported_fallback_reason = fallback_reason.clone();
            v.decode_path = decode_path;
            v.fallback_reason = fallback_reason;
        }
        match frame {
            Some(frame) => {
                #[cfg(windows)]
                let handoff = frame.d3d12_handoff();
                if !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(frame)) {
                    return;
                }
                #[cfg(windows)]
                if let Some(handoff) = handoff {
                    if let Err(reason) = handoff.wait(&stop_flag) {
                        if stop_flag.load(Ordering::Relaxed) {
                            return;
                        }
                        if !source.fallback_zero_copy(format!(
                            "zero-copy consume handoff failed: {reason}"
                        )) {
                            return;
                        }
                    } else if !stop_flag.load(Ordering::Relaxed) {
                        source.mark_zero_copy_engaged();
                    }
                    if let Some(video) = diag_state.lock_unpoisoned().diagnostics.video.as_mut() {
                        video.decode_path = source.decode_path().to_string();
                        video.fallback_reason = source.fallback_reason().map(str::to_owned);
                    }
                }
            }
            None => {
                if source.failed() {
                    log::error!(
                        "Video decode stopped without signalling EOF: {}",
                        source
                            .fallback_reason()
                            .unwrap_or("unknown decoder failure")
                    );
                    send_video_message(&frame_tx, &stop_flag, VideoMessage::Failed);
                } else {
                    send_video_message(&frame_tx, &stop_flag, VideoMessage::Eof);
                }
                break;
            }
        }
    }
}

const DECODE_TIMING_WINDOW: usize = 50;

#[derive(Default)]
struct TimingWindow {
    samples_ms: std::collections::VecDeque<f64>,
}

impl TimingWindow {
    fn record(&mut self, elapsed: Duration) -> f64 {
        if self.samples_ms.len() == DECODE_TIMING_WINDOW {
            self.samples_ms.pop_front();
        }
        self.samples_ms.push_back(elapsed.as_secs_f64() * 1000.0);
        self.samples_ms.iter().sum::<f64>() / self.samples_ms.len() as f64
    }
}

#[derive(Default)]
struct VideoTimingWindows {
    decode: TimingWindow,
    hw_transfer: TimingWindow,
    plane_copy: TimingWindow,
}

fn timed_read_frame(
    source: &mut VideoSource,
    timing_windows: &mut VideoTimingWindows,
    timings: &VideoTimings,
    stop_flag: &AtomicBool,
) -> Option<VideoFrame> {
    let frame = source.read_frame_cancellable(stop_flag);
    let frame_timings = source.last_timings();
    timings
        .decode
        .set_ms(timing_windows.decode.record(frame_timings.decode));
    timings
        .hw_transfer
        .set_ms(timing_windows.hw_transfer.record(frame_timings.hw_transfer));
    timings
        .plane_copy
        .set_ms(timing_windows.plane_copy.record(frame_timings.plane_copy));
    frame
}

/// Pixel-map decode thread: self-paced by wall-clock PTS (no vsync consumer),
/// loops by reopening the source, blanks to black on a OneShot end.
pub(crate) fn pixmap_decode_thread(
    path: &str,
    loop_mode: cuepool_core::LoopMode,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    frame_tx: std::sync::mpsc::SyncSender<VideoFrame>,
    frame_pool: Arc<FramePool>,
    hap_acceleration: HapAcceleration,
) {
    let looping = matches!(
        loop_mode,
        cuepool_core::LoopMode::Looped | cuepool_core::LoopMode::LoopedInfinite
    );
    let mut last_dims = (0u32, 0u32);
    let hap_fallback_session = HapFallbackSession::default();
    'outer: loop {
        let mut source = match VideoSource::open_with_hap_acceleration_and_session_cancellable(
            path,
            Arc::clone(&frame_pool),
            hap_acceleration.clone(),
            hap_fallback_session.clone(),
            Arc::clone(&stop_flag),
        ) {
            Ok(s) => s,
            Err(_) if stop_flag.load(Ordering::Acquire) => return,
            Err(e) => {
                log::error!("PixelMap: failed to open {}: {e}", path);
                return;
            }
        };
        let mut start = Instant::now();
        while !stop_flag.load(Ordering::Relaxed) {
            let Some(frame) = source.read_frame_cancellable(&stop_flag) else {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                if source.failed() {
                    log::error!(
                        "PixelMap decode stopped without looping: {}",
                        source
                            .fallback_reason()
                            .unwrap_or("unknown decoder failure")
                    );
                    return;
                }
                if looping {
                    continue 'outer; // reopen from the top
                }
                if loop_mode == cuepool_core::LoopMode::OneShot && last_dims.0 > 0 {
                    // Blank to black so the LEDs go dark, mirroring video-cue end.
                    let (w, h) = last_dims;
                    let _ =
                        frame_tx.send(VideoFrame::new(w, h, vec![0; (w * h * 4) as usize], 0.0));
                }
                return; // HoldLast: exit holding the last frame
            };
            last_dims = (frame.width, frame.height);
            // Wall-clock pacing: sleep until this frame is due. A pause holds
            // the clock — the paused span shifts the timeline so the remaining
            // frames keep their original cadence on resume, and the last sent
            // frame stays on the LEDs meanwhile.
            let mut due = start + Duration::from_secs_f64(frame.pts.max(0.0));
            loop {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
                }
                if pause_flag.load(Ordering::Relaxed) {
                    let paused_at = Instant::now();
                    while pause_flag.load(Ordering::Relaxed) {
                        if stop_flag.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    let paused_for = paused_at.elapsed();
                    start += paused_for;
                    due += paused_for;
                    continue;
                }
                if Instant::now() >= due {
                    break;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            // Blocking send is fine: the consumer drains every lighting tick,
            // and a dropped receiver ends the thread via the send error.
            if frame_tx.send(frame).is_err() {
                return;
            }
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_averages_the_last_fifty_samples() {
        let mut timing = TimingWindow::default();
        for ms in 1..=50 {
            timing.record(Duration::from_millis(ms));
        }
        assert!((timing.record(Duration::from_millis(51)) - 26.5).abs() < 1e-9);
    }

    #[test]
    fn frame_pacing_looks_ahead_on_ticks_and_catches_up_on_timeouts() {
        let position = Duration::from_secs(10);
        let interval = Duration::from_millis(20);
        let decide = |stepping, woke_on_tick, age, interval| {
            frame_pacing_decision(Some(position), stepping, woke_on_tick, age, interval, None)
        };

        let tick = decide(false, true, Some(Duration::from_millis(1)), Some(interval));
        assert_eq!(tick.target, Some(position + interval));
        assert!(tick.tick_paced);

        let timeout = decide(false, false, Some(Duration::from_millis(5)), Some(interval));
        assert_eq!(timeout.target, Some(position));
        assert!(timeout.tick_paced);

        let absent = decide(false, false, None, None);
        assert_eq!(absent.target, Some(position));
        assert!(!absent.tick_paced);

        let stale = decide(
            false,
            false,
            Some(Duration::from_millis(61)),
            Some(interval),
        );
        assert_eq!(stale.target, Some(position));
        assert!(!stale.tick_paced);

        let stepping = decide(true, false, Some(Duration::from_millis(1)), Some(interval));
        assert_eq!(stepping.target, Some(position));
        assert!(!stepping.tick_paced);

        assert_eq!(
            update_vsync_interval(None, Duration::from_millis(2), 1),
            VSYNC_INTERVAL_MIN
        );
        assert_eq!(
            update_vsync_interval(None, Duration::from_millis(100), 2),
            VSYNC_INTERVAL_MAX
        );
    }

    /// The hitch this guards: with a raw clock target, sub-ms wake jitter on a
    /// frame boundary flips the drain between 0 and 2 due frames. Snapping to the
    /// peeked pts must pick the same frame either side of the boundary.
    #[test]
    fn tick_target_snaps_to_the_peeked_frame_across_wake_jitter() {
        let interval = Duration::from_millis(20);
        let pts = 10.02_f64; // frame due one interval after position 10.000s
        let snap = |position_us: u64| {
            frame_pacing_decision(
                Some(Duration::from_micros(position_us)),
                false,
                true,
                Some(Duration::from_millis(1)),
                Some(interval),
                Some(pts),
            )
            .target
        };

        // 300µs either side of the boundary: same frame selected, exactly.
        let want = Some(Duration::from_secs_f64(pts));
        assert_eq!(snap(10_000_300), want);
        assert_eq!(snap(9_999_700), want);

        // Beyond half an interval the clock has genuinely moved on: fall back to
        // the raw target so the drain resyncs.
        assert_eq!(
            frame_pacing_decision(
                Some(Duration::from_millis(10_015)),
                false,
                true,
                Some(Duration::from_millis(1)),
                Some(interval),
                Some(pts),
            )
            .target,
            Some(Duration::from_millis(10_035)),
        );
    }

    #[test]
    fn canvas_commands_coalesce_to_latest_state_after_drop() {
        let commands = coalesce_canvas_commands([
            CanvasCommand::Resize {
                w: 640,
                h: 480,
                force: false,
            },
            CanvasCommand::Image("discarded.png".into(), CanvasFit::default()),
            CanvasCommand::Overlay(None),
            CanvasCommand::Drop,
            CanvasCommand::Resize {
                w: 1280,
                h: 720,
                force: false,
            },
            CanvasCommand::BlankCanvas,
            CanvasCommand::Overlay(None),
            CanvasCommand::Resize {
                w: 1920,
                h: 1080,
                force: true,
            },
            CanvasCommand::Image("latest.png".into(), CanvasFit::default()),
            CanvasCommand::Overlay(None),
        ]);

        assert_eq!(commands.len(), 4);
        assert!(matches!(commands[0], CanvasCommand::Drop));
        assert!(matches!(
            commands[1],
            CanvasCommand::Resize {
                w: 1920,
                h: 1080,
                force: true
            }
        ));
        assert!(matches!(&commands[2], CanvasCommand::Image(path, _) if path == "latest.png"));
        assert!(matches!(commands[3], CanvasCommand::Overlay(None)));
    }
}
