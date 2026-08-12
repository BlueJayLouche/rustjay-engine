//! CuePool binary — custom winit event loop with dual windows.
//!
//! - Control window: egui UI (replaces eframe)
//! - Video output windows: one render thread per output, each blocking on its
//!   own display's vsync (Fifo) — ungenlocked outputs never serialize.
//! - Audio engine: cpal output with master clock for A/V sync
//! - Video decode: background thread feeding a bounded channel; a consume
//!   thread picks the frame due against the wall-clock video clock and uploads
//!   it to the shared canvas texture the render threads sample. All non-egui
//!   GPU work lives on the consume thread (Windows/NVIDIA WSI stalls any
//!   thread that submits behind vsync-blocked swapchains).

use cuepool_audio::{AudioEngine, FileDecoder, SampleProvider};
use cuepool_core::{
    AudioOutputDriver, CanvasFit, LockExt, MidiTrigger, MidiTriggerKind, SerializedColour, Timespan,
};
use cuepool_gui::{AppCommand, CuePoolApp, OutputDiagnostics, SharedStateHandle, VideoDiagnostics};
use cuepool_gui::app::CueState;
use cuepool_protocols::midi::mtc::{MtcFrameRate, MtcReceiver, MtcState};
use cuepool_protocols::midi::{MidiEvent, MidiManager};
use cuepool_protocols::msc::{MscCommandFlags, MscEvent, MscManager};
use cuepool_protocols::osc::{OscEvent, OscManager};
use cuepool_video::{VideoFrame, VideoSource};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use human_panic::Metadata;

mod lighting_engine;
use lighting_engine::LightingEngine;
mod mtc_follow;
use mtc_follow::MtcFollowState;
mod recorder;
use recorder::Recorder;


/// Decode-channel depth (frames). A small buffer absorbs decode jitter; the
/// backpressure it provides paces decode to the display refresh.
const VIDEO_QUEUE_CAP: usize = 3;

/// Max squared position distance (px²) for recalling an output to a saved monitor.
/// Positions are fixed for an installed wall, so this just allows minor slop while
/// keeping projectors 1920 px apart unambiguous, and leaves an output windowed if
/// its monitor isn't present (rather than grabbing the wrong one).
const MONITOR_MATCH_DIST_SQ: i64 = 200 * 200;

/// Distinct colours for the Identify overlay (one per output window, by order).
const IDENTIFY_COLORS: [wgpu::Color; 6] = [
    wgpu::Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }, // red
    wgpu::Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }, // green
    wgpu::Color { r: 0.0, g: 0.2, b: 1.0, a: 1.0 }, // blue
    wgpu::Color { r: 1.0, g: 1.0, b: 0.0, a: 1.0 }, // yellow
    wgpu::Color { r: 1.0, g: 0.0, b: 1.0, a: 1.0 }, // magenta
    wgpu::Color { r: 0.0, g: 1.0, b: 1.0, a: 1.0 }, // cyan
];
const IDENTIFY_COLOR_NAMES: [&str; 6] = ["RED", "GREEN", "BLUE", "YELLOW", "MAGENTA", "CYAN"];

fn configured_audio_error(
    driver: AudioOutputDriver,
    device: &str,
    error: &cuepool_audio::AudioError,
) -> String {
    let device = if device.is_empty() { "<default>" } else { device };
    format!("configured {driver} output device '{device}' failed: {error}")
}

/// Build a stable-ish descriptor from a winit monitor (name + resolution + position).
fn monitor_descriptor(m: &winit::monitor::MonitorHandle) -> cuepool_core::MonitorId {
    let pos = m.position();
    let size = m.size();
    cuepool_core::MonitorId {
        name: m.name().unwrap_or_default(),
        width: size.width,
        height: size.height,
        pos_x: pos.x,
        pos_y: pos.y,
    }
}

/// User events sent to the main event loop from background threads.
#[derive(Debug)]
enum AppEvent {
    /// The consume thread uploaded the stream's final due frame.
    VideoEof(u64),
    /// An output worker needs winit to recreate its window-owned surface.
    OutputSurfaceLost(WindowId),
    /// The shared GPU device is gone; recovery requires rebuilding all resources.
    DeviceLost,
}

/// Ordered decode output. EOF follows the final frame through the same bounded
/// channel so it cannot overtake buffered frames.
enum VideoMessage {
    Frame(VideoFrame),
    Eof,
}

/// Per-window identifiers so we can route events.
struct WindowIds {
    control: WindowId,
    video: Vec<WindowId>,
}

#[derive(Clone)]
struct ActiveCue {
    qid: rust_decimal::Decimal,
    name: String,
    input: std::sync::Arc<cuepool_audio::MixerInput>,
    state: CueState,
    /// Shared counter incremented by LoopProcessor on each loop boundary.
    loop_counter: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    /// Last known loop count (used to detect new loops).
    video_loop_count: u32,
    /// Loop boundaries in frames, for computing loop-relative position.
    loop_start_frame: u64,
    loop_end_frame: u64,
    /// Tail fade-out (seconds) — begins `fade_out` before the cue's natural end.
    fade_out: f32,
    fade_type: cuepool_core::FadeType,
    fade_out_started: bool,
    /// Stop action scheduled by a StopCue targeting this cue.
    pending_stop: Option<PendingStop>,
}

/// A cue that is waiting for its delay timer to expire before playing.
struct DelayedCue {
    cue: cuepool_core::Cue,
    start_at: std::time::Instant,
}

/// Pending stop action scheduled by a StopCue with mode != Immediate.
#[derive(Clone, Copy)]
struct PendingStop {
    mode: cuepool_core::StopMode,
    fade_out_time: f32,
    fade_type: cuepool_core::FadeType,
}

/// One projector output window. The main (winit) thread owns the window itself
/// (events, fullscreen toggles); the surface, its config and the slice renderer
/// live on a dedicated render thread that blocks on THIS display's vsync
/// (Fifo), so ungenlocked outputs never serialize against each other.
struct OutputWindow {
    id: WindowId,
    window: Arc<Window>,
    /// Baked snapshot: display name (identify/diagnostics) and the fallback
    /// used when the live projection outputs list has no entry for this window.
    output_config: cuepool_core::ProjectorOutput,
    /// Latest window size forwarded to the render thread (packed `w<<32 | h`).
    size: Arc<AtomicU64>,
    /// Stop signal for the render thread.
    stop: Arc<AtomicBool>,
    /// Presents completed by the render thread (drained ~1 Hz for diagnostics).
    presented: Arc<AtomicU32>,
    present_mode: wgpu::PresentMode,
    format: wgpu::TextureFormat,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for OutputWindow {
    /// Signal the render thread, but never let a wedged driver call freeze winit.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let deadline = Instant::now() + Duration::from_millis(250);
            while !join.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if join.is_finished() {
                let _ = join.join();
            } else {
                // Detaching is the lesser evil: the worker owns its Surface and
                // cloned GPU/state handles. `create_surface(Arc<Window>)` keeps
                // that window alive until the worker eventually drops the Surface.
                log::error!(
                    "Output '{}' render thread did not stop within 250 ms; detaching",
                    self.output_config.name,
                );
            }
        }
    }
}

fn pack_size(w: u32, h: u32) -> u64 {
    ((w as u64) << 32) | h as u64
}

fn unpack_size(packed: u64) -> (u32, u32) {
    ((packed >> 32) as u32, packed as u32)
}

/// Frame content published by the consume thread for the output render threads.
/// Everything needed to draw one output frame, snapped under one brief lock.
/// The canvas/overlay TEXTURES are shared GPU resources — new video frames
/// land in them via queue uploads, so a stable `generation` still shows fresh
/// content; `generation` only bumps when a field here changes.
struct OutputFrameState {
    canvas_view: Option<wgpu::TextureView>,
    /// Linear (non-sRGB) canvas view, read by the winit thread's pixel sampler
    /// (canvas-source lighting segments). Not used by the render threads.
    canvas_render_view: Option<wgpu::TextureView>,
    overlay_view: Option<wgpu::TextureView>,
    canvas_size: [u32; 2],
    /// Anything to show (video frame, still image, or text overlay).
    has_content: bool,
    /// Stop-cue picture fade (1.0 = full brightness).
    opacity: f32,
    identify: bool,
    /// Live projection outputs (source rect / edge blend edits apply live).
    outputs: Vec<cuepool_core::ProjectorOutput>,
    generation: u64,
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

/// Video playback control shared between the winit thread and the video
/// consume thread. The winit thread owns user-driven mutations (play, stop,
/// pause, seek, step, MTC nudges); the consume thread owns the decode-channel
/// drain, the canvas/overlay textures, and the frame-state publish.
///
/// Lock order: this is a LEAF lock — never held while taking the GUI state
/// lock or the frame bundle lock, and those two are never held while taking
/// this one. The 1 s step-back `recv_timeout` happens AFTER the request is
/// taken out and the guard dropped.
struct VideoControl {
    /// Playback identity. Every play/stop transition invalidates receiver and
    /// frame work captured under an older epoch.
    stream_epoch: u64,
    /// Current decode channel receiver, installed by `spawn_video_decode` on
    /// the winit thread and taken out by the consume thread. A new receiver
    /// means a new stream: the consume thread drops its peeked frame.
    frame_rx: Option<std::sync::mpsc::Receiver<VideoMessage>>,
    /// Wall-clock playback anchor (real time = A/V sync reference).
    clock: Option<Instant>,
    /// Set while paused, to freeze `clock` across the pause.
    pause_started: Option<Instant>,
    /// Mirror of `App::paused`.
    paused: bool,
    /// A frame-step was requested while paused: consume one video frame.
    step_pending: bool,
    /// PTS of the frame the consume thread has peeked but not yet shown
    /// (read by `frame_step` on the winit thread).
    peek_pts: Option<f64>,
    /// PTS of the most recently consumed frame (frame-step-back anchor).
    last_pts: Option<f64>,
    /// Frame-step-back request: the frozen position to snap to once the
    /// re-seeked decode thread delivers its first frame.
    step_back: Option<f64>,
    /// The clock delta a completed step-back applied; the winit thread folds
    /// it into the show clock (`show_paused_offset`) on its next tick.
    step_back_delta: Option<f64>,
    /// Stop-cue picture fade: (start, duration_secs). The winit thread stops
    /// playback when it completes; the consume thread only reads it for opacity.
    fade: Option<(Instant, f32)>,
    /// MTC-hold position mirror (the MTC master owns the position).
    hold_position: Option<f64>,
    /// Mirrors of `App::current_video_qid.is_some()` / `current_text_qid.is_some()`.
    video_active: bool,
    text_active: bool,
    /// A frame has been uploaded to the canvas (written by the consume thread,
    /// cleared by the winit thread on stop/start).
    canvas_has_frame: bool,
    /// Identify flash state, refreshed by the winit thread each tick.
    identify: bool,
    /// Canvas fit mode for frame uploads, pushed by the winit thread.
    fit: CanvasFit,
    /// Live projection outputs, pushed by the winit thread when they change.
    outputs: Vec<cuepool_core::ProjectorOutput>,
    outputs_gen: u64,
    /// Decode-starvation counter (consume → winit diagnostics; swapped out ~1 Hz).
    starved: u32,
}

impl Default for VideoControl {
    fn default() -> Self {
        Self {
            stream_epoch: 0,
            frame_rx: None,
            clock: None,
            pause_started: None,
            paused: false,
            step_pending: false,
            peek_pts: None,
            last_pts: None,
            step_back: None,
            step_back_delta: None,
            fade: None,
            hold_position: None,
            video_active: false,
            text_active: false,
            canvas_has_frame: false,
            identify: false,
            fit: CanvasFit::default(),
            outputs: Vec::new(),
            outputs_gen: 0,
            starved: 0,
        }
    }
}

/// Cue-driven canvas/overlay work for the consume thread (rare; the video
/// frames themselves flow through the decode channel). Keeps every non-egui
/// GPU call off the winit thread: on Windows+NVIDIA Vulkan WSI, a main-thread
/// `write_texture`/submit stalls 20-60 ms behind the vsync-blocked render
/// threads, which dragged the whole event loop to ~10 Hz.
enum CanvasCommand {
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
fn coalesce_canvas_commands(commands: impl IntoIterator<Item = CanvasCommand>) -> Vec<CanvasCommand> {
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

fn fade_elapsed(start: Instant, pause_started: Option<Instant>) -> Duration {
    pause_started.unwrap_or_else(Instant::now).saturating_duration_since(start)
}

fn shift_fade_start_after_pause(start: Instant, pause_started: Instant, resumed_at: Instant) -> Instant {
    start + resumed_at.saturating_duration_since(start.max(pause_started))
}

/// Raise the Windows timer resolution to 1 ms for the process lifetime.
/// winit does not do this itself: without it, `ControlFlow::WaitUntil` and
/// `thread::sleep` quantize to the 15.6 ms default, capping the main-loop tick
/// and the consume thread's frame pacing at ~64 Hz (and wrecking 50 Hz
/// cadences). Direct winmm FFI — no crate dependency.
#[cfg(windows)]
mod win_timer {
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

/// True when the parts of the projection that window creation bakes in differ:
/// output count and monitor assignment. Everything else travels per-frame now:
/// source rect and edge blend ride the live uniforms, and the canvas texture is
/// resized on playback start, so geometry/canvas edits must NOT rebuild windows
/// (a DragValue edit would otherwise storm window recreation and bury the GUI).
/// ponytail: `pixel_perfect` (the sampler filter baked at renderer creation)
/// goes stale when a geometry edit flips whether output size == source size —
/// a filtering nit, not a correctness bug; upgrade path is a manual rebuild via
/// "Open Projection Output Windows".
fn projection_structure_changed(
    built: &cuepool_core::ProjectionConfig,
    live: &cuepool_core::ProjectionConfig,
) -> bool {
    // Same fallback as create_output_windows: no configured outputs = one default.
    let default;
    let live_outputs: &[cuepool_core::ProjectorOutput] = if live.outputs.is_empty() {
        default = cuepool_core::ProjectorOutput::default_single();
        std::slice::from_ref(&default)
    } else {
        live.outputs.as_slice()
    };
    if built.outputs.len() != live_outputs.len() {
        return true;
    }
    built.outputs.iter().zip(live_outputs).any(|(b, l)| {
        b.monitor_id != l.monitor_id || b.fullscreen_monitor != l.fullscreen_monitor
    })
}

struct App {
    // ── wgpu core ──
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// `Surface::configure` takes this exclusively; GPU queue/present cycles
    /// take it shared. Lock it without `VideoControl` or `frame_state` held;
    /// configure paths never take either user mutex while holding the gate.
    configure_gate: Arc<RwLock<()>>,

    // ── control window (egui) ──
    control_window: Option<Arc<Window>>,
    control_surface: Option<wgpu::Surface<'static>>,
    control_config: Option<wgpu::SurfaceConfiguration>,

    // ── projection output windows ──
    /// The Text cue currently shown on the overlay.
    current_text_qid: Option<rust_decimal::Decimal>,
    output_windows: Vec<OutputWindow>,
    /// Projection settings the open output windows were built from (effective
    /// outputs). `about_to_wait` rebuilds the windows when the live show file
    /// diverges from this structurally.
    output_windows_built_from: Option<cuepool_core::ProjectionConfig>,
    /// Frame content shared with the per-output render threads. Published by
    /// the consume thread; render threads lock it briefly to re-snapshot when
    /// the generation bumps.
    frame_state: Arc<Mutex<OutputFrameState>>,
    /// Playback control shared with the consume thread (see `VideoControl`).
    video_control: Arc<Mutex<VideoControl>>,
    /// Cue-driven canvas/overlay work for the consume thread (rare).
    canvas_cmd_tx: std::sync::mpsc::Sender<CanvasCommand>,
    /// Stop signal + handle for the consume thread (joined on graceful exit).
    consume_stop: Arc<AtomicBool>,
    consume_join: Option<std::thread::JoinHandle<()>>,
    /// One-shot guard for the about-to-wait consumer watchdog.
    consume_failure_reported: bool,
    /// Last projection outputs pushed to the consume thread (change detect).
    published_outputs: Vec<cuepool_core::ProjectorOutput>,

    // ── egui ──
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,
    /// Resolved font-file paths already registered with the egui context.
    registered_fonts: std::collections::HashSet<String>,

    // ── app state ──
    cuepool: CuePoolApp,
    window_ids: Option<WindowIds>,

    // ── audio ──
    /// `None` means output configuration failed. Keeping this optional is the
    /// fail-closed boundary: no old/default stream survives a requested ASIO failure.
    audio_engine: Option<AudioEngine>,
    active_cues: Vec<ActiveCue>,
    delayed_cues: Vec<DelayedCue>,
    paused: bool,
    show_start_time: Option<Instant>,
    show_start_clock: Option<std::time::Duration>,
    /// Audio-clock time when the show was paused — freezes the show clock.
    show_pause_started: Option<std::time::Duration>,
    /// Seconds subtracted from the raw show clock (accumulated pause time,
    /// minus any frame-step advances made while paused).
    show_paused_offset: f64,
    triggered_timecodes: Vec<rust_decimal::Decimal>,
    /// TimeCode cues with a duration currently occupying time.
    active_timecodes: Vec<(rust_decimal::Decimal, std::time::Instant)>,

    // ── video playback ──
    /// Kept for render threads to request winit-side surface rebuilds; the
    /// consume thread gets its own clone at construction.
    event_loop_proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    /// The decode channel, clock, pause/step/fade state and frame consumption
    /// live in `video_control` (shared with the consume thread, which owns the
    /// canvas upload + convert + publish path). The decode thread is a bounded
    /// producer; backpressure keeps decode a few frames ahead of the clock.
    video_stop_flag: Arc<AtomicBool>,
    video_pause_flag: Arc<AtomicBool>,
    /// QID of the cue whose video is currently playing (for loop sync).
    current_video_qid: Option<rust_decimal::Decimal>,
    /// Last `SharedState.project_generation` we acted on. A change means a project
    /// was loaded, so the output windows must rebuild for its projection settings.
    last_project_generation: u64,
    /// When the control window was last asked to repaint (throttles its ~60 Hz redraw).
    last_control_redraw: std::time::Instant,
    /// Last seen set of physical monitors, to detect hotplug / projector warm-up and
    /// re-apply the output→monitor assignment.
    last_monitor_set: Vec<cuepool_core::MonitorId>,
    /// Throttle for the (OS-querying) monitor enumeration.
    last_monitor_check: std::time::Instant,
    /// While Some and unexpired, each output window flashes a distinct colour so the
    /// operator can see which window is on which projector.
    identify_until: Option<std::time::Instant>,
    /// Frame-pacing diagnostics, printed once/sec when QPLAYER_FPS_DEBUG is set.
    fps_debug: bool,
    /// about_to_wait iterations since the last 1 Hz diagnostics flush — the
    /// main-loop liveness diagnostic (a GPU-stalled loop shows up here).
    dbg_ticks: u32,
    dbg_last_log: std::time::Instant,

    // ── protocols ──
    osc_manager: Option<OscManager>,
    osc_rx: Option<std::sync::mpsc::Receiver<OscEvent>>,
    #[allow(dead_code)]
    msc_manager: Option<MscManager>,
    msc_rx: Option<std::sync::mpsc::Receiver<MscEvent>>,
    midi_manager: Option<MidiManager>,
    last_discovery: Instant,

    // ── MTC follow ──
    /// Listens on all MIDI ports for timecode (independent of `midi_manager`,
    /// which only handles voice-message triggers).
    mtc_receiver: MtcReceiver,
    /// The video cue currently following MTC, if any.
    mtc_follow: Option<MtcFollowState>,
    /// Last measured drift (target − video position) while following, for the GUI.
    mtc_drift: Option<f64>,
    /// Last frame rate we warned about (rate-limits the non-25fps warning).
    mtc_warned_fps: Option<MtcFrameRate>,

    // ── lighting ──
    lighting: LightingEngine,
    recorder: Recorder,
    /// Canvas downsampler for pixel-map segments (lazy; needs the device).
    pixel_sampler: Option<cuepool_video::PixelSampler>,
    last_pixel_sample: Instant,
    /// Dedicated pixel-map texture fed by PixelMap cues (LED content
    /// independent of the projector canvas). Sized to the playing media.
    pixmap_texture: Option<cuepool_video::CanvasTexture>,
    pixmap_yuv: Option<cuepool_video::YuvConverter>,
    pixmap_frame_rx: Option<std::sync::mpsc::Receiver<VideoFrame>>,
    pixmap_stop_flag: Arc<AtomicBool>,

    // ── trigger state ──
    wall_clock_fired: std::collections::HashMap<rust_decimal::Decimal, Instant>,
    timecode_fired: std::collections::HashSet<rust_decimal::Decimal>,

    // ── polish ──
    last_window_title: String,
    autosave_running: Arc<AtomicBool>,
    modifiers: winit::keyboard::ModifiersState,

    // ── plugins ──
}

impl App {
    fn new(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    ) -> Self {
        let cuepool = CuePoolApp::new();

        // Protocol settings from project settings (fallback to defaults)
        let (nic, subnet, osc_rx_port, osc_tx_port, is_remote_host, enable_remote_control) = {
            match cuepool.state().lock() {
                Ok(state) => {
                    let settings = &state.show_file.show_settings;
                    let nic_str = settings.osc_nic.parse::<Ipv4Addr>().unwrap_or(Ipv4Addr::new(127,0,0,1));
                    let subnet_str = Ipv4Addr::new(255,255,255,0); // TODO: expose subnet in settings
                    let rx = settings.osc_rx_port as u16;
                    let tx = settings.osc_tx_port as u16;
                    // Port flipping: if remote control enabled and NOT host, swap ports
                    let (rx, tx) = if settings.enable_remote_control && !settings.is_remote_host {
                        (tx, rx)
                    } else {
                        (rx, tx)
                    };
                    (nic_str, subnet_str, rx, tx, settings.is_remote_host, settings.enable_remote_control)
                }
                Err(_) => {
                    (Ipv4Addr::new(127,0,0,1), Ipv4Addr::new(255,255,255,0), 9000u16, 9001u16, true, false)
                }
            }
        };

        let (osc_manager, osc_rx) = {
            let (tx, rx) = std::sync::mpsc::channel();
            match OscManager::new(nic, osc_rx_port, osc_tx_port, subnet, tx) {
                Ok(m) => {
                    log::info!("OSC manager started on {}:{} (TX: {}), remote_control={} is_host={}",
                        nic, osc_rx_port, osc_tx_port, enable_remote_control, is_remote_host);
                    (Some(m), Some(rx))
                }
                Err(e) => {
                    log::error!("Failed to start OSC manager: {e}");
                    (None, Some(rx))
                }
            }
        };

        let (msc_manager, msc_rx) = {
            let (tx, rx) = std::sync::mpsc::channel();
            match MscManager::new(nic, 7000, 7001, subnet, tx.clone()) {
                Ok(m) => {
                    log::info!("MSC manager started on {}:7000", nic);
                    // Wire default MSC subscriptions
                    m.subscribe(MscCommandFlags::GO | MscCommandFlags::TIMED_GO, move |pkt| {
                        let event = match &pkt.data {
                            cuepool_protocols::msc::MscData::Go { qid, executor, page } => {
                                Some(MscEvent::Go { qid: qid.clone(), executor: *executor, page: *page })
                            }
                            cuepool_protocols::msc::MscData::TimedGo { qid, executor, page, time } => {
                                Some(MscEvent::TimedGo { qid: qid.clone(), executor: *executor, page: *page, time: *time })
                            }
                            _ => None,
                        };
                        if let Some(ev) = event {
                            let _ = tx.send(ev);
                        }
                    });
                    (Some(m), Some(rx))
                }
                Err(e) => {
                    log::error!("Failed to start MSC manager: {e}");
                    (None, Some(rx))
                }
            }
        };

        let midi_manager = match MidiManager::new() {
            Ok(m) => {
                log::info!("MIDI input manager started");
                Some(m)
            }
            Err(e) => {
                log::warn!("MIDI input unavailable: {e}");
                None
            }
        };

        let autosave_running = Arc::new(AtomicBool::new(true));
        spawn_autosave_thread(Arc::clone(&cuepool.state()), Arc::clone(&autosave_running));

        // Show-control UI: labels (cue names, meters, status) must not be
        // drag-selectable — egui's global label selection also has a
        // stuck-drag failure mode where selection follows the cursor after a
        // click. TextEdits keep their own selection either way.
        let egui_ctx = egui::Context::default();
        egui_ctx.global_style_mut(|style| {
            style.interaction.selectable_labels = false;
            style.interaction.multi_widget_text_select = false;
        });

        // Static diagnostics for the Status window (Help → Status…): filled
        // once here; the live counters refresh ~once per second in the tick.
        {
            let info = adapter.get_info();
            let ffmpeg = cuepool_video::ffmpeg_version();
            let mut state = cuepool.state().lock_unpoisoned();
            let d = &mut state.diagnostics;
            d.app_version = env!("CARGO_PKG_VERSION").into();
            d.os = std::env::consts::OS.into();
            d.arch = std::env::consts::ARCH.into();
            d.gpu_name = info.name;
            d.gpu_backend = format!("{:?}", info.backend);
            d.gpu_driver = info.driver;
            d.gpu_driver_info = info.driver_info;
            d.ffmpeg_version = format!(
                "libavutil {}.{}.{}",
                ffmpeg >> 16,
                (ffmpeg >> 8) & 0xff,
                ffmpeg & 0xff,
            );
            for var in ["QPLAYER_PRESENT_MODE", "QPLAYER_FPS_DEBUG", "QPLAYER_NO_HWACCEL"] {
                if let Ok(value) = std::env::var(var) {
                    d.env_overrides.push((var.into(), value));
                }
            }
        }

        // Video consume thread: owns the decode-channel drain, canvas/overlay
        // textures, YUV convert and the frame-state publish — every non-egui
        // GPU call. Keeps the winit thread (egui + window lifecycle) free of
        // the Windows/NVIDIA WSI stall behind the vsync-blocked render threads.
        let video_control = Arc::new(Mutex::new(VideoControl::default()));
        let frame_state = Arc::new(Mutex::new(OutputFrameState::default()));
        let configure_gate = Arc::new(RwLock::new(()));
        let (canvas_cmd_tx, canvas_cmd_rx) = std::sync::mpsc::channel::<CanvasCommand>();
        let consume_stop = Arc::new(AtomicBool::new(false));
        let consume_join = {
            let device = device.clone();
            let queue = queue.clone();
            let control = Arc::clone(&video_control);
            let frame = Arc::clone(&frame_state);
            let configure_gate = Arc::clone(&configure_gate);
            let stop = Arc::clone(&consume_stop);
            let proxy = proxy.clone();
            std::thread::Builder::new()
                .name("video-consume".into())
                .spawn(move || {
                    video_consume_thread(
                        device,
                        queue,
                        configure_gate,
                        control,
                        frame,
                        canvas_cmd_rx,
                        proxy,
                        stop,
                    )
                })
                .expect("spawn video consume thread")
        };

        let mut app = Self {
            instance,
            adapter,
            device,
            queue,
            configure_gate,
            control_window: None,
            control_surface: None,
            control_config: None,
            egui_ctx,
            egui_state: None,
            registered_fonts: std::collections::HashSet::new(),
            egui_renderer: None,
            cuepool,
            window_ids: None,
            audio_engine: None,
            event_loop_proxy: proxy,
            current_text_qid: None,
            output_windows: Vec::new(),
            output_windows_built_from: None,
            frame_state,
            video_control,
            canvas_cmd_tx,
            consume_stop,
            consume_join: Some(consume_join),
            consume_failure_reported: false,
            published_outputs: Vec::new(),
            video_stop_flag: Arc::new(AtomicBool::new(false)),
            video_pause_flag: Arc::new(AtomicBool::new(false)),
            current_video_qid: None,
            last_project_generation: 0,
            last_control_redraw: std::time::Instant::now(),
            last_monitor_set: Vec::new(),
            last_monitor_check: std::time::Instant::now(),
            identify_until: None,
            fps_debug: std::env::var("QPLAYER_FPS_DEBUG").is_ok(),
            dbg_ticks: 0,
            dbg_last_log: std::time::Instant::now(),
            osc_manager,
            osc_rx,
            msc_manager,
            msc_rx,
            midi_manager,
            last_discovery: Instant::now(),
            mtc_receiver: MtcReceiver::new(),
            mtc_follow: None,
            mtc_drift: None,
            mtc_warned_fps: None,
            last_window_title: String::new(),
            autosave_running,
            active_cues: Vec::new(),
            delayed_cues: Vec::new(),
            paused: false,
            show_start_time: None,
            show_start_clock: None,
            show_pause_started: None,
            show_paused_offset: 0.0,
            triggered_timecodes: Vec::new(),
            active_timecodes: Vec::new(),
            modifiers: winit::keyboard::ModifiersState::empty(),
            lighting: LightingEngine::default(),
            recorder: Recorder::new(),
            pixel_sampler: None,
            last_pixel_sample: Instant::now(),
            pixmap_texture: None,
            pixmap_yuv: None,
            pixmap_frame_rx: None,
            pixmap_stop_flag: Arc::new(AtomicBool::new(false)),
            wall_clock_fired: std::collections::HashMap::new(),
            timecode_fired: std::collections::HashSet::new(),
        };
        app.apply_audio_settings();
        app
    }

    /// Create the control window + surface + egui state.
    fn create_control_window(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    winit::window::WindowAttributes::default()
                        .with_title("CuePool")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0)),
                )
                .expect("create control window"),
        );

        let surface = self
            .instance
            .create_surface(Arc::clone(&window))
            .expect("create control surface");

        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&self.adapter, size.width, size.height)
            .expect("control surface config");
        // Non-vsync present for the CONTROL window: on this single-threaded loop a
        // vsync-blocked control present serializes with the output window's vsync
        // present and roughly halves the output's effective frame rate. Tearing on
        // the operator GUI is irrelevant; output windows keep Fifo for clean playback.
        let caps = surface.get_capabilities(&self.adapter);
        if caps.present_modes.contains(&wgpu::PresentMode::Mailbox) {
            config.present_mode = wgpu::PresentMode::Mailbox;
        } else if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            config.present_mode = wgpu::PresentMode::Immediate;
        }
        {
            let _configure_guard = self
                .configure_gate
                .write()
                .unwrap_or_else(|e| e.into_inner());
            surface.configure(&self.device, &config);
        }

        let egui_state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            None,
            None,
            None,
        );

        let egui_renderer = egui_wgpu::Renderer::new(
            &self.device,
            config.format,
            egui_wgpu::RendererOptions {
                dithering: false,
                ..Default::default()
            },
        );

        let control_id = window.id();
        self.control_window = Some(window);
        self.control_surface = Some(surface);
        self.control_config = Some(config);
        self.egui_state = Some(egui_state);
        self.egui_renderer = Some(egui_renderer);

        self.window_ids = Some(WindowIds {
            control: control_id,
            video: Vec::new(),
        });
    }

    /// Toggle fullscreen on all output windows and update cursor visibility.
    fn toggle_output_fullscreen(&self) {
        for out in &self.output_windows {
            let currently_fullscreen = out.window.fullscreen().is_some();
            if currently_fullscreen {
                out.window.set_fullscreen(None);
                out.window.set_cursor_visible(true);
            } else {
                out.window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                out.window.set_cursor_visible(false);
            }
        }
    }

    /// Create (or recreate) the video output window (starts windowed).
    /// Create (or recreate) one output window per configured projector output.
    fn create_output_windows(&mut self, event_loop: &ActiveEventLoop) {
        let projection = {
            let state = self.cuepool.state().lock_unpoisoned();
            state.show_file.projection.clone()
        };

        // Close existing output windows so we can honour new output counts/sizes.
        self.output_windows.clear();
        if let Some(ids) = self.window_ids.as_mut() {
            ids.video.clear();
        }

        // If nothing is configured yet, fall back to a single 1920x1080 output so
        // video playback still produces a window.
        let outputs: Vec<_> = if projection.outputs.is_empty() {
            vec![cuepool_core::ProjectorOutput::default_single()]
        } else {
            projection.outputs.clone()
        };

        // Snapshot what these windows are being built from (fallback applied), for
        // the structural-divergence check in about_to_wait.
        self.output_windows_built_from = Some(cuepool_core::ProjectionConfig {
            outputs: outputs.clone(),
            ..projection
        });

        let monitors: Vec<_> = event_loop.available_monitors().collect();

        // Resolve each output to a physical monitor by saved position descriptor
        // (survives reboots / projector warm-up reorder), falling back to the legacy
        // index for old projects. `assigned[i]` = Some(monitor index) or None (windowed).
        let mon_descs: Vec<cuepool_core::MonitorId> =
            monitors.iter().map(monitor_descriptor).collect();
        let wanted: Vec<Option<cuepool_core::MonitorId>> =
            outputs.iter().map(|o| o.monitor_id.clone()).collect();
        let mut assigned =
            cuepool_core::resolve_monitor_assignment(&wanted, &mon_descs, MONITOR_MATCH_DIST_SQ);
        let mut used = vec![false; monitors.len()];
        for a in assigned.iter().flatten() {
            used[*a] = true;
        }
        for (o, a) in outputs.iter().zip(assigned.iter_mut()) {
            if a.is_none() {
                if let Some(idx) = o.fullscreen_monitor {
                    if idx < monitors.len() && !used[idx] {
                        used[idx] = true;
                        *a = Some(idx);
                    }
                }
            }
        }

        // Windowed (un-assigned) outputs are tiled side-by-side at a preview size
        // that fits across the screen; assigned outputs go borderless-fullscreen.
        let windowed_count = assigned.iter().filter(|a| a.is_none()).count().max(1);
        let (screen_w, screen_h) = event_loop
            .primary_monitor()
            .or_else(|| monitors.first().cloned())
            .map(|m| {
                let sf = m.scale_factor();
                let s = m.size();
                (s.width as f64 / sf, s.height as f64 / sf)
            })
            .unwrap_or((1440.0, 900.0));
        let gap = 12.0;
        let tile_w = (((screen_w * 0.96) - gap * (windowed_count as f64 + 1.0))
            / windowed_count as f64)
            .max(160.0);
        let mut windowed_idx = 0usize;
        let mut pending_outputs = Vec::with_capacity(outputs.len());

        for (out_idx, output) in outputs.iter().enumerate() {
            let mut attrs = winit::window::WindowAttributes::default()
                .with_title(format!("CuePool Output {}", output.name))
                .with_visible(true);

            if let Some(mon_idx) = assigned[out_idx] {
                if let Some(monitor) = monitors.get(mon_idx) {
                    attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(
                        monitor.clone(),
                    ))));
                }
            } else {
                let aspect =
                    output.output_height.max(1) as f64 / output.output_width.max(1) as f64;
                let h = (tile_w * aspect).min(screen_h * 0.7);
                let x = gap + windowed_idx as f64 * (tile_w + gap);
                attrs = attrs
                    .with_inner_size(winit::dpi::LogicalSize::new(tile_w, h))
                    .with_position(winit::dpi::LogicalPosition::new(x, 80.0));
                windowed_idx += 1;
            }

            let window = Arc::new(
                event_loop.create_window(attrs).expect("create output window"),
            );

            let surface = self
                .instance
                .create_surface(Arc::clone(&window))
                .expect("create output surface");

            let size = window.inner_size();
            let mut config = surface
                .get_default_config(&self.adapter, size.width, size.height)
                .expect("output surface config");
            let caps = surface.get_capabilities(&self.adapter);
            // The edge-blend brightness ramp is a linear-light multiply; it's only
            // correct if the GPU re-encodes to sRGB on write. Windows backends
            // default to a non-sRGB surface, so the blend band crushes to black.
            // Force an sRGB surface format when the surface offers one.
            if let Some(srgb) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
                config.format = srgb;
            }
            // Present mode: EVERY output blocks on its own display's vsync
            // (Fifo). Each output renders on its own thread now, so a blocking
            // acquire paces only that thread — ungenlocked projectors no longer
            // serialize their vsync waits against each other. Override with
            // QPLAYER_PRESENT_MODE=fifo|fifo_relaxed|mailbox|immediate to force
            // one mode on every output.
            let want = match std::env::var("QPLAYER_PRESENT_MODE").as_deref() {
                Ok("mailbox") => wgpu::PresentMode::Mailbox,
                Ok("immediate") => wgpu::PresentMode::Immediate,
                Ok("fifo_relaxed") => wgpu::PresentMode::FifoRelaxed,
                _ => wgpu::PresentMode::Fifo,
            };
            config.present_mode = if caps.present_modes.contains(&want) {
                want
            } else {
                wgpu::PresentMode::Fifo
            };
            if matches!(
                config.present_mode,
                wgpu::PresentMode::Mailbox | wgpu::PresentMode::Immediate
            ) {
                log::warn!(
                    "Output '{}' uses {:?}, which free-runs; throttling to ~120 fps for safety",
                    output.name,
                    config.present_mode
                );
            }
            {
                let _configure_guard = self
                    .configure_gate
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                surface.configure(&self.device, &config);
            }

            if std::env::var("QPLAYER_FPS_DEBUG").is_ok() {
                let refresh = window
                    .current_monitor()
                    .and_then(|m| m.refresh_rate_millihertz())
                    .map(|mhz| format!("{:.2} Hz", mhz as f64 / 1000.0))
                    .unwrap_or_else(|| "?".into());
                eprintln!(
                    "OUTPUT '{}': present_mode={:?} format={:?} refresh={} fullscreen={}",
                    output.name,
                    config.present_mode,
                    config.format,
                    refresh,
                    window.fullscreen().is_some(),
                );
            }

            let pixel_perfect =
                output.output_width == output.source_width && output.output_height == output.source_height;
            let renderer = cuepool_video::ProjectionRenderer::new(
                &self.device,
                config.format,
                pixel_perfect,
            );

            let size_atomic = Arc::new(AtomicU64::new(pack_size(size.width, size.height)));
            let stop = Arc::new(AtomicBool::new(false));
            let presented = Arc::new(AtomicU32::new(0));
            let present_mode = config.present_mode;
            let format = config.format;
            pending_outputs.push((
                out_idx,
                output.clone(),
                window,
                surface,
                config,
                renderer,
                size_atomic,
                stop,
                presented,
                present_mode,
                format,
            ));
        }

        // All surfaces must be configured before any render thread can submit.
        for (
            out_idx,
            output_config,
            window,
            surface,
            config,
            renderer,
            size_atomic,
            stop,
            presented,
            present_mode,
            format,
        ) in pending_outputs
        {
            let video_id = window.id();
            let frame_state = Arc::clone(&self.frame_state);
            let configure_gate = Arc::clone(&self.configure_gate);
            let thread_size = Arc::clone(&size_atomic);
            let thread_stop = Arc::clone(&stop);
            let thread_presented = Arc::clone(&presented);
            let device = self.device.clone();
            let queue = self.queue.clone();
            let event_loop_proxy = self.event_loop_proxy.clone();
            let fallback_output = output_config.clone();
            let join = std::thread::Builder::new()
                .name(format!("output-render-{}", output_config.name))
                .spawn(move || {
                    output_render_thread(
                        surface,
                        config,
                        renderer,
                        device,
                        queue,
                        configure_gate,
                        event_loop_proxy,
                        frame_state,
                        thread_size,
                        thread_stop,
                        thread_presented,
                        video_id,
                        out_idx,
                        fallback_output,
                    );
                })
                .expect("spawn output render thread");
            self.output_windows.push(OutputWindow {
                id: video_id,
                window,
                output_config,
                size: size_atomic,
                stop,
                presented,
                present_mode,
                format,
                join: Some(join),
            });

            if let Some(ids) = self.window_ids.as_mut() {
                ids.video.push(video_id);
            }
        }

        // Freshly created (fullscreen) output windows grab the foreground — on
        // Windows they bury the control window, and auto-rebuilds make that a
        // surprise. Pull the GUI back to the front.
        if let Some(control) = &self.control_window {
            control.focus_window();
        }
    }



    /// Handle a `Go` command: start audio (and video if cue is VideoCue).
    /// Also handles `WithLast` trigger mode for subsequent cues.
    fn handle_go(&mut self, event_loop: &ActiveEventLoop) {
        // Start the show clock on first Go
        if self.show_start_time.is_none() {
            self.show_start_time = Some(Instant::now());
            self.show_start_clock = Some(self.audio_clock());
            self.show_paused_offset = 0.0;
            self.show_pause_started = self.paused.then(|| self.audio_clock());
            self.triggered_timecodes.clear();
            self.active_timecodes.clear();
            self.timecode_fired.clear();
        }

        let (start_qid, start_idx) = {
            let state = self.cuepool.state().lock_unpoisoned();
            let qid = state.selected_cue_id;
            let idx = qid.and_then(|q| state.show_file.cues.iter().position(|c| c.base().qid == q));
            (qid, idx)
        };

        let Some(start_qid) = start_qid else {
            log::info!("Go pressed but no cue selected");
            return;
        };
        let Some(start_idx) = start_idx else {
            log::warn!("Selected cue Q{} not found in cue list", start_qid);
            return;
        };


        // Play the selected cue and all consecutive WithLast followers
        let cues_to_play = {
            let state = self.cuepool.state().lock_unpoisoned();
            let mut result = Vec::new();
            for i in start_idx..state.show_file.cues.len() {
                let cue = &state.show_file.cues[i];
                if !cue.enabled() {
                    if i == start_idx {
                        // The primary cue we wanted to play is disabled — stop here
                        break;
                    }
                    // A WithLast follower is disabled — skip it but keep looking for more followers
                    continue;
                }
                if i == start_idx || cue.base().trigger == cuepool_core::TriggerMode::WithLast {
                    result.push(cue.clone());
                } else {
                    break;
                }
            }
            result
        };

        for cue in cues_to_play {
            self.play_cue(&cue, event_loop);
        }

        // Advance the playhead so the next Go fires the following cue (QLab-style
        // stepping). Skip the cues that auto-fired alongside this one. A goto cue
        // sets its own standby (the target), so don't override it here.
        let next_qid = {
            let state = self.cuepool.state().lock_unpoisoned();
            let fired_goto =
                matches!(state.show_file.cues.get(start_idx), Some(cuepool_core::Cue::Goto { .. }));
            if fired_goto {
                None
            } else {
                next_standby_qid(&state.show_file.cues, start_idx)
            }
        };
        if let Some(next_qid) = next_qid {
            if let Ok(mut state) = self.cuepool.state().lock() {
                state.selected_cue_id = Some(next_qid);
            }
        }
    }

    /// Look up a cue by QID and play it. Used by MIDI/hotkey/wall-clock/timecode triggers.
    fn play_cue_by_qid(&mut self, qid: rust_decimal::Decimal, event_loop: &ActiveEventLoop) {
        let cue = {
            let state = self.cuepool.state().lock_unpoisoned();
            state.show_file.cues.iter().find(|c| c.base().qid == qid).cloned()
        };
        if let Some(cue) = cue {
            self.play_cue(&cue, event_loop);
        } else {
            log::warn!("Trigger referenced unknown cue Q{}", qid);
        }
    }

    /// Is this cue currently producing output? True while its audio is playing
    /// (or paused) or while its video/still is the one on screen.
    fn cue_is_active(&self, qid: rust_decimal::Decimal) -> bool {
        self.current_video_qid == Some(qid)
            || self.active_cues.iter().any(|ac| {
                ac.qid == qid
                    && matches!(
                        ac.state,
                        CueState::Playing | CueState::PlayingLooped | CueState::Paused
                    )
            })
    }

    fn play_cue(&mut self, cue: &cuepool_core::Cue, event_loop: &ActiveEventLoop) {
        if !cue.enabled() {
            log::info!("Skipping disabled cue Q{}", cue.base().qid);
            return;
        }

        let qid = cue.base().qid;
        let name = cue.base().name.clone();
        let delay = cue.base().delay;

        // Re-trigger guard: a cue marked not-re-triggerable is ignored if it is
        // already playing (stops stacked audio / flashing video from a double Go).
        if !cue.base().retriggerable && self.cue_is_active(qid) {
            log::info!("Ignoring re-trigger of Q{qid} (not re-triggerable, still playing)");
            return;
        }

        // Remote cue delegation: if remote_node is set and not local, send OSC instead
        let remote_node = cue.base().remote_node.clone();
        if !remote_node.is_empty() {
            let (enable_remote, local_name) = {
                let Ok(state) = self.cuepool.state().lock() else { return; };
                (state.show_file.show_settings.enable_remote_control,
                 state.show_file.show_settings.node_name.clone())
            };
            if enable_remote && remote_node != local_name {
                if let Some(osc) = &self.osc_manager {
                    let qid_str = qid.to_string();
                    let _ = osc.send(rosc::OscMessage {
                        addr: "/qplayer/remote/go".into(),
                        args: vec![
                            rosc::OscType::String(remote_node),
                            rosc::OscType::String(qid_str),
                        ],
                    });
                    log::info!("Delegated Q{} to remote node {}", qid, cue.base().remote_node);
                }
                return;
            }
        }

        // If cue has a delay, schedule it instead of playing immediately.
        // Store it with the delay stripped: the replay re-enters play_cue,
        // which would otherwise reschedule it forever.
        if delay.as_secs_f64() > 0.0 {
            log::info!("Delaying cue Q{} by {:.2}s", qid, delay.as_secs_f64());
            let mut cue = cue.clone();
            cue.base_mut().delay = cuepool_core::Timespan::ZERO;
            self.delayed_cues.push(DelayedCue {
                cue,
                start_at: std::time::Instant::now() + std::time::Duration::from_secs_f64(delay.as_secs_f64()),
            });
            return;
        }

        // Check if cue is already preloaded — if so, just activate it
        if let Some(idx) = self.active_cues.iter().position(|ac| ac.qid == qid && ac.state == CueState::Ready) {
            let ac = &mut self.active_cues[idx];
            ac.input.set_active(true);
            let new_state = if cue.base().loop_mode == cuepool_core::LoopMode::Looped || cue.base().loop_mode == cuepool_core::LoopMode::LoopedInfinite {
                CueState::PlayingLooped
            } else {
                CueState::Playing
            };
            ac.state = new_state;
            log::info!("Activated preloaded cue Q{}", qid);
            return;
        }

        match cue {
            cuepool_core::Cue::Sound { path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, routing, .. } => {
                log::info!("Go SoundCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, *start_time, *duration, *volume, *fade_in, *fade_out, *fade_type, *eq, *pan, routing.clone(), false);
            }
            cuepool_core::Cue::Video { path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, routing, follow_mtc, mtc_start, .. } => {
                log::info!("Go VideoCue: {}", path);
                if *follow_mtc {
                    // MTC follow: the video plays silent (audio comes from the
                    // MTC master, e.g. Pro Tools), loads, and HOLDS on frame 0
                    // until MTC plays. GO on the same cue re-arms a fresh hold.
                    self.play_video(path, qid, event_loop);
                    self.mtc_follow = Some(MtcFollowState {
                        qid,
                        path: path.clone(),
                        offset_secs: mtc_start.as_secs_f64(),
                        hold_position: Some(0.0),
                        last_tick: Instant::now(),
                        last_mtc_secs: 0.0,
                        last_mtc_at: Instant::now(),
                    });
                } else {
                    // A plain video cue takes over the output — drop any MTC follow.
                    self.mtc_follow = None;
                    self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, *start_time, *duration, *volume, *fade_in, *fade_out, *fade_type, *eq, *pan, routing.clone(), false);
                    self.play_video(path, qid, event_loop);
                }
            }
            cuepool_core::Cue::Stop { stop_qid, stop_mode, fade_out_time, fade_type, stop_all, .. } => {
                if *stop_all {
                    log::info!("Go StopCue -> stop all (transport Stop)");
                    self.stop_all();
                } else {
                    log::info!("Go StopCue -> stop Q{}", stop_qid);
                    self.handle_stop_cue(*stop_qid, *stop_mode, *fade_out_time, *fade_type);
                }
            }
            cuepool_core::Cue::Volume { sound_qid, volume, fade_time, fade_type, .. } => {
                log::info!("Go VolumeCue -> adjust Q{} to {:.1} dB", sound_qid, 20.0 * volume.log10());
                self.handle_volume_cue(*sound_qid, *volume, *fade_time, *fade_type);
            }
            cuepool_core::Cue::Osc { command, .. } => {
                log::info!("Go OSCCue: {}", command);
                if let Some(remainder) = strip_udp_prefix(command) {
                    // Raw UDP command (e.g. BrightSign): `udp:payload` goes to
                    // the default target, `udp:name:payload` or
                    // `udp:10.0.0.5:payload` to a specific player. No OSC encoding.
                    let (host, port, payload) = {
                        let Ok(state) = self.cuepool.state().lock() else { return; };
                        let settings = &state.show_file.show_settings;
                        let (host, payload) = resolve_udp_command(remainder, &settings.udp_targets, &settings.udp_tx_host);
                        (host, settings.udp_tx_port, payload.to_string())
                    };
                    send_udp_command(&payload, &host, port);
                } else if let Some(osc) = &self.osc_manager {
                    if let Ok(msg) = parse_osc_command(command) {
                        if let Err(e) = osc.send(msg) {
                            log::error!("OSC send failed: {}", e);
                        }
                    } else {
                        log::error!("Invalid OSC command: {}", command);
                    }
                } else {
                    log::warn!("OSC manager not available, cannot send: {}", command);
                }
            }
            cuepool_core::Cue::Group { .. } => {
                // A group owns the cues whose `parent` points at it. Going the
                // group fires that whole block (each via the normal play path, so
                // per-cue delay and the enabled flag still apply).
                let members: Vec<cuepool_core::Cue> = {
                    let state = self.cuepool.state().lock_unpoisoned();
                    state
                        .show_file
                        .cues
                        .iter()
                        // Exclude self: a group can never be its own member (guards
                        // against stray self-referential data causing recursion).
                        // AfterLast members don't fire at go — they chain off the
                        // preceding member's completion like anywhere else.
                        .filter(|c| {
                            c.base().parent == Some(qid)
                                && c.base().qid != qid
                                && c.base().trigger != cuepool_core::TriggerMode::AfterLast
                        })
                        .cloned()
                        .collect()
                };
                log::info!("Go GroupCue Q{} — firing {} member(s)", qid, members.len());
                for member in members {
                    self.play_cue(&member, event_loop);
                }
            }
            cuepool_core::Cue::TimeCode { start_time, duration, .. } => {
                log::info!("Go TimeCode cue Q{} at {:.2}s", qid, start_time.as_secs_f64());
                let duration_secs = duration.as_secs_f64();
                if duration_secs > 0.0 {
                    if let Some(show_start) = self.show_start_time {
                        let deadline = show_start + std::time::Duration::from_secs_f64(start_time.as_secs_f64() + duration_secs);
                        self.active_timecodes.push((qid, deadline));
                    }
                } else {
                    // Zero-duration TimeCode marker: trigger AfterLast chain immediately.
                    self.play_after_last_chain(qid, event_loop);
                }
            }
            cuepool_core::Cue::Text { text, font_size, font_colour, fit, font, .. } => {
                log::info!("Go TextCue Q{}: '{}'", qid, text);
                self.ensure_outputs_and_canvas(event_loop);
                let family = self.text_font_family(font);
                // Text renders on the overlay layer, over whatever video/image
                // is playing. With nothing underneath, blank the canvas so a
                // stale last frame doesn't reappear behind the text.
                let blank = self.current_video_qid.is_none()
                    && !self.video_control.lock_unpoisoned().canvas_has_frame;
                if blank {
                    let _ = self.canvas_cmd_tx.send(CanvasCommand::BlankCanvas);
                }
                let (cw, ch) = {
                    let state = self.cuepool.state().lock_unpoisoned();
                    (state.show_file.projection.canvas_width, state.show_file.projection.canvas_height)
                };
                let mut shown = false;
                match self.rasterize_text_block(
                    text, *font_size, *font_colour, family, cw, ch, *fit,
                ) {
                    // Rasterizing is CPU/egui work; the texture upload rides the
                    // consume thread (queued after the Resize above, so the
                    // overlay exists by then).
                    Some(frame) => {
                        let _ = self
                            .canvas_cmd_tx
                            .send(CanvasCommand::Overlay(Some((frame, *fit))));
                        shown = true;
                    }
                    // Empty text: clear the overlay.
                    None => {
                        let _ = self.canvas_cmd_tx.send(CanvasCommand::Overlay(None));
                    }
                }
                self.set_current_text_qid(shown.then_some(qid));
            }
            cuepool_core::Cue::Image { path, fit, .. } => {
                log::info!("Go ImageCue Q{}: {}", qid, path);
                self.ensure_outputs_and_canvas(event_loop);
                // A still replaces video output: clear video playback state so the
                // consume thread stops PTS-matching against stale frames (its
                // receiver drop also retires the decode thread). Cleared BEFORE
                // sending the upload command so no late video frame lands over it.
                {
                    let mut ctl = self.video_control.lock_unpoisoned();
                    ctl.stream_epoch += 1;
                    ctl.clock = None;
                    ctl.frame_rx = None;
                    ctl.peek_pts = None;
                    ctl.last_pts = None;
                }
                self.set_current_video_qid(Some(qid));
                let resolved = self.resolve_path(path).unwrap_or_else(|| path.to_string());
                let _ = self
                    .canvas_cmd_tx
                    .send(CanvasCommand::Image(resolved, *fit));
            }
            cuepool_core::Cue::Goto { target_qid, .. } => {
                log::info!("Go GotoCue Q{} -> arm Q{}", qid, target_qid);
                // Resolve the goto chain to a non-goto cue, guarding against cycles
                // (A->B->A) so we never recurse forever.
                let final_target = {
                    let state = self.cuepool.state().lock_unpoisoned();
                    resolve_goto_target(&state.show_file.cues, qid, *target_qid)
                };
                let Some(target) = final_target else {
                    log::warn!("Goto cue Q{}: cyclic or unknown target; ignoring", qid);
                    return;
                };
                // A goto just moves the playhead: arm the target as the next
                // standby cue (the following GO fires it). It does not fire it.
                // handle_go skips its post-fire advance for goto cues so this
                // arming stands.
                if let Ok(mut state) = self.cuepool.state().lock() {
                    state.selected_cue_id = Some(target);
                }
            }
            cuepool_core::Cue::PixelMap { path, .. } => {
                log::info!("Go PixelMapCue Q{}: {}", qid, path);
                self.play_pixmap(path, cue.base().loop_mode);
            }
            cuepool_core::Cue::Lighting { snapshot, fade_time, fade_type, .. } => {
                log::info!(
                    "Go LightingCue Q{} — {} fixture(s), fade {:.2}s",
                    qid,
                    snapshot.len(),
                    fade_time
                );
                self.lighting.go(snapshot, *fade_time, *fade_type);
            }
            cuepool_core::Cue::DmxShow { path, fade_in, fade_out, fade_type, priority, .. } => {
                let resolved = self.resolve_path(path).unwrap_or_else(|| path.to_string());
                match rustjay_lighting::read_rec(&resolved) {
                    Ok(events) => {
                        log::info!(
                            "Go DmxShowCue Q{}: {} — {} event(s), {:.1}s, priority {}",
                            qid,
                            path,
                            events.len(),
                            rustjay_lighting::rec_duration_ms(&events) as f32 / 1000.0,
                            priority
                        );
                        self.lighting.go_show(
                            qid,
                            events,
                            *priority,
                            *fade_in,
                            *fade_out,
                            *fade_type,
                            cue.base().loop_mode,
                            cue.base().loop_count,
                        );
                    }
                    Err(e) => log::error!("DmxShow cue Q{qid} failed to load '{resolved}': {e}"),
                }
            }
            other => {
                log::info!("Go on unsupported cue type: {:?}", std::mem::discriminant(other));
            }
        }

        // Instant cue types complete the moment they execute — continue an
        // AfterLast chain now. Sound/Video chain from check_finished_cues when
        // playback ends, TimeCode from its marker/deadline, Group members chain
        // individually, and a Goto only moves the playhead.
        if !matches!(
            cue,
            cuepool_core::Cue::Sound { .. }
                | cuepool_core::Cue::Video { .. }
                | cuepool_core::Cue::TimeCode { .. }
                | cuepool_core::Cue::Group { .. }
                | cuepool_core::Cue::Goto { .. }
                | cuepool_core::Cue::DmxShow { .. }
        ) {
            self.play_after_last_chain(qid, event_loop);
        }
    }

    /// Play media into the dedicated pixel-map texture. Stills upload once;
    /// videos get a self-paced decode thread (wall-clock PTS — no A/V sync or
    /// vsync consumer here, LEDs don't need it).
    fn play_pixmap(&mut self, path: &str, loop_mode: cuepool_core::LoopMode) {
        // Replace any previous pixmap stream (per-thread flag, same reasoning
        // as play_video).
        self.pixmap_stop_flag.store(true, Ordering::Relaxed);
        self.pixmap_stop_flag = Arc::new(AtomicBool::new(false));
        self.pixmap_frame_rx = None;

        let resolved = self.resolve_path(path).unwrap_or_else(|| path.to_string());

        // Still image → single upload, no thread.
        if let Ok(img) = image::open(&resolved) {
            let img = img.to_rgba8();
            let (w, h) = (img.width(), img.height());
            self.ensure_pixmap_texture(w, h);
            let _configure_guard = self
                .configure_gate
                .read()
                .unwrap_or_else(|e| e.into_inner());
            self.pixmap_texture.as_ref().unwrap().upload_frame(
                &self.queue,
                &VideoFrame::new(w, h, img.into_raw(), 0.0),
                cuepool_core::CanvasFit::Stretch, // same dims → exact copy
            );
            return;
        }

        // Video → decode thread feeding a small bounded channel.
        let (tx, rx) = std::sync::mpsc::sync_channel::<VideoFrame>(3);
        self.pixmap_frame_rx = Some(rx);
        let stop = Arc::clone(&self.pixmap_stop_flag);
        std::thread::Builder::new()
            .name("pixmap-decode".into())
            .spawn(move || pixmap_decode_thread(&resolved, loop_mode, stop, tx))
            .expect("spawn pixmap decode thread");
    }

    /// Get the pixmap texture, (re)created at the given size.
    fn ensure_pixmap_texture(&mut self, w: u32, h: u32) -> &cuepool_video::CanvasTexture {
        let recreate = self
            .pixmap_texture
            .as_ref()
            .is_none_or(|t| t.width != w || t.height != h);
        if recreate {
            self.pixmap_texture = Some(cuepool_video::CanvasTexture::new(&self.device, w, h));
        }
        self.pixmap_texture.as_ref().unwrap()
    }

    /// Drain pending pixmap frames and upload the newest to the pixmap texture.
    fn upload_pixmap_frames(&mut self) {
        let Some(rx) = &self.pixmap_frame_rx else { return };
        let mut latest: Option<VideoFrame> = None;
        while let Ok(f) = rx.try_recv() {
            latest = Some(f);
        }
        let Some(frame) = latest else { return };
        let (w, h) = (frame.width, frame.height);
        let configure_gate = Arc::clone(&self.configure_gate);
        let _configure_guard = configure_gate.read().unwrap_or_else(|e| e.into_inner());
        if frame.rgba().is_some() {
            self.ensure_pixmap_texture(w, h);
            let tex = self.pixmap_texture.as_ref().unwrap();
            tex.upload_frame(&self.queue, &frame, cuepool_core::CanvasFit::Stretch);
        } else {
            // YUV planes → GPU convert pass straight into the pixmap texture.
            self.ensure_pixmap_texture(w, h);
            if self.pixmap_yuv.is_none() {
                self.pixmap_yuv = Some(cuepool_video::YuvConverter::new(
                    &self.device,
                    wgpu::TextureFormat::Rgba8Unorm,
                ));
            }
            let conv = self.pixmap_yuv.as_mut().unwrap();
            conv.upload(&self.device, &self.queue, &frame, [w, h], cuepool_core::CanvasFit::Stretch);
            let tex = self.pixmap_texture.as_ref().unwrap();
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("pixmap-yuv") });
            conv.encode(&mut encoder, &tex.render_view());
            self.queue.submit(Some(encoder.finish()));
        }
    }

    /// Create output windows if none exist, and make sure the consume thread's
    /// canvas/overlay match the projection canvas size (for Text/Image cues).
    fn ensure_outputs_and_canvas(&mut self, event_loop: &ActiveEventLoop) {
        if self.output_windows.is_empty() {
            self.create_output_windows(event_loop);
        }
        let (w, h) = {
            let state = self.cuepool.state().lock_unpoisoned();
            (state.show_file.projection.canvas_width, state.show_file.projection.canvas_height)
        };
        let _ = self.canvas_cmd_tx.send(CanvasCommand::Resize { w, h, force: false });
    }

    /// Blank the text overlay and forget the active Text cue.
    fn clear_text_overlay(&mut self) {
        self.set_current_text_qid(None);
        let _ = self.canvas_cmd_tx.send(CanvasCommand::Overlay(None));
    }

    /// Set the current video cue and mirror its presence to the consume thread
    /// (drives the published `has_content`).
    fn set_current_video_qid(&mut self, qid: Option<rust_decimal::Decimal>) {
        self.current_video_qid = qid;
        self.video_control.lock_unpoisoned().video_active = qid.is_some();
    }

    /// Set the current Text cue and mirror its presence to the consume thread.
    fn set_current_text_qid(&mut self, qid: Option<rust_decimal::Decimal>) {
        self.current_text_qid = qid;
        self.video_control.lock_unpoisoned().text_active = qid.is_some();
    }

    /// Resolve a Text cue's font path to an egui font family, registering the
    /// file on first use. Empty/unreadable paths fall back to the built-in font.
    ///
    /// Registered fonts land in the atlas at the next `begin_pass`, so a font
    /// first seen in the same tick it is rendered falls back once (see
    /// `rasterize_text_block`); the inspector pre-registers on pick to avoid this.
    fn text_font_family(&mut self, font_path: &str) -> egui::FontFamily {
        if font_path.is_empty() {
            return egui::FontFamily::Proportional;
        }
        let resolved = self.resolve_path(font_path).unwrap_or_else(|| font_path.to_string());
        if !self.registered_fonts.contains(&resolved) {
            match std::fs::read(&resolved) {
                Ok(bytes) => {
                    self.egui_ctx.add_font(egui::epaint::text::FontInsert::new(
                        &resolved,
                        egui::FontData::from_owned(bytes),
                        vec![egui::epaint::text::InsertFontFamily {
                            family: egui::FontFamily::Name(resolved.clone().into()),
                            priority: egui::epaint::text::FontPriority::Highest,
                        }],
                    ));
                    self.registered_fonts.insert(resolved.clone());
                }
                Err(e) => {
                    log::error!("Text cue font '{resolved}': {e}; using built-in font");
                    return egui::FontFamily::Proportional;
                }
            }
        }
        egui::FontFamily::Name(resolved.into())
    }

    /// Rasterise a text string into a tight RGBA8 block for canvas composition.
    ///
    /// ponytail: reuses the egui font atlas instead of pulling in a text crate.
    /// Atlas glyph bitmaps are in texels (points × pixels_per_point) while galley
    /// positions are in points, so everything is mapped to texel space — copying
    /// point-sized rects out of the atlas is what clipped glyphs to their top-left
    /// quarter on 2× displays. The layout font size is pre-scaled toward the
    /// canvas target so the nearest-neighbour compose only does a small residual
    /// resize and text stays crisp. Returns None for empty/whitespace text.
    #[allow(clippy::too_many_arguments)]
    fn rasterize_text_block(
        &self,
        text: &str,
        font_size: f32,
        colour: SerializedColour,
        family: egui::FontFamily,
        canvas_w: u32,
        canvas_h: u32,
        fit: CanvasFit,
    ) -> Option<VideoFrame> {
        let ppp = self.egui_ctx.pixels_per_point();

        let (family, natural) = self.egui_ctx.fonts_mut(|fonts| {
            // A family registered this tick isn't in the atlas until the next
            // begin_pass — fall back for this render rather than panicking.
            let family = if fonts.definitions().families.contains_key(&family) {
                family
            } else {
                log::warn!("font family {family:?} not active yet; using built-in for this render");
                egui::FontFamily::Proportional
            };
            let galley = fonts.layout(
                text.into(),
                egui::FontId::new(font_size, family.clone()),
                egui::Color32::WHITE,
                f32::INFINITY,
            );
            (family, galley.rect.size())
        });
        let (nw, nh) = (natural.x * ppp, natural.y * ppp);
        if nw <= 0.0 || nh <= 0.0 {
            return None;
        }

        // Pre-scale so the block roughly matches its final canvas size. Stretch
        // pre-scales uniformly (Fit-like); the compose stretches the long axis.
        let scale = match fit {
            CanvasFit::Fill => (canvas_w as f32 / nw).max(canvas_h as f32 / nh),
            CanvasFit::Fit | CanvasFit::Stretch => {
                (canvas_w as f32 / nw).min(canvas_h as f32 / nh)
            }
        };
        let layout_size = (font_size * scale).clamp(1.0, 512.0);

        let (galley, font_image) = self.egui_ctx.fonts_mut(|fonts| {
            let galley = fonts.layout(
                text.into(),
                egui::FontId::new(layout_size, family),
                egui::Color32::WHITE,
                f32::INFINITY,
            );
            (galley, fonts.image())
        });

        let size = galley.rect.size();
        let bw = (size.x * ppp).ceil().max(1.0) as u32;
        let bh = (size.y * ppp).ceil().max(1.0) as u32;
        let mut rgba = vec![0u8; (bw as usize) * (bh as usize) * 4];
        let (r, g, b, a) = (
            (colour.r * 255.0) as u8,
            (colour.g * 255.0) as u8,
            (colour.b * 255.0) as u8,
            (colour.a * 255.0) as u8,
        );
        let atlas_w = font_image.size[0];
        let atlas_h = font_image.size[1];

        for placed in &galley.rows {
            for glyph in &placed.glyphs {
                let uv = &glyph.uv_rect;
                // Glyph bitmap extent in atlas texels (max is exclusive).
                let gw = uv.max[0].saturating_sub(uv.min[0]) as u32;
                let gh = uv.max[1].saturating_sub(uv.min[1]) as u32;
                let dx0 = ((placed.pos.x + glyph.pos.x + uv.offset.x) * ppp).round() as i32;
                let dy0 = ((placed.pos.y + glyph.pos.y + uv.offset.y) * ppp).round() as i32;

                for gy in 0..gh {
                    let dy = dy0 + gy as i32;
                    if dy < 0 || dy >= bh as i32 {
                        continue;
                    }
                    let sy = uv.min[1] as usize + gy as usize;
                    if sy >= atlas_h {
                        continue;
                    }
                    for gx in 0..gw {
                        let dx = dx0 + gx as i32;
                        if dx < 0 || dx >= bw as i32 {
                            continue;
                        }
                        let sx = uv.min[0] as usize + gx as usize;
                        if sx >= atlas_w {
                            continue;
                        }
                        let coverage = font_image.pixels[sy * atlas_w + sx].a() as f32 / 255.0;
                        if coverage > 0.0 {
                            let di = (dy as usize * bw as usize + dx as usize) * 4;
                            rgba[di] = r;
                            rgba[di + 1] = g;
                            rgba[di + 2] = b;
                            rgba[di + 3] = rgba[di + 3].max((a as f32 * coverage) as u8);
                        }
                    }
                }
            }
        }

        Some(VideoFrame::new(bw, bh, rgba, 0.0))
    }

    /// Resolve a file path: try absolute, then relative to project, then search project tree.
    fn resolve_path(&self, path: &str) -> Option<String> {
        let p = std::path::Path::new(path);
        if p.is_absolute() && p.exists() {
            return Some(path.to_string());
        }
        // Try relative to project directory
        let project_dir = self.cuepool.state().lock().ok()?.project_path
            .as_ref()?
            .parent()
            .map(|p| p.to_path_buf())?;
        let relative = project_dir.join(p);
        if relative.exists() {
            return Some(relative.to_string_lossy().to_string());
        }
        // Search project tree for matching filename
        let file_name = p.file_name()?;
        let found = std::fs::read_dir(&project_dir).ok()?.find_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_file() && path.file_name()? == file_name {
                Some(path)
            } else if path.is_dir() {
                Self::find_in_dir(&path, file_name)
            } else {
                None
            }
        });
        found.map(|p| p.to_string_lossy().to_string())
    }

    fn find_in_dir(dir: &std::path::Path, target: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_file() && path.file_name()? == target {
                return Some(path);
            } else if path.is_dir() {
                if let Some(found) = Self::find_in_dir(&path, target) {
                    return Some(found);
                }
            }
        }
        None
    }

    fn play_audio(
        &mut self,
        path: &str,
        qid: rust_decimal::Decimal,
        name: &str,
        loop_mode: cuepool_core::LoopMode,
        loop_count: i32,
        start_time: cuepool_core::Timespan,
        duration: cuepool_core::Timespan,
        volume: f32,
        fade_in: f32,
        fade_out: f32,
        fade_type: cuepool_core::FadeType,
        eq: Option<cuepool_core::EQSettings>,
        pan: f32,
        routing: cuepool_core::AudioRouting,
        preload_only: bool,
    ) {
        let (requested_driver, requested_device, configured_error) = {
            let state = self.cuepool.state().lock_unpoisoned();
            (
                state.show_file.show_settings.audio_output_driver,
                state.show_file.show_settings.audio_output_device.clone(),
                state.audio_error.clone(),
            )
        };
        let Some(audio_engine) = self.audio_engine.as_ref().filter(|engine| {
            engine.driver() == requested_driver
                && (requested_device.is_empty() || requested_device == engine.device_name())
        }) else {
            let reason = configured_error.unwrap_or_else(|| {
                format!(
                    "configured {requested_driver} output device '{}' is not active",
                    if requested_device.is_empty() {
                        "<default>"
                    } else {
                        &requested_device
                    }
                )
            });
            log::error!("Cannot play audio cue Q{qid}: audio playback is disabled: {reason}");
            return;
        };
        let resolved = self.resolve_path(path).unwrap_or_else(|| path.to_string());
        if resolved != path {
            log::info!("Resolved path '{}' -> '{}'", path, resolved);
        }
        match FileDecoder::open(&resolved) {
            Ok(decoder) => {
                let sample_rate = decoder.sample_rate();
                // input.position()/length() are reported in device-rate samples (post-resample),
                // so anything compared against them (loop bounds, fade trigger) must scale too.
                let out_scale = audio_engine.sample_rate() as f64 / sample_rate as f64;
                let start_frame = (start_time.as_secs_f64() * sample_rate as f64) as u64;
                let end_frame = if duration.as_secs_f64() > 0.0 {
                    start_frame + (duration.as_secs_f64() * sample_rate as f64) as u64
                } else {
                    0 // auto-detect from source length
                };

                // Create a shared loop counter so the main thread can detect loop boundaries
                // and synchronise video restarts + progress-bar resets.
                let is_looped = loop_mode == cuepool_core::LoopMode::Looped
                    || loop_mode == cuepool_core::LoopMode::LoopedInfinite;
                let loop_counter: Option<std::sync::Arc<std::sync::atomic::AtomicU32>> = if is_looped {
                    Some(std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)))
                } else {
                    None
                };

                let loop_proc = {
                    let proc = cuepool_audio::LoopProcessor::new(Box::new(decoder));
                    proc.set_loop(start_frame, end_frame, loop_mode, loop_count as u32);
                    if let Some(ref counter) = loop_counter {
                        proc.with_loop_counter(std::sync::Arc::clone(counter))
                    } else {
                        proc
                    }
                };

                let mut source: Box<dyn SampleProvider> = Box::new(loop_proc);

                // Per-cue EQ (4-band + HPF/LPF), applied before fade. `Some` means the user
                // enabled EQ in the inspector; the inner `enabled` flag is redundant with the
                // Option, so force it on (also covers show-files saved before this fix).
                if let Some(mut eq_settings) = eq {
                    eq_settings.enabled = true;
                    source = Box::new(cuepool_audio::EqProcessor::new(source, eq_settings));
                }

                // Wire fade processor for fade-in
                if fade_in > 0.0 {
                    let fade_proc = cuepool_audio::FadeProcessor::new(source, 0.0);
                    let fade_in_frames = (fade_in * sample_rate as f32) as u32;
                    fade_proc.start_fade(1.0, fade_in_frames, fade_type);
                    source = Box::new(fade_proc);
                }

                let input = audio_engine.play(source);
                input.set_volume(volume);
                input.set_pan(pan);
                input.set_routing(routing.out_pair, routing.send, routing.crosspoints);

                if preload_only {
                    input.set_active(false);
                }

                let state = if preload_only {
                    CueState::Ready
                } else if is_looped {
                    CueState::PlayingLooped
                } else {
                    CueState::Playing
                };
                self.active_cues.push(ActiveCue {
                    qid,
                    name: name.to_string(),
                    input,
                    state,
                    loop_counter,
                    video_loop_count: 0,
                    // Device-rate frames, to match input.position()/length() (post-resample).
                    loop_start_frame: (start_frame as f64 * out_scale) as u64,
                    loop_end_frame: (end_frame as f64 * out_scale) as u64,
                    fade_out,
                    fade_type,
                    fade_out_started: false,
                    pending_stop: None,
                });
            }
            Err(e) => {
                if let cuepool_audio::DecodeError::NoAudioTrack = e {
                    log::info!("No audio stream in {} — playing silent", path);
                } else {
                    log::error!("Failed to open audio for {}: {}", path, e);
                }
            }
        }
    }

    fn handle_stop_cue(&mut self, stop_qid: rust_decimal::Decimal, stop_mode: cuepool_core::StopMode, fade_out_time: f32, fade_type: cuepool_core::FadeType) {
        let mut handled = false;

        let idx = self.active_cues.iter().position(|ac| ac.qid == stop_qid);
        if let Some(idx) = idx {
            if stop_mode == cuepool_core::StopMode::LoopEnd {
                self.active_cues[idx].pending_stop = Some(PendingStop {
                    mode: stop_mode,
                    fade_out_time,
                    fade_type,
                });
                log::info!("LoopEnd stop scheduled for Q{}", stop_qid);
                self.reset_show_clock();
                return;
            }

            let input = &self.active_cues[idx].input;
            if fade_out_time > 0.0 {
                let sample_rate = self.audio_sample_rate();
                let fade_frames = (fade_out_time * sample_rate as f32) as u32;
                input.start_fade(0.0, fade_frames.max(1), fade_type);
                log::info!("Fade-out Q{} over {} frames", stop_qid, fade_frames);
            } else {
                input.set_active(false);
                input.set_volume(0.0);
                self.active_cues[idx].state = CueState::Done;
            }
            handled = true;
        } else if self.lighting.stop_show(stop_qid, fade_out_time, fade_type) {
            // Recorded DMX shows live in the lighting engine, not active_cues.
            log::info!("Stop DmxShow Q{} (fade {:.2}s)", stop_qid, fade_out_time);
            handled = true;
        } else if self.current_text_qid == Some(stop_qid) {
            // Text cues live on the overlay, not in the audio-backed active list.
            self.clear_text_overlay();
            handled = true;
        }

        // The picture is tracked outside active_cues (a video file with no
        // audio track never lands there), so check it separately — otherwise a
        // Stop cue mutes the soundtrack but leaves the image running.
        if self.current_video_qid == Some(stop_qid) {
            if stop_mode != cuepool_core::StopMode::LoopEnd {
                if fade_out_time > 0.0 {
                    // Ramp the canvas to black over the fade time (matching the
                    // audio fade); playback stops when the ramp reaches zero.
                    // ponytail: picture ramp is always linear; fade_type only
                    // shapes the audio.
                    self.video_control.lock_unpoisoned().fade =
                        Some((std::time::Instant::now(), fade_out_time));
                } else {
                    self.stop_video_playback();
                }
            }
            handled = true;
        }

        if handled {
            self.reset_show_clock();
        } else {
            log::warn!("StopCue target Q{} not found in active cues", stop_qid);
        }
    }

    /// Stop and reset the show timecode: the display returns to --:--:--.--,
    /// armed timecode triggers are cleared, and the next Go restarts the clock
    /// from zero. Fired by both the transport Stop and Stop cues.
    fn reset_show_clock(&mut self) {
        self.show_start_time = None;
        self.show_start_clock = None;
        self.show_pause_started = None;
        self.show_paused_offset = 0.0;
        self.timecode_fired.clear();
    }

    /// The panel's current take path (`/recorder/*` verbs operate on it).
    fn recorder_file(&self) -> String {
        self.cuepool
            .state()
            .lock()
            .map(|s| s.recorder_file.clone())
            .unwrap_or_default()
    }

    /// Play a `.dmxrec` take through the lighting output on the sentinel
    /// preview id (`-1`) — panel preview and `/recorder/play`.
    fn preview_recording(&mut self, file: &str) {
        let resolved = self.resolve_path(file).unwrap_or_else(|| file.to_string());
        match rustjay_lighting::read_rec(&resolved) {
            Ok(events) => {
                log::info!("Recorder preview: '{resolved}' ({} events)", events.len());
                self.lighting.go_show(
                    rust_decimal::Decimal::NEGATIVE_ONE,
                    events,
                    100,
                    0.0,
                    0.0,
                    cuepool_core::FadeType::Linear,
                    cuepool_core::LoopMode::OneShot,
                    1,
                );
            }
            Err(e) => log::error!("Recorder preview failed to load '{resolved}': {e}"),
        }
    }

    /// Check if an incoming remote OSC command targets this node.
    fn is_remote_target_match(&self, target: &str) -> bool {
        let local_name = {
            let Ok(state) = self.cuepool.state().lock() else { return false; };
            state.show_file.show_settings.node_name.clone()
        };
        target == local_name || target == "*"
    }

    /// Preload the selected cue: decode and add to mixer as inactive (Ready state).
    fn handle_preload(&mut self, _event_loop: &ActiveEventLoop) {
        let cue = {
            let state = self.cuepool.state().lock_unpoisoned();
            state.selected_cue().cloned()
        };

        let Some(cue) = cue else {
            log::info!("Preload pressed but no cue selected");
            return;
        };

        let qid = cue.base().qid;
        let name = cue.base().name.clone();

        // Skip if already preloaded or playing
        if self.active_cues.iter().any(|ac| ac.qid == qid) {
            log::info!("Cue Q{} is already loaded", qid);
            return;
        }

        match cue {
            cuepool_core::Cue::Sound { ref path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, ref routing, .. } => {
                log::info!("Preload SoundCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, start_time, duration, volume, fade_in, fade_out, fade_type, eq, pan, routing.clone(), true);
            }
            cuepool_core::Cue::Video { ref path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, ref routing, .. } => {
                log::info!("Preload VideoCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, start_time, duration, volume, fade_in, fade_out, fade_type, eq, pan, routing.clone(), true);
            }
            other => {
                log::info!("Preload not supported for cue type: {:?}", std::mem::discriminant(&other));
            }
        }
    }

    /// Apply the project's exact driver/device request. Any failure drops the
    /// previous stream before reporting the error, so ASIO can never fall back
    /// to WASAPI or continue through a stale device.
    fn apply_audio_settings(&mut self) {
        let (driver, configured_device, audio_ok) = {
            let state = self.cuepool.state().lock_unpoisoned();
            (
                state.show_file.show_settings.audio_output_driver,
                state.show_file.show_settings.audio_output_device.clone(),
                state.audio_error.is_none(),
            )
        };

        if audio_ok
            && self.audio_engine.as_ref().is_some_and(|engine| {
                engine.driver() == driver
                    && (configured_device.is_empty() || configured_device == engine.device_name())
            })
        {
            return;
        }

        self.stop_all();
        self.audio_engine = None;

        let setup = AudioEngine::configure(driver, &configured_device);
        if let Some(error) = setup.device_list_error {
            log::error!("Could not list {driver} output devices: {error}");
        }
        let devices = setup.device_names;

        match setup.engine {
            Ok(engine) => {
                let device_name = engine.device_name().to_string();
                self.audio_engine = Some(engine);
                let mut state = self.cuepool.state().lock_unpoisoned();
                state.audio_devices = devices;
                state.audio_device_name = device_name.clone();
                state.audio_error = None;
                // ASIO has no system default. Persist CPAL's selected first
                // driver so the same hardware is requested next load.
                if driver == AudioOutputDriver::ASIO
                    && state.show_file.show_settings.audio_output_device.is_empty()
                {
                    state.show_file.show_settings.audio_output_device = device_name;
                    state.dirty = true;
                }
                log::info!("Applied {driver} audio output configuration");
            }
            Err(error) => {
                let message = configured_audio_error(driver, &configured_device, &error);
                log::error!("{message}; audio playback is disabled");
                let mut state = self.cuepool.state().lock_unpoisoned();
                state.audio_devices = devices;
                state.audio_device_name = if configured_device.is_empty() {
                    "<default>".to_string()
                } else {
                    configured_device
                };
                state.audio_error = Some(message);
                state.show_settings_window = true;
            }
        }
    }

    fn audio_clock(&self) -> Duration {
        self.audio_engine
            .as_ref()
            .map(AudioEngine::playback_time)
            .or_else(|| self.show_start_time.map(|start| start.elapsed()))
            .unwrap_or_default()
    }

    fn audio_sample_rate(&self) -> u32 {
        self.audio_engine
            .as_ref()
            .map(AudioEngine::sample_rate)
            .unwrap_or(48_000)
    }

    /// Start a cue's tail fade-out when playback reaches `fade_out` seconds before
    /// its end. Mirrors C# SoundCue, where FadeOut begins (Duration - FadeOut)
    /// before the natural end. Looping cues are skipped (state != Playing).
    fn check_fade_outs(&mut self) {
        let sr = self.audio_sample_rate();
        for ac in &mut self.active_cues {
            if ac.fade_out <= 0.0 || ac.fade_out_started || ac.state != CueState::Playing {
                continue;
            }
            // End position in interleaved (stereo) samples.
            let end_samples = if ac.loop_end_frame > 0 {
                ac.loop_end_frame as usize * 2
            } else if let Some(len) = ac.input.length() {
                len
            } else {
                continue; // unknown length — can't schedule a tail fade
            };
            let fade_frames = (ac.fade_out * sr as f32) as u32;
            let trigger = end_samples.saturating_sub(fade_frames as usize * 2);
            if ac.input.position() >= trigger {
                ac.input.start_fade(0.0, fade_frames.max(1), ac.fade_type);
                ac.fade_out_started = true;
                log::info!("Tail fade-out Q{} over {} frames", ac.qid, fade_frames);
            }
        }
    }

    /// Check for cues that have finished playing naturally and trigger AfterLast chains.
    fn check_finished_cues(&mut self, event_loop: &ActiveEventLoop) {
        // Mark finished cues as Done and collect their QIDs.
        // Cues explicitly set to Done (e.g. immediate StopCue) are also removed here.
        let finished_qids: Vec<rust_decimal::Decimal> = {
            let mut qids = Vec::new();
            for ac in &mut self.active_cues {
                if ac.input.is_finished() || ac.state == CueState::Done {
                    ac.state = CueState::Done;
                    qids.push(ac.qid);
                }
            }
            qids
        };

        for qid in finished_qids {
            self.active_cues.retain(|ac| ac.qid != qid);
            log::info!("Cue Q{} finished — checking AfterLast chain", qid);
            self.play_after_last_chain(qid, event_loop);
        }

        // Recorded DMX shows finish inside the lighting engine's tick.
        // The panel preview plays on the sentinel qid -1 — never a cue.
        for qid in self.lighting.take_finished_shows() {
            if qid == rust_decimal::Decimal::NEGATIVE_ONE {
                continue;
            }
            log::info!("DmxShow Q{} finished — checking AfterLast chain", qid);
            self.play_after_last_chain(qid, event_loop);
        }
    }

    /// Fire the next AfterLast cue following `finished_qid` in the cue list.
    /// Only the first enabled follower fires here — every cue continues the
    /// chain itself when it completes (instant cues from play_cue, audio/video
    /// from check_finished_cues, timecode from its marker/deadline), so firing
    /// more would double-trigger. Disabled followers are skipped over.
    fn play_after_last_chain(&mut self, finished_qid: rust_decimal::Decimal, event_loop: &ActiveEventLoop) {
        let next = {
            let state = self.cuepool.state().lock_unpoisoned();
            next_after_last(&state.show_file.cues, finished_qid).cloned()
        };
        if let Some(cue) = next {
            self.play_cue(&cue, event_loop);
        }
    }

    /// Execute scheduled LoopEnd stops when their target reaches the loop boundary.
    fn check_pending_stops(&mut self) {
        let sr = self.audio_sample_rate();
        for ac in &mut self.active_cues {
            let Some(ref pending) = ac.pending_stop else { continue };
            if pending.mode != cuepool_core::StopMode::LoopEnd {
                continue;
            }
            // End position in interleaved (stereo) samples.
            let end_samples = if ac.loop_end_frame > 0 {
                ac.loop_end_frame as usize * 2
            } else if let Some(len) = ac.input.length() {
                len
            } else {
                continue; // unknown length — can't schedule a loop-end stop
            };
            let fade_frames = (pending.fade_out_time * sr as f32) as u32;
            let trigger = end_samples.saturating_sub(fade_frames as usize * 2);
            if ac.input.position() >= trigger {
                if pending.fade_out_time > 0.0 {
                    ac.input.start_fade(0.0, fade_frames.max(1), pending.fade_type);
                    log::info!("LoopEnd fade-out Q{} over {} frames", ac.qid, fade_frames);
                } else {
                    ac.input.set_active(false);
                    ac.input.set_volume(0.0);
                    ac.state = CueState::Done;
                }
                ac.pending_stop = None;
            }
        }
    }

    fn handle_volume_cue(&mut self, sound_qid: rust_decimal::Decimal, target_volume: f32, fade_time: f32, fade_type: cuepool_core::FadeType) {
        let target = self.active_cues.iter().find(|ac| ac.qid == sound_qid);
        if let Some(ac) = target {
            let input = &ac.input;
            if fade_time > 0.0 {
                let sample_rate = self.audio_sample_rate();
                let fade_frames = (fade_time * sample_rate as f32) as u32;
                input.start_fade(target_volume.max(0.0), fade_frames.max(1), fade_type);
                log::info!("Volume fade Q{} to {} over {} frames", sound_qid, target_volume, fade_frames);
            } else {
                input.set_volume(target_volume.max(0.0));
            }
        } else {
            log::warn!("VolumeCue target Q{} not found in active cues", sound_qid);
        }
    }

    fn play_video(&mut self, path: &str, qid: rust_decimal::Decimal, event_loop: &ActiveEventLoop) {
        // Only open output windows on the first video; looping should not respawn them.
        if self.output_windows.is_empty() {
            self.create_output_windows(event_loop);
        }
        // A newly-started video should always play, even if the system was paused.
        self.video_pause_flag.store(false, Ordering::Relaxed);

        let projection = {
            let state = self.cuepool.state().lock_unpoisoned();
            state.show_file.projection.clone()
        };
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.stream_epoch += 1;
            // Start the playback clock now; PTS are matched against it (and frames
            // late vs the clock are skipped, so video catches up to audio even if
            // decode open / first-frame took a while).
            ctl.clock = Some(std::time::Instant::now());
            ctl.pause_started = None;
            ctl.peek_pts = None;
            ctl.last_pts = None;
            ctl.canvas_has_frame = false;
            // A new video always starts at full brightness, cancelling any
            // Stop-cue fade still in flight.
            ctl.fade = None;
            // A step-back aimed at the previous stream must not replay here
            // (the epoch gate no longer consumes it against a dead stream).
            ctl.step_back = None;
            ctl.fit = projection.fit;
        }
        self.set_current_video_qid(Some(qid));
        // (Re)create the consume thread's canvas at the projection size; `force`
        // clears the previous clip's last frame even when the dims match.
        let _ = self.canvas_cmd_tx.send(CanvasCommand::Resize {
            w: projection.canvas_width,
            h: projection.canvas_height,
            force: true,
        });

        self.spawn_video_decode(path, None);
    }

    /// (Re)spawn the video decode thread. `start_before`: seek so the first
    /// frame delivered is the last one with a PTS strictly below this
    /// timestamp (frame-step-back), followed by the frames after it.
    fn spawn_video_decode(&mut self, path: &str, start_before: Option<f64>) {
        // Kill any previous decode thread by signalling its own stop flag, then
        // install a fresh one for the new thread. Per-thread flags (vs. a shared flag
        // reset to false) guarantee the old thread exits — resetting a shared flag
        // could revive a still-sleeping old thread and leak it across every loop.
        self.video_stop_flag.store(true, Ordering::Relaxed);
        self.video_stop_flag = Arc::new(AtomicBool::new(false));
        // Resolve the path relative to the project dir first (same as
        // play_audio) so a packed project's "Media/<file>" relative path
        // opens instead of failing — audio resolved but video didn't.
        let path = self.resolve_path(path).unwrap_or_else(|| path.to_string());
        // Bounded channel = backpressure: the decode thread can't outrun the consumer
        // (the consume thread matching PTS against the wall-clock video clock), so
        // decode runs at real-time rate — no free-running decoder to drift against
        // the clock. The small buffer absorbs decode jitter. Pacing is the wall
        // clock, not the audio clock, so it can't freeze if the audio device sleeps.
        let (frame_tx, frame_rx) =
            std::sync::mpsc::sync_channel::<VideoMessage>(VIDEO_QUEUE_CAP);
        // Installing a new receiver also tells the consume thread to drop its
        // peeked frame and invalidates EOF already queued by the old decoder.
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.stream_epoch += 1;
            ctl.frame_rx = Some(frame_rx);
        }
        let stop_flag = Arc::clone(&self.video_stop_flag);
        let pause_flag = Arc::clone(&self.video_pause_flag);
        let diag_state = Arc::clone(self.cuepool.state());

        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                video_decode_thread(&path, start_before, stop_flag, pause_flag, frame_tx, diag_state);
            })
            .expect("spawn video decode thread");
    }

    /// Restart the current video decode thread (used when audio loops).
    fn restart_video(&mut self, path: &str, qid: rust_decimal::Decimal, event_loop: &ActiveEventLoop) {
        // play_video already signals the old thread's stop flag and installs a fresh
        // one, so there's no manual stop/sleep dance (and no shared-flag revival race).
        self.play_video(path, qid, event_loop);
        log::info!("Restarted video for Q{qid} on loop");
    }

    /// A project was created or loaded — stop everything from the previous project
    /// and close its output windows (which would otherwise keep playing with the
    /// old projection geometry). The windows reopen with the new project's geometry
    /// when the next video/image/text cue plays, or via the projection-output menu.
    fn reset_for_project_change(&mut self) {
        self.stop_all();
        self.lighting.shutdown();
        self.pixmap_texture = None;
        self.pixmap_yuv = None;
        self.pixel_sampler = None;
        self.output_windows.clear();
        self.output_windows_built_from = None;
        if let Some(ids) = self.window_ids.as_mut() {
            ids.video.clear();
        }
        // Drop the canvas so it is recreated at the new project's canvas size.
        self.set_current_text_qid(None);
        let _ = self.canvas_cmd_tx.send(CanvasCommand::Drop);
    }

    /// Stop video/image playback: signal the decode thread, drop the frame
    /// channel (its send fails with Disconnected and the thread exits), and
    /// clear presentation state so the next published frame goes black.
    fn stop_video_playback(&mut self) {
        self.video_stop_flag.store(true, Ordering::Relaxed);
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.stream_epoch += 1;
            ctl.frame_rx = None;
            ctl.clock = None;
            ctl.peek_pts = None;
            ctl.last_pts = None;
            ctl.canvas_has_frame = false;
            ctl.fade = None;
            ctl.hold_position = None;
            ctl.step_back = None;
        }
        self.set_current_video_qid(None);
        self.cuepool.state().lock_unpoisoned().diagnostics.video = None;
    }

    fn stop_all(&mut self) {
        self.stop_video_playback();
        self.video_pause_flag.store(false, Ordering::Relaxed);
        // StopAll releases the MTC-follow cue too.
        self.mtc_follow = None;
        if let Some(engine) = &self.audio_engine {
            engine.stop_all();
        }
        self.active_cues.clear();
        self.delayed_cues.clear();
        self.active_timecodes.clear();
        self.reset_show_clock();
        self.paused = false;
        self.video_control.lock_unpoisoned().paused = false;
        // Halt any in-flight lighting fade; levels hold (blackout is a cue's job).
        self.lighting.stop_fade();
        // Recorded DMX shows stop dead — their channels release to the looks.
        self.lighting.stop_all_shows();
        // Stop the pixmap stream and blank its texture so pixel-mapped LEDs go dark.
        self.pixmap_stop_flag.store(true, Ordering::Relaxed);
        self.pixmap_frame_rx = None;
        if let Some(tex) = self.pixmap_texture.as_ref() {
            let (w, h) = (tex.width, tex.height);
            let blank = vec![0u8; (w * h * 4) as usize];
            let _configure_guard = self
                .configure_gate
                .read()
                .unwrap_or_else(|e| e.into_inner());
            tex.upload_rgba(&self.queue, &blank);
        }
        self.clear_text_overlay();
        // The render threads present every vsync, so the cleared/black state
        // shows on the next publish without any explicit repaint request.
    }

    /// Persist settings and hard-exit the process. A graceful `event_loop.exit()`
    /// returns through `run_app` and runs Rust drops (wgpu device/surfaces, threads)
    /// which can wedge the main thread on macOS (beachball); the OS reclaims
    /// everything on `process::exit`, just like the Ctrl-C handler and Dock-quit.
    fn hard_exit(&self) -> ! {
        let recent_files = self
            .cuepool
            .state()
            .lock()
            .map(|s| s.recent_files.clone())
            .unwrap_or_default();
        save_settings(&AppSettings { recent_files });
        #[cfg(windows)]
        win_timer::release();
        std::process::exit(0);
    }

    fn pause_all(&mut self) {
        for ac in &mut self.active_cues {
            ac.input.set_active(false);
            if ac.state == CueState::Playing || ac.state == CueState::PlayingLooped {
                ac.state = CueState::Paused;
            }
        }
        self.video_pause_flag.store(true, Ordering::Relaxed);
        self.paused = true;
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.paused = true;
            ctl.pause_started = Some(std::time::Instant::now());
        }
        // Freeze the show clock — timecode must not advance (or fire) mid-pause.
        if self.show_pause_started.is_none() {
            self.show_pause_started = Some(self.audio_clock());
        }
        log::info!("Paused {} cue(s)", self.active_cues.len());
    }

    fn resume_all(&mut self) {
        for ac in &mut self.active_cues {
            ac.input.set_active(true);
            if ac.state == CueState::Paused {
                ac.state = CueState::Playing;
            }
        }
        self.video_pause_flag.store(false, Ordering::Relaxed);
        self.paused = false;
        // Advance the playback clock past the paused interval so video resumes where
        // it left off instead of jumping forward.
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.paused = false;
            if let Some(t) = ctl.pause_started.take() {
                let resumed_at = std::time::Instant::now();
                if let Some(c) = ctl.clock.as_mut() {
                    *c += resumed_at.saturating_duration_since(t);
                }
                if let Some((start, _)) = ctl.fade.as_mut() {
                    *start = shift_fade_start_after_pause(*start, t, resumed_at);
                }
            }
        }
        // Unfreeze the show clock: the paused interval joins the offset.
        if let Some(p) = self.show_pause_started.take() {
            self.show_paused_offset +=
                (self.audio_clock().saturating_sub(p)).as_secs_f64();
        }
        log::info!("Resumed {} cue(s)", self.active_cues.len());
    }

    /// Show-clock elapsed seconds — frozen while paused, adjusted for
    /// accumulated pause time and frame-step advances. None before the first Go.
    fn show_elapsed(&self) -> Option<f64> {
        let start = self.show_start_clock?;
        let now = self
            .show_pause_started
            .unwrap_or_else(|| self.audio_clock());
        Some((now.as_secs_f64() - start.as_secs_f64() - self.show_paused_offset).max(0.0))
    }

    /// The frozen playback position while paused. `clock.elapsed()` keeps
    /// growing through a pause (the interval is only re-added on resume), so
    /// position math while paused must anchor on `pause_started`.
    /// While an MTC-follow cue is holding (MTC stopped), the hold position
    /// wins over the wall clock — the MTC master owns the position.
    fn video_paused_position(&self) -> Option<Duration> {
        if let Some(h) = self.mtc_follow.as_ref().and_then(|f| f.hold_position) {
            return Some(Duration::from_secs_f64(h.max(0.0)));
        }
        let ctl = self.video_control.lock_unpoisoned();
        let clock = ctl.clock?;
        Some(match ctl.pause_started {
            Some(paused_at) => paused_at.duration_since(clock),
            None => clock.elapsed(),
        })
    }

    /// Shift the video position by `delta_secs` (pure presentation-clock slew;
    /// the decoder is untouched). Positive = further into the clip.
    fn nudge_video_clock(&mut self, delta_secs: f64) {
        let mut ctl = self.video_control.lock_unpoisoned();
        let Some(c) = ctl.clock.as_mut() else { return };
        if delta_secs >= 0.0 {
            if let Some(shifted) = c.checked_sub(Duration::from_secs_f64(delta_secs)) {
                *c = shifted;
            }
        } else {
            *c += Duration::from_secs_f64(-delta_secs);
        }
    }

    /// Re-anchor the playback clock so the current position is exactly `target`.
    fn mtc_reanchor(&mut self, target: f64) {
        if let Some(c) = Instant::now().checked_sub(Duration::from_secs_f64(target.max(0.0))) {
            self.video_control.lock_unpoisoned().clock = Some(c);
        }
    }

    /// Big jump (locate, loop-back, drift > 250 ms): re-seek the forward-only
    /// decoder and re-anchor the clock. Needed even for forward jumps — a large
    /// one would otherwise starve the renderer while decode catches up.
    fn mtc_hard_sync(&mut self, target: f64) {
        let Some(follow) = self.mtc_follow.as_ref() else { return };
        let path = follow.path.clone();
        log::info!("[MTC] Hard sync Q{} to {:.2}s", follow.qid, target);
        self.spawn_video_decode(&path, Some(target));
        self.mtc_reanchor(target);
    }

    /// Drive the MTC-follow cue from the latest MTC state. No-op without one.
    fn drive_mtc_follow(&mut self, mtc: &MtcState) {
        self.mtc_drift = None;
        let Some(follow) = self.mtc_follow.as_mut() else { return };
        let offset_secs = follow.offset_secs;
        let dt = follow.last_tick.elapsed().as_secs_f64();
        follow.last_tick = Instant::now();
        let holding = follow.hold_position;

        // MTC publishes a complete timecode only every 2 frames (~80 ms at
        // 25 fps). Between updates the true position keeps advancing at
        // realtime, so extrapolate — otherwise the drift measurement
        // sawtooths by ±2 frames and the nudge controller biases the video
        // late by up to the deadband. No extrapolation while stopped.
        let mtc_secs = mtc.position.as_seconds_f64();
        if mtc_secs != follow.last_mtc_secs {
            follow.last_mtc_secs = mtc_secs;
            follow.last_mtc_at = Instant::now();
        }
        let extrapolated = if mtc.playing {
            mtc_secs + follow.last_mtc_at.elapsed().as_secs_f64()
        } else {
            mtc_secs
        };

        // Locates before the start offset clamp to frame 0 (and hold there).
        let target = (extrapolated - offset_secs).max(0.0);

        // Warn (once per rate change) if the MTC fps isn't the expected 25.
        if mtc.running
            && mtc.position.frame_rate != MtcFrameRate::Fps25
            && self.mtc_warned_fps != Some(mtc.position.frame_rate)
        {
            self.mtc_warned_fps = Some(mtc.position.frame_rate);
            log::warn!(
                "[MTC] Source is {}, expected 25fps — video sync may drift",
                mtc.position.frame_rate.name()
            );
        }

        let current = self
            .video_paused_position()
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);

        if mtc.playing {
            if let Some(h) = holding {
                // Stopped→playing transition: jump if far off, else re-anchor
                // so the position continues smoothly from the target.
                if (target - h).abs() > mtc_follow::HARD_SYNC_SECS {
                    self.mtc_hard_sync(target);
                } else {
                    self.mtc_reanchor(target);
                }
            } else {
                let drift = target - current;
                self.mtc_drift = Some(drift);
                match mtc_follow::drift_action(drift, dt) {
                    mtc_follow::MtcAdjust::Nudge(d) => self.nudge_video_clock(d),
                    mtc_follow::MtcAdjust::HardSync => self.mtc_hard_sync(target),
                    mtc_follow::MtcAdjust::None | mtc_follow::MtcAdjust::Hold => {}
                }
            }
            if let Some(f) = self.mtc_follow.as_mut() {
                f.hold_position = None;
            }
            self.video_control.lock_unpoisoned().hold_position = None;
        } else {
            // Running-but-not-playing (full-frame locate) or fully stopped:
            // snap to the target if off, then freeze there.
            if (target - current).abs() > mtc_follow::DEADBAND_SECS {
                self.mtc_hard_sync(target);
            }
            if let Some(f) = self.mtc_follow.as_mut() {
                f.hold_position = Some(target);
            }
            self.video_control.lock_unpoisoned().hold_position = Some(target);
        }
    }

    /// Step one video frame forward while paused; show clock follows in
    /// lockstep. Without a video playing, advances by one display frame.
    fn frame_step(&mut self) {
        if !self.paused {
            return;
        }
        let mut ctl = self.video_control.lock_unpoisoned();
        if ctl.clock.is_some() {
            // The next frame's PTS comes from the consume thread's peek mirror
            // (it owns the channel now). Right after a pause the peek may not
            // be filled yet — the step just works on the next keypress.
            // Position math is inlined from the guard: `video_paused_position`
            // would re-lock this same mutex (non-reentrant).
            let pos = ctl
                .hold_position
                .map(|h| Duration::from_secs_f64(h.max(0.0)))
                .or_else(|| {
                    ctl.clock.map(|c| match ctl.pause_started {
                        Some(p) => p.duration_since(c),
                        None => c.elapsed(),
                    })
                })
                .unwrap_or_default()
                .as_secs_f64();
            if let Some(f_pts) = ctl.peek_pts {
                let delta = f_pts - pos;
                if delta > 0.0 {
                    // Moving the clock's epoch back makes the next frame due.
                    if let Some(c) = ctl.clock.and_then(|c| c.checked_sub(Duration::from_secs_f64(delta)))
                    {
                        ctl.clock = Some(c);
                        self.show_paused_offset -= delta;
                    }
                }
                ctl.step_pending = true;
                return;
            }
            log::debug!("Frame step: no decoded frame available yet");
            return;
        }
        drop(ctl);
        // No video: advance the frozen clock by one display frame.
        let fps = {
            let Ok(state) = self.cuepool.state().lock() else { return };
            state.show_file.show_settings.timecode_fps.max(1.0)
        };
        self.show_paused_offset -= 1.0 / fps as f64;
    }

    /// Step one video frame back while paused. The decoder is forward-only,
    /// so this restarts the decode thread with a seek to just before the
    /// current frame; the consume thread waits for the delivered frame and
    /// snaps the (frozen) clock to its exact PTS. Without a video playing,
    /// rewinds one display frame.
    fn frame_step_back(&mut self) {
        if !self.paused {
            return;
        }
        let has_clock = self.video_control.lock_unpoisoned().clock.is_some();
        if has_clock {
            let pos = self.video_paused_position().unwrap_or_default().as_secs_f64();
            let cur = self.video_control.lock_unpoisoned().last_pts.unwrap_or(pos);
            if cur <= 0.0 {
                return;
            }
            let path = {
                let Ok(state) = self.cuepool.state().lock() else { return };
                let Some(qid) = self.current_video_qid else { return };
                let Some(path) = state.show_file.cues.iter().find_map(|c| match c {
                    cuepool_core::Cue::Video { path, .. } if c.base().qid == qid => {
                        Some(path.clone())
                    }
                    _ => None,
                }) else {
                    return;
                };
                path
            };
            self.spawn_video_decode(&path, Some(cur));
            // The consume thread does the (blocking) wait for the sought frame,
            // snaps the clock, and reports the delta back for the show clock.
            self.video_control.lock_unpoisoned().step_back = Some(pos);
            return;
        }
        // No video: rewind the frozen clock by one display frame.
        let fps = {
            let Ok(state) = self.cuepool.state().lock() else { return };
            state.show_file.show_settings.timecode_fps.max(1.0)
        };
        self.show_paused_offset += 1.0 / fps as f64;
    }

    fn handle_dropped_file(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());

        // Open project files directly
        if ext.as_deref() == Some("qproj") {
            if let Ok(mut state) = self.cuepool.state().lock() {
                state.command_queue.push(cuepool_gui::AppCommand::OpenProject {
                    path: path.to_path_buf(),
                });
            }
            return;
        }

        let is_video = matches!(ext.as_deref(), Some("mp4") | Some("mov") | Some("mkv") | Some("avi"));
        let is_audio = matches!(
            ext.as_deref(),
            Some("wav") | Some("mp3") | Some("flac") | Some("ogg") | Some("aiff") | Some("wma")
        );
        if !is_video && !is_audio {
            log::warn!("Dropped file has unsupported extension: {:?}", path);
            return;
        }

        let path_str = path.to_string_lossy().to_string();
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Dropped")
            .to_string();

        if let Ok(mut state) = self.cuepool.state().lock() {
            let snapshot = cuepool_gui::app::Snapshot::from_state(&state);
            state.undo_redo.push(snapshot);

            let next_qid = state.show_file.choose_qid(state.selected_cue_id);

            let base = cuepool_core::CueBase {
                qid: next_qid,
                name,
                ..Default::default()
            };

            let cue = if is_video {
                cuepool_core::Cue::Video {
                    base,
                    path: path_str,
                    start_time: cuepool_core::Timespan::ZERO,
                    duration: cuepool_core::Timespan::ZERO,
                    volume: 1.0,
                    pan: 0.0,
                    fade_in: 0.0,
                    fade_out: 0.0,
                    fade_type: cuepool_core::FadeType::Linear,
                    eq: None,
                    routing: cuepool_core::AudioRouting::default(),
                    follow_mtc: false,
                    mtc_start: cuepool_core::Timespan::from_secs_f64(3600.0),
                }
            } else {
                cuepool_core::Cue::Sound {
                    base,
                    path: path_str,
                    start_time: cuepool_core::Timespan::ZERO,
                    duration: cuepool_core::Timespan::ZERO,
                    volume: 1.0,
                    pan: 0.0,
                    fade_in: 0.0,
                    fade_out: 0.0,
                    fade_type: cuepool_core::FadeType::Linear,
                    eq: None,
                    routing: cuepool_core::AudioRouting::default(),
                }
            };

            state.show_file.cues.push(cue);
            state.dirty = true;
            log::info!("Added dropped file as cue {}: {:?}", next_qid, path);
        }
    }

    /// Drain any AppCommands queued by the UI and execute them.
    fn process_commands(&mut self, event_loop: &ActiveEventLoop) {
        let commands = {
            let Ok(mut state) = self.cuepool.state().lock() else { return };
            let cmds = state.command_queue.clone();
            state.command_queue.clear();
            cmds
        };

        for cmd in commands {
            match cmd {
                AppCommand::Go => self.handle_go(event_loop),
                AppCommand::Stop => self.stop_all(),
                AppCommand::Pause => {
                    if self.paused {
                        self.resume_all();
                    } else {
                        self.pause_all();
                    }
                }
                AppCommand::SetLimiterThreshold(threshold) => {
                    if let Some(engine) = &self.audio_engine {
                        engine.set_limiter_threshold(threshold);
                        log::info!("Set master limiter threshold to {:.2} dB", 20.0 * threshold.log10());
                    }
                }
                AppCommand::SetAudioDriver(driver) => {
                    {
                        let mut state = self.cuepool.state().lock_unpoisoned();
                        state.show_file.show_settings.audio_output_driver = driver;
                        state.show_file.show_settings.audio_output_device.clear();
                        state.dirty = true;
                    }
                    self.apply_audio_settings();
                }
                AppCommand::SetAudioDevice(name) => {
                    {
                        let mut state = self.cuepool.state().lock_unpoisoned();
                        state.show_file.show_settings.audio_output_device = name;
                        state.dirty = true;
                    }
                    self.apply_audio_settings();
                }
                AppCommand::ApplyAudioSettings => self.apply_audio_settings(),
                AppCommand::Preload => {
                    self.handle_preload(event_loop);
                }
                AppCommand::ToggleVideoWindow => {
                    if !self.output_windows.is_empty() {
                        self.output_windows.clear();
                        self.output_windows_built_from = None;
                        if let Some(ids) = self.window_ids.as_mut() {
                            ids.video.clear();
                        }
                    } else {
                        // Show/create (even if no video is playing, show black windows)
                        self.create_output_windows(event_loop);
                    }
                }
                AppCommand::ToggleVideoFullscreen => {
                    self.toggle_output_fullscreen();
                }
                AppCommand::OpenProjectionOutputs => {
                    self.create_output_windows(event_loop);
                }
                AppCommand::SaveProject | AppCommand::SaveProjectAs { .. } => {}
                AppCommand::LearnMidiTrigger { qid } => {
                    if let Ok(mut state) = self.cuepool.state().lock() {
                        state.pending_midi_learn = Some(qid);
                        log::info!("Listening for MIDI trigger on Q{} — play a note/CC", qid);
                    }
                }
                AppCommand::CaptureTimecodeTrigger { qid } => {
                    if let Ok(mut state) = self.cuepool.state().lock() {
                        state.pending_timecode_capture = Some(qid);
                        log::info!("Capturing timecode trigger on Q{} at next tick", qid);
                    }
                }
                AppCommand::LightingLivePush { snapshot } => {
                    // Inspector live mode: snap the edited looks onto the live
                    // state (LTP — untouched fixtures hold their levels).
                    self.lighting.go(&snapshot, 0.0, cuepool_core::FadeType::Linear);
                }
                AppCommand::RecorderRecord { file } => {
                    let file = self.resolve_path(&file).unwrap_or(file);
                    self.recorder.record_toggle(&file);
                }
                AppCommand::RecorderDiscard => self.recorder.discard(),
                AppCommand::RecorderRevert { file } => {
                    let file = self.resolve_path(&file).unwrap_or(file);
                    self.recorder.revert(&file);
                }
                AppCommand::RecorderSetMonitor(on) => self.recorder.monitor = on,
                AppCommand::RecorderPreview { file } => self.preview_recording(&file),
                AppCommand::RecorderStopPreview => {
                    self.lighting.stop_show(
                        rust_decimal::Decimal::NEGATIVE_ONE,
                        0.0,
                        cuepool_core::FadeType::Linear,
                    );
                }
                AppCommand::RecorderClearLive => self.recorder.clear_live(),
                AppCommand::RecorderScrub { frame } => self.recorder.set_scrub(frame),
                AppCommand::FrameStep => self.frame_step(),
                AppCommand::FrameStepBack => self.frame_step_back(),
                _ => {}
            }
        }
    }

    /// Drain MIDI input events and fire any cues whose MIDI trigger matches.
    fn process_midi_events(&mut self, event_loop: &ActiveEventLoop) {
        // Drain all pending events first, so firing cues (which needs `&mut self`)
        // doesn't conflict with the borrow of `self.midi_manager`.
        let events: Vec<MidiEvent> = {
            let Some(manager) = &self.midi_manager else { return };
            std::iter::from_fn(|| manager.try_recv()).collect()
        };
        for ev in events {
            log::debug!("MIDI event: {ev:?}");

            // If MIDI learn is pending, store the first event as the cue's trigger.
            let learn_qid = {
                let Ok(state) = self.cuepool.state().lock() else { continue };
                state.pending_midi_learn
            };
            if let Some(qid) = learn_qid {
                let learned: Option<MidiTrigger> = match self.cuepool.state().lock() {
                    Ok(mut state) => {
                        let trigger = match ev {
                        MidiEvent::NoteOn { channel, note, velocity } => MidiTrigger {
                            channel,
                            kind: MidiTriggerKind::NoteOn,
                            note_or_cc: note,
                            velocity_min: velocity,
                        },
                        MidiEvent::NoteOff { channel, note, velocity } => MidiTrigger {
                            channel,
                            kind: MidiTriggerKind::NoteOff,
                            note_or_cc: note,
                            velocity_min: velocity,
                        },
                        MidiEvent::CC { channel, cc, value } => MidiTrigger {
                            channel,
                            kind: MidiTriggerKind::CC,
                            note_or_cc: cc,
                            velocity_min: value,
                        },
                    };
                        if let Some(cue) = state.show_file.cues.iter_mut().find(|c| c.base().qid == qid) {
                            cue.base_mut().triggers.midi = Some(trigger);
                            Some(trigger)
                        } else {
                            None
                        }
                    }
                    Err(_) => None,
                };
                if let Ok(mut state) = self.cuepool.state().lock() {
                    if learned.is_some() {
                        state.dirty = true;
                    }
                    state.pending_midi_learn = None;
                }
                if let Some(trigger) = learned {
                    log::info!("Learned MIDI trigger for Q{}: {:?}", qid, trigger);
                }
                continue;
            }

            // Recorder live bridge: CC# → DMX channel (1-based) on the
            // configured universe, 0–127 scaled to 0–255.
            if let MidiEvent::CC { cc, value, .. } = ev {
                let bridge = {
                    let Ok(state) = self.cuepool.state().lock() else { continue };
                    state.recorder_midi_enabled.then_some(state.recorder_midi_universe)
                };
                if let Some(universe) = bridge {
                    self.recorder.live_input(universe, cc as u16, (value as u16 * 255 / 127) as u8);
                }
            }

            let cues: Vec<_> = {
                let Ok(state) = self.cuepool.state().lock() else { continue };
                state.show_file.cues.iter().filter(|c| c.enabled()).cloned().collect()
            };
            for cue in cues {
                let trigger = cue.base().triggers.midi.as_ref();
                let Some(trigger) = trigger else { continue };
                let matches = match (ev, trigger.kind) {
                    (MidiEvent::NoteOn { channel, note, velocity }, MidiTriggerKind::NoteOn)
                    | (MidiEvent::NoteOff { channel, note, velocity }, MidiTriggerKind::NoteOff) => {
                        channel == trigger.channel && note == trigger.note_or_cc && velocity >= trigger.velocity_min
                    }
                    (MidiEvent::CC { channel, cc, value }, MidiTriggerKind::CC) => {
                        channel == trigger.channel && cc == trigger.note_or_cc && value >= trigger.velocity_min
                    }
                    _ => false,
                };
                if matches {
                    let qid = cue.base().qid;
                    log::info!("MIDI trigger matched Q{}", qid);
                    self.play_cue_by_qid(qid, event_loop);
                }
            }
        }
    }

    /// Fire the first enabled cue whose hotkey trigger matches `key_name`.
    fn fire_hotkey_trigger(&mut self, key_name: &str, event_loop: &ActiveEventLoop) {
        let cues: Vec<_> = {
            let Ok(state) = self.cuepool.state().lock() else { return };
            state.show_file.cues.iter().filter(|c| c.enabled()).cloned().collect()
        };
        for cue in cues {
            if cue
                .base()
                .triggers
                .hotkey
                .as_ref()
                .map(|t| t.key.eq_ignore_ascii_case(key_name))
                .unwrap_or(false)
            {
                let qid = cue.base().qid;
                log::info!("Hotkey trigger fired Q{} via '{}'", qid, key_name);
                self.play_cue_by_qid(qid, event_loop);
                break;
            }
        }
    }

    /// Poll wall-clock triggers and fire any cue whose scheduled time has arrived.
    fn poll_wall_clock_triggers(&mut self, event_loop: &ActiveEventLoop) {
        let now = chrono::Local::now();
        let current = now.time();
        let cues: Vec<_> = {
            let Ok(state) = self.cuepool.state().lock() else { return };
            state.show_file.cues.iter().filter(|c| c.enabled()).cloned().collect()
        };

        for cue in cues {
            let Some(trigger) = cue.base().triggers.wall_clock.as_ref() else { continue };
            let parsed = chrono::NaiveTime::parse_from_str(&trigger.time, "%H:%M:%S")
                .or_else(|_| chrono::NaiveTime::parse_from_str(&trigger.time, "%I:%M:%S %p"));
            let Ok(target) = parsed else { continue };

            let diff = current.signed_duration_since(target).num_seconds().abs();
            if diff <= 1 {
                let qid = cue.base().qid;
                let should_fire = self
                    .wall_clock_fired
                    .get(&qid)
                    .map(|t| t.elapsed() > Duration::from_secs(2))
                    .unwrap_or(true);
                if should_fire {
                    log::info!("Wall-clock trigger fired Q{} at {}", qid, trigger.time);
                    self.wall_clock_fired.insert(qid, Instant::now());
                    self.play_cue_by_qid(qid, event_loop);
                }
            }
        }
    }

    /// Poll timecode triggers against the show clock; publishes the clock and
    /// the next armed trigger to the GUI. The clock is frozen while paused
    /// (see [`Self::show_elapsed`]) and triggers never fire mid-pause.
    fn poll_timecode_triggers(&mut self, event_loop: &ActiveEventLoop) {
        let elapsed = self.show_elapsed();

        // Publish clock + next armed trigger; handle a pending capture.
        let capture_qid = {
            let Ok(mut state) = self.cuepool.state().lock() else { return };
            state.show_time = elapsed;
            state.show_paused = self.paused;
            state.next_timecode = elapsed.and_then(|now| {
                state
                    .show_file
                    .cues
                    .iter()
                    .filter(|c| c.enabled() && !self.timecode_fired.contains(&c.base().qid))
                    .filter_map(|c| {
                        let t = c.base().triggers.timecode.as_ref()?.time.as_secs_f64();
                        (t >= now).then_some((c.base().qid, t))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
            });
            state.pending_timecode_capture
        };
        let Some(elapsed) = elapsed else { return };

        // Capture current show time into a cue's timecode trigger if
        // requested — works while paused (that's the frame-step workflow).
        if let Some(qid) = capture_qid {
            if let Ok(mut state) = self.cuepool.state().lock() {
                if let Some(cue) = state.show_file.cues.iter_mut().find(|c| c.base().qid == qid) {
                    cue.base_mut().triggers.timecode = Some(cuepool_core::TimecodeTrigger {
                        time: Timespan::from_secs_f64(elapsed),
                    });
                    state.dirty = true;
                    log::info!("Captured timecode trigger for Q{} at {:.2}s", qid, elapsed);
                }
                state.pending_timecode_capture = None;
            }
        }

        // Frozen clock: never fire while paused (a just-captured or stepped-past
        // trigger would fire instantly). Anything passed by stepping fires on
        // resume.
        if self.paused {
            return;
        }

        let cues: Vec<_> = {
            let Ok(state) = self.cuepool.state().lock() else { return };
            state.show_file.cues.iter().filter(|c| c.enabled()).cloned().collect()
        };

        for cue in cues {
            let Some(trigger) = cue.base().triggers.timecode.as_ref() else { continue };
            let qid = cue.base().qid;
            let target = trigger.time.as_secs_f64();
            if elapsed >= target && !self.timecode_fired.contains(&qid) {
                log::info!("Timecode trigger fired Q{} at {:.2}s", qid, target);
                self.timecode_fired.insert(qid);
                self.play_cue_by_qid(qid, event_loop);
            }
        }
    }

    /// Drain OSC/MSC events and translate them into AppCommands.
    fn process_protocol_events(&mut self) {
        if let Some(rx) = &self.osc_rx {
            while let Ok(ev) = rx.try_recv() {
                log::debug!("OSC event: {ev:?}");
                match ev {
                    OscEvent::Go { qid } => {
                        if let Some(qid_str) = qid {
                            if let Ok(qid_dec) = qid_str.parse::<rust_decimal::Decimal>() {
                                let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                        }
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::Go);
                        }
                    }
                    OscEvent::Stop { qid: _ } => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::Stop);
                        }
                    }
                    OscEvent::Pause { .. } => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::Pause);
                        }
                    }
                    OscEvent::Unpause { .. } => {
                        if self.paused {
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Pause);
                            }
                        }
                    }
                    OscEvent::Select { qid } => {
                        if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                            let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                        }
                    }
                    OscEvent::Up => {}
                    OscEvent::Down => {}
                    OscEvent::Save => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::SaveProject);
                        }
                    }
                    OscEvent::DmxChannel { universe, channel, value } => {
                        // Wire channel is 1-based; recorder is 0-based.
                        self.recorder.live_input(universe, channel - 1, value);
                    }
                    OscEvent::RecorderRecord => {
                        let file = self.recorder_file();
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::RecorderRecord { file });
                        }
                    }
                    OscEvent::RecorderStop => {
                        // Recording → stop & keep; idle → stop preview.
                        // stub: no OSC status feedback yet.
                        if self.recorder.recording() {
                            let file = self.recorder_file();
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::RecorderRecord { file });
                            }
                        } else if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::RecorderStopPreview);
                        }
                    }
                    OscEvent::RecorderPlay => {
                        let file = self.recorder_file();
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::RecorderPreview { file });
                        }
                    }
                    OscEvent::RecorderDiscard => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::RecorderDiscard);
                        }
                    }
                    OscEvent::RecorderRevert => {
                        let file = self.recorder_file();
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::RecorderRevert { file });
                        }
                    }
                    OscEvent::RecorderSelect { name } => {
                        let mut file = name;
                        if !file.ends_with(".dmxrec") {
                            file.push_str(".dmxrec");
                        }
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            log::info!("Recorder: selected take '{file}' via OSC");
                            state.recorder_file = file;
                        }
                    }
                    OscEvent::RemotePing => {
                        if let Some(osc) = &self.osc_manager {
                            let _ = osc.send(rosc::OscMessage {
                                addr: "/qplayer/remote/pong".into(),
                                args: vec![],
                            });
                        }
                    }
                    OscEvent::RemoteDiscovery { name, addr } => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            let local_name = state.show_file.show_settings.node_name.clone();
                            if name != local_name {
                                let now = Instant::now();
                                let nodes = &mut state.show_file.show_settings.remote_nodes;
                                if let Some(idx) = nodes.iter().position(|n| n.name == name) {
                                    nodes[idx].last_seen = Some(now);
                                    if let Some(a) = addr {
                                        nodes[idx].address = a.to_string();
                                    }
                                } else {
                                    nodes.push(cuepool_core::RemoteNode {
                                        name: name.clone(),
                                        address: addr.map(|a| a.to_string()).unwrap_or_default(),
                                        last_seen: Some(now),
                                    });
                                    log::info!("Discovered remote node: {} at {:?}", name, addr);
                                }
                            }
                        }
                    }
                    OscEvent::RemoteGo { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Go);
                            }
                        }
                    }
                    OscEvent::RemoteStop { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Stop);
                            }
                        }
                    }
                    OscEvent::RemotePause { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Pause);
                            }
                        }
                    }
                    OscEvent::RemoteUnpause { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if self.paused {
                                if let Ok(mut state) = self.cuepool.state().lock() {
                                    state.command_queue.push(AppCommand::Pause);
                                }
                            }
                        }
                    }
                    OscEvent::RemotePreload { target, qid, time: _ } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Preload);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(rx) = &self.msc_rx {
            while let Ok(ev) = rx.try_recv() {
                log::debug!("MSC event: {ev:?}");
                match ev {
                    MscEvent::Go { qid, .. } | MscEvent::TimedGo { qid, .. } => {
                        if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                            let _ = self.cuepool.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                        }
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::Go);
                        }
                    }
                    MscEvent::Stop { .. } => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::Stop);
                        }
                    }
                    MscEvent::Resume { .. } => {}
                    _ => {}
                }
            }
        }

        // Discovery broadcast every 1 second
        if self.last_discovery.elapsed() >= Duration::from_secs(1) {
            self.last_discovery = Instant::now();
            let node_name = {
                let Ok(state) = self.cuepool.state().lock() else { return; };
                state.show_file.show_settings.node_name.clone()
            };
            if let Some(osc) = &self.osc_manager {
                let _ = osc.send(rosc::OscMessage {
                    addr: "/qplayer/remote/discovery".into(),
                    args: vec![rosc::OscType::String(node_name)],
                });
            }
        }

        // Remote node liveness: mark nodes inactive after 5s without discovery
        {
            let Ok(mut state) = self.cuepool.state().lock() else { return; };
            let now = Instant::now();
            for node in &mut state.show_file.show_settings.remote_nodes {
                if let Some(last) = node.last_seen {
                    if now.duration_since(last) > Duration::from_secs(5) {
                        // Node timed out — keep it in the list but last_seen is stale
                    }
                }
            }
        }
    }

    /// Render the control window (egui).
    fn update_window_title(&mut self) {
        let (path, dirty) = {
            let Ok(state) = self.cuepool.state().lock() else { return };
            (state.project_path.clone(), state.dirty)
        };
        let name = path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        let title = if dirty {
            format!("CuePool — {} *", name)
        } else {
            format!("CuePool — {}", name)
        };
        if self.last_window_title != title {
            self.last_window_title = title.clone();
            if let Some(window) = self.control_window.as_ref() {
                window.set_title(&title);
            }
        }
    }

    /// Engine housekeeping that must run every loop iteration regardless of
    /// window visibility: cue lifecycle (fades, stops, finish chains), video
    /// loop restarts, delayed and TimeCode cues, and queued commands. Windows
    /// stops delivering paint events to a minimized/covered control window,
    /// so none of this can live in render_control — looping videos froze on
    /// their last frame with the GUI hidden because the loop-restart poll
    /// below never ran until the GUI was focused again.
    fn tick_engine(&mut self, event_loop: &ActiveEventLoop) {
        self.check_fade_outs();
        self.check_pending_stops();
        self.check_finished_cues(event_loop);

        // Check for video cues that have looped and restart their video threads.
        // MTC-follow cues never loop on their own — the MTC master owns position.
        if self.mtc_follow.is_none() {
        if let Some(video_qid) = self.current_video_qid {
            if let Some(ac) = self.active_cues.iter_mut().find(|ac| ac.qid == video_qid) {
                if let Some(ref counter) = ac.loop_counter {
                    let current = counter.load(Ordering::Relaxed);
                    if current > ac.video_loop_count {
                        ac.video_loop_count = current;
                        // Look up the cue's video path in the show file
                        let path = {
                            let Ok(state) = self.cuepool.state().lock() else { return };
                            state.show_file.cues.iter()
                                .find(|c| c.base().qid == video_qid)
                                .and_then(|cue| match cue {
                                    cuepool_core::Cue::Video { path, .. } => Some(path.clone()),
                                    _ => None,
                                })
                        };
                        if let Some(path) = path {
                            self.restart_video(&path, video_qid, event_loop);
                        }
                    }
                }
            }
        }
        }

        // Check for delayed cues whose timer has expired
        {
            let now = std::time::Instant::now();
            let mut ready = Vec::new();
            self.delayed_cues.retain(|dc| {
                if dc.start_at <= now {
                    ready.push(dc.cue.clone());
                    false
                } else {
                    true
                }
            });
            for cue in ready {
                self.play_cue(&cue, event_loop);
            }
        }

        // Check for TimeCode cues whose start time has been reached
        if let Some(start) = self.show_start_time {
            let elapsed = start.elapsed().as_secs_f64();
            let timecode_cues = {
                let Ok(state) = self.cuepool.state().lock() else { return; };
                state.show_file.cues.iter()
                    .filter_map(|cue| match cue {
                        cuepool_core::Cue::TimeCode { base, start_time, .. } => {
                            if start_time.as_secs_f64() > 0.0
                                && elapsed >= start_time.as_secs_f64()
                                && !self.triggered_timecodes.contains(&base.qid)
                                && cue.enabled()
                            {
                                Some(cue.clone())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            for cue in timecode_cues {
                let qid = cue.base().qid;
                log::info!("TimeCode cue Q{} triggered at {:.2}s", qid, elapsed);
                self.triggered_timecodes.push(qid);
                self.play_cue(&cue, event_loop);
            }
        }

        // Check for TimeCode cues whose duration has elapsed and fire their AfterLast chains.
        {
            let now = Instant::now();
            let mut expired = Vec::new();
            self.active_timecodes.retain(|(qid, deadline)| {
                if now >= *deadline {
                    expired.push(*qid);
                    false
                } else {
                    true
                }
            });
            for qid in expired {
                log::info!("TimeCode cue Q{} duration elapsed", qid);
                self.play_after_last_chain(qid, event_loop);
            }
        }

        // Process commands queued by the GUI and OSC/remote handlers; refresh
        // the mixer snapshot after, so any play() calls are reflected before
        // the next audio callback fires.
        self.process_commands(event_loop);
        if let Some(engine) = &self.audio_engine {
            engine.refresh();
        }
    }

    fn render_control(&mut self) {
        self.update_window_title();
        // Read before egui_state's mutable borrow below (E0502 otherwise).
        let sample_rate = (self.audio_sample_rate() as f64).max(1.0);

        // Acquire (under the shared gate) BEFORE running the egui pass: bailing
        // out after `run` would discard its texture deltas and desync the atlas.
        let submit_guard = self
            .configure_gate
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(surface) = self.control_surface.as_ref() else { return };
        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(o) | wgpu::CurrentSurfaceTexture::Suboptimal(o) => o,
            // Control window covered/minimized — skip this frame quietly (no spam).
            wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated => {
                drop(submit_guard);
                log::debug!("Control surface outdated, reconfiguring");
                let Some(surface) = self.control_surface.as_ref() else { return };
                let Some(config) = self.control_config.as_ref() else { return };
                let _configure_guard = self
                    .configure_gate
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                surface.configure(&self.device, config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                drop(submit_guard);
                log::warn!("Control surface lost, recreating");
                let Some(window) = self.control_window.as_ref() else { return };
                let Some(config) = self.control_config.as_ref() else { return };
                let surface = self
                    .instance
                    .create_surface(Arc::clone(window))
                    .expect("recreate control surface");
                {
                    let _configure_guard = self
                        .configure_gate
                        .write()
                        .unwrap_or_else(|e| e.into_inner());
                    surface.configure(&self.device, config);
                }
                self.control_surface = Some(surface);
                return;
            }
            err => {
                log::warn!("Control surface acquire failed: {err:?}");
                return;
            }
        };
        let Some(config) = self.control_config.as_ref() else { return };
        let Some(window) = self.control_window.as_ref() else { return };
        let Some(egui_state) = self.egui_state.as_mut() else { return };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else { return };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let raw_input = egui_state.take_egui_input(window);
        // Sync active cue state into the GUI shared state
        {
            let sr = sample_rate;
            // Interleaved stereo samples → seconds.
            let secs = |samples: usize| (samples as f64 / 2.0 / sr) as f32;
            let mut gui_active: Vec<cuepool_gui::ActiveCueInfo> = self.active_cues.iter().map(|ac| {
                // For looping cues with explicit loop boundaries, show loop-relative
                // position so the progress bar resets to 0 on each loop iteration.
                let loop_length_frames = ac.loop_end_frame.saturating_sub(ac.loop_start_frame) as usize;
                let (position, length) = if ac.state == CueState::PlayingLooped && loop_length_frames > 0 {
                    let total_frames = ac.input.position() / 2; // mixer is stereo
                    let rel_frames = total_frames % loop_length_frames;
                    (rel_frames * 2, Some(loop_length_frames * 2))
                } else {
                    (ac.input.position(), ac.input.length())
                };
                cuepool_gui::ActiveCueInfo {
                    qid: ac.qid,
                    name: ac.name.clone(),
                    volume: ac.input.volume(),
                    paused: !ac.input.is_active(),
                    position_secs: secs(position),
                    length_secs: length.map(secs),
                    state: ac.state,
                }
            }).collect();
            // Video clock state for the synthesized entry below — read BEFORE
            // the GUI state lock so the two locks are never held together.
            let (video_paused, video_pos_secs) = {
                let ctl = self.video_control.lock_unpoisoned();
                let pos = match (ctl.clock, ctl.pause_started) {
                    (Some(clock), Some(paused_at)) => paused_at.duration_since(clock).as_secs_f32(),
                    (Some(clock), None) => clock.elapsed().as_secs_f32(),
                    _ => 0.0,
                };
                (ctl.pause_started.is_some(), pos)
            };
            if let Ok(mut state) = self.cuepool.state().lock() {
                // A video with no audio track (or whose audio failed to open) has
                // no mixer input and thus no ActiveCue — but it is on screen.
                // Synthesize a panel entry from the video clock so it still shows
                // as active. Position comes from the video clock (frozen across
                // pause); length from the cue's duration field when set.
                if let Some(vqid) = self.current_video_qid {
                    if !gui_active.iter().any(|c| c.qid == vqid) {
                        if let Some(cue) = state.show_file.cues.iter().find(|c| c.base().qid == vqid) {
                            let paused = video_paused;
                            let position_secs = video_pos_secs;
                            let dur = match cue {
                                cuepool_core::Cue::Video { duration, .. } => duration.as_secs_f64() as f32,
                                _ => 0.0,
                            };
                            gui_active.push(cuepool_gui::ActiveCueInfo {
                                qid: vqid,
                                name: cue.base().name.clone(),
                                volume: 0.0,
                                paused,
                                position_secs,
                                length_secs: (dur > 0.0).then_some(dur),
                                state: if paused { CueState::Paused } else { CueState::Playing },
                            });
                        }
                    }
                }
                state.active_cues = gui_active;
            }
        }

        // Sync master meter data into the GUI shared state
        if let Some(engine) = &self.audio_engine {
            let meters = engine.read_meters();
            let peak_l_db = if meters.peak_l > 0.0 { 20.0 * meters.peak_l.log10() } else { -f32::INFINITY };
            let peak_r_db = if meters.peak_r > 0.0 { 20.0 * meters.peak_r.log10() } else { -f32::INFINITY };
            let rms_l_db = if meters.rms_l > 0.0 { 20.0 * meters.rms_l.log10() } else { -f32::INFINITY };
            let rms_r_db = if meters.rms_r > 0.0 { 20.0 * meters.rms_r.log10() } else { -f32::INFINITY };
            let limiter_gr_db = engine.read_limiter_gr_db();
            if let Ok(mut state) = self.cuepool.state().lock() {
                state.meter_data = cuepool_gui::GuiMeterData {
                    peak_l_db,
                    peak_r_db,
                    rms_l_db,
                    rms_r_db,
                    clipped: false, // TODO: expose clip flag from MeteringProcessor
                    limiter_gr_db,
                };
            }
        } else {
            self.cuepool.state().lock_unpoisoned().meter_data =
                cuepool_gui::GuiMeterData::default();
        }

        let full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            self.cuepool.update(ui);
        });
        egui_state.handle_platform_output(window, full_output.platform_output);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [config.width, config.height],
            pixels_per_point: window.scale_factor() as f32 * self.egui_ctx.zoom_factor(),
        };

        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("control-encoder"),
        });

        for (id, image_delta) in &full_output.textures_delta.set {
            egui_renderer.update_texture(&self.device, &self.queue, *id, image_delta);
        }
        egui_renderer.update_buffers(&self.device, &self.queue, &mut encoder, &paint_jobs, &screen_descriptor);

        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("control-render-pass"),
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
            egui_renderer.render(&mut render_pass.forget_lifetime(), &paint_jobs, &screen_descriptor);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
        // Commands queued during the UI frame drain in tick_engine, which runs
        // in about_to_wait later this same iteration.
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.control_window.is_none() {
            self.create_control_window(event_loop);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        match event {
            AppEvent::VideoEof(epoch) => {
                let current_epoch = self.video_control.lock_unpoisoned().stream_epoch;
                if epoch != current_epoch {
                    log::debug!("Ignoring stale video EOF epoch {epoch} (current {current_epoch})");
                    return;
                }
                log::info!("Video EOF");
                // MTC follow: hold the last frame on the canvas past the end —
                // looping, re-locating and blanking are all owned by the MTC
                // master, not by the clip's EOF.
                if self.mtc_follow.is_some() {
                    return;
                }
                // What the output window shows after a clip ends:
                //   Looped/LoopedInfinite -> restart (video-only here; audio-backed
                //     clips restart via the audio loop_counter, so skip those),
                //   HoldLast -> keep the final frame on screen,
                //   OneShot (default) -> blank the window to black.
                if let Some(qid) = self.current_video_qid {
                    let has_audio_cue = self.active_cues.iter().any(|ac| ac.qid == qid);
                    let cue_info = {
                        let state = self.cuepool.state().lock_unpoisoned();
                        state.show_file.cues.iter()
                            .find(|c| c.base().qid == qid)
                            .and_then(|c| match c {
                                cuepool_core::Cue::Video { path, .. } => {
                                    Some((c.base().loop_mode, path.clone()))
                                }
                                _ => None,
                            })
                    };
                    match cue_info {
                        Some((
                            cuepool_core::LoopMode::Looped | cuepool_core::LoopMode::LoopedInfinite,
                            path,
                        )) => {
                            if !has_audio_cue {
                                self.restart_video(&path, qid, event_loop);
                            }
                        }
                        Some((cuepool_core::LoopMode::HoldLast, _)) => {
                            // Hold the last frame — leave the video state untouched.
                        }
                        _ => {
                            // OneShot (or cue gone): blank the output to black.
                            self.stop_video_playback();
                            // Blank the canvas texture too: with a text overlay
                            // active the canvas still renders, and the clip's
                            // last frame must not linger behind the text.
                            let _ = self.canvas_cmd_tx.send(CanvasCommand::BlankCanvas);
                        }
                    }
                }
            }
            AppEvent::OutputSurfaceLost(window_id) => {
                if self.output_windows.iter().any(|out| out.id == window_id) {
                    log::warn!("Output surface lost — rebuilding output windows");
                    self.create_output_windows(event_loop);
                }
            }
            AppEvent::DeviceLost => {
                log::error!("GPU device lost; CuePool cannot recover without a restart — exiting");
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let is_control = self
            .window_ids
            .as_ref()
            .map(|ids| ids.control == window_id)
            .unwrap_or(false);
        let is_video = self
            .window_ids
            .as_ref()
            .map(|ids| ids.video.contains(&window_id))
            .unwrap_or(false);

        if is_control {
            let egui_consumed = if let (Some(egui_state), Some(window)) =
                (self.egui_state.as_mut(), self.control_window.as_ref())
            {
                egui_state.on_window_event(window, &event).consumed
            } else {
                false
            };

            match event {
                WindowEvent::CloseRequested => {
                    // Unsaved changes / running cues -> show the in-app quit-confirm
                    // modal (a native dialog deadlocks the loop). Otherwise quit now.
                    let dirty = self.cuepool.state().lock().map(|s| s.dirty).unwrap_or(false);
                    if !self.active_cues.is_empty() || dirty {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.pending_close_confirm = true;
                        }
                    } else {
                        self.hard_exit();
                    }
                }
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        if let Some(config) = self.control_config.as_mut() {
                            config.width = size.width;
                            config.height = size.height;
                        }
                        if let Some(surface) = self.control_surface.as_ref() {
                            if let Some(config) = self.control_config.as_ref() {
                                let _configure_guard = self
                                    .configure_gate
                                    .write()
                                    .unwrap_or_else(|e| e.into_inner());
                                surface.configure(&self.device, config);
                            }
                        }
                    }
                }
                WindowEvent::DroppedFile(path) => {
                    self.handle_dropped_file(&path);
                }
                WindowEvent::RedrawRequested => {
                    self.render_control();
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    self.modifiers = modifiers.state();
                }
                WindowEvent::KeyboardInput { event: key_event, .. } if !egui_consumed => {
                    // Toggle the video-output window fullscreen from the control window
                    // (Ctrl/Cmd+F or F11) so it works while operating the cue list.
                    // Creates the output window first if it isn't open yet.
                    if key_event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, KeyCode, PhysicalKey};
                        let is_f11 = matches!(key_event.physical_key, PhysicalKey::Code(KeyCode::F11));
                        let is_f = key_event.logical_key == Key::Character("f".into());
                        let has_ctrl = self.modifiers.control_key() || self.modifiers.super_key();
                        if is_f11 || (is_f && has_ctrl) {
                            if self.output_windows.is_empty() {
                                self.create_output_windows(event_loop);
                            }
                            self.toggle_output_fullscreen();
                        } else {
                            // Check cue hotkey triggers (only bare keys, not Ctrl/Cmd combos).
                            let key_name = match &key_event.logical_key {
                                Key::Character(s) => s.to_string(),
                                Key::Named(n) => format!("{:?}", n),
                                _ => String::new(),
                            };
                            if !key_name.is_empty() && !has_ctrl {
                                self.fire_hotkey_trigger(&key_name, event_loop);
                            }
                        }
                    }
                }
                _ => {}
            }
        } else if is_video {
            match event {
                WindowEvent::CloseRequested => {
                    self.output_windows.retain(|out| out.id != window_id);
                    if let Some(ids) = self.window_ids.as_mut() {
                        ids.video.retain(|id| *id != window_id);
                    }
                    if self.output_windows.is_empty() {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.show_video_window = false;
                        }
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey, PhysicalKey};
                        let is_esc = event.logical_key == Key::Named(NamedKey::Escape);
                        let is_f11 = matches!(event.physical_key, PhysicalKey::Code(winit::keyboard::KeyCode::F11));
                        let is_f = event.logical_key == Key::Character("f".into());
                        let has_ctrl = self.modifiers.control_key() || self.modifiers.super_key();

                        if let Some(out) = self.output_windows.iter().find(|out| out.id == window_id) {
                            // Esc always exits fullscreen
                            if is_esc {
                                out.window.set_fullscreen(None);
                                out.window.set_cursor_visible(true);
                            }
                            // F11 toggles fullscreen
                            else if is_f11 {
                                let currently = out.window.fullscreen().is_some();
                                if currently {
                                    out.window.set_fullscreen(None);
                                    out.window.set_cursor_visible(true);
                                } else {
                                    out.window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                                    out.window.set_cursor_visible(false);
                                }
                            }
                            // Ctrl+F or Cmd+F toggles fullscreen
                            else if is_f && has_ctrl {
                                let currently = out.window.fullscreen().is_some();
                                if currently {
                                    out.window.set_fullscreen(None);
                                    out.window.set_cursor_visible(true);
                                } else {
                                    out.window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                                    out.window.set_cursor_visible(false);
                                }
                            }
                        }

                        // Cue hotkey triggers also fire while an output window has
                        // focus (e.g. fullscreen on a projector), matching the
                        // control window. Skip Esc/F11 and Ctrl/Cmd combos so they
                        // keep their fullscreen behaviour.
                        if !is_esc && !is_f11 && !has_ctrl {
                            let key_name = match &event.logical_key {
                                Key::Character(s) => s.to_string(),
                                Key::Named(n) => format!("{:?}", n),
                                _ => String::new(),
                            };
                            if !key_name.is_empty() {
                                self.fire_hotkey_trigger(&key_name, event_loop);
                            }
                        }
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    self.modifiers = modifiers.state();
                }
                WindowEvent::Resized(size) => {
                    // Forward the new size to the render thread; it reconfigures
                    // its own surface before the next acquire.
                    if let Some(out) = self.output_windows.iter().find(|out| out.id == window_id) {
                        out.size.store(pack_size(size.width, size.height), Ordering::Relaxed);
                    }
                }
                // Output windows are presented by their own render threads;
                // RedrawRequested carries no work for them.
                _ => {}
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.dbg_ticks += 1;
        // Quit confirmed by the in-app modal — hard-exit (see hard_exit for why).
        if self.cuepool.state().lock().map(|s| s.quit).unwrap_or(false) {
            self.hard_exit();
        }

        if !self.consume_failure_reported
            && self.consume_join.as_ref().is_some_and(|join| join.is_finished())
        {
            const ERROR: &str = "video-consume thread exited unexpectedly; video output is frozen";
            self.consume_failure_reported = true;
            log::error!("{ERROR}");
            self.cuepool.state().lock_unpoisoned().diagnostics.consumer_error = Some(ERROR.into());
        }

        // A new or loaded project bumps project_generation — stop the old project's
        // cues and close its output windows.
        let project_generation = self.cuepool.state().lock_unpoisoned().project_generation;
        if project_generation != self.last_project_generation {
            self.last_project_generation = project_generation;
            self.reset_for_project_change();
            self.apply_audio_settings();
        }

        // Structural projection edits (output count, monitor assignment) only take
        // effect when the output windows are rebuilt — do it here instead of making
        // the operator hit "Open Projection Output Windows". Cheap field compares,
        // no-op when nothing changed.
        let rebuild_outputs = {
            let state = self.cuepool.state().lock_unpoisoned();
            self.output_windows_built_from
                .as_ref()
                .is_some_and(|built| projection_structure_changed(built, &state.show_file.projection))
        };
        if rebuild_outputs {
            log::info!("Projection structure changed — rebuilding output windows");
            self.create_output_windows(event_loop);
        }

        self.process_midi_events(event_loop);
        // MTC receive (hot-plug refresh is internally throttled) → drive any
        // MTC-follow video cue → publish status for the transport readout.
        self.mtc_receiver.refresh();
        self.mtc_receiver.tick();
        let mtc = self.mtc_receiver.clone_state();
        self.drive_mtc_follow(&mtc);
        if let Ok(mut state) = self.cuepool.state().lock() {
            state.mtc_running = mtc.running;
            state.mtc_playing = mtc.playing;
            state.mtc_timecode_secs = mtc.position.as_seconds_f64();
            state.mtc_fps = mtc.position.frame_rate.fps() as f64;
            state.mtc_source = mtc.source_device.clone();
            state.mtc_drift_ms = self.mtc_drift.map(|d| d * 1000.0);
        }
        self.process_protocol_events();
        self.poll_wall_clock_triggers(event_loop);
        self.poll_timecode_triggers(event_loop);
        self.tick_engine(event_loop);

        // Lighting: sender lifecycle + fade advance + DMX submit (self-throttled).
        {
            let cfg = self.cuepool.state().lock_unpoisoned().show_file.lighting.clone();

            // Pixel-map segments: downsample each segment's source texture →
            // engine overlay. Throttled to the DMX rate.
            if cfg.enabled && self.last_pixel_sample.elapsed().as_secs_f32() >= 1.0 / cfg.fps.max(1.0) {
                self.upload_pixmap_frames();
                // Raw (non-sRGB) views: bytes as stored, display-referred —
                // the colour pipeline's gamma does the linearisation.
                // The canvas lives on the consume thread; its linear view is
                // republished through the bundle on every recreate.
                // ponytail: the pixmap/sample GPU calls stay on the winit
                // thread (they only run when lighting pixel-map segments are
                // enabled, at the DMX rate — never in the failing video-only
                // rig config). Upgrade path: move sampling into the consume
                // thread if a lighting show ever hits the WSI stall.
                let canvas_view = self.frame_state.lock_unpoisoned().canvas_render_view.clone();
                let pixmap_view = self.pixmap_texture.as_ref().map(|t| t.render_view());
                let batch: Vec<(&wgpu::TextureView, u32, [f32; 4], u32, u32)> = cfg
                    .active_segments()
                    .filter_map(|s| {
                        let view = match s.source {
                            cuepool_core::lighting::SegmentSource::Canvas => canvas_view.as_ref(),
                            cuepool_core::lighting::SegmentSource::PixelMap => pixmap_view.as_ref(),
                        }?;
                        Some((view, s.id, s.region, s.cols, s.rows))
                    })
                    .collect();
                if !batch.is_empty() {
                    self.last_pixel_sample = Instant::now();
                    let configure_gate = Arc::clone(&self.configure_gate);
                    let sampler = self
                        .pixel_sampler
                        .get_or_insert_with(|| cuepool_video::PixelSampler::new(&self.device));
                    let ready = sampler.collect(&self.device);
                    if !ready.is_empty() {
                        // Publish to the GUI grid preview, then feed the engine.
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            for (id, cols, rows, rgba) in &ready {
                                state.lighting_preview.insert(*id, (*cols, *rows, rgba.clone()));
                            }
                        }
                        for (id, cols, rows, rgba) in ready {
                            self.lighting.set_segment_pixels(id, cols, rows, rgba);
                        }
                    }
                    let _configure_guard =
                        configure_gate.read().unwrap_or_else(|e| e.into_inner());
                    sampler.sample(&self.device, &self.queue, &batch);
                }
            }

            // Recorder: drain received DMX into the pass and refresh the
            // monitor overlay before the lighting engine composites.
            self.recorder.tick(&mut self.lighting);
            if let Ok(mut state) = self.cuepool.state().lock() {
                state.recorder_status = self.recorder.snapshot();
            }

            self.lighting.tick(&cfg);
        }

        // Publish the current monitors (for the projection-panel dropdown) and detect
        // hotplug / projector warm-up. Throttled — enumerating monitors hits the OS.
        if self.last_monitor_check.elapsed() >= std::time::Duration::from_millis(1000) {
            self.last_monitor_check = std::time::Instant::now();
            let current: Vec<cuepool_core::MonitorId> = event_loop
                .available_monitors()
                .map(|m| monitor_descriptor(&m))
                .collect();
            if current != self.last_monitor_set {
                self.last_monitor_set = current.clone();
                if let Ok(mut s) = self.cuepool.state().lock() {
                    s.available_monitors = current;
                }
                // Re-apply output→monitor assignment (a projector finished warming up
                // after boot, or was power-cycled) so the wall self-heals.
                if !self.output_windows.is_empty() {
                    log::info!("Monitor set changed — rebuilding output windows");
                    self.create_output_windows(event_loop);
                }
            }
        }

        // Identify: flash each output a distinct colour so the operator can map
        // windows to physical projectors.
        let identify_req = self
            .cuepool
            .state()
            .lock()
            .map(|mut s| std::mem::take(&mut s.identify_outputs))
            .unwrap_or(false);
        if identify_req {
            self.identify_until =
                Some(std::time::Instant::now() + std::time::Duration::from_secs(8));
            for (i, out) in self.output_windows.iter().enumerate() {
                log::warn!(
                    "Identify: '{}' = {}",
                    out.output_config.name,
                    IDENTIFY_COLOR_NAMES[i % IDENTIFY_COLOR_NAMES.len()]
                );
            }
        }

        // Push winit-side state to the consume thread: canvas fit, changed
        // projection outputs, identify flag. Also fold in a completed
        // frame-step-back clock delta and finish a completed stop-cue fade.
        // (Decode consumption, canvas upload and the frame-state publish all
        // live on the consume thread — no GPU work happens on this thread.)
        {
            let (fit, outputs_changed) = {
                let state = self.cuepool.state().lock_unpoisoned();
                let live = &state.show_file.projection;
                let changed = if live.outputs != self.published_outputs {
                    self.published_outputs = live.outputs.clone();
                    Some(self.published_outputs.clone())
                } else {
                    None
                };
                (live.fit, changed)
            };
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.fit = fit;
            if let Some(outputs) = outputs_changed {
                ctl.outputs = outputs;
                ctl.outputs_gen += 1;
            }
            ctl.identify = self.identify_until.is_some_and(|t| std::time::Instant::now() < t);
            if let Some(delta) = ctl.step_back_delta.take() {
                self.show_paused_offset += delta;
            }
            let fade_done = ctl
                .fade
                .is_some_and(|(start, dur)| fade_elapsed(start, ctl.pause_started).as_secs_f32() >= dur);
            drop(ctl);
            if fade_done {
                self.stop_video_playback();
            }
        }

        if self.dbg_last_log.elapsed() >= std::time::Duration::from_secs(1) {
            let secs = self.dbg_last_log.elapsed().as_secs_f64();
            let starved_per_sec =
                std::mem::replace(&mut self.video_control.lock_unpoisoned().starved, 0) as f64 / secs;
            let ticks_per_sec = self.dbg_ticks as f64 / secs;
            // Publish the counters (plus a fresh output snapshot) to the Status
            // window. The output list is rebuilt here rather than patched on
            // create/destroy, so every mutation site stays in sync for free.
            // Per-output presented/s comes from each render thread's counter —
            // the field diagnostic for vsync-starvation on one output.
            let mut total_presented = 0.0;
            let outputs: Vec<OutputDiagnostics> = self
                .output_windows
                .iter()
                .map(|out| {
                    let presented_per_sec =
                        out.presented.swap(0, Ordering::Relaxed) as f64 / secs;
                    total_presented += presented_per_sec;
                    OutputDiagnostics {
                        name: out.output_config.name.clone(),
                        size: unpack_size(out.size.load(Ordering::Relaxed)),
                        present_mode: format!("{:?}", out.present_mode),
                        format: format!("{:?}", out.format),
                        refresh: out
                            .window
                            .current_monitor()
                            .and_then(|m| m.refresh_rate_millihertz())
                            .map(|mhz| format!("{:.2} Hz", mhz as f64 / 1000.0))
                            .unwrap_or_else(|| "?".into()),
                        fullscreen: out.window.fullscreen().is_some(),
                        presented_per_sec,
                    }
                })
                .collect();
            if self.fps_debug {
                let per_output = outputs
                    .iter()
                    .map(|o| format!("'{}' {:.0}/s", o.name, o.presented_per_sec))
                    .collect::<Vec<_>>()
                    .join(" | ");
                eprintln!(
                    "VIDEO DIAG: loop {:.0}/s | starved {:.0}/s | presented {}",
                    ticks_per_sec, starved_per_sec, per_output,
                );
            }
            let mut state = self.cuepool.state().lock_unpoisoned();
            let d = &mut state.diagnostics;
            d.presented_per_sec = total_presented;
            d.starved_per_sec = starved_per_sec;
            d.event_loop_per_sec = ticks_per_sec;
            d.outputs = outputs;
            drop(state);
            self.dbg_ticks = 0;
            self.dbg_last_log = std::time::Instant::now();
        }

        // The control window redraws on a ~60 Hz throttle so its (non-vsync) UI
        // stays live without busy-spinning the GPU.
        if self.last_control_redraw.elapsed() >= std::time::Duration::from_millis(16) {
            self.last_control_redraw = std::time::Instant::now();
            if let Some(window) = self.control_window.as_ref() {
                window.request_redraw();
            }
        }

        // Main-loop pacing: nothing on this thread blocks on vsync (each output
        // paces itself in its render thread) and no GPU work happens here
        // (decode consumption + uploads live on the consume thread) — so
        // ControlFlow::Poll would free-spin. Wake at ~250 Hz for cue timing and
        // UI; OS events still wake the loop instantly. On Windows this cadence
        // needs the 1 ms timer resolution raised at startup (see win_timer).
        event_loop.set_control_flow(ControlFlow::WaitUntil(
            std::time::Instant::now() + std::time::Duration::from_millis(4),
        ));
    }
}

/// After firing the cue at `start_idx`, return the next cue to put on standby:
/// skip the cues that auto-fired alongside it — a Group's members (everything up
/// to the next Group), or a `WithLast`/`AfterLast` continuation chain — and land
/// on the next manually-triggered cue. `None` at the end of the list.
fn next_standby_qid(
    cues: &[cuepool_core::Cue],
    start_idx: usize,
) -> Option<rust_decimal::Decimal> {
    let mut i = start_idx + 1;
    if matches!(cues.get(start_idx), Some(cuepool_core::Cue::Group { .. })) {
        // A group fired all its members (cues whose parent is this group) — skip
        // past them to the next standby.
        let gid = cues[start_idx].base().qid;
        while i < cues.len() && cues[i].base().parent == Some(gid) {
            i += 1;
        }
    } else {
        while i < cues.len()
            && matches!(
                cues[i].base().trigger,
                cuepool_core::TriggerMode::WithLast | cuepool_core::TriggerMode::AfterLast
            )
        {
            i += 1;
        }
    }
    cues.get(i).map(|c| c.base().qid)
}

/// The first enabled `AfterLast` cue directly following `finished_qid` — the
/// next link of a completion chain. Disabled followers are skipped; anything
/// else (or an unknown qid) ends the chain.
fn next_after_last(
    cues: &[cuepool_core::Cue],
    finished_qid: rust_decimal::Decimal,
) -> Option<&cuepool_core::Cue> {
    let idx = cues.iter().position(|c| c.base().qid == finished_qid)?;
    cues[idx + 1..]
        .iter()
        .take_while(|c| c.base().trigger == cuepool_core::TriggerMode::AfterLast)
        .find(|c| c.enabled())
}

/// Follow a goto chain from `first_target` to the first non-goto cue. Returns
/// `None` on a cycle (including a self-target) or a dead end, so the caller never
/// recurses into `play_cue` indefinitely. `goto_qid` is the originating goto cue.
fn resolve_goto_target(
    cues: &[cuepool_core::Cue],
    goto_qid: rust_decimal::Decimal,
    first_target: rust_decimal::Decimal,
) -> Option<rust_decimal::Decimal> {
    let mut current = first_target;
    let mut visited = std::collections::HashSet::from([goto_qid]);
    loop {
        if !visited.insert(current) {
            return None; // cycle
        }
        match cues.iter().find(|c| c.base().qid == current) {
            Some(cuepool_core::Cue::Goto { target_qid, .. }) => current = *target_qid,
            Some(_) => return Some(current),
            None => return None, // dead end
        }
    }
}

/// Video consume thread: owns the canvas/overlay textures, the YUV converter,
/// the decode-channel drain and the frame-state publish — every non-egui GPU
/// call. Moving this off the winit thread is the Windows fix: NVIDIA's Vulkan
/// WSI serializes a thread's GPU calls behind the vsync-blocked render
/// threads (20-60 ms per call), which dragged the whole event loop to ~10 Hz.
///
/// The loop is paced by the video clock: compute the next due frame's PTS
/// against the clock, sleep until due, drain due frames, upload the newest,
/// submit the YUV convert, publish. Paused/idle: a light 2 ms poll so
/// frame-step/stop stay responsive without busy-spinning.
///
/// Lock order: `control` and `frame_state` are both leaf locks, never held
/// together across GPU calls and never held while sleeping or blocking on
/// `recv_timeout`.
fn video_consume_thread(
    device: wgpu::Device,
    queue: wgpu::Queue,
    configure_gate: Arc<RwLock<()>>,
    control: Arc<Mutex<VideoControl>>,
    frame_state: Arc<Mutex<OutputFrameState>>,
    cmd_rx: std::sync::mpsc::Receiver<CanvasCommand>,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    stop: Arc<AtomicBool>,
) {
    let mut canvas: Option<cuepool_video::CanvasTexture> = None;
    let mut overlay: Option<cuepool_video::CanvasTexture> = None;
    let mut yuv_converter: Option<cuepool_video::YuvConverter> = None;
    // Decode channel taken out of the control struct; None when stopped/EOF.
    let mut rx: Option<std::sync::mpsc::Receiver<VideoMessage>> = None;
    // Epoch captured with `rx`; all frame work is discarded if control moves on.
    let mut rx_epoch: Option<u64> = None;
    // Next decoded frame, peeked but not yet due.
    let mut peek: Option<VideoFrame> = None;
    // Live outputs copy (re-cloned only when `outputs_gen` advances).
    let mut outputs: Vec<cuepool_core::ProjectorOutput> = Vec::new();
    let mut outputs_gen = 0u64;

    while !stop.load(Ordering::Relaxed) {
        // ── Cue-driven canvas/overlay commands (rare) ──
        let mut views_dirty = false;
        for cmd in coalesce_canvas_commands(cmd_rx.try_iter()) {
            match cmd {
                CanvasCommand::Resize { w, h, force } => {
                    if force || canvas.as_ref().is_none_or(|c| c.width != w || c.height != h) {
                        canvas = Some(cuepool_video::CanvasTexture::new(&device, w, h));
                        views_dirty = true;
                    }
                    if overlay.as_ref().is_none_or(|o| o.width != w || o.height != h) {
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
                        c.upload_rgba(&queue, &blank);
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
                                let _configure_guard = configure_gate
                                    .read()
                                    .unwrap_or_else(|e| e.into_inner());
                                c.upload_frame(&queue, &frame, fit);
                            }
                            Err(e) => log::error!("Image cue failed to load '{path}': {e}"),
                        }
                    }
                }
                CanvasCommand::Overlay(content) => {
                    if let Some(ov) = overlay.as_ref() {
                        match content {
                            Some((frame, fit)) => {
                                let _configure_guard = configure_gate
                                    .read()
                                    .unwrap_or_else(|e| e.into_inner());
                                ov.upload_frame(&queue, &frame, fit);
                            }
                            None => {
                                let blank = vec![0u8; (ov.width * ov.height * 4) as usize];
                                let _configure_guard = configure_gate
                                    .read()
                                    .unwrap_or_else(|e| e.into_inner());
                                ov.upload_rgba(&queue, &blank)
                            }
                        }
                    }
                }
            }
        }

        let mut eof_epoch = None;

        // ── Control handshake: new stream, stop, step-back ──
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
        // Frame-step-back: wait (blocking, but on THIS thread, so the GUI never
        // freezes) for the sought frame, snap the frozen clock to its exact
        // PTS, and report the delta back for the show clock. Taken AFTER the
        // channel refresh above so we wait on the NEW (re-seeked) receiver.
        let step_back = {
            let mut ctl = control.lock_unpoisoned();
            (rx_epoch == Some(ctl.stream_epoch))
                .then(|| ctl.step_back.take())
                .flatten()
        };
        if let Some(pos) = step_back {
            peek = None;
            let delivered = rx.as_ref().map(|r| r.recv_timeout(Duration::from_millis(1000)));
            let mut ctl = control.lock_unpoisoned();
            if rx_epoch != Some(ctl.stream_epoch) {
                rx = None;
                rx_epoch = None;
            } else {
                match delivered {
                    Some(Ok(VideoMessage::Frame(f))) => {
                        let delta = pos - f.pts;
                        if delta > 0.0 {
                            if let Some(c) = ctl.clock {
                                // Moving the epoch forward rewinds the paused position.
                                ctl.clock = Some(c + Duration::from_secs_f64(delta));
                            }
                            ctl.step_back_delta = Some(delta);
                        }
                        ctl.peek_pts = Some(f.pts);
                        peek = Some(f);
                        ctl.step_pending = true;
                    }
                    Some(Ok(VideoMessage::Eof)) => {
                        rx = None;
                        eof_epoch = rx_epoch;
                        ctl.peek_pts = None;
                    }
                    Some(Err(_)) => log::warn!("Frame step back: no frame delivered after seek"),
                    None => {}
                }
            }
        }

        // ── Due-frame selection against the video clock ──
        let (target, fit) = {
            let mut ctl = control.lock_unpoisoned();
            let stream_current = rx_epoch == Some(ctl.stream_epoch);
            let stepping = stream_current && std::mem::take(&mut ctl.step_pending);
            let target = if stream_current && (!ctl.paused || stepping) {
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
                pos.map(|t| if stepping { t + Duration::from_micros(1) } else { t })
            } else {
                None
            };
            (target, ctl.fit)
        };

        // Keep one frame peeked while paused, and notice an immediately-following
        // EOF even without a running target clock. A future frame still holds EOF
        // behind it until playback resumes and presents that frame.
        if peek.is_none() && eof_epoch.is_none() {
            match rx.as_ref().map(|r| r.try_recv()) {
                Some(Ok(VideoMessage::Frame(f))) => peek = Some(f),
                Some(Ok(VideoMessage::Eof)) => {
                    rx = None;
                    eof_epoch = rx_epoch;
                }
                Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => rx = None,
                _ => {}
            }
        }

        let mut consumed: Option<VideoFrame> = None;
        if let Some(target) = target {
            loop {
                if peek.is_none() {
                    match rx.as_ref().map(|r| r.try_recv()) {
                        Some(Ok(VideoMessage::Frame(f))) => peek = Some(f),
                        Some(Ok(VideoMessage::Eof)) => {
                            rx = None;
                            eof_epoch = rx_epoch;
                        }
                        Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => rx = None,
                        _ => {}
                    }
                }
                match peek.as_ref() {
                    Some(f) if Duration::from_secs_f64(f.pts.max(0.0)) <= target => {
                        consumed = peek.take();
                    }
                    _ => break, // next frame not due yet, or channel empty
                }
            }
        }

        // Check immediately before GPU work without holding the control lock
        // across uploads/submits. The write-back below checks again afterward.
        if consumed.is_some()
            && rx_epoch.is_none_or(|epoch| control.lock_unpoisoned().stream_epoch != epoch)
        {
            consumed = None;
            rx = None;
            rx_epoch = None;
            peek = None;
            eof_epoch = None;
        }

        // ── Upload the newest due frame to the canvas (GPU work) ──
        if let Some(frame) = consumed {
            match frame.pixels {
                cuepool_video::FramePixels::Rgba(_) => {
                    if let Some(c) = canvas.as_ref() {
                        let _configure_guard =
                            configure_gate.read().unwrap_or_else(|e| e.into_inner());
                        c.upload_frame(&queue, &frame, fit);
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
                        conv.upload(&device, &queue, &frame, [c.width, c.height], fit);
                        let mut encoder =
                            device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("canvas-yuv-convert"),
                            });
                        conv.encode(&mut encoder, &c.render_view());
                        queue.submit(std::iter::once(encoder.finish()));
                    }
                }
            }
            let mut ctl = control.lock_unpoisoned();
            if rx_epoch == Some(ctl.stream_epoch) {
                ctl.last_pts = Some(frame.pts);
                ctl.canvas_has_frame = true;
                ctl.peek_pts = peek.as_ref().map(|f| f.pts);
            } else {
                drop(ctl);
                rx = None;
                rx_epoch = None;
                peek = None;
                eof_epoch = None;
            }
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
                eof_epoch = None;
            }
        }

        // The marker is observed only after every preceding frame has left the
        // FIFO. Emit after the final due upload/write-back, tagged for winit.
        if let Some(epoch) = eof_epoch {
            if rx_epoch == Some(epoch) {
                let _ = proxy.send_event(AppEvent::VideoEof(epoch));
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
                let has_content =
                    ctl.video_active || ctl.canvas_has_frame || ctl.text_active;
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

        // ── Pace: sleep until the next frame is due, or a light idle poll ──
        let sleep_for = match target {
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
        if !sleep_for.is_zero() {
            std::thread::sleep(sleep_for);
        }
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
fn output_render_thread(
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    renderer: cuepool_video::ProjectionRenderer,
    device: wgpu::Device,
    queue: wgpu::Queue,
    configure_gate: Arc<RwLock<()>>,
    event_loop_proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    frame_state: Arc<Mutex<OutputFrameState>>,
    size: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    presented: Arc<AtomicU32>,
    window_id: WindowId,
    out_index: usize,
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
                    &queue,
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
        surface_texture.present();
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
/// timestamp (frame-step-back), then continue with the frames after it.
fn video_decode_thread(
    path: &str,
    start_before: Option<f64>,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    frame_tx: std::sync::mpsc::SyncSender<VideoMessage>,
    diag_state: SharedStateHandle,
) {
    let mut source = match VideoSource::open(path) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to open video source {}: {e}", path);
            return;
        }
    };

    // Publish what's decoding to the Status window (Help → Status…).
    diag_state.lock_unpoisoned().diagnostics.video = Some(VideoDiagnostics {
        path: path.to_string(),
        width: source.width(),
        height: source.height(),
        decode_path: source.decode_path().to_string(),
    });

    if let Some(t) = start_before {
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
            match source.read_frame() {
                Some(f) if f.pts + 1e-4 < t => prev = Some(f),
                Some(f) => {
                    if let Some(p) = prev.take() {
                        if !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(p)) {
                            return;
                        }
                    }
                    if !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(f)) {
                        return;
                    }
                    break;
                }
                None => {
                    // t is past the last frame: deliver that frame and end.
                    if let Some(p) = prev.take() {
                        if !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(p)) {
                            return;
                        }
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
    // Re-publish the decode path once frames actually flow: at open it's
    // tentative (hw device created ≠ hw decode engaged — e.g. Hap has none).
    let mut diag_path_pending = true;
    while !stop_flag.load(Ordering::Relaxed) {
        match source.read_frame() {
            Some(frame) => {
                if std::mem::take(&mut diag_path_pending) {
                    if let Some(v) = diag_state.lock_unpoisoned().diagnostics.video.as_mut() {
                        v.decode_path = source.decode_path().to_string();
                    }
                }
                if !send_video_message(&frame_tx, &stop_flag, VideoMessage::Frame(frame)) {
                    return;
                }
            }
            None => {
                send_video_message(&frame_tx, &stop_flag, VideoMessage::Eof);
                break;
            }
        }
    }
}

/// Pixel-map decode thread: self-paced by wall-clock PTS (no vsync consumer),
/// loops by reopening the source, blanks to black on a OneShot end.
fn pixmap_decode_thread(
    path: &str,
    loop_mode: cuepool_core::LoopMode,
    stop_flag: Arc<AtomicBool>,
    frame_tx: std::sync::mpsc::SyncSender<VideoFrame>,
) {
    let looping = matches!(
        loop_mode,
        cuepool_core::LoopMode::Looped | cuepool_core::LoopMode::LoopedInfinite
    );
    let mut last_dims = (0u32, 0u32);
    'outer: loop {
        let mut source = match VideoSource::open(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("PixelMap: failed to open {}: {e}", path);
                return;
            }
        };
        let start = Instant::now();
        while !stop_flag.load(Ordering::Relaxed) {
            let Some(frame) = source.read_frame() else {
                if looping {
                    continue 'outer; // reopen from the top
                }
                if loop_mode == cuepool_core::LoopMode::OneShot && last_dims.0 > 0 {
                    // Blank to black so the LEDs go dark, mirroring video-cue end.
                    let (w, h) = last_dims;
                    let _ = frame_tx.send(VideoFrame::new(w, h, vec![0; (w * h * 4) as usize], 0.0));
                }
                return; // HoldLast: exit holding the last frame
            };
            last_dims = (frame.width, frame.height);
            // Wall-clock pacing: sleep until this frame is due.
            let due = start + Duration::from_secs_f64(frame.pts.max(0.0));
            while Instant::now() < due {
                if stop_flag.load(Ordering::Relaxed) {
                    return;
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

/// Autosave background thread: writes dirty show file to rotating backups every 60 s.
fn spawn_autosave_thread(state: SharedStateHandle, running: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut slot = 0usize;
        let mut elapsed = 0u64;
        while running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            if !running.load(Ordering::Relaxed) {
                break;
            }
            elapsed += 1;
            if elapsed < 60 {
                continue;
            }
            elapsed = 0;
            let (should_save, path, autosave_enabled) = {
                let Ok(state) = state.lock() else { continue };
                (state.dirty, state.project_path.clone(), state.show_file.show_settings.autosave_enabled)
            };
            if !autosave_enabled || !should_save {
                continue;
            }
            let Some(_project_path) = path else { continue };

            let dir = dirs::data_dir()
                .unwrap_or_else(|| std::env::temp_dir())
                .join("CuePool");
            if let Err(e) = std::fs::create_dir_all(&dir) {
                log::warn!("Autosave: failed to create dir {:?}: {}", dir, e);
                continue;
            }

            slot = (slot % 5) + 1;
            let backup_path = dir.join(format!("autoback_{}.qproj", slot));
            let json = {
                let Ok(state) = state.lock() else { continue };
                match serde_json::to_string_pretty(&state.show_file) {
                    Ok(j) => j,
                    Err(e) => {
                        log::warn!("Autosave: serialization failed: {}", e);
                        continue;
                    }
                }
            };
            if let Err(e) = std::fs::write(&backup_path, json) {
                log::warn!("Autosave: failed to write {:?}: {}", backup_path, e);
            } else {
                log::info!("Autosaved to {:?}", backup_path);
            }
        }
    });
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct AppSettings {
    recent_files: Vec<std::path::PathBuf>,
}

fn settings_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|p| p.join("CuePool").join("settings.json"))
}

fn load_settings() -> AppSettings {
    if let Some(path) = settings_path() {
        if let Ok(data) = std::fs::read_to_string(&path) {
            if let Ok(settings) = serde_json::from_str(&data) {
                return settings;
            }
        }
    }
    AppSettings::default()
}

fn save_settings(settings: &AppSettings) {
    if let Some(path) = settings_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string_pretty(settings) {
            let _ = std::fs::write(path, data);
        }
    }
}

/// Attempt an emergency save before the process exits.
fn emergency_save(state: &SharedStateHandle) {
    let (json, path) = {
        let Ok(state) = state.lock() else { return };
        let json = match serde_json::to_string_pretty(&state.show_file) {
            Ok(j) => j,
            Err(e) => {
                log::error!("Emergency save: serialization failed: {}", e);
                return;
            }
        };
        (json, state.project_path.clone())
    };

    let dir = dirs::data_dir()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("CuePool");
    let _ = std::fs::create_dir_all(&dir);

    // Prefer crash_recovery.qproj, but if a project_path exists, also save there
    let crash_path = dir.join("crash_recovery.qproj");
    if let Err(e) = std::fs::write(&crash_path, &json) {
        log::error!("Emergency save: failed to write {:?}: {}", crash_path, e);
    } else {
        log::info!("Emergency save written to {:?}", crash_path);
    }

    if let Some(project_path) = path {
        if let Err(e) = std::fs::write(&project_path, &json) {
            log::error!("Emergency save: failed to overwrite {:?}: {}", project_path, e);
        } else {
            log::info!("Emergency save overwritten {:?}", project_path);
        }
    }
}

/// If a cue command starts with `udp:` (case-insensitive), return the trimmed
/// remainder to send as a raw UDP datagram instead of an OSC packet.
fn strip_udp_prefix(command: &str) -> Option<&str> {
    let command = command.trim();
    if command.get(..4)?.eq_ignore_ascii_case("udp:") {
        Some(command[4..].trim())
    } else {
        None
    }
}

/// Resolve a `udp:` remainder into (host, payload).
///
/// If the remainder contains a `:`, the trimmed segment before it is a target
/// candidate: a case-insensitive match against `targets` names wins, then a
/// literal IPv4 address; payload is everything after that colon. Anything
/// else (no colon, or an unresolved candidate) sends the whole remainder to
/// `default_host` — keeping bare `udp:PLAY x.mp4` cues and colon-containing
/// filenames working.
fn resolve_udp_command<'a>(remainder: &'a str, targets: &[cuepool_core::UdpTarget], default_host: &str) -> (String, &'a str) {
    if let Some(idx) = remainder.find(':') {
        let candidate = remainder[..idx].trim();
        let payload = remainder[idx + 1..].trim();
        if let Some(t) = targets.iter().find(|t| t.name.eq_ignore_ascii_case(candidate)) {
            log::info!("UDP target '{}' resolved to {}", t.name, t.host);
            return (t.host.clone(), payload);
        }
        if candidate.parse::<std::net::Ipv4Addr>().is_ok() {
            log::info!("UDP target '{}' used as a literal IPv4 address", candidate);
            return (candidate.to_string(), payload);
        }
        log::warn!("UDP target '{}' not found in registry, treating whole command as payload", candidate);
    }
    (default_host.to_string(), remainder)
}

/// Send `payload` as a single raw UTF-8 UDP datagram to `host:port`.
/// Broadcast targets require `set_broadcast(true)`; failures are logged.
fn send_udp_command(payload: &str, host: &str, port: u16) {
    if payload.is_empty() {
        log::warn!("UDP command is empty, nothing sent");
        return;
    }
    let send = || -> std::io::Result<()> {
        let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_broadcast(true)?;
        socket.send_to(payload.as_bytes(), (host, port))?;
        Ok(())
    };
    match send() {
        Ok(_) => log::info!("UDP TX -> {}:{}: {}", host, port, payload),
        Err(e) => log::error!("UDP send to {}:{} failed: {}", host, port, e),
    }
}

/// Parse an OSC command string like `/qplayer/go,5,hello` into an `OscMessage`.
/// The first segment (before any comma) is the OSC address.
/// Remaining segments are auto-typed arguments: int → float → string.
fn parse_osc_command(command: &str) -> anyhow::Result<rosc::OscMessage> {
    if command.is_empty() {
        anyhow::bail!("Empty OSC command");
    }
    let parts: Vec<&str> = command.split(',').collect();
    let addr = parts[0].trim().to_string();
    if !addr.starts_with('/') {
        anyhow::bail!("OSC address must start with /: {}", addr);
    }
    let mut args = Vec::new();
    for part in &parts[1..] {
        let s = part.trim();
        if s.is_empty() {
            continue;
        }
        // Try int first
        if let Ok(i) = s.parse::<i32>() {
            args.push(rosc::OscType::Int(i));
            continue;
        }
        // Try float
        if let Ok(f) = s.parse::<f32>() {
            args.push(rosc::OscType::Float(f));
            continue;
        }
        // Default to string
        args.push(rosc::OscType::String(s.to_string()));
    }
    Ok(rosc::OscMessage { addr, args })
}

fn main() -> anyhow::Result<()> {
    // Single instance guard. On unix the name is a filesystem path, and
    // Finder launches apps with cwd=/ (read-only) — use an absolute temp
    // path, and never crash over the guard (worst case: two instances).
    #[cfg(unix)]
    let lock_name = std::env::temp_dir()
        .join("CuePool.lock")
        .to_string_lossy()
        .into_owned();
    #[cfg(not(unix))]
    let lock_name = "CuePool".to_string();
    let single = single_instance::SingleInstance::new(&lock_name).ok();
    if let Some(s) = &single
        && !s.is_single()
    {
        log::warn!("Another instance of CuePool is already running. Exiting.");
        return Ok(());
    }

    human_panic::setup_panic!(
        Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .authors("CuePool Contributors")
            .homepage("https://github.com/BlueJayLouche/CuePool")
    );

    cuepool_gui::logging::init_logger();

    // 1 ms timer resolution so WaitUntil/sleep don't quantize to 15.6 ms.
    #[cfg(windows)]
    win_timer::raise();

    let event_loop = EventLoop::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let proxy = event_loop.create_proxy();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });

    // Create a headless adapter first (we'll create surfaces after windows exist)
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .map_err(|e| anyhow::anyhow!("no wgpu adapter: {e}"))?;

    let (device, queue) = pollster::block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("cuepool-device"),
            // Required for the 10-bit planar (p10le) GPU path.
            required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
    ))?;
    let device_lost_proxy = proxy.clone();
    device.set_device_lost_callback(move |reason, message| {
        log::error!("GPU device lost ({reason:?}): {message}");
        if device_lost_proxy.send_event(AppEvent::DeviceLost).is_err() {
            log::error!("Cannot report GPU device loss: event loop closed");
        }
    });

    let mut app = App::new(instance, adapter, device, queue, proxy);

    // Load app-level settings. Project audio settings live in the show file
    // and are applied when its project generation changes.
    let settings = load_settings();
    if let Ok(mut state) = app.cuepool.state().lock() {
        state.recent_files = settings.recent_files;
    }

    // Optional CLI: `cuepool path/to/show.qproj` opens a project on startup.
    if let Some(path) = std::env::args_os().nth(1).map(std::path::PathBuf::from) {
        if path.extension().and_then(|e| e.to_str()) == Some("qproj") && path.exists() {
            if let Ok(mut state) = app.cuepool.state().lock() {
                state.command_queue.push(cuepool_gui::AppCommand::OpenProject { path });
            }
        } else {
            log::warn!("Ignoring CLI argument (expected an existing .qproj file): {:?}", path);
        }
    }

    // Ctrl-C / SIGTERM handler for graceful emergency save
    {
        let state = Arc::clone(app.cuepool.state());
        ctrlc::set_handler(move || {
            log::info!("SIGINT received, performing emergency save...");
            emergency_save(&state);
            std::process::exit(0);
        })?;
    }

    event_loop.run_app(&mut app)?;

    // Save persisted settings
    let recent_files = app.cuepool.state().lock().map(|s| s.recent_files.clone()).unwrap_or_default();
    save_settings(&AppSettings { recent_files });

    // Graceful exit (never reached via hard_exit, which process::exit()s):
    // stop and join the consume thread like the render threads.
    app.consume_stop.store(true, Ordering::Relaxed);
    if let Some(join) = app.consume_join.take() {
        let _ = join.join();
    }
    // Signal autosave thread to stop
    app.autosave_running.store(false, Ordering::Relaxed);

    #[cfg(windows)]
    win_timer::release();

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canvas_commands_coalesce_to_latest_state_after_drop() {
        let commands = coalesce_canvas_commands([
            CanvasCommand::Resize { w: 640, h: 480, force: false },
            CanvasCommand::Image("discarded.png".into(), CanvasFit::default()),
            CanvasCommand::Overlay(None),
            CanvasCommand::Drop,
            CanvasCommand::Resize { w: 1280, h: 720, force: false },
            CanvasCommand::BlankCanvas,
            CanvasCommand::Overlay(None),
            CanvasCommand::Resize { w: 1920, h: 1080, force: true },
            CanvasCommand::Image("latest.png".into(), CanvasFit::default()),
            CanvasCommand::Overlay(None),
        ]);

        assert_eq!(commands.len(), 4);
        assert!(matches!(commands[0], CanvasCommand::Drop));
        assert!(matches!(
            commands[1],
            CanvasCommand::Resize { w: 1920, h: 1080, force: true }
        ));
        assert!(matches!(&commands[2], CanvasCommand::Image(path, _) if path == "latest.png"));
        assert!(matches!(commands[3], CanvasCommand::Overlay(None)));
    }

    #[test]
    fn picture_fade_freezes_when_paused_before_or_after_it_starts() {
        let origin = Instant::now();
        let paused_at = origin + Duration::from_secs(2);
        let resumed_at = origin + Duration::from_secs(7);

        assert_eq!(fade_elapsed(origin, Some(paused_at)), Duration::from_secs(2));
        let shifted = shift_fade_start_after_pause(origin, paused_at, resumed_at);
        assert_eq!(resumed_at.duration_since(shifted), Duration::from_secs(2));

        let started_while_paused = origin + Duration::from_secs(4);
        assert_eq!(fade_elapsed(started_while_paused, Some(paused_at)), Duration::ZERO);
        let shifted = shift_fade_start_after_pause(started_while_paused, paused_at, resumed_at);
        assert_eq!(shifted, resumed_at);
    }

    #[test]
    fn test_parse_osc_command_address_only() {
        let msg = parse_osc_command("/qplayer/go").unwrap();
        assert_eq!(msg.addr, "/qplayer/go");
        assert!(msg.args.is_empty());
    }

    #[test]
    fn test_parse_osc_command_with_args() {
        let msg = parse_osc_command("/qplayer/go,5,3.14,hello").unwrap();
        assert_eq!(msg.addr, "/qplayer/go");
        assert_eq!(msg.args.len(), 3);
        assert_eq!(msg.args[0], rosc::OscType::Int(5));
        assert_eq!(msg.args[1], rosc::OscType::Float(3.14));
        assert_eq!(msg.args[2], rosc::OscType::String("hello".into()));
    }

    #[test]
    fn test_parse_osc_command_invalid_address() {
        let err = parse_osc_command("cuepool/go");
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_osc_command_empty() {
        let err = parse_osc_command("");
        assert!(err.is_err());
    }

    #[test]
    fn test_strip_udp_prefix() {
        assert_eq!(strip_udp_prefix("udp:PLAY myfile.mp4"), Some("PLAY myfile.mp4"));
        assert_eq!(strip_udp_prefix("  UDP:stop  "), Some("stop"));
        assert_eq!(strip_udp_prefix("udp:"), Some(""));
        assert_eq!(strip_udp_prefix("/qplayer/go,5"), None);
        assert_eq!(strip_udp_prefix("udp"), None);
        assert_eq!(strip_udp_prefix(""), None);
    }

    #[test]
    fn test_resolve_udp_command() {
        let targets = vec![
            cuepool_core::UdpTarget { name: "left".into(), host: "10.0.0.11".into() },
            cuepool_core::UdpTarget { name: "right".into(), host: "brightsign-right.local".into() },
        ];
        let default = "255.255.255.255";
        fn resolve<'a>(cmd: &'a str, targets: &[cuepool_core::UdpTarget], default: &str) -> (String, &'a str) {
            resolve_udp_command(strip_udp_prefix(cmd).unwrap(), targets, default)
        }

        // Named target hit
        assert_eq!(resolve("udp:left:PLAY a.mp4", &targets, default), ("10.0.0.11".to_string(), "PLAY a.mp4"));
        // Case-insensitive name match
        assert_eq!(resolve("udp:LEFT:stop", &targets, default), ("10.0.0.11".to_string(), "stop"));
        // Hostname target
        assert_eq!(resolve("udp:right:reboot", &targets, default), ("brightsign-right.local".to_string(), "reboot"));
        // Raw IPv4 escape hatch
        assert_eq!(resolve("udp:10.0.0.99:reboot", &targets, default), ("10.0.0.99".to_string(), "reboot"));
        // Unknown name falls back to whole payload + default host
        assert_eq!(resolve("udp:lef:PLAY a.mp4", &targets, default), (default.to_string(), "lef:PLAY a.mp4"));
        // No colon: whole remainder is the payload
        assert_eq!(resolve("udp:PLAY a.mp4", &targets, default), (default.to_string(), "PLAY a.mp4"));
        // Colon-containing filename with no target: unresolved candidate falls back
        assert_eq!(resolve("udp:PLAY C:drive.mp4", &targets, default), (default.to_string(), "PLAY C:drive.mp4"));
        // Empty payload after a resolved target (send_udp_command warns and skips)
        assert_eq!(resolve("udp:left:", &targets, default), ("10.0.0.11".to_string(), ""));
    }

    #[test]
    fn after_last_chain_fires_one_link_at_a_time() {
        use cuepool_core::TriggerMode::{AfterLast, Go};
        let cues = vec![dummy(1, Go), dummy(2, AfterLast), dummy(3, AfterLast), dummy(4, Go)];
        // Each completion fires exactly the next link; a non-AfterLast cue ends it.
        let q = |n: i64| rust_decimal::Decimal::from(n);
        assert_eq!(next_after_last(&cues, q(1)).map(|c| c.base().qid), Some(q(2)));
        assert_eq!(next_after_last(&cues, q(2)).map(|c| c.base().qid), Some(q(3)));
        assert_eq!(next_after_last(&cues, q(3)), None);
        assert_eq!(next_after_last(&cues, q(99)), None);

        // A disabled link is skipped over, not a dead end.
        let mut cues = cues;
        cues[1].base_mut().enabled = false;
        assert_eq!(next_after_last(&cues, q(1)).map(|c| c.base().qid), Some(q(3)));
    }

    fn dummy(qid: i64, trigger: cuepool_core::TriggerMode) -> cuepool_core::Cue {
        cuepool_core::Cue::Dummy {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                trigger,
                ..Default::default()
            },
        }
    }
    fn group(qid: i64) -> cuepool_core::Cue {
        cuepool_core::Cue::Group {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                ..Default::default()
            },
        }
    }
    fn member(qid: i64, group: i64) -> cuepool_core::Cue {
        cuepool_core::Cue::Dummy {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                parent: Some(rust_decimal::Decimal::from(group)),
                ..Default::default()
            },
        }
    }

    #[test]
    fn step_skips_withlast_and_afterlast_chain() {
        use cuepool_core::TriggerMode::*;
        let cues = vec![dummy(1, Go), dummy(2, WithLast), dummy(3, AfterLast), dummy(4, Go)];
        // Going Q1 also auto-fires Q2/Q3 -> next standby is Q4.
        assert_eq!(next_standby_qid(&cues, 0), Some(rust_decimal::Decimal::from(4)));
    }

    #[test]
    fn step_plain_go_advances_by_one() {
        use cuepool_core::TriggerMode::Go;
        let cues = vec![dummy(1, Go), dummy(2, Go), dummy(3, Go)];
        assert_eq!(next_standby_qid(&cues, 0), Some(rust_decimal::Decimal::from(2)));
    }

    #[test]
    fn step_over_group_skips_members() {
        use cuepool_core::TriggerMode::Go;
        // Group Q10 owns Q11/Q12; Q30 is a free cue after the group.
        let cues = vec![group(10), member(11, 10), member(12, 10), dummy(30, Go)];
        // Going group Q10 fires its members -> next standby is the free cue Q30.
        assert_eq!(next_standby_qid(&cues, 0), Some(rust_decimal::Decimal::from(30)));
    }

    #[test]
    fn step_at_end_returns_none() {
        let cues = vec![dummy(1, cuepool_core::TriggerMode::Go)];
        assert_eq!(next_standby_qid(&cues, 0), None);
    }

    fn goto(qid: i64, target: i64) -> cuepool_core::Cue {
        cuepool_core::Cue::Goto {
            base: cuepool_core::CueBase {
                qid: rust_decimal::Decimal::from(qid),
                ..Default::default()
            },
            target_qid: rust_decimal::Decimal::from(target),
        }
    }
    fn dec(n: i64) -> rust_decimal::Decimal {
        rust_decimal::Decimal::from(n)
    }

    #[test]
    fn goto_resolves_direct_target() {
        let cues = vec![goto(1, 2), dummy(2, cuepool_core::TriggerMode::Go)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(2)), Some(dec(2)));
    }

    #[test]
    fn goto_resolves_through_chain() {
        // Q1 -> Q2(goto) -> Q3(real)
        let cues = vec![goto(1, 2), goto(2, 3), dummy(3, cuepool_core::TriggerMode::Go)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(2)), Some(dec(3)));
    }

    #[test]
    fn goto_self_target_is_none() {
        let cues = vec![goto(1, 1)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(1)), None);
    }

    #[test]
    fn goto_cycle_is_none() {
        // Q1 -> Q2 -> Q1 : the bug that crashed (stack overflow); must be None.
        let cues = vec![goto(1, 2), goto(2, 1)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(2)), None);
    }

    #[test]
    fn goto_dead_end_is_none() {
        let cues = vec![goto(1, 99)];
        assert_eq!(resolve_goto_target(&cues, dec(1), dec(99)), None);
    }
}
