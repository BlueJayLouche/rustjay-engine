//! CuePool binary — custom winit event loop with native control, status, and output windows.
//!
//! - Control window: egui UI (replaces eframe)
//! - Video output windows: one render thread per output, each blocking on its
//!   own display's vsync (Fifo) — ungenlocked outputs never serialize.
//! - Audio engine: cpal output with master clock for A/V sync
//! - Video decode: background thread feeding a bounded channel; a consume
//!   thread quantizes uploads to reference-output presents while ticks are
//!   healthy, selecting one refresh ahead against the wall-clock video clock.
//!   Timeout wakes retain wall-clock pacing when ticks stop. All non-egui GPU
//!   work lives on the consume thread (Windows/NVIDIA WSI stalls any thread
//!   that submits behind vsync-blocked swapchains).

use cuepool::{EngineAction, EngineCommand, EngineEvent, ShowEngine};
use cuepool_audio::{AudioEngine, QueueOutput};
use cuepool_core::{
    AudioOutputDriver, CanvasFit, LockExt, MidiTrigger, MidiTriggerKind, SerializedColour,
    TimecodeFrameRate, TimecodeSourceKind, Timespan,
};
use cuepool_gui::app::CueState;
use cuepool_gui::logging::PERSIST_TARGET;
use cuepool_gui::{AppCommand, CuePoolApp, OutputDiagnostics, VideoTimings};
use cuepool_protocols::ltc::LtcGenerator;
use cuepool_protocols::midi::mtc::MtcReceiver;
use cuepool_protocols::midi::{MidiEvent, MidiManager};
use cuepool_protocols::msc::{MscCommandFlags, MscEvent, MscManager};
use cuepool_protocols::osc::{OscEvent, OscManager};
use cuepool_protocols::timecode::{MtcFrameRate, TimecodeSource, TimecodeState};
use cuepool_video::{
    FramePool, HapAcceleration, HapFallbackSession, VideoFrame, ZeroCopyAvailability,
    ZeroCopyPreference,
};
use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use human_panic::Metadata;

mod api;
use api::{ApiCommand, ApiCommandOutcome, ApiRuntime};
mod lighting_engine;
use lighting_engine::LightingEngine;
mod ltc_source;
use ltc_source::{LtcReceiver, to_mtc_frame_rate};
mod mtc_follow;
use mtc_follow::MtcFollowState;
mod output_window;
use output_window::{
    OutputWindow, WindowIds, monitor_descriptor, pack_size, projection_structure_changed,
    unpack_size,
};
mod persist;
use persist::{emergency_save, spawn_autosave_thread};
mod recorder;
use recorder::Recorder;
mod remote_commands;
use remote_commands::{parse_osc_command, resolve_udp_command, send_udp_command, strip_udp_prefix};
mod settings;
use settings::{AppProfile, load_settings, save_settings_from_state};
mod video_pipeline;
mod video_timing;
#[cfg(windows)]
use video_pipeline::win_timer;
use video_pipeline::{
    CanvasCommand, OutputFrameState, VideoControl, VideoMessage, VideoSeekFrameRequest,
    pixmap_decode_thread, video_consume_thread, video_decode_thread,
};
use video_timing::{fade_elapsed, shift_fade_start_after_pause};

/// Decode-channel depth (frames). A small buffer absorbs decode jitter; the
/// backpressure it provides paces decode to the display refresh.
const VIDEO_QUEUE_CAP: usize = 3;
/// Two streams × channel/peek slack × the largest decoded plane count.
const FRAME_POOL_CAP: usize = 2 * (VIDEO_QUEUE_CAP + 2) * 3;
const API_SHUTDOWN_EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// Retries for a failed video open before the cue is given up. Attempts are
/// immediate: a damaged file fails all of them in a few milliseconds, while a
/// storage stall long enough to outlast them is a fault the operator has to
/// know about anyway.
const MAX_VIDEO_OPEN_RETRIES: u32 = 3;

/// Max squared position distance (px²) for recalling an output to a saved monitor.
/// Positions are fixed for an installed wall, so this just allows minor slop while
/// keeping projectors 1920 px apart unambiguous, and leaves an output windowed if
/// its monitor isn't present (rather than grabbing the wrong one).
const MONITOR_MATCH_DIST_SQ: i64 = 200 * 200;

/// Distinct colours for the Identify overlay (one per output window, by order).
const IDENTIFY_COLORS: [wgpu::Color; 6] = [
    wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    }, // red
    wgpu::Color {
        r: 0.0,
        g: 1.0,
        b: 0.0,
        a: 1.0,
    }, // green
    wgpu::Color {
        r: 0.0,
        g: 0.2,
        b: 1.0,
        a: 1.0,
    }, // blue
    wgpu::Color {
        r: 1.0,
        g: 1.0,
        b: 0.0,
        a: 1.0,
    }, // yellow
    wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    }, // magenta
    wgpu::Color {
        r: 0.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    }, // cyan
];
const IDENTIFY_COLOR_NAMES: [&str; 6] = ["RED", "GREEN", "BLUE", "YELLOW", "MAGENTA", "CYAN"];

struct StatusWindow {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
}

impl StatusWindow {
    fn resize(
        &mut self,
        device: &wgpu::Device,
        configure_gate: &RwLock<()>,
        size: winit::dpi::PhysicalSize<u32>,
    ) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.config.width = size.width;
        self.config.height = size.height;
        let _configure_guard = configure_gate.write().unwrap_or_else(|e| e.into_inner());
        self.surface.configure(device, &self.config);
    }

    fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &cuepool_video::SharedQueue,
        configure_gate: &RwLock<()>,
        cuepool: &mut CuePoolApp,
    ) {
        let submit_guard = configure_gate.read().unwrap_or_else(|e| e.into_inner());
        let output = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(output)
            | wgpu::CurrentSurfaceTexture::Suboptimal(output) => output,
            wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                drop(submit_guard);
                let _configure_guard = configure_gate.write().unwrap_or_else(|e| e.into_inner());
                self.surface.configure(device, &self.config);
                return;
            }
            error => {
                log::warn!("Status surface acquire failed: {error:?}");
                return;
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let mut full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| cuepool.show_status(ui));
            });
        });
        self.egui_state
            .handle_platform_output(&self.window, full_output.platform_output);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: self.window.scale_factor() as f32 * self.egui_ctx.zoom_factor(),
        };
        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("status-encoder"),
        });
        // egui 0.36 requires every texture delta to be applied or cleared
        // before TexturesDelta drops (debug assert), so drain rather than
        // iterate by reference.
        for (id, image_deltas) in full_output.textures_delta.set.drain() {
            for image_delta in image_deltas {
                self.egui_renderer
                    .update_texture(device, queue.queue(), id, &image_delta);
            }
        }
        self.egui_renderer.update_buffers(
            device,
            queue.queue(),
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );
        {
            let render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("status-render-pass"),
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
            self.egui_renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }
        queue.submit(std::iter::once(encoder.finish()));
        queue.queue().present(output);
        for id in full_output.textures_delta.free.drain() {
            self.egui_renderer.free_texture(&id);
        }
    }
}

fn configured_audio_error(
    driver: AudioOutputDriver,
    device: &str,
    error: &cuepool_audio::AudioError,
) -> String {
    let device = if device.is_empty() {
        "<default>"
    } else {
        device
    };
    format!("configured {driver} output device '{device}' failed: {error}")
}

fn remote_discovery_message(settings: &cuepool_core::ShowSettings) -> Option<rosc::OscMessage> {
    settings.enable_remote_control.then(|| rosc::OscMessage {
        addr: "/qplayer/remote/discovery".into(),
        args: vec![rosc::OscType::String(settings.node_name.clone())],
    })
}

fn gpu_display_context(
    adapter: &wgpu::Adapter,
    monitor: Option<&cuepool_core::MonitorId>,
) -> String {
    let info = adapter.get_info();
    let monitor = monitor
        .map(cuepool_core::MonitorId::label)
        .unwrap_or_else(|| "windowed/unassigned".into());
    format!(
        "adapter='{}' backend={:?} driver='{}' driver_info='{}'; monitor={monitor}",
        info.name, info.backend, info.driver, info.driver_info
    )
}

fn control_surface_retry_delay(failures: u32) -> Duration {
    let exponent = failures.saturating_sub(1).min(6);
    Duration::from_millis((100_u64 << exponent).min(5_000))
}

/// User events sent to the main event loop from background threads.
#[derive(Debug)]
enum AppEvent {
    /// The consume thread uploaded the stream's final due frame.
    VideoEof(u64),
    /// The decoder terminated without a recoverable frame; unlike EOF this never loops.
    VideoFailed(u64),
    /// An output worker needs winit to recreate its window-owned surface.
    OutputSurfaceLost(WindowId),
    /// The shared GPU device is gone; recovery requires rebuilding all resources.
    DeviceLost,
}

enum TerminalError {
    Startup(String),
    Runtime(&'static str),
}

struct VideoDecodeRequest {
    path: String,
    start_before: Option<f64>,
    seek_frame: Option<VideoSeekFrameRequest>,
    clamp_to_media: bool,
    hap_fallback_session: HapFallbackSession,
}

fn take_ready_video_decode(
    join: &mut Option<std::thread::JoinHandle<()>>,
    pending: &mut Option<VideoDecodeRequest>,
) -> Option<VideoDecodeRequest> {
    if join.as_ref().is_some_and(|join| !join.is_finished()) {
        return None;
    }
    if let Some(join) = join.take() {
        let _ = join.join();
    }
    pending.take()
}

fn queue_latest_video_decode(
    pending: &mut Option<VideoDecodeRequest>,
    request: VideoDecodeRequest,
) {
    *pending = Some(request);
}

fn clamp_video_seek_secs(target: f64, length_secs: Option<f64>) -> f64 {
    match length_secs.filter(|length| length.is_finite() && *length > 0.0) {
        Some(length) => target.min(length.next_down()),
        None if target.is_finite() => target,
        None => 0.0,
    }
}

fn video_media_secs(timeline_secs: f64, media_offset_secs: f64) -> f64 {
    timeline_secs + media_offset_secs.max(0.0)
}

fn video_timeline_secs(media_secs: f64, media_offset_secs: f64) -> f64 {
    (media_secs - media_offset_secs.max(0.0)).max(0.0)
}

fn video_seek_clock(
    now: Instant,
    target_secs: f64,
    paused: bool,
) -> Option<(Instant, Option<Instant>)> {
    let target = Duration::try_from_secs_f64(target_secs).ok()?;
    let clock = now.checked_sub(target)?;
    Some((clock, paused.then_some(now)))
}

fn video_start_clock(
    now: Instant,
    media_offset_secs: f64,
    engine_now: Duration,
    clock_origin: Duration,
) -> Option<Instant> {
    let elapsed = engine_now.saturating_sub(clock_origin).as_secs_f64();
    video_seek_clock(now, video_media_secs(elapsed, media_offset_secs), false)
        .map(|(clock, _)| clock)
}

/// Build the chase source selected by the project settings: MTC (listens on
/// all MIDI ports) or LTC (decodes from one channel of one audio input
/// device).
fn build_timecode_source(
    config: &(TimecodeSourceKind, AudioOutputDriver, String, u16),
) -> Box<dyn TimecodeSource> {
    match config.0 {
        TimecodeSourceKind::Mtc => Box::new(MtcReceiver::new()),
        TimecodeSourceKind::Ltc => Box::new(LtcReceiver::new(config.1, &config.2, config.3)),
    }
}

/// Open the configured LTC output. `None` when disabled or when the device
/// can't be opened right now (the per-tick path retries on a throttle).
fn open_ltc_output(
    config: &(bool, AudioOutputDriver, String, u16, TimecodeFrameRate, f64),
) -> Option<(QueueOutput, LtcGenerator)> {
    let (enabled, driver, device, channel, fps, start) = config;
    if !enabled {
        return None;
    }
    match QueueOutput::start(*driver, device, *channel) {
        Ok(out) => {
            log::info!(
                "[LTC-out] Generating {} on '{}' (start {:.2}s)",
                fps.name(),
                out.device_name(),
                start
            );
            let generator = LtcGenerator::new(out.sample_rate(), to_mtc_frame_rate(*fps), *start);
            Some((out, generator))
        }
        Err(e) => {
            log::warn!("[LTC-out] Cannot open output: {e}");
            None
        }
    }
}

struct App {
    // ── wgpu core ──
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    /// All submissions route through this serialized wrapper so a zero-copy
    /// decode-fence wait can never attach to another thread's submission.
    queue: Arc<cuepool_video::SharedQueue>,
    /// `Surface::configure` takes this exclusively; GPU queue/present cycles
    /// take it shared. Lock it without `VideoControl` or `frame_state` held;
    /// configure paths never take either user mutex while holding the gate.
    configure_gate: Arc<RwLock<()>>,

    // ── control window (egui) ──
    control_window: Option<Arc<Window>>,
    control_surface: Option<wgpu::Surface<'static>>,
    control_config: Option<wgpu::SurfaceConfiguration>,
    status_window: Option<StatusWindow>,
    control_surface_retry_at: Option<Instant>,
    control_surface_retry_failures: u32,

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
    /// Monotonic present counter + wakeup from reference output 0.
    vsync_tick: Arc<(Mutex<u64>, Condvar)>,
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
    profile: AppProfile,
    show_engine: ShowEngine,
    engine_epoch: Instant,
    window_ids: Option<WindowIds>,
    terminal_error: Option<TerminalError>,
    shutdown_started_at: Option<Instant>,

    // ── playback adapter state ──
    paused: bool,

    // ── video playback ──
    /// Kept for render threads to request winit-side surface rebuilds; the
    /// consume thread gets its own clone at construction.
    event_loop_proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    /// The decode channel, clock, pause/step/fade state and frame consumption
    /// live in `video_control` (shared with the consume thread, which owns the
    /// canvas upload + convert + publish path). The decode thread is a bounded
    /// producer; backpressure keeps decode a few frames ahead of the clock.
    video_stop_flag: Arc<AtomicBool>,
    video_decode_join: Option<std::thread::JoinHandle<()>>,
    pending_video_decode: Option<VideoDecodeRequest>,
    video_pause_flag: Arc<AtomicBool>,
    frame_pool: Arc<FramePool>,
    zero_copy: ZeroCopyAvailability,
    hap_acceleration: HapAcceleration,
    hap_fallback_session: HapFallbackSession,
    hap_fallback_instance_id: Option<u64>,
    /// Consecutive failed opens of the current video. A storage hiccup or a
    /// file briefly unavailable retries; a genuinely damaged file gives up
    /// after MAX_VIDEO_OPEN_RETRIES and tells the operator.
    video_open_retries: u32,
    /// QID of the cue whose video is currently playing (for loop sync).
    current_video_qid: Option<rust_decimal::Decimal>,
    current_video_instance_id: Option<u64>,
    /// Last `SharedState.project_generation` we acted on. A change means a project
    /// was loaded, so the output windows must rebuild for its projection settings.
    last_project_generation: u64,
    /// Throttle the engine-owned active-cue snapshot shared with UI and API readers.
    last_active_cue_publish: Instant,
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
    /// Per-second max duration of one `about_to_wait` pass and one
    /// `render_control` pass — splits a slow main loop into engine-tick time
    /// versus egui/WSI time. Reset at the 1 Hz diagnostics flush.
    dbg_about_max_us: u32,
    dbg_render_max_us: u32,
    dbg_render_count: u32,
    /// Present mode the control surface actually negotiated (the outputs table
    /// only covers projector surfaces, and this one decides whether the winit
    /// thread blocks on vsync).
    control_present_mode: String,

    // ── localhost automation API ──
    api: Option<ApiRuntime>,

    // ── protocols ──
    osc_manager: Option<OscManager>,
    osc_rx: Option<std::sync::mpsc::Receiver<OscEvent>>,
    #[allow(dead_code)]
    msc_manager: Option<MscManager>,
    msc_rx: Option<std::sync::mpsc::Receiver<MscEvent>>,
    midi_manager: Option<MidiManager>,
    last_discovery: Instant,

    // ── Timecode follow ──
    /// The active timecode source the show chases (MTC or LTC, selected in
    /// project settings).
    timecode_source: Box<dyn TimecodeSource>,
    /// The settings `timecode_source` was built from — a settings edit or a
    /// project load rebuilds the source when this no longer matches.
    /// (source kind, LTC driver, LTC device, LTC channel).
    timecode_config: (TimecodeSourceKind, AudioOutputDriver, String, u16),
    /// Last audio-input device scan for the settings window's LTC device list.
    last_input_scan: Instant,

    // ── LTC generate ──
    /// Active LTC output: queue-fed stream + show-clock encoder.
    ltc_out: Option<(QueueOutput, LtcGenerator)>,
    /// The settings `ltc_out` was built from:
    /// (enabled, driver, device, channel, fps, start).
    ltc_out_config: (bool, AudioOutputDriver, String, u16, TimecodeFrameRate, f64),
    /// Retry throttle while the configured LTC output device is unavailable.
    ltc_out_retry: Instant,
    /// Encode scratch, kept out of the per-frame path.
    ltc_scratch: Vec<f32>,
    /// The video cue currently following MTC, if any.
    mtc_follow: Option<MtcFollowState>,
    /// Last measured drift (target − video position) while following, for the GUI.
    mtc_drift: Option<f64>,
    /// Last frame rate we warned about (rate-limits the non-25fps warning).
    mtc_warned_fps: Option<MtcFrameRate>,
    /// When the last hard sync reopened the media, so a scrubbing source
    /// cannot drive a continuous stream of container opens.
    last_hard_sync: Option<Instant>,

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
    pixmap_hap: Option<cuepool_video::HapConverter>,
    pixmap_frame_rx: Option<std::sync::mpsc::Receiver<VideoFrame>>,
    pixmap_stop_flag: Arc<AtomicBool>,
    /// Shared with the pixmap decode thread and kept for the app's lifetime, so
    /// a cue fired while the show is paused starts paused too.
    pixmap_pause_flag: Arc<AtomicBool>,
    current_pixmap_qid: Option<rust_decimal::Decimal>,

    // ── trigger state ──
    wall_clock_fired: std::collections::HashMap<rust_decimal::Decimal, Instant>,
    timecode_fired: std::collections::HashSet<rust_decimal::Decimal>,

    // ── polish ──
    last_window_title: String,
    autosave_running: Arc<AtomicBool>,
    modifiers: winit::keyboard::ModifiersState,
    // ── plugins ──
}

fn drain_app_commands(
    state: &cuepool_gui::SharedStateHandle,
    mut dispatch: impl FnMut(AppCommand) -> Result<(), AppCommand>,
) {
    let commands = {
        let Ok(mut state) = state.lock() else { return };
        std::mem::take(&mut state.command_queue)
    };
    let unhandled: Vec<_> = commands
        .into_iter()
        .filter_map(|command| dispatch(command).err())
        .collect();
    if !unhandled.is_empty()
        && let Ok(mut state) = state.lock()
    {
        state.command_queue.extend(unhandled);
    }
}

enum ShowControlCommand {
    OpenProject(Box<cuepool_gui::PreparedProject>),
    SelectCue(rust_decimal::Decimal),
    Go,
    Stop,
    Pause,
    Resume,
    Preload,
    Seek { instance_id: u64, seconds: f32 },
    Shutdown,
}

/// The window icon, decoded once from the mark shipped in `packaging/`.
///
/// Windows and Linux read this for the taskbar and title bar; the packaged
/// `.ico` only reaches the Start-menu shortcut. macOS ignores it and takes the
/// bundle's `AppIcon.icns`. A missing or corrupt icon is not worth refusing to
/// launch over, so this degrades to the platform default.
fn window_icon() -> Option<winit::window::Icon> {
    const ICON_PNG: &[u8] = include_bytes!("../../../packaging/window-icon.png");
    let image = image::load_from_memory(ICON_PNG)
        .inspect_err(|error| log::warn!("window icon failed to decode: {error}"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    winit::window::Icon::from_rgba(image.into_raw(), width, height)
        .inspect_err(|error| log::warn!("window icon rejected by winit: {error}"))
        .ok()
}

impl App {
    // ponytail: Keep startup wiring explicit; introduce a GPU context only if more state is added.
    #[allow(clippy::too_many_arguments)]
    fn new(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: Arc<cuepool_video::SharedQueue>,
        proxy: winit::event_loop::EventLoopProxy<AppEvent>,
        zero_copy: ZeroCopyAvailability,
        hap_acceleration: HapAcceleration,
        cuepool: CuePoolApp,
        profile: AppProfile,
    ) -> Self {
        let show_engine = ShowEngine::new(cuepool.state().clone(), None);
        // Protocol settings from project settings (fallback to defaults)
        let (tx_host, osc_rx_port, osc_tx_port, is_remote_host, enable_remote_control) = {
            match cuepool.state().lock() {
                Ok(state) => {
                    let settings = &state.show_file.show_settings;
                    // An unparseable destination is a typo, not a preference —
                    // say so rather than falling back silently, which is how
                    // #213 stayed hidden for months.
                    let tx_host = settings
                        .osc_tx_host
                        .parse::<Ipv4Addr>()
                        .unwrap_or_else(|_| {
                            log::warn!(
                                "OSC destination '{}' is not an IPv4 address; falling back to \
                                 loopback. Outbound OSC will not leave this machine.",
                                settings.osc_tx_host
                            );
                            Ipv4Addr::LOCALHOST
                        });
                    let rx = settings.osc_rx_port as u16;
                    let tx = settings.osc_tx_port as u16;
                    // Port flipping: if remote control enabled and NOT host, swap ports
                    let (rx, tx) = if settings.enable_remote_control && !settings.is_remote_host {
                        (tx, rx)
                    } else {
                        (rx, tx)
                    };
                    (
                        tx_host,
                        rx,
                        tx,
                        settings.is_remote_host,
                        settings.enable_remote_control,
                    )
                }
                Err(_) => (Ipv4Addr::LOCALHOST, 9000u16, 9001u16, true, false),
            }
        };

        let (osc_manager, osc_rx) = {
            let (tx, rx) = std::sync::mpsc::channel();
            match OscManager::new(tx_host, osc_rx_port, osc_tx_port, tx) {
                Ok(m) => {
                    log::info!(
                        "OSC manager listening on 0.0.0.0:{}, sending to {}:{}, \
                         remote_control={} is_host={}",
                        osc_rx_port,
                        tx_host,
                        osc_tx_port,
                        enable_remote_control,
                        is_remote_host
                    );
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
            match MscManager::new(tx_host, 7000, 7001, tx.clone()) {
                Ok(m) => {
                    log::info!("MSC manager listening on 0.0.0.0:7000, sending to {tx_host}:7001");
                    // Wire default MSC subscriptions
                    m.subscribe(
                        MscCommandFlags::GO | MscCommandFlags::TIMED_GO,
                        move |pkt| {
                            let event = match &pkt.data {
                                cuepool_protocols::msc::MscData::Go {
                                    qid,
                                    executor,
                                    page,
                                } => Some(MscEvent::Go {
                                    qid: qid.clone(),
                                    executor: *executor,
                                    page: *page,
                                }),
                                cuepool_protocols::msc::MscData::TimedGo {
                                    qid,
                                    executor,
                                    page,
                                    time,
                                } => Some(MscEvent::TimedGo {
                                    qid: qid.clone(),
                                    executor: *executor,
                                    page: *page,
                                    time: *time,
                                }),
                                _ => None,
                            };
                            if let Some(ev) = event {
                                let _ = tx.send(ev);
                            }
                        },
                    );
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
        spawn_autosave_thread(Arc::clone(cuepool.state()), Arc::clone(&autosave_running));

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
            d.app_version = cuepool_gui::build_identity();
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
            for var in [
                "RUST_LOG",
                "QPLAYER_PRESENT_MODE",
                "QPLAYER_FPS_DEBUG",
                "QPLAYER_NO_HWACCEL",
                "QPLAYER_ZEROCOPY",
            ] {
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
        let vsync_tick = Arc::new((Mutex::new(0u64), Condvar::new()));
        let configure_gate = Arc::new(RwLock::new(()));
        let (canvas_cmd_tx, canvas_cmd_rx) = std::sync::mpsc::channel::<CanvasCommand>();
        let consume_stop = Arc::new(AtomicBool::new(false));
        let frame_pool = Arc::new(FramePool::new(FRAME_POOL_CAP));
        let consume_join = {
            let device = device.clone();
            let queue = queue.clone();
            let control = Arc::clone(&video_control);
            let frame = Arc::clone(&frame_state);
            let vsync_tick = Arc::clone(&vsync_tick);
            let configure_gate = Arc::clone(&configure_gate);
            let stop = Arc::clone(&consume_stop);
            let frame_pool = Arc::clone(&frame_pool);
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
                        vsync_tick,
                        canvas_cmd_rx,
                        proxy,
                        stop,
                        frame_pool,
                    )
                })
                .expect("spawn video consume thread")
        };

        let api = match api::start(Arc::clone(cuepool.state()), profile.name().into()) {
            Ok(api) => Some(api),
            Err(error) => {
                log::error!("CuePool API unavailable: {error}");
                None
            }
        };

        let (timecode_config, ltc_out_config) = {
            let state = cuepool.state().lock_unpoisoned();
            let settings = &state.show_file.show_settings;
            (
                (
                    settings.timecode_source,
                    settings.ltc_input_driver,
                    settings.ltc_input_device.clone(),
                    settings.ltc_input_channel,
                ),
                (
                    settings.ltc_output_enabled,
                    settings.ltc_output_driver,
                    settings.ltc_output_device.clone(),
                    settings.ltc_output_channel,
                    settings.ltc_output_fps,
                    settings.ltc_output_start.as_secs_f64(),
                ),
            )
        };
        let timecode_source = build_timecode_source(&timecode_config);
        let ltc_out = open_ltc_output(&ltc_out_config);

        let mut app = Self {
            instance,
            adapter,
            device,
            queue,
            configure_gate,
            control_window: None,
            control_surface: None,
            control_config: None,
            status_window: None,
            control_surface_retry_at: None,
            control_surface_retry_failures: 0,
            egui_ctx,
            egui_state: None,
            registered_fonts: std::collections::HashSet::new(),
            egui_renderer: None,
            cuepool,
            profile,
            show_engine,
            engine_epoch: Instant::now(),
            window_ids: None,
            terminal_error: None,
            shutdown_started_at: None,
            event_loop_proxy: proxy,
            current_text_qid: None,
            output_windows: Vec::new(),
            output_windows_built_from: None,
            frame_state,
            video_control,
            vsync_tick,
            canvas_cmd_tx,
            consume_stop,
            consume_join: Some(consume_join),
            consume_failure_reported: false,
            published_outputs: Vec::new(),
            video_stop_flag: Arc::new(AtomicBool::new(false)),
            video_decode_join: None,
            pending_video_decode: None,
            video_pause_flag: Arc::new(AtomicBool::new(false)),
            frame_pool,
            zero_copy,
            hap_acceleration,
            hap_fallback_session: HapFallbackSession::default(),
            hap_fallback_instance_id: None,
            video_open_retries: 0,
            current_video_qid: None,
            current_video_instance_id: None,
            last_project_generation: 0,
            last_active_cue_publish: Instant::now() - Duration::from_secs(1),
            last_control_redraw: std::time::Instant::now(),
            last_monitor_set: Vec::new(),
            last_monitor_check: std::time::Instant::now(),
            identify_until: None,
            fps_debug: std::env::var("QPLAYER_FPS_DEBUG").is_ok(),
            dbg_ticks: 0,
            dbg_last_log: std::time::Instant::now(),
            dbg_about_max_us: 0,
            dbg_render_max_us: 0,
            dbg_render_count: 0,
            control_present_mode: String::new(),
            api,
            osc_manager,
            osc_rx,
            msc_manager,
            msc_rx,
            midi_manager,
            last_discovery: Instant::now(),
            timecode_source,
            timecode_config,
            last_input_scan: Instant::now() - Duration::from_secs(10),
            ltc_out,
            ltc_out_config,
            // Try again on the first tick if startup open failed.
            ltc_out_retry: Instant::now() - Duration::from_secs(10),
            ltc_scratch: Vec::new(),
            mtc_follow: None,
            mtc_drift: None,
            mtc_warned_fps: None,
            last_hard_sync: None,
            last_window_title: String::new(),
            autosave_running,
            paused: false,
            modifiers: winit::keyboard::ModifiersState::empty(),
            lighting: LightingEngine::default(),
            recorder: Recorder::new(),
            pixel_sampler: None,
            last_pixel_sample: Instant::now(),
            pixmap_texture: None,
            pixmap_yuv: None,
            pixmap_hap: None,
            pixmap_frame_rx: None,
            pixmap_stop_flag: Arc::new(AtomicBool::new(false)),
            pixmap_pause_flag: Arc::new(AtomicBool::new(false)),
            current_pixmap_qid: None,
            wall_clock_fired: std::collections::HashMap::new(),
            timecode_fired: std::collections::HashSet::new(),
        };
        app.apply_audio_settings();
        app
    }

    fn create_configured_control_surface(
        &self,
        window: &Arc<Window>,
        required_format: Option<wgpu::TextureFormat>,
    ) -> Result<(wgpu::Surface<'static>, wgpu::SurfaceConfiguration), String> {
        let monitor = window
            .current_monitor()
            .map(|monitor| monitor_descriptor(&monitor));
        let context = gpu_display_context(&self.adapter, monitor.as_ref());
        let surface = self
            .instance
            .create_surface(Arc::clone(window))
            .map_err(|error| format!("control surface creation failed: {error}; {context}"))?;

        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return Err(format!(
                "control surface configuration failed: window size is {}x{}; {context}",
                size.width, size.height
            ));
        }
        let mut config = surface
            .get_default_config(&self.adapter, size.width, size.height)
            .ok_or_else(|| {
                format!(
                    "control surface configuration failed: no supported configuration for {}x{}; {context}",
                    size.width, size.height
                )
            })?;
        let caps = surface.get_capabilities(&self.adapter);
        if let Some(format) = required_format {
            if !caps.formats.contains(&format) {
                return Err(format!(
                    "control surface configuration failed: original format {format:?} is no longer supported; {context}"
                ));
            }
            config.format = format;
        }
        // Non-vsync present for the CONTROL window: on this single-threaded loop a
        // vsync-blocked control present serializes with the output window's vsync
        // present and roughly halves the output's effective frame rate. Tearing on
        // the operator GUI is irrelevant; output windows keep Fifo for clean playback.
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

        Ok((surface, config))
    }

    /// Create the control window + surface + egui state.
    fn create_control_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let monitor = event_loop
            .primary_monitor()
            .or_else(|| event_loop.available_monitors().next())
            .map(|monitor| monitor_descriptor(&monitor));
        let context = gpu_display_context(&self.adapter, monitor.as_ref());
        let window = Arc::new(
            event_loop
                .create_window(
                    winit::window::WindowAttributes::default()
                        .with_title("CuePool")
                        .with_window_icon(window_icon())
                        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0)),
                )
                .map_err(|error| format!("control window creation failed: {error}; {context}"))?,
        );

        let (surface, config) = self.create_configured_control_surface(&window, None)?;

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
        self.control_present_mode = format!("{:?}", config.present_mode);
        self.control_window = Some(window);
        self.control_surface = Some(surface);
        self.control_config = Some(config);
        self.egui_state = Some(egui_state);
        self.egui_renderer = Some(egui_renderer);

        self.window_ids = Some(WindowIds {
            control: control_id,
            video: Vec::new(),
        });
        Ok(())
    }

    fn retry_control_surface(&mut self) {
        let Some(retry_at) = self.control_surface_retry_at else {
            return;
        };
        if retry_at > Instant::now() {
            return;
        }
        let Some(window) = self.control_window.as_ref().cloned() else {
            return;
        };
        let Some(format) = self.control_config.as_ref().map(|config| config.format) else {
            return;
        };

        match self.create_configured_control_surface(&window, Some(format)) {
            Ok((surface, config)) => {
                let failures = self.control_surface_retry_failures;
                self.control_present_mode = format!("{:?}", config.present_mode);
                self.control_surface = Some(surface);
                self.control_config = Some(config);
                self.control_surface_retry_at = None;
                self.control_surface_retry_failures = 0;
                let monitor = window
                    .current_monitor()
                    .map(|monitor| monitor_descriptor(&monitor));
                let context = gpu_display_context(&self.adapter, monitor.as_ref());
                if failures == 0 {
                    log::info!("Control surface recovered immediately; {context}");
                } else {
                    log::info!(
                        "Control surface recovered after {failures} failed attempt(s); {context}"
                    );
                }
            }
            Err(error) => {
                self.control_surface_retry_failures =
                    self.control_surface_retry_failures.saturating_add(1);
                let failures = self.control_surface_retry_failures;
                let delay = control_surface_retry_delay(failures);
                self.control_surface_retry_at = Some(Instant::now() + delay);
                log::error!(
                    "Control surface recovery attempt {failures} failed: {error}; retrying in {} ms",
                    delay.as_millis()
                );
            }
        }
    }

    fn create_status_window(&mut self, event_loop: &ActiveEventLoop) -> anyhow::Result<()> {
        let window = Arc::new(
            event_loop.create_window(
                winit::window::WindowAttributes::default()
                    .with_title("CuePool Status")
                    .with_inner_size(winit::dpi::LogicalSize::new(460.0, 600.0))
                    .with_min_inner_size(winit::dpi::LogicalSize::new(360.0, 320.0)),
            )?,
        );
        let surface = self.instance.create_surface(Arc::clone(&window))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&self.adapter, size.width, size.height)
            .ok_or_else(|| anyhow::anyhow!("no compatible Status surface configuration"))?;
        let capabilities = surface.get_capabilities(&self.adapter);
        if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Mailbox)
        {
            config.present_mode = wgpu::PresentMode::Mailbox;
        } else if capabilities
            .present_modes
            .contains(&wgpu::PresentMode::Immediate)
        {
            config.present_mode = wgpu::PresentMode::Immediate;
        }
        {
            let _configure_guard = self
                .configure_gate
                .write()
                .unwrap_or_else(|e| e.into_inner());
            surface.configure(&self.device, &config);
        }

        let egui_ctx = egui::Context::default();
        egui_ctx.set_global_style((*self.egui_ctx.global_style()).clone());
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
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
        window.request_redraw();
        self.status_window = Some(StatusWindow {
            window,
            surface,
            config,
            egui_ctx,
            egui_state,
            egui_renderer,
        });
        Ok(())
    }

    fn engine_now(&self) -> Duration {
        self.show_engine
            .audio_engine()
            .map(AudioEngine::playback_time)
            .unwrap_or_else(|| self.engine_epoch.elapsed())
    }

    fn run_engine_command(
        &mut self,
        command: EngineCommand,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), String> {
        let actions = self.show_engine.command(command, self.engine_now());
        self.apply_engine_actions(actions, event_loop)
    }

    fn apply_engine_actions(
        &mut self,
        actions: Vec<EngineAction>,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), String> {
        for action in actions {
            match action {
                EngineAction::PlayVideo {
                    qid,
                    instance_id,
                    clock_origin,
                    path,
                    start_time,
                    duration,
                    follow_mtc,
                    mtc_start,
                    ..
                } => {
                    self.play_video(
                        &path,
                        qid,
                        instance_id,
                        clock_origin,
                        start_time,
                        duration,
                        event_loop,
                    );
                    if follow_mtc {
                        self.mtc_follow = Some(MtcFollowState {
                            qid,
                            path,
                            offset_secs: mtc_start.as_secs_f64(),
                            hold_position: Some(0.0),
                            last_tick: Instant::now(),
                            last_mtc_secs: 0.0,
                            last_mtc_at: Instant::now(),
                        });
                    } else {
                        self.mtc_follow = None;
                    }
                }
                EngineAction::SeekVideo {
                    qid,
                    path,
                    target_secs,
                    media_offset_secs,
                    ..
                } => self.seek_video_cue(qid, &path, target_secs, media_offset_secs)?,
                EngineAction::StopVideo { fade_out_secs } => {
                    if fade_out_secs > 0.0 {
                        self.video_control.lock_unpoisoned().fade =
                            Some((Instant::now(), fade_out_secs));
                    } else {
                        self.stop_video_playback();
                        let _ = self.canvas_cmd_tx.send(CanvasCommand::BlankCanvas);
                    }
                }
                EngineAction::SetVideoPaused(paused) => self.set_video_paused(paused),
                EngineAction::FireExternal(cue) => self.apply_external_cue(&cue, event_loop)?,
                EngineAction::StopExternal {
                    qid,
                    mode,
                    fade_out_secs,
                    fade_type,
                } => {
                    if self.lighting.stop_show(qid, fade_out_secs, fade_type) {
                        log::info!("Stop DmxShow Q{qid} (fade {fade_out_secs:.2}s)");
                    }
                    if self.current_text_qid == Some(qid) {
                        self.clear_text_overlay();
                    }
                    if self.current_pixmap_qid == Some(qid)
                        && mode != cuepool_core::StopMode::LoopEnd
                    {
                        self.stop_pixmap();
                    }
                    if self.current_video_qid == Some(qid)
                        && self.show_engine.current_video_qid() != Some(qid)
                        && mode != cuepool_core::StopMode::LoopEnd
                    {
                        if fade_out_secs > 0.0 {
                            self.video_control.lock_unpoisoned().fade =
                                Some((Instant::now(), fade_out_secs));
                        } else {
                            self.stop_video_playback();
                        }
                    }
                }
                EngineAction::StopAllExternal => self.stop_external_outputs(),
                EngineAction::RemoteGo { node, qid } => {
                    if let Some(osc) = &self.osc_manager {
                        let _ = osc.send(rosc::OscMessage {
                            addr: "/qplayer/remote/go".into(),
                            args: vec![
                                rosc::OscType::String(node),
                                rosc::OscType::String(qid.to_string()),
                            ],
                        });
                    }
                }
                EngineAction::Trace(event) => log::debug!("Engine: {event:?}"),
            }
        }
        Ok(())
    }

    fn set_video_paused(&mut self, paused: bool) {
        self.video_pause_flag.store(paused, Ordering::Relaxed);
        // The pixel map is a separate stream with its own clock; the show
        // transport pauses both together.
        self.pixmap_pause_flag.store(paused, Ordering::Relaxed);
        self.paused = paused;
        let mut ctl = self.video_control.lock_unpoisoned();
        ctl.paused = paused;
        if paused {
            ctl.pause_started = Some(Instant::now());
        } else if let Some(paused_at) = ctl.pause_started.take() {
            let resumed_at = Instant::now();
            if let Some(clock) = ctl.clock.as_mut() {
                *clock += resumed_at.saturating_duration_since(paused_at);
            }
            if let Some((start, _)) = ctl.fade.as_mut() {
                *start = shift_fade_start_after_pause(*start, paused_at, resumed_at);
            }
        }
    }

    fn stop_external_outputs(&mut self) {
        self.mtc_follow = None;
        self.lighting.stop_fade();
        self.lighting.stop_all_shows();
        self.stop_pixmap();
        self.clear_text_overlay();
    }

    /// End the pixel-map stream and blank its texture so the LEDs go dark,
    /// mirroring how stopping a video cue clears the canvas.
    fn stop_pixmap(&mut self) {
        self.pixmap_stop_flag.store(true, Ordering::Relaxed);
        self.pixmap_frame_rx = None;
        self.current_pixmap_qid = None;
        if let Some(texture) = self.pixmap_texture.as_ref() {
            let blank = vec![0; (texture.width * texture.height * 4) as usize];
            let _configure_guard = self
                .configure_gate
                .read()
                .unwrap_or_else(|error| error.into_inner());
            texture.upload_rgba(self.queue.queue(), &blank);
        }
    }

    fn apply_external_cue(
        &mut self,
        cue: &cuepool_core::Cue,
        event_loop: &ActiveEventLoop,
    ) -> Result<(), String> {
        let qid = cue.base().qid;
        match cue {
            cuepool_core::Cue::Osc { command, .. } => {
                if let Some(remainder) = strip_udp_prefix(command) {
                    let (host, port, payload) = {
                        let state = self.cuepool.state().lock_unpoisoned();
                        let settings = &state.show_file.show_settings;
                        let (host, payload) = resolve_udp_command(
                            remainder,
                            &settings.udp_targets,
                            &settings.udp_tx_host,
                        );
                        (host, settings.udp_tx_port, payload.to_string())
                    };
                    send_udp_command(&payload, &host, port)
                        .map_err(|error| format!("UDP send to {host}:{port} failed: {error}"))?;
                } else if let Some(osc) = &self.osc_manager {
                    let message = parse_osc_command(command)
                        .map_err(|error| format!("invalid OSC command: {error}"))?;
                    osc.send(message)
                        .map_err(|error| format!("OSC send failed: {error}"))?;
                } else {
                    return Err("OSC manager is unavailable".into());
                }
            }
            cuepool_core::Cue::Text {
                text,
                font_size,
                font_colour,
                fit,
                font,
                ..
            } => {
                self.ensure_outputs_and_canvas(event_loop);
                let family = self.text_font_family(font);
                let blank = self.current_video_qid.is_none()
                    && !self.video_control.lock_unpoisoned().canvas_has_frame;
                if blank {
                    let _ = self.canvas_cmd_tx.send(CanvasCommand::BlankCanvas);
                }
                let (width, height) = {
                    let state = self.cuepool.state().lock_unpoisoned();
                    (
                        state.show_file.projection.canvas_width,
                        state.show_file.projection.canvas_height,
                    )
                };
                let shown = if let Some(frame) = self.rasterize_text_block(
                    text,
                    *font_size,
                    *font_colour,
                    family,
                    width,
                    height,
                    *fit,
                ) {
                    let _ = self
                        .canvas_cmd_tx
                        .send(CanvasCommand::Overlay(Some((frame, *fit))));
                    true
                } else {
                    let _ = self.canvas_cmd_tx.send(CanvasCommand::Overlay(None));
                    false
                };
                self.set_current_text_qid(shown.then_some(qid));
            }
            cuepool_core::Cue::Image { path, fit, .. } => {
                self.ensure_outputs_and_canvas(event_loop);
                self.video_stop_flag.store(true, Ordering::Relaxed);
                self.pending_video_decode = None;
                {
                    let mut control = self.video_control.lock_unpoisoned();
                    control.stream_epoch += 1;
                    control.clock = None;
                    control.frame_rx = None;
                    control.peek_pts = None;
                    control.last_pts = None;
                }
                self.set_current_video_qid(Some(qid));
                let resolved = self.resolve_path(path).unwrap_or_else(|| path.clone());
                let _ = self
                    .canvas_cmd_tx
                    .send(CanvasCommand::Image(resolved, *fit));
            }
            cuepool_core::Cue::PixelMap { path, .. } => {
                self.play_pixmap(path, qid, cue.base().loop_mode)
            }
            cuepool_core::Cue::Lighting {
                snapshot,
                fade_time,
                fade_type,
                ..
            } => {
                self.lighting.go(snapshot, *fade_time, *fade_type);
            }
            cuepool_core::Cue::DmxShow {
                path,
                fade_in,
                fade_out,
                fade_type,
                priority,
                ..
            } => {
                let resolved = self.resolve_path(path).unwrap_or_else(|| path.clone());
                match rustjay_lighting::read_rec(&resolved) {
                    Ok(events) => self.lighting.go_show(
                        qid,
                        events,
                        *priority,
                        *fade_in,
                        *fade_out,
                        *fade_type,
                        cue.base().loop_mode,
                        cue.base().loop_count,
                    ),
                    Err(error) => {
                        log::error!("DmxShow cue Q{qid} failed to load '{resolved}': {error}");
                        return Err(format!("DmxShow cue Q{qid} failed to load: {error}"));
                    }
                }
            }
            cuepool_core::Cue::Dummy { .. } => {}
            _ => log::debug!("Engine emitted a non-external cue action for Q{qid}"),
        }
        Ok(())
    }

    /// Handle a `Go` command: start audio (and video if cue is VideoCue).
    /// Also handles `WithLast` trigger mode for subsequent cues.
    fn handle_go(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        if self
            .cuepool
            .state()
            .lock_unpoisoned()
            .selected_cue()
            .is_none()
        {
            return Err("no cue is selected".into());
        }
        self.run_engine_command(EngineCommand::Go, event_loop)
    }

    /// Look up a cue by QID and play it. Used by MIDI/hotkey/wall-clock/timecode triggers.
    fn play_cue_by_qid(&mut self, qid: rust_decimal::Decimal, event_loop: &ActiveEventLoop) {
        if let Err(error) = self.run_engine_command(EngineCommand::Fire(qid), event_loop) {
            log::error!("Cue Q{qid} failed: {error}");
        }
    }

    /// Play media into the dedicated pixel-map texture. Stills upload once;
    /// videos get a self-paced decode thread (wall-clock PTS — no A/V sync or
    /// vsync consumer here, LEDs don't need it).
    fn play_pixmap(
        &mut self,
        path: &str,
        qid: rust_decimal::Decimal,
        loop_mode: cuepool_core::LoopMode,
    ) {
        // Replace any previous pixmap stream (per-thread flag, same reasoning
        // as play_video).
        self.pixmap_stop_flag.store(true, Ordering::Relaxed);
        self.pixmap_stop_flag = Arc::new(AtomicBool::new(false));
        self.pixmap_frame_rx = None;
        self.current_pixmap_qid = Some(qid);

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
                self.queue.queue(),
                &VideoFrame::new(w, h, img.into_raw(), 0.0),
                cuepool_core::CanvasFit::Stretch, // same dims → exact copy
            );
            return;
        }

        // Video → decode thread feeding a small bounded channel.
        let (tx, rx) = std::sync::mpsc::sync_channel::<VideoFrame>(3);
        self.pixmap_frame_rx = Some(rx);
        let stop = Arc::clone(&self.pixmap_stop_flag);
        let pause = Arc::clone(&self.pixmap_pause_flag);
        let frame_pool = Arc::clone(&self.frame_pool);
        let hap_acceleration = self.hap_acceleration.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("pixmap-decode".into())
            .spawn(move || {
                pixmap_decode_thread(
                    &resolved,
                    loop_mode,
                    stop,
                    pause,
                    tx,
                    frame_pool,
                    hap_acceleration,
                )
            })
        {
            // Drop the receiver again so the render tick sees "no stream"
            // instead of a channel that never fills; the cue degrades to a
            // dark pixmap rather than aborting mid-show.
            self.pixmap_frame_rx = None;
            log::error!("PixelMap cue degraded: could not spawn decode thread: {e}");
        }
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
        let Some(rx) = &self.pixmap_frame_rx else {
            return;
        };
        let mut latest: Option<VideoFrame> = None;
        let disconnected = loop {
            match rx.try_recv() {
                Ok(frame) => {
                    if let Some(discarded) = latest.replace(frame) {
                        self.frame_pool.recycle_frame(discarded);
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break false,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break true,
            }
        };
        if disconnected {
            self.pixmap_frame_rx = None;
            self.current_pixmap_qid = None;
        }
        let Some(frame) = latest else { return };
        let (w, h) = (frame.width, frame.height);
        let configure_gate = Arc::clone(&self.configure_gate);
        let _configure_guard = configure_gate.read().unwrap_or_else(|e| e.into_inner());
        if frame.rgba().is_some() {
            self.ensure_pixmap_texture(w, h);
            let tex = self.pixmap_texture.as_ref().unwrap();
            tex.upload_frame(self.queue.queue(), &frame, cuepool_core::CanvasFit::Stretch);
        } else if matches!(&frame.pixels, cuepool_video::FramePixels::Hap { .. }) {
            self.ensure_pixmap_texture(w, h);
            if self.pixmap_hap.is_none() {
                self.pixmap_hap = Some(cuepool_video::HapConverter::new(
                    &self.device,
                    wgpu::TextureFormat::Rgba8Unorm,
                ));
            }
            let conv = self.pixmap_hap.as_mut().unwrap();
            match conv.upload(
                &self.device,
                self.queue.queue(),
                &frame,
                [w, h],
                cuepool_core::CanvasFit::Stretch,
            ) {
                Ok(()) => {
                    let tex = self.pixmap_texture.as_ref().unwrap();
                    let mut encoder =
                        self.device
                            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                                label: Some("pixmap-hap"),
                            });
                    conv.encode(&mut encoder, &tex.render_view());
                    self.queue.submit(Some(encoder.finish()));
                }
                Err(error) => log::warn!("PixelMap HAP upload skipped: {error}"),
            }
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
            conv.upload(
                &self.device,
                self.queue.queue(),
                &frame,
                [w, h],
                cuepool_core::CanvasFit::Stretch,
            );
            let tex = self.pixmap_texture.as_ref().unwrap();
            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("pixmap-yuv"),
                });
            conv.encode(&mut encoder, &tex.render_view());
            self.queue.submit(Some(encoder.finish()));
        }
        self.frame_pool.recycle_frame(frame);
    }

    /// Create output windows if none exist, and make sure the consume thread's
    /// canvas/overlay match the projection canvas size (for Text/Image cues).
    fn ensure_outputs_and_canvas(&mut self, event_loop: &ActiveEventLoop) {
        if self.output_windows.is_empty() {
            self.create_output_windows(event_loop);
        }
        let (w, h) = {
            let state = self.cuepool.state().lock_unpoisoned();
            (
                state.show_file.projection.canvas_width,
                state.show_file.projection.canvas_height,
            )
        };
        let _ = self
            .canvas_cmd_tx
            .send(CanvasCommand::Resize { w, h, force: false });
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
        // The caller that starts a video assigns its new instance immediately
        // afterwards. Clearing here prevents an Image cue (which shares this
        // content slot) from inheriting a seekable video's runtime identity.
        self.current_video_instance_id = None;
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
        let resolved = self
            .resolve_path(font_path)
            .unwrap_or_else(|| font_path.to_string());
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
            CanvasFit::Fit | CanvasFit::Stretch => (canvas_w as f32 / nw).min(canvas_h as f32 / nh),
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
        let project_dir = self
            .cuepool
            .state()
            .lock()
            .ok()?
            .project_path
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
            } else if path.is_dir()
                && let Some(found) = Self::find_in_dir(&path, target)
            {
                return Some(found);
            }
        }
        None
    }

    // ponytail: Preserve the cue-to-engine mapping; add a parameter object when this API changes.
    #[allow(clippy::too_many_arguments)]
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
            let Ok(state) = self.cuepool.state().lock() else {
                return false;
            };
            state.show_file.show_settings.node_name.clone()
        };
        target == local_name || target == "*"
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
            && self.show_engine.audio_engine().is_some_and(|engine| {
                engine.driver() == driver
                    && (configured_device.is_empty() || configured_device == engine.device_name())
            })
        {
            return;
        }

        let now = self.engine_now();
        let _ = self.show_engine.command(EngineCommand::Stop, now);
        self.stop_video_playback();
        self.set_video_paused(false);
        self.stop_external_outputs();
        self.show_engine.replace_audio_engine(None);

        let setup = AudioEngine::configure(driver, &configured_device);
        if let Some(error) = setup.device_list_error {
            log::error!("Could not list {driver} output devices: {error}");
        }
        let devices = setup.devices;

        match setup.engine {
            Ok(engine) => {
                let device_name = engine.device_name().to_string();
                self.show_engine.replace_audio_engine(Some(engine));
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

    fn seek_video_cue(
        &mut self,
        qid: rust_decimal::Decimal,
        path: &str,
        target_secs: f64,
        media_offset_secs: f64,
    ) -> Result<(), String> {
        let paused = self.video_control.lock_unpoisoned().paused;
        let now = Instant::now();
        let media_target_secs = video_media_secs(target_secs, media_offset_secs);
        let Some((clock, pause_started)) = video_seek_clock(now, media_target_secs, paused) else {
            log::debug!("Video seek target {target_secs:.3}s is outside the clock range");
            return Err(format!(
                "video seek target {target_secs:.3}s is outside the clock range"
            ));
        };
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.clock = Some(clock);
            ctl.pause_started = pause_started;
            ctl.peek_pts = None;
            ctl.last_pts = None;
            ctl.timeline_offset_secs = media_offset_secs;
            if ctl.hold_position.is_some() {
                ctl.hold_position = Some(target_secs);
            }
        }
        if let Some(follow) = self.mtc_follow.as_mut()
            && follow.qid == qid
            && follow.hold_position.is_some()
        {
            follow.hold_position = Some(target_secs);
        }
        self.spawn_video_decode(
            path,
            Some(media_target_secs),
            paused.then_some(VideoSeekFrameRequest {
                position: media_target_secs,
                adjust_show_clock: false,
            }),
            true,
        );
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // Mirrors the PlayVideo engine action at the runtime boundary.
    fn play_video(
        &mut self,
        path: &str,
        qid: rust_decimal::Decimal,
        instance_id: u64,
        clock_origin: Duration,
        start_time: cuepool_core::Timespan,
        duration: cuepool_core::Timespan,
        event_loop: &ActiveEventLoop,
    ) {
        // A fresh cue gets a fresh retry budget.
        self.video_open_retries = 0;
        if self.hap_fallback_instance_id != Some(instance_id) {
            self.hap_fallback_session = HapFallbackSession::default();
            self.hap_fallback_instance_id = Some(instance_id);
        }
        // Only open output windows on the first video; looping should not respawn them.
        if self.output_windows.is_empty() {
            self.create_output_windows(event_loop);
        }
        let engine_now = self.engine_now();
        // A newly-started video should always play, even if the system was paused.
        self.video_pause_flag.store(false, Ordering::Relaxed);

        let projection = {
            let state = self.cuepool.state().lock_unpoisoned();
            state.show_file.projection.clone()
        };
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.stream_epoch += 1;
            // Preserve the audio-master cue origin across output-window creation.
            // Frames late vs this clock are skipped, so decoder startup catches up
            // without turning setup time into a permanent audio lead.
            let media_offset = start_time.as_secs_f64().max(0.0);
            ctl.clock = video_start_clock(Instant::now(), media_offset, engine_now, clock_origin);
            ctl.pause_started = None;
            ctl.peek_pts = None;
            ctl.last_pts = None;
            ctl.canvas_has_frame = false;
            ctl.media_length_secs =
                (duration.as_secs_f64() > 0.0).then(|| media_offset + duration.as_secs_f64());
            ctl.timeline_offset_secs = media_offset;
            // A new video always starts at full brightness, cancelling any
            // Stop-cue fade still in flight.
            ctl.fade = None;
            // A step-back aimed at the previous stream must not replay here
            // (the epoch gate no longer consumes it against a dead stream).
            ctl.seek_frame = None;
            ctl.fit = projection.fit;
        }
        self.set_current_video_qid(Some(qid));
        self.current_video_instance_id = Some(instance_id);
        // (Re)create the consume thread's canvas at the projection size; `force`
        // clears the previous clip's last frame even when the dims match.
        let _ = self.canvas_cmd_tx.send(CanvasCommand::Resize {
            w: projection.canvas_width,
            h: projection.canvas_height,
            force: true,
        });

        let media_offset = start_time.as_secs_f64().max(0.0);
        self.spawn_video_decode(
            path,
            (media_offset > 0.0).then_some(media_offset),
            None,
            false,
        );
    }

    /// (Re)spawn the video decode thread. `start_before`: seek so the first
    /// frame delivered is the last one with a PTS strictly below this
    /// timestamp (seeking and frame-step-back), followed by the frames after it.
    /// `clamp_to_media` covers SeekCue when container duration is learned only
    /// after the decoder opens; clock correction stays on the decode thread.
    fn spawn_video_decode(
        &mut self,
        path: &str,
        start_before: Option<f64>,
        seek_frame: Option<VideoSeekFrameRequest>,
        clamp_to_media: bool,
    ) {
        let path = self.resolve_path(path).unwrap_or_else(|| path.to_string());
        queue_latest_video_decode(
            &mut self.pending_video_decode,
            VideoDecodeRequest {
                path,
                start_before,
                seek_frame,
                clamp_to_media,
                hap_fallback_session: self.hap_fallback_session.clone(),
            },
        );
        self.video_stop_flag.store(true, Ordering::Relaxed);
        // Retire the old receiver immediately. If its FFmpeg open is slow to
        // cancel, the last picture stays held instead of old queued frames
        // being presented against the newly-seeked clock.
        {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.stream_epoch += 1;
            ctl.frame_rx = None;
            ctl.peek_pts = None;
        }
        self.start_pending_video_decode();
    }

    fn start_pending_video_decode(&mut self) {
        let Some(request) =
            take_ready_video_decode(&mut self.video_decode_join, &mut self.pending_video_decode)
        else {
            return;
        };
        self.video_stop_flag = Arc::new(AtomicBool::new(false));
        let VideoDecodeRequest {
            path,
            start_before,
            seek_frame,
            clamp_to_media,
            hap_fallback_session,
        } = request;
        // Bounded channel = backpressure: the decode thread can't outrun the consumer
        // (the consume thread matching PTS against the wall-clock video clock), so
        // decode runs at real-time rate — no free-running decoder to drift against
        // the clock. The small buffer absorbs decode jitter. Pacing is the wall
        // clock, not the audio clock, so it can't freeze if the audio device sleeps.
        let (frame_tx, frame_rx) = std::sync::mpsc::sync_channel::<VideoMessage>(VIDEO_QUEUE_CAP);
        let timings = VideoTimings::default();
        // Installing a new receiver also tells the consume thread to drop its
        // peeked frame and invalidates EOF already queued by the old decoder.
        let stream_epoch = {
            let mut ctl = self.video_control.lock_unpoisoned();
            ctl.stream_epoch += 1;
            ctl.frame_rx = Some(frame_rx);
            ctl.seek_frame = seek_frame;
            ctl.timings = timings.clone();
            ctl.stream_epoch
        };
        let stop_flag = Arc::clone(&self.video_stop_flag);
        let pause_flag = Arc::clone(&self.video_pause_flag);
        let frame_pool = Arc::clone(&self.frame_pool);
        let diag_state = Arc::clone(self.cuepool.state());
        let video_control = Arc::clone(&self.video_control);
        let zero_copy = self.zero_copy.clone();
        let hap_acceleration = self.hap_acceleration.clone();

        match std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                video_decode_thread(
                    &path,
                    start_before,
                    stop_flag,
                    pause_flag,
                    frame_tx,
                    diag_state,
                    video_control,
                    stream_epoch,
                    clamp_to_media,
                    frame_pool,
                    timings,
                    zero_copy,
                    hap_acceleration,
                    hap_fallback_session,
                );
            }) {
            Ok(join) => self.video_decode_join = Some(join),
            Err(e) => {
                self.video_control.lock_unpoisoned().frame_rx = None;
                log::error!("Video cue degraded: could not spawn decode thread: {e}");
            }
        }
    }

    /// A project was created or loaded — stop everything from the previous project
    /// and close its output windows (which would otherwise keep playing with the
    /// old projection geometry). The windows reopen with the new project's geometry
    /// when the next video/image/text cue plays, or via the projection-output menu.
    fn reset_for_project_change(&mut self, event_loop: &ActiveEventLoop) {
        let actions = self.show_engine.reset_for_project_change();
        if let Err(error) = self.apply_engine_actions(actions, event_loop) {
            log::error!("Project reset action failed: {error}");
        }
        self.lighting.shutdown();
        self.pixmap_texture = None;
        self.pixmap_yuv = None;
        self.pixmap_hap = None;
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
        self.pending_video_decode = None;
        self.hap_fallback_session = HapFallbackSession::default();
        self.hap_fallback_instance_id = None;
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
            ctl.seek_frame = None;
            ctl.media_length_secs = None;
            ctl.timeline_offset_secs = 0.0;
        }
        self.set_current_video_qid(None);
        self.cuepool.state().lock_unpoisoned().diagnostics.video = None;
    }

    /// Persist settings and hard-exit the process. A graceful `event_loop.exit()`
    /// returns through `run_app` and runs Rust drops (wgpu device/surfaces, threads)
    /// which can wedge the main thread on macOS (beachball); the OS reclaims
    /// everything on `process::exit`, just like the Ctrl-C handler and Dock-quit.
    fn hard_exit(&self, reason: &str) -> ! {
        log::info!(target: PERSIST_TARGET, "Shutdown requested: {reason}");
        save_settings_from_state(&self.profile, self.cuepool.state());
        #[cfg(windows)]
        win_timer::release();
        log::info!(target: PERSIST_TARGET, "Shutdown complete: {reason}");
        log::logger().flush();
        std::process::exit(0);
    }

    /// Show-clock elapsed seconds — frozen while paused, adjusted for
    /// accumulated pause time and frame-step advances. None before the first Go.
    fn show_elapsed(&self) -> Option<f64> {
        self.show_engine.snapshot().show_elapsed_secs
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
    /// Hard sync by reopening the media at `target`.
    ///
    /// Rate-limited, because every call is a full container open (index parse
    /// included) and `drive_mtc_follow` runs each tick: while an operator
    /// scrubs the timecode source the target moves continuously, and
    /// unthrottled this becomes a back-to-back stream of opens on a large
    /// master. Skipping is safe rather than lossy — the next eligible tick
    /// syncs to the *then* current target, so scrub bursts coalesce to the
    /// latest position instead of replaying every intermediate one.
    /// Returns whether the reopen actually happened.
    fn mtc_hard_sync(&mut self, target: f64) -> bool {
        let Some(follow) = self.mtc_follow.as_ref() else {
            return false;
        };
        if self
            .last_hard_sync
            .is_some_and(|at| at.elapsed() < mtc_follow::HARD_SYNC_REOPEN_FLOOR)
        {
            log::trace!("[MTC] Hard sync to {target:.2}s coalesced");
            return false;
        }
        self.last_hard_sync = Some(Instant::now());
        let path = follow.path.clone();
        log::info!("[MTC] Hard sync Q{} to {:.2}s", follow.qid, target);
        // Clamp: the timecode source is free to run past the end of the clip
        // (a short insert against a full-length show track), and an unclamped
        // seek past the media just puts the demuxer off the end. SeekCue and
        // the automation API already clamp; this path was the outlier.
        self.spawn_video_decode(&path, Some(target), None, true);
        self.mtc_reanchor(target);
        true
    }

    /// Drive the timecode-follow cue from the latest source state. No-op
    /// without one.
    fn drive_mtc_follow(&mut self, mtc: &TimecodeState) {
        self.mtc_drift = None;
        let Some(follow) = self.mtc_follow.as_mut() else {
            return;
        };
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
                    mtc_follow::MtcAdjust::HardSync => {
                        self.mtc_hard_sync(target);
                    }
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
            let may_reopen = self
                .last_hard_sync
                .is_none_or(|at| at.elapsed() >= mtc_follow::HARD_SYNC_REOPEN_FLOOR);
            let action = mtc_follow::locate_action(target - current, may_reopen);
            if action.sync {
                self.mtc_hard_sync(target);
            }
            if action.hold {
                if let Some(f) = self.mtc_follow.as_mut() {
                    f.hold_position = Some(target);
                }
                self.video_control.lock_unpoisoned().hold_position = Some(target);
            }
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
                    if let Some(c) = ctl
                        .clock
                        .and_then(|c| c.checked_sub(Duration::from_secs_f64(delta)))
                    {
                        ctl.clock = Some(c);
                        self.show_engine.adjust_show_time(delta);
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
            let Ok(state) = self.cuepool.state().lock() else {
                return;
            };
            state.show_file.show_settings.timecode_fps.max(1.0)
        };
        self.show_engine.adjust_show_time(1.0 / fps as f64);
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
            let pos = self
                .video_paused_position()
                .unwrap_or_default()
                .as_secs_f64();
            let cur = self.video_control.lock_unpoisoned().last_pts.unwrap_or(pos);
            if cur <= 0.0 {
                return;
            }
            let path = {
                let Ok(state) = self.cuepool.state().lock() else {
                    return;
                };
                let Some(qid) = self.current_video_qid else {
                    return;
                };
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
            // The consume thread does the (blocking) wait for the sought frame,
            // snaps the clock, and reports the delta back for the show clock.
            self.spawn_video_decode(
                &path,
                Some(cur),
                Some(VideoSeekFrameRequest {
                    position: pos,
                    adjust_show_clock: true,
                }),
                false,
            );
            return;
        }
        // No video: rewind the frozen clock by one display frame.
        let fps = {
            let Ok(state) = self.cuepool.state().lock() else {
                return;
            };
            state.show_file.show_settings.timecode_fps.max(1.0)
        };
        self.show_engine.adjust_show_time(-1.0 / fps as f64);
    }

    fn handle_dropped_file(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());

        // Open project files directly
        if ext.as_deref() == Some("qproj") {
            self.cuepool.state().lock_unpoisoned().command_queue.push(
                cuepool_gui::AppCommand::OpenProject {
                    path: path.to_path_buf(),
                },
            );
            return;
        }

        let is_video = matches!(
            ext.as_deref(),
            Some("mp4") | Some("mov") | Some("mkv") | Some("avi")
        );
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

    fn process_api_commands(&mut self, event_loop: &ActiveEventLoop) {
        let requests: Vec<_> = match self.api.as_mut() {
            Some(api) => std::iter::from_fn(|| api.try_recv()).collect(),
            None => return,
        };
        for request in requests {
            let outcome = if self.shutdown_started_at.is_some() {
                ApiCommandOutcome::Rejected("shutdown is in progress".into())
            } else {
                match self.apply_api_command(request.command, request.prepared_project, event_loop)
                {
                    Ok(message) => ApiCommandOutcome::Applied(message),
                    Err(message) => ApiCommandOutcome::Rejected(message),
                }
            };
            self.publish_active_cues_if_due(true);
            if let Some(api) = self.api.as_ref() {
                api.complete(request.id, outcome);
            }
        }
    }

    fn apply_api_command(
        &mut self,
        command: ApiCommand,
        prepared_project: Option<cuepool_gui::PreparedProject>,
        event_loop: &ActiveEventLoop,
    ) -> Result<String, String> {
        let command = match command {
            ApiCommand::OpenProject { .. } => ShowControlCommand::OpenProject(Box::new(
                prepared_project
                    .ok_or_else(|| "project was not prepared by the API".to_string())?,
            )),
            ApiCommand::SelectCue { qid } => ShowControlCommand::SelectCue(
                qid.parse::<rust_decimal::Decimal>()
                    .map_err(|error| format!("invalid cue qid '{qid}': {error}"))?,
            ),
            ApiCommand::Go => ShowControlCommand::Go,
            ApiCommand::Stop => ShowControlCommand::Stop,
            ApiCommand::Pause => ShowControlCommand::Pause,
            ApiCommand::Resume => ShowControlCommand::Resume,
            ApiCommand::Preload => ShowControlCommand::Preload,
            ApiCommand::Seek {
                instance_id,
                seconds,
            } => ShowControlCommand::Seek {
                instance_id,
                seconds,
            },
            ApiCommand::Shutdown => ShowControlCommand::Shutdown,
        };
        self.execute_show_control(command, event_loop)
    }

    fn execute_show_control(
        &mut self,
        command: ShowControlCommand,
        event_loop: &ActiveEventLoop,
    ) -> Result<String, String> {
        match command {
            ShowControlCommand::OpenProject(project) => {
                if self.show_control_is_active() {
                    return Err(
                        "the current show is active; stop it before opening a project".into(),
                    );
                }
                let path = project.path.display().to_string();
                self.cuepool.apply_unattended_project(*project)?;
                self.reset_for_project_change(event_loop);
                self.apply_audio_settings();
                self.last_project_generation =
                    self.cuepool.state().lock_unpoisoned().project_generation;
                Ok(format!("opened project {path}"))
            }
            ShowControlCommand::SelectCue(qid) => {
                self.cuepool.select_cue(qid)?;
                Ok(format!("selected cue Q{qid}"))
            }
            ShowControlCommand::Go => {
                self.handle_go(event_loop)?;
                Ok("GO applied".into())
            }
            ShowControlCommand::Stop => {
                self.run_engine_command(EngineCommand::Stop, event_loop)?;
                Ok("all cues stopped".into())
            }
            ShowControlCommand::Pause => {
                if self.paused {
                    return Err("playback is already paused".into());
                }
                if self.show_engine.snapshot().show_elapsed_secs.is_none() {
                    return Err("playback is not running".into());
                }
                self.run_engine_command(EngineCommand::Pause, event_loop)?;
                Ok("playback paused".into())
            }
            ShowControlCommand::Resume => {
                if !self.paused {
                    return Err("playback is not paused".into());
                }
                self.run_engine_command(EngineCommand::Resume, event_loop)?;
                Ok("playback resumed".into())
            }
            ShowControlCommand::Preload => {
                let cue = self
                    .cuepool
                    .state()
                    .lock_unpoisoned()
                    .selected_cue()
                    .cloned();
                let Some(cue) = cue else {
                    return Err("no cue is selected".into());
                };
                if !matches!(
                    cue,
                    cuepool_core::Cue::Sound { .. } | cuepool_core::Cue::Video { .. }
                ) {
                    return Err(format!(
                        "preload is not supported for cue Q{}",
                        cue.base().qid
                    ));
                }
                self.run_engine_command(EngineCommand::Preload, event_loop)?;
                Ok("selected cue preloaded".into())
            }
            ShowControlCommand::Seek {
                instance_id,
                seconds,
            } => {
                let snapshot = self.show_engine.snapshot();
                let found = snapshot
                    .active_cues
                    .iter()
                    .any(|cue| cue.instance_id == instance_id)
                    || snapshot
                        .video
                        .as_ref()
                        .is_some_and(|video| video.instance_id == instance_id);
                if !found {
                    return Err(format!("cue instance {instance_id} is not active"));
                }
                self.run_engine_command(
                    EngineCommand::Seek {
                        instance_id,
                        secs: seconds,
                    },
                    event_loop,
                )?;
                Ok(format!(
                    "cue instance {instance_id} seeked to {seconds:.3}s"
                ))
            }
            ShowControlCommand::Shutdown => {
                let dirty = self.cuepool.state().lock_unpoisoned().dirty;
                if let Some(message) = shutdown_rejection(dirty, self.show_control_is_active()) {
                    return Err(message.into());
                }
                self.shutdown_started_at = Some(Instant::now());
                if let Some(api) = self.api.as_ref() {
                    api.mark_stopping();
                }
                Ok(format!(
                    "profile '{}' accepted shutdown",
                    self.profile.name()
                ))
            }
        }
    }

    fn show_control_is_active(&self) -> bool {
        let engine = self.show_engine.snapshot();
        !engine.active_cues.is_empty()
            || engine.show_elapsed_secs.is_some()
            || engine.video.is_some()
            || self.current_video_qid.is_some()
            || self.current_text_qid.is_some()
            || self.pending_video_decode.is_some()
            || self.current_pixmap_qid.is_some()
            || self.mtc_follow.is_some()
            || self.recorder.recording()
            || self.lighting.is_active()
    }

    /// Drain any AppCommands queued by the UI and execute them.
    fn process_commands(&mut self, event_loop: &ActiveEventLoop) {
        // Commands this drain doesn't own (e.g. OpenProject and SaveProject,
        // which the GUI drain handles) go back on the queue instead of being
        // dropped. This loop runs every about_to_wait (~250/s) while the GUI
        // drains only on control-window redraws, so this side usually wins
        // the race — silently discarding GUI-only commands broke `cuepool
        // <show.qproj>` startup loads and OSC-triggered saves (#138).
        let state = Arc::clone(self.cuepool.state());
        drain_app_commands(&state, |cmd| {
            match cmd {
                AppCommand::Go => {
                    let _ = self.execute_show_control(ShowControlCommand::Go, event_loop);
                }
                AppCommand::Stop => {
                    let _ = self.execute_show_control(ShowControlCommand::Stop, event_loop);
                }
                AppCommand::Pause => {
                    let command = if self.paused {
                        ShowControlCommand::Resume
                    } else {
                        ShowControlCommand::Pause
                    };
                    let _ = self.execute_show_control(command, event_loop);
                }
                AppCommand::SetLimiterThreshold(threshold) => {
                    if let Some(audio) = self.show_engine.audio_engine() {
                        audio.set_limiter_threshold(threshold);
                        log::info!(
                            "Set master limiter threshold to {:.2} dB",
                            20.0 * threshold.log10()
                        );
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
                    let _ = self.execute_show_control(ShowControlCommand::Preload, event_loop);
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
                // SaveProject/SaveProjectAs fall through to `unhandled` below —
                // the GUI drain owns them.
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
                    self.lighting
                        .go(&snapshot, 0.0, cuepool_core::FadeType::Linear);
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
                AppCommand::SeekCue { instance_id, secs } => {
                    let _ = self.execute_show_control(
                        ShowControlCommand::Seek {
                            instance_id,
                            seconds: secs,
                        },
                        event_loop,
                    );
                }
                other => return Err(other),
            }
            Ok(())
        });
    }

    /// Drain MIDI input events and fire any cues whose MIDI trigger matches.
    fn process_midi_events(&mut self, event_loop: &ActiveEventLoop) {
        // Drain all pending events first, so firing cues (which needs `&mut self`)
        // doesn't conflict with the borrow of `self.midi_manager`.
        let events: Vec<MidiEvent> = {
            let Some(manager) = &self.midi_manager else {
                return;
            };
            std::iter::from_fn(|| manager.try_recv()).collect()
        };
        for ev in events {
            log::debug!("MIDI event: {ev:?}");

            // If MIDI learn is pending, store the first event as the cue's trigger.
            let learn_qid = {
                let Ok(state) = self.cuepool.state().lock() else {
                    continue;
                };
                state.pending_midi_learn
            };
            if let Some(qid) = learn_qid {
                let learned: Option<MidiTrigger> = match self.cuepool.state().lock() {
                    Ok(mut state) => {
                        let trigger = match ev {
                            MidiEvent::NoteOn {
                                channel,
                                note,
                                velocity,
                            } => MidiTrigger {
                                channel,
                                kind: MidiTriggerKind::NoteOn,
                                note_or_cc: note,
                                velocity_min: velocity,
                            },
                            MidiEvent::NoteOff {
                                channel,
                                note,
                                velocity,
                            } => MidiTrigger {
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
                        if let Some(cue) = state
                            .show_file
                            .cues
                            .iter_mut()
                            .find(|c| c.base().qid == qid)
                        {
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
                    let Ok(state) = self.cuepool.state().lock() else {
                        continue;
                    };
                    state
                        .recorder_midi_enabled
                        .then_some(state.recorder_midi_universe)
                };
                if let Some(universe) = bridge {
                    self.recorder
                        .live_input(universe, cc as u16, (value as u16 * 255 / 127) as u8);
                }
            }

            let cues: Vec<_> = {
                let Ok(state) = self.cuepool.state().lock() else {
                    continue;
                };
                state
                    .show_file
                    .cues
                    .iter()
                    .filter(|c| c.enabled())
                    .cloned()
                    .collect()
            };
            for cue in cues {
                let trigger = cue.base().triggers.midi.as_ref();
                let Some(trigger) = trigger else { continue };
                let matches = match (ev, trigger.kind) {
                    (
                        MidiEvent::NoteOn {
                            channel,
                            note,
                            velocity,
                        },
                        MidiTriggerKind::NoteOn,
                    )
                    | (
                        MidiEvent::NoteOff {
                            channel,
                            note,
                            velocity,
                        },
                        MidiTriggerKind::NoteOff,
                    ) => {
                        channel == trigger.channel
                            && note == trigger.note_or_cc
                            && velocity >= trigger.velocity_min
                    }
                    (MidiEvent::CC { channel, cc, value }, MidiTriggerKind::CC) => {
                        channel == trigger.channel
                            && cc == trigger.note_or_cc
                            && value >= trigger.velocity_min
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
            let Ok(state) = self.cuepool.state().lock() else {
                return;
            };
            state
                .show_file
                .cues
                .iter()
                .filter(|c| c.enabled())
                .cloned()
                .collect()
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
            let Ok(state) = self.cuepool.state().lock() else {
                return;
            };
            state
                .show_file
                .cues
                .iter()
                .filter(|c| c.enabled())
                .cloned()
                .collect()
        };

        for cue in cues {
            let Some(trigger) = cue.base().triggers.wall_clock.as_ref() else {
                continue;
            };
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
            let Ok(mut state) = self.cuepool.state().lock() else {
                return;
            };
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
        if let Some(qid) = capture_qid
            && let Ok(mut state) = self.cuepool.state().lock()
        {
            if let Some(cue) = state
                .show_file
                .cues
                .iter_mut()
                .find(|c| c.base().qid == qid)
            {
                cue.base_mut().triggers.timecode = Some(cuepool_core::TimecodeTrigger {
                    time: Timespan::from_secs_f64(elapsed),
                });
                state.dirty = true;
                log::info!("Captured timecode trigger for Q{} at {:.2}s", qid, elapsed);
            }
            state.pending_timecode_capture = None;
        }

        // Frozen clock: never fire while paused (a just-captured or stepped-past
        // trigger would fire instantly). Anything passed by stepping fires on
        // resume.
        if self.paused {
            return;
        }

        let cues: Vec<_> = {
            let Ok(state) = self.cuepool.state().lock() else {
                return;
            };
            state
                .show_file
                .cues
                .iter()
                .filter(|c| c.enabled())
                .cloned()
                .collect()
        };

        for cue in cues {
            let Some(trigger) = cue.base().triggers.timecode.as_ref() else {
                continue;
            };
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
                        if let Some(qid_str) = qid
                            && let Ok(qid_dec) = qid_str.parse::<rust_decimal::Decimal>()
                        {
                            let _ = self
                                .cuepool
                                .state()
                                .lock()
                                .map(|mut s| s.selected_cue_id = Some(qid_dec));
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
                        if self.paused
                            && let Ok(mut state) = self.cuepool.state().lock()
                        {
                            state.command_queue.push(AppCommand::Pause);
                        }
                    }
                    OscEvent::Select { qid } => {
                        if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                            let _ = self
                                .cuepool
                                .state()
                                .lock()
                                .map(|mut s| s.selected_cue_id = Some(qid_dec));
                        }
                    }
                    OscEvent::Up => {}
                    OscEvent::Down => {}
                    OscEvent::Save => {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::SaveProject);
                        }
                    }
                    OscEvent::DmxChannel {
                        universe,
                        channel,
                        value,
                    } => {
                        // Wire channel is 1-based; recorder is 0-based.
                        self.recorder.live_input(universe, channel - 1, value);
                    }
                    OscEvent::RecorderRecord => {
                        let file = self.recorder_file();
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state
                                .command_queue
                                .push(AppCommand::RecorderRecord { file });
                        }
                    }
                    OscEvent::RecorderStop => {
                        // Recording → stop & keep; idle → stop preview.
                        // stub: no OSC status feedback yet.
                        if self.recorder.recording() {
                            let file = self.recorder_file();
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state
                                    .command_queue
                                    .push(AppCommand::RecorderRecord { file });
                            }
                        } else if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::RecorderStopPreview);
                        }
                    }
                    OscEvent::RecorderPlay => {
                        let file = self.recorder_file();
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state
                                .command_queue
                                .push(AppCommand::RecorderPreview { file });
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
                            state
                                .command_queue
                                .push(AppCommand::RecorderRevert { file });
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
                    OscEvent::RemotePing { src } => {
                        // Reply to whoever asked, rather than broadcasting on the
                        // TX port: a broadcast pong carries no identity, so with
                        // more than one CuePool on the subnet the requester cannot
                        // tell which machine answered. The payload stays empty so
                        // existing health checks keep matching it byte for byte.
                        if let Some(osc) = &self.osc_manager {
                            let _ = osc.send_to(
                                rosc::OscMessage {
                                    addr: "/qplayer/remote/pong".into(),
                                    args: vec![],
                                },
                                src,
                            );
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
                                let _ = self
                                    .cuepool
                                    .state()
                                    .lock()
                                    .map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Go);
                            }
                        }
                    }
                    OscEvent::RemoteStop { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self
                                    .cuepool
                                    .state()
                                    .lock()
                                    .map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Stop);
                            }
                        }
                    }
                    OscEvent::RemotePause { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self
                                    .cuepool
                                    .state()
                                    .lock()
                                    .map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.cuepool.state().lock() {
                                state.command_queue.push(AppCommand::Pause);
                            }
                        }
                    }
                    OscEvent::RemoteUnpause { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self
                                    .cuepool
                                    .state()
                                    .lock()
                                    .map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if self.paused
                                && let Ok(mut state) = self.cuepool.state().lock()
                            {
                                state.command_queue.push(AppCommand::Pause);
                            }
                        }
                    }
                    OscEvent::RemotePreload {
                        target,
                        qid,
                        time: _,
                    } if self.is_remote_target_match(&target) => {
                        if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                            let _ = self
                                .cuepool
                                .state()
                                .lock()
                                .map(|mut s| s.selected_cue_id = Some(qid_dec));
                        }
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.command_queue.push(AppCommand::Preload);
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
                            let _ = self
                                .cuepool
                                .state()
                                .lock()
                                .map(|mut s| s.selected_cue_id = Some(qid_dec));
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
            let message = {
                let Ok(state) = self.cuepool.state().lock() else {
                    return;
                };
                remote_discovery_message(&state.show_file.show_settings)
            };
            if let (Some(osc), Some(message)) = (&self.osc_manager, message) {
                let _ = osc.send(message);
            }
        }

        // Remote node liveness: mark nodes inactive after 5s without discovery
        {
            let Ok(mut state) = self.cuepool.state().lock() else {
                return;
            };
            let now = Instant::now();
            for node in &mut state.show_file.show_settings.remote_nodes {
                if let Some(last) = node.last_seen
                    && now.duration_since(last) > Duration::from_secs(5)
                {
                    // Node timed out — keep it in the list but last_seen is stale
                }
            }
        }
    }

    /// Render the control window (egui).
    fn update_window_title(&mut self) {
        let (path, dirty) = {
            let Ok(state) = self.cuepool.state().lock() else {
                return;
            };
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
        self.start_pending_video_decode();
        self.process_commands(event_loop);
        let now = self.engine_now();
        let actions = self.show_engine.tick(now);
        if let Err(error) = self.apply_engine_actions(actions, event_loop) {
            log::error!("Engine tick action failed: {error}");
        }
        for qid in self.lighting.take_finished_shows() {
            if qid != rust_decimal::Decimal::NEGATIVE_ONE {
                let now = self.engine_now();
                let actions = self
                    .show_engine
                    .event(EngineEvent::ExternalFinished { qid }, now);
                if let Err(error) = self.apply_engine_actions(actions, event_loop) {
                    log::error!("Engine completion action failed for Q{qid}: {error}");
                }
            }
        }
        self.publish_active_cues();
    }

    fn publish_active_cues(&mut self) {
        self.publish_active_cues_if_due(false);
    }

    fn publish_active_cues_if_due(&mut self, force: bool) {
        if !force && self.last_active_cue_publish.elapsed() < Duration::from_millis(16) {
            return;
        }
        self.last_active_cue_publish = Instant::now();

        let engine_snapshot = self.show_engine.snapshot();
        let mut published: Vec<cuepool_gui::ActiveCueInfo> = engine_snapshot
            .active_cues
            .iter()
            .map(|cue| cuepool_gui::ActiveCueInfo {
                instance_id: cue.instance_id,
                qid: cue.qid,
                name: cue.name.clone(),
                paused: cue.state == CueState::Paused,
                position_secs: cue.position_secs as f32,
                length_secs: cue.length_secs.map(|length| length as f32),
                state: cue.state,
            })
            .collect();

        let (video_paused, video_position, video_length) = {
            let control = self.video_control.lock_unpoisoned();
            let media_position = match (control.clock, control.pause_started) {
                (Some(clock), Some(paused_at)) => paused_at.duration_since(clock).as_secs_f64(),
                (Some(clock), None) => clock.elapsed().as_secs_f64(),
                _ => 0.0,
            };
            (
                control.pause_started.is_some(),
                video_timeline_secs(media_position, control.timeline_offset_secs) as f32,
                control
                    .media_length_secs
                    .map(|length| video_timeline_secs(length, control.timeline_offset_secs) as f32),
            )
        };

        if let Ok(mut state) = self.cuepool.state().lock() {
            if let Some(qid) = engine_snapshot
                .video
                .as_ref()
                .map(|video| video.qid)
                .or(self.current_video_qid)
                && !published.iter().any(|cue| cue.qid == qid)
                && let Some(cue) = state
                    .show_file
                    .cues
                    .iter()
                    .find(|cue| cue.base().qid == qid)
            {
                let configured_duration = match cue {
                    cuepool_core::Cue::Video { duration, .. } => duration.as_secs_f64() as f32,
                    _ => 0.0,
                };
                published.push(cuepool_gui::ActiveCueInfo {
                    instance_id: engine_snapshot
                        .video
                        .as_ref()
                        .map(|video| video.instance_id)
                        .unwrap_or_default(),
                    qid,
                    name: cue.base().name.clone(),
                    paused: video_paused,
                    position_secs: video_position,
                    length_secs: (configured_duration > 0.0)
                        .then_some(configured_duration)
                        .or(video_length),
                    state: if video_paused {
                        CueState::Paused
                    } else {
                        CueState::Playing
                    },
                });
            }

            for qid in [self.current_text_qid, self.current_pixmap_qid]
                .into_iter()
                .flatten()
                .chain(self.lighting.active_show_qids())
            {
                if !published.iter().any(|cue| cue.qid == qid)
                    && let Some(cue) = state
                        .show_file
                        .cues
                        .iter()
                        .find(|cue| cue.base().qid == qid)
                {
                    published.push(cuepool_gui::ActiveCueInfo {
                        qid,
                        name: cue.base().name.clone(),
                        state: CueState::Playing,
                        ..Default::default()
                    });
                }
            }
            state.active_cues = published;
        }
    }
    fn render_control(&mut self) {
        let render_started = Instant::now();
        self.render_control_inner();
        let us = render_started.elapsed().as_micros().min(u32::MAX as u128) as u32;
        self.dbg_render_max_us = self.dbg_render_max_us.max(us);
        self.dbg_render_count += 1;
    }

    fn render_control_inner(&mut self) {
        self.update_window_title();
        if self.control_surface.is_none() {
            self.retry_control_surface();
        }

        // Acquire (under the shared gate) BEFORE running the egui pass: bailing
        // out after `run` would discard its texture deltas and desync the atlas.
        let submit_guard = self
            .configure_gate
            .read()
            .unwrap_or_else(|e| e.into_inner());
        let Some(surface) = self.control_surface.as_ref() else {
            return;
        };
        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(o)
            | wgpu::CurrentSurfaceTexture::Suboptimal(o) => o,
            // Control window covered/minimized — skip this frame quietly (no spam).
            wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated => {
                drop(submit_guard);
                log::debug!("Control surface outdated, reconfiguring");
                let Some(surface) = self.control_surface.as_ref() else {
                    return;
                };
                let Some(config) = self.control_config.as_ref() else {
                    return;
                };
                let _configure_guard = self
                    .configure_gate
                    .write()
                    .unwrap_or_else(|e| e.into_inner());
                surface.configure(&self.device, config);
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                drop(submit_guard);
                let monitor = self
                    .control_window
                    .as_ref()
                    .and_then(|window| window.current_monitor())
                    .map(|monitor| monitor_descriptor(&monitor));
                let context = gpu_display_context(&self.adapter, monitor.as_ref());
                log::warn!("Control surface lost; parking it for recovery; {context}");
                self.control_surface = None;
                self.control_surface_retry_failures = 0;
                self.control_surface_retry_at = Some(Instant::now());
                self.retry_control_surface();
                return;
            }
            err => {
                log::warn!("Control surface acquire failed: {err:?}");
                return;
            }
        };
        let Some(config) = self.control_config.as_ref() else {
            return;
        };
        let Some(window) = self.control_window.as_ref() else {
            return;
        };
        let Some(egui_state) = self.egui_state.as_mut() else {
            return;
        };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else {
            return;
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let raw_input = egui_state.take_egui_input(window);

        // Sync master meter data into the GUI shared state
        if let Some(engine) = self.show_engine.audio_engine() {
            let meters = engine.read_meters();
            let peak_l_db = if meters.peak_l > 0.0 {
                20.0 * meters.peak_l.log10()
            } else {
                -f32::INFINITY
            };
            let peak_r_db = if meters.peak_r > 0.0 {
                20.0 * meters.peak_r.log10()
            } else {
                -f32::INFINITY
            };
            let rms_l_db = if meters.rms_l > 0.0 {
                20.0 * meters.rms_l.log10()
            } else {
                -f32::INFINITY
            };
            let rms_r_db = if meters.rms_r > 0.0 {
                20.0 * meters.rms_r.log10()
            } else {
                -f32::INFINITY
            };
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

        let mut full_output = self.egui_ctx.run_ui(raw_input, |ui| {
            self.cuepool.update(ui);
        });
        egui_state.handle_platform_output(window, full_output.platform_output);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [config.width, config.height],
            pixels_per_point: window.scale_factor() as f32 * self.egui_ctx.zoom_factor(),
        };

        let paint_jobs = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("control-encoder"),
            });

        for (id, image_deltas) in full_output.textures_delta.set.drain() {
            for image_delta in image_deltas {
                egui_renderer.update_texture(&self.device, self.queue.queue(), id, &image_delta);
            }
        }
        egui_renderer.update_buffers(
            &self.device,
            self.queue.queue(),
            &mut encoder,
            &paint_jobs,
            &screen_descriptor,
        );

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
            egui_renderer.render(
                &mut render_pass.forget_lifetime(),
                &paint_jobs,
                &screen_descriptor,
            );
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.queue.queue().present(output);
        for id in full_output.textures_delta.free.drain() {
            egui_renderer.free_texture(&id);
        }
        // Commands queued during the UI frame drain in tick_engine, which runs
        // in about_to_wait later this same iteration.
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.control_window.is_none()
            && let Err(error) = self.create_control_window(event_loop)
        {
            self.terminal_error = Some(TerminalError::Startup(error));
            event_loop.exit();
            return;
        }
        if let Some(api) = self.api.as_ref() {
            api.mark_ready();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: AppEvent) {
        if self.shutdown_started_at.is_some() {
            return;
        }
        match event {
            AppEvent::VideoEof(epoch) => {
                let current_epoch = self.video_control.lock_unpoisoned().stream_epoch;
                if epoch != current_epoch {
                    log::debug!("Ignoring stale video EOF epoch {epoch} (current {current_epoch})");
                    return;
                }
                log::info!("Video EOF");
                if let Some(video) = self.show_engine.snapshot().video {
                    let now = self.engine_now();
                    let actions = self.show_engine.event(
                        EngineEvent::VideoEof {
                            instance_id: video.instance_id,
                            epoch: video.epoch,
                        },
                        now,
                    );
                    if let Err(error) = self.apply_engine_actions(actions, event_loop) {
                        log::error!("Video EOF action failed: {error}");
                    }
                }
            }
            AppEvent::VideoFailed(epoch) => {
                let current_epoch = self.video_control.lock_unpoisoned().stream_epoch;
                if epoch != current_epoch {
                    log::debug!(
                        "Ignoring stale video failure epoch {epoch} (current {current_epoch})"
                    );
                    return;
                }
                // A failed open is often transient — storage stalling, a
                // volume dropping out, a file still being copied — so retry
                // from the current position before giving the cue up. A
                // genuinely damaged file exhausts the budget in a few fast
                // attempts and reports.
                if let Some(video) = self.show_engine.snapshot().video
                    && self.video_open_retries < MAX_VIDEO_OPEN_RETRIES
                {
                    self.video_open_retries += 1;
                    let attempt = self.video_open_retries;
                    log::warn!(
                        "Video decoder failed on '{}'; retrying from {:.2}s ({attempt}/{MAX_VIDEO_OPEN_RETRIES})",
                        video.path,
                        video.position_secs
                    );
                    let path = video.path.clone();
                    self.spawn_video_decode(&path, Some(video.position_secs), None, false);
                    return;
                }
                log::error!("Video decoder failed; stopping the current picture without looping");
                if let Some(video) = self.show_engine.snapshot().video {
                    // The operator needs this on screen, not only in the log:
                    // the visible symptom is a picture that stops for no
                    // stated reason, which reads as the show being broken.
                    let name = std::path::Path::new(&video.path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| video.path.clone());
                    self.cuepool
                        .state()
                        .lock_unpoisoned()
                        .report_operator_error(format!(
                            "Video stopped: '{name}' could not be read. Check the file and its storage."
                        ));
                    let failed_diagnostics = self
                        .cuepool
                        .state()
                        .lock_unpoisoned()
                        .diagnostics
                        .video
                        .clone();
                    let now = self.engine_now();
                    let actions = self.show_engine.event(
                        EngineEvent::VideoFailed {
                            instance_id: video.instance_id,
                            epoch: video.epoch,
                        },
                        now,
                    );
                    if let Err(error) = self.apply_engine_actions(actions, event_loop) {
                        log::error!("Video failure action failed: {error}");
                    }
                    if let Some(diagnostics) = failed_diagnostics {
                        self.cuepool.state().lock_unpoisoned().diagnostics.video =
                            Some(diagnostics);
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
                let error = "GPU device lost; CuePool cannot recover without a restart";
                log::error!("{error} — exiting");
                self.terminal_error = Some(TerminalError::Runtime(error));
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
        if self.shutdown_started_at.is_some() {
            return;
        }
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
        let is_status = self
            .status_window
            .as_ref()
            .is_some_and(|status| status.window.id() == window_id);

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
                    let dirty = self
                        .cuepool
                        .state()
                        .lock()
                        .map(|s| s.dirty)
                        .unwrap_or(false);
                    if !self.show_engine.snapshot().active_cues.is_empty() || dirty {
                        if let Ok(mut state) = self.cuepool.state().lock() {
                            state.pending_close_confirm = true;
                        }
                    } else {
                        self.hard_exit("control window closed");
                    }
                }
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        if let Some(config) = self.control_config.as_mut() {
                            config.width = size.width;
                            config.height = size.height;
                        }
                        if let Some(surface) = self.control_surface.as_ref()
                            && let Some(config) = self.control_config.as_ref()
                        {
                            let _configure_guard = self
                                .configure_gate
                                .write()
                                .unwrap_or_else(|e| e.into_inner());
                            surface.configure(&self.device, config);
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
                WindowEvent::KeyboardInput {
                    event: key_event, ..
                } if !egui_consumed && key_event.state == winit::event::ElementState::Pressed => {
                    // Toggle the video-output window fullscreen from the control window
                    // (Ctrl/Cmd+F or F11) so it works while operating the cue list.
                    // Creates the output window first if it isn't open yet.
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
                _ => {}
            }
        } else if is_status {
            let repaint = self
                .status_window
                .as_mut()
                .map(|status| {
                    status
                        .egui_state
                        .on_window_event(&status.window, &event)
                        .repaint
                })
                .unwrap_or(false);
            match event {
                WindowEvent::CloseRequested => {
                    self.status_window = None;
                    self.cuepool.state().lock_unpoisoned().show_status_window = false;
                }
                WindowEvent::Resized(size) => {
                    if let Some(status) = self.status_window.as_mut() {
                        status.resize(&self.device, &self.configure_gate, size);
                        status.window.request_redraw();
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Some(status) = self.status_window.as_mut() {
                        status.render(
                            &self.device,
                            &self.queue,
                            &self.configure_gate,
                            &mut self.cuepool,
                        );
                    }
                }
                _ => {
                    if repaint && let Some(status) = self.status_window.as_ref() {
                        status.window.request_redraw();
                    }
                }
            }
        } else if is_video {
            match event {
                WindowEvent::CloseRequested => {
                    self.output_windows.retain(|out| out.id != window_id);
                    if let Some(ids) = self.window_ids.as_mut() {
                        ids.video.retain(|id| *id != window_id);
                    }
                    if self.output_windows.is_empty()
                        && let Ok(mut state) = self.cuepool.state().lock()
                    {
                        state.show_video_window = false;
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey, PhysicalKey};
                        let is_esc = event.logical_key == Key::Named(NamedKey::Escape);
                        let is_f11 = matches!(
                            event.physical_key,
                            PhysicalKey::Code(winit::keyboard::KeyCode::F11)
                        );
                        let is_f = event.logical_key == Key::Character("f".into());
                        let has_ctrl = self.modifiers.control_key() || self.modifiers.super_key();

                        if let Some(out) =
                            self.output_windows.iter().find(|out| out.id == window_id)
                        {
                            // Esc always exits fullscreen
                            if is_esc {
                                out.window.set_fullscreen(None);
                                out.window.set_cursor_visible(true);
                            }
                            // F11, Ctrl+F, or Cmd+F toggles fullscreen
                            else if is_f11 || (is_f && has_ctrl) {
                                let currently = out.window.fullscreen().is_some();
                                if currently {
                                    out.window.set_fullscreen(None);
                                    out.window.set_cursor_visible(true);
                                } else {
                                    out.window.set_fullscreen(Some(
                                        winit::window::Fullscreen::Borderless(None),
                                    ));
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
                        out.size
                            .store(pack_size(size.width, size.height), Ordering::Relaxed);
                    }
                }
                // Output windows are presented by their own render threads;
                // RedrawRequested carries no work for them.
                _ => {}
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let about_started = Instant::now();
        self.dbg_ticks += 1;
        if let Some(api) = self.api.as_ref() {
            api.mark_alive();
        }
        if let Some(started_at) = self.shutdown_started_at {
            self.process_api_commands(event_loop);
            let response_delivered = self
                .api
                .as_ref()
                .is_some_and(ApiRuntime::shutdown_response_delivered);
            if response_delivered || started_at.elapsed() >= API_SHUTDOWN_EXIT_TIMEOUT {
                if !response_delivered {
                    log::warn!(
                        "API shutdown acknowledgement was not delivered within {} seconds; exiting",
                        API_SHUTDOWN_EXIT_TIMEOUT.as_secs()
                    );
                }
                self.hard_exit("authenticated API shutdown");
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(4),
            ));
            return;
        }
        // Quit confirmed by the in-app modal — hard-exit (see hard_exit for why).
        if self.cuepool.state().lock().map(|s| s.quit).unwrap_or(false) {
            self.hard_exit("operator confirmed discard and quit");
        }

        let show_status = self
            .cuepool
            .state()
            .lock()
            .map(|state| state.show_status_window)
            .unwrap_or(false);
        if show_status && self.status_window.is_none() {
            if let Err(error) = self.create_status_window(event_loop) {
                log::error!("Could not open Status window: {error:#}");
                self.cuepool.state().lock_unpoisoned().show_status_window = false;
            }
        } else if !show_status {
            self.status_window = None;
        }

        if !self.consume_failure_reported
            && self
                .consume_join
                .as_ref()
                .is_some_and(|join| join.is_finished())
        {
            const ERROR: &str = "video-consume thread exited unexpectedly; video output is frozen";
            self.consume_failure_reported = true;
            log::error!("{ERROR}");
            self.cuepool
                .state()
                .lock_unpoisoned()
                .diagnostics
                .consumer_error = Some(ERROR.into());
        }

        // A new or loaded project bumps project_generation — stop the old project's
        // cues and close its output windows.
        let project_generation = self.cuepool.state().lock_unpoisoned().project_generation;
        if project_generation != self.last_project_generation {
            self.last_project_generation = project_generation;
            self.reset_for_project_change(event_loop);
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
                .is_some_and(|built| {
                    projection_structure_changed(built, &state.show_file.projection)
                })
        };
        if rebuild_outputs {
            log::info!("Projection structure changed — rebuilding output windows");
            self.create_output_windows(event_loop);
        }

        self.process_midi_events(event_loop);
        // Timecode receive → drive any timecode-follow video cue → publish
        // status for the transport readout. A settings edit or project load
        // rebuilds the source when the chase config changed.
        {
            let state = self.cuepool.state().lock_unpoisoned();
            let settings = &state.show_file.show_settings;
            let config = (
                settings.timecode_source,
                settings.ltc_input_driver,
                settings.ltc_input_device.clone(),
                settings.ltc_input_channel,
            );
            if config != self.timecode_config {
                log::info!("[timecode] Chase source: {} ({})", config.0, config.2);
                drop(state);
                self.timecode_source = build_timecode_source(&config);
                self.timecode_config = config;
            }
        }
        self.timecode_source.refresh();
        self.timecode_source.tick();
        let tc = self.timecode_source.clone_state();
        self.drive_mtc_follow(&tc);
        // Keep the settings window's LTC device lists fresh (throttled scan,
        // scoped to each selected driver).
        let (ltc_in_driver, ltc_out_driver) = {
            let state = self.cuepool.state().lock_unpoisoned();
            let settings = &state.show_file.show_settings;
            (settings.ltc_input_driver, settings.ltc_output_driver)
        };
        let device_lists = if self.last_input_scan.elapsed().as_secs() >= 5 {
            self.last_input_scan = Instant::now();
            Some((
                cuepool_audio::list_input_devices(ltc_in_driver).unwrap_or_default(),
                AudioEngine::list_devices(ltc_out_driver)
                    .map(|devices| devices.into_iter().map(|d| d.name).collect())
                    .unwrap_or_default(),
            ))
        } else {
            None
        };
        if let Ok(mut state) = self.cuepool.state().lock() {
            state.mtc_running = tc.running;
            state.mtc_playing = tc.playing;
            state.mtc_timecode_secs = tc.position.as_seconds_f64();
            state.mtc_fps = tc.position.frame_rate.fps() as f64;
            state.mtc_source = tc.source_device.clone();
            state.mtc_drift_ms = self.mtc_drift.map(|d| d * 1000.0);
            if let Some((inputs, outputs)) = device_lists {
                state.ltc_input_devices = inputs;
                state.ltc_output_devices = outputs;
            }
        }

        // LTC generate: rebuild on settings change, retry while the device is
        // unavailable, and top up the output queue from the show clock.
        let ltc_out_config = {
            let state = self.cuepool.state().lock_unpoisoned();
            let s = &state.show_file.show_settings;
            (
                s.ltc_output_enabled,
                s.ltc_output_driver,
                s.ltc_output_device.clone(),
                s.ltc_output_channel,
                s.ltc_output_fps,
                s.ltc_output_start.as_secs_f64(),
            )
        };
        if ltc_out_config != self.ltc_out_config {
            log::info!(
                "[LTC-out] {} (device '{}')",
                if ltc_out_config.0 {
                    "enabled"
                } else {
                    "disabled"
                },
                ltc_out_config.2
            );
            self.ltc_out = open_ltc_output(&ltc_out_config);
            self.ltc_out_config = ltc_out_config;
            self.ltc_out_retry = Instant::now();
        }
        if self.ltc_out_config.0
            && self.ltc_out.is_none()
            && self.ltc_out_retry.elapsed().as_secs() >= 5
        {
            self.ltc_out_retry = Instant::now();
            self.ltc_out = open_ltc_output(&self.ltc_out_config);
        }
        if let Some((out, generator)) = &mut self.ltc_out
            && let Some(show_secs) = self.show_engine.snapshot().show_elapsed_secs
        {
            // ~100 ms lookahead so the callback never starves between ticks;
            // on a backwards jump flush the queue so the new position is
            // heard immediately. Paused/stopped clocks emit nothing — the
            // queue drains to silence and downstream gear holds.
            if generator.encode_up_to(show_secs + 0.1, &mut self.ltc_scratch) {
                out.clear();
            }
            out.push(&self.ltc_scratch);
            self.ltc_scratch.clear();
        }
        self.process_protocol_events();
        // Preserve arrival order across control surfaces: protocol/UI commands
        // already queued for this iteration execute before newer API requests.
        self.process_commands(event_loop);
        self.process_api_commands(event_loop);
        if self.shutdown_started_at.is_some() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(4),
            ));
            return;
        }
        self.poll_wall_clock_triggers(event_loop);
        self.poll_timecode_triggers(event_loop);
        self.tick_engine(event_loop);
        self.upload_pixmap_frames();

        // Lighting: sender lifecycle + fade advance + DMX submit (self-throttled).
        {
            let cfg = self
                .cuepool
                .state()
                .lock_unpoisoned()
                .show_file
                .lighting
                .clone();

            // Pixel-map segments: downsample each segment's source texture →
            // engine overlay. Throttled to the DMX rate.
            if cfg.enabled
                && self.last_pixel_sample.elapsed().as_secs_f32() >= 1.0 / cfg.fps.max(1.0)
            {
                // Raw (non-sRGB) views: bytes as stored, display-referred —
                // the colour pipeline's gamma does the linearisation.
                // The canvas lives on the consume thread; its linear view is
                // republished through the bundle on every recreate.
                // ponytail: the pixmap/sample GPU calls stay on the winit
                // thread (they only run when lighting pixel-map segments are
                // enabled, at the DMX rate — never in the failing video-only
                // rig config). Upgrade path: move sampling into the consume
                // thread if a lighting show ever hits the WSI stall.
                let canvas_view = self
                    .frame_state
                    .lock_unpoisoned()
                    .canvas_render_view
                    .clone();
                // A PixelMap-source segment owns its DMX addresses only while a
                // PixelMap cue is running. The texture outlives the cue, so
                // without this gate the segment keeps sampling the last (or
                // blanked) frame and writing it over those channels forever,
                // and no lighting cue can reach those fixtures again.
                let pixmap_view = self
                    .current_pixmap_qid
                    .is_some()
                    .then(|| self.pixmap_texture.as_ref().map(|t| t.render_view()))
                    .flatten();
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
                // Segments whose source went away stop owning their addresses:
                // drop the stale pixels so the overlay skips them next render.
                for id in cfg
                    .active_segments()
                    .map(|s| s.id)
                    .filter(|id| !batch.iter().any(|(_, sampled, ..)| sampled == id))
                    .collect::<Vec<_>>()
                {
                    self.lighting.clear_segment_pixels(id);
                }
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
                                state
                                    .lighting_preview
                                    .insert(*id, (*cols, *rows, rgba.clone()));
                            }
                        }
                        for (id, cols, rows, rgba) in ready {
                            self.lighting.set_segment_pixels(id, cols, rows, rgba);
                        }
                    }
                    let _configure_guard = configure_gate.read().unwrap_or_else(|e| e.into_inner());
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
            for out in &self.output_windows {
                log::warn!(
                    "Identify: '{}' = {}",
                    out.output_config.name,
                    IDENTIFY_COLOR_NAMES[out.configured_index % IDENTIFY_COLOR_NAMES.len()]
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
            ctl.identify = self
                .identify_until
                .is_some_and(|t| std::time::Instant::now() < t);
            if let Some(delta) = ctl.seek_show_delta.take() {
                self.show_engine.adjust_show_time(-delta);
            }
            let fade_done = ctl.fade.is_some_and(|(start, dur)| {
                fade_elapsed(start, ctl.pause_started).as_secs_f32() >= dur
            });
            drop(ctl);
            if fade_done {
                self.stop_video_playback();
            }
        }

        if self.dbg_last_log.elapsed() >= std::time::Duration::from_secs(1) {
            let secs = self.dbg_last_log.elapsed().as_secs_f64();
            let (
                starved_per_sec,
                uploads_per_sec,
                dropped_per_sec,
                consume_iters,
                consume_ticks,
                consume_coalesced,
                consume_max_iter_us,
            ) = {
                let mut ctl = self.video_control.lock_unpoisoned();
                (
                    std::mem::replace(&mut ctl.starved, 0) as f64 / secs,
                    std::mem::replace(&mut ctl.uploads, 0) as f64 / secs,
                    std::mem::replace(&mut ctl.dropped, 0) as f64 / secs,
                    std::mem::replace(&mut ctl.consume_iters, 0) as f64 / secs,
                    std::mem::replace(&mut ctl.consume_ticks, 0) as f64 / secs,
                    std::mem::replace(&mut ctl.consume_coalesced, 0) as f64 / secs,
                    std::mem::replace(&mut ctl.consume_max_iter_us, 0),
                )
            };
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
                    let presented_per_sec = out.presented.swap(0, Ordering::Relaxed) as f64 / secs;
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
            let mut state = self.cuepool.state().lock_unpoisoned();
            let d = &mut state.diagnostics;
            d.presented_per_sec = total_presented;
            d.starved_per_sec = starved_per_sec;
            d.uploads_per_sec = uploads_per_sec;
            d.dropped_per_sec = dropped_per_sec;
            d.event_loop_per_sec = ticks_per_sec;
            d.outputs = outputs;
            if self.fps_debug {
                let per_output = d
                    .outputs
                    .iter()
                    .map(|o| format!("'{}' {:.0}/s", o.name, o.presented_per_sec))
                    .collect::<Vec<_>>()
                    .join(" | ");
                // Same counters the Status window shows, on stderr for headless
                // capture: delivery rates plus the per-frame timing split.
                let timings = d.video.as_ref().map(|v| {
                    format!(
                        " | decode {:.2} ms | upload {:.2} ms | conv-submit {:.2} ms",
                        v.timings.decode.get_ms(),
                        v.timings.upload.get_ms(),
                        v.timings.conversion_submit.get_ms(),
                    )
                });
                eprintln!(
                    "VIDEO DIAG: loop {:.0}/s | uploads {:.0}/s | dropped {:.0}/s | starved {:.0}/s | presented {}{}",
                    ticks_per_sec,
                    uploads_per_sec,
                    dropped_per_sec,
                    starved_per_sec,
                    per_output,
                    timings.as_deref().unwrap_or(""),
                );
            }
            let mut rows = Vec::new();
            if wgpu::frame_pacing_diag::enabled() {
                let s = wgpu::frame_pacing_diag::snapshot_and_reset();
                let fmt = |b: &wgpu::frame_pacing_diag::BucketSnapshot| {
                    let m = |m: &wgpu::frame_pacing_diag::MetricSnapshot| {
                        if m.count == 0 {
                            "-".to_string()
                        } else {
                            format!(
                                "{:.1}/{:.1}ms x{}",
                                m.total_us as f64 / m.count as f64 / 1000.0,
                                m.max_us as f64 / 1000.0,
                                m.count
                            )
                        }
                    };
                    format!(
                        "lockwait {} | hal {} | acqhal {} | preshal {} | total {} | acqwait {} | acqhold {} | preswait {}",
                        m(&b.submit_fence_wait),
                        m(&b.submit_hal_call),
                        m(&b.acquire_hal_call),
                        m(&b.present_hal_call),
                        m(&b.submit_total),
                        m(&b.acquire_fence_wait),
                        m(&b.acquire_hold),
                        m(&b.present_fence_wait),
                    )
                };
                rows.push(("video-consume".to_string(), fmt(&s.consume)));
                rows.push(("output-render-*".to_string(), fmt(&s.render)));
                rows.push(("other threads".to_string(), fmt(&s.other)));
            }
            // CuePool-level pacing rows: where the consume thread's ticks went
            // and how long the main thread's two phases run. `coalesced` counts
            // vsync ticks observed late (each is a frame that could no longer
            // be shown); `max-iter` is the slowest consume-loop pass.
            rows.push((
                "consume-loop".to_string(),
                format!(
                    "iters {consume_iters:.0}/s | ticks {consume_ticks:.0}/s | coalesced {consume_coalesced:.0}/s | max-iter {:.1}ms",
                    consume_max_iter_us as f64 / 1000.0,
                ),
            ));
            rows.push((
                "main-loop".to_string(),
                format!(
                    "about_to_wait max {:.1}ms | render_control x{} max {:.1}ms | control-surface {}",
                    std::mem::replace(&mut self.dbg_about_max_us, 0) as f64 / 1000.0,
                    std::mem::replace(&mut self.dbg_render_count, 0),
                    std::mem::replace(&mut self.dbg_render_max_us, 0) as f64 / 1000.0,
                    if self.control_present_mode.is_empty() {
                        "?"
                    } else {
                        &self.control_present_mode
                    },
                ),
            ));
            if self.fps_debug {
                for (name, row) in &rows {
                    eprintln!("WGPU DIAG {name}: {row}");
                }
            }
            d.frame_pacing = rows;
            drop(state);
            if let Some(status) = self.status_window.as_ref() {
                status.window.request_redraw();
            }
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

        let us = about_started.elapsed().as_micros().min(u32::MAX as u128) as u32;
        self.dbg_about_max_us = self.dbg_about_max_us.max(us);

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

fn shutdown_rejection(dirty: bool, active: bool) -> Option<&'static str> {
    if active {
        Some("cues are active; stop playback before shutting down")
    } else if dirty {
        Some("the project has unsaved changes; save or discard them before shutting down")
    } else {
        None
    }
}

fn resolve_cli_project_path(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if !resolved
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("qproj"))
    {
        return Err(format!(
            "Cannot open CLI project '{}': expected a .qproj file",
            resolved.display()
        ));
    }
    if !resolved.is_file() {
        return Err(format!(
            "Cannot open CLI project '{}': path is not an existing file",
            resolved.display()
        ));
    }
    Ok(resolved)
}

const CLI_USAGE: &str = "Usage: cuepool [--zero-copy | --no-zero-copy] [--project <path> | <path>]";

#[derive(Debug, PartialEq, Eq)]
struct CliOptions {
    project: Option<PathBuf>,
    zero_copy: Option<bool>,
}

fn parse_cli(args: impl IntoIterator<Item = OsString>) -> Result<CliOptions, String> {
    let mut options = CliOptions {
        project: None,
        zero_copy: None,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == "--zero-copy" || arg == "--no-zero-copy" {
            if options.zero_copy.replace(arg == "--zero-copy").is_some() {
                return Err(format!(
                    "Only one of --zero-copy and --no-zero-copy may be provided. {CLI_USAGE}"
                ));
            }
            continue;
        }

        let path = if arg == "--project" {
            args.next()
                .ok_or_else(|| format!("Missing path after --project. {CLI_USAGE}"))?
        } else if arg.to_string_lossy().starts_with('-') {
            return Err(format!(
                "Unknown option '{}'. {CLI_USAGE}",
                arg.to_string_lossy()
            ));
        } else {
            arg
        };

        if options.project.replace(PathBuf::from(path)).is_some() {
            return Err(format!(
                "Only one project path may be provided. {CLI_USAGE}"
            ));
        }
    }
    Ok(options)
}

fn resolve_zero_copy_preference(
    cli_override: Option<bool>,
    env_fallback: ZeroCopyPreference,
) -> ZeroCopyPreference {
    match cli_override {
        Some(true) => ZeroCopyPreference::Enabled,
        Some(false) => ZeroCopyPreference::Disabled,
        None => env_fallback,
    }
}

fn prefer_dx12_backend(zero_copy_enabled: bool) -> bool {
    cfg!(windows) || zero_copy_enabled
}

fn load_startup_project(cuepool: &CuePoolApp, path: &Path) -> Result<(), String> {
    let data = std::fs::read_to_string(path)
        .map_err(|error| format!("Cannot read startup project '{}': {error}", path.display()))?;
    let mut state = cuepool.state().lock_unpoisoned();
    state
        .load_show_file(path, &data)
        .map_err(|error| format!("Cannot parse startup project '{}': {error}", path.display()))?;
    state.push_recent_file(path);
    Ok(())
}

fn startup_error(title: &str, message: String) -> anyhow::Error {
    log::error!("{message}");
    let _ = rfd::MessageDialog::new()
        .set_title(title)
        .set_description(&message)
        .set_level(rfd::MessageLevel::Error)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
    anyhow::anyhow!(message)
}

fn optional_feature_candidates(
    zero_copy: wgpu::Features,
    hap: wgpu::Features,
) -> Vec<wgpu::Features> {
    let mut candidates = Vec::with_capacity(4);
    for features in [zero_copy | hap, zero_copy, hap, wgpu::Features::empty()] {
        if !candidates.contains(&features) {
            candidates.push(features);
        }
    }
    candidates
}

fn main() -> anyhow::Result<()> {
    let profile = AppProfile::from_env()
        .map_err(|message| startup_error("Could not start CuePool", message))?;
    let log_file = match cuepool_gui::logging::init_logger(&profile.persistent_log_path()) {
        Ok(path) => path.display().to_string(),
        Err(error) => format!("unavailable: {error}"),
    };

    human_panic::setup_panic!(
        Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .authors("CuePool Contributors")
            .homepage("https://github.com/BlueJayLouche/CuePool")
    );
    let human_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log::error!(target: PERSIST_TARGET, "CuePool crashed: {info}");
        log::logger().flush();
        human_panic_hook(info);
    }));

    log::info!(
        target: PERSIST_TARGET,
        "CuePool {} starting: profile={}",
        cuepool_gui::build_identity(),
        profile.name()
    );
    let result = run(log_file, profile);
    match &result {
        Ok(()) => log::info!(target: PERSIST_TARGET, "CuePool shutdown complete"),
        Err(error) => log::error!(target: PERSIST_TARGET, "CuePool exiting with error: {error:#}"),
    }
    log::logger().flush();
    result
}

fn run(log_file: String, profile: AppProfile) -> anyhow::Result<()> {
    // Before anything opens media: otherwise FFmpeg writes to stderr, past the
    // log file and past the operator's ability to tell noise from a fault.
    cuepool_video::install_ffmpeg_logging();
    let argv: Vec<OsString> = std::env::args_os().collect();
    let cwd = std::env::current_dir().map_err(|error| {
        startup_error(
            "Could not start CuePool",
            format!("Cannot read the startup working directory: {error}"),
        )
    })?;
    log::info!(
        "CuePool startup: argv={argv:?}, working_directory={}",
        cwd.display()
    );

    let cli = parse_cli(argv.iter().skip(1).cloned())
        .map_err(|message| startup_error("Could not start CuePool", message))?;
    let project_path = cli
        .project
        .map(|path| resolve_cli_project_path(&path, &cwd))
        .transpose()
        .map_err(|message| startup_error("Could not open project", message))?;
    if let Some(path) = &project_path {
        log::info!("CuePool startup: resolved_project={}", path.display());
    } else {
        log::info!("CuePool startup: resolved_project=<none>");
    }

    let lock_name = profile.lock_name();
    let single = single_instance::SingleInstance::new(&lock_name).map_err(|error| {
        startup_error(
            "Could not start CuePool",
            format!("Cannot establish the CuePool single-instance guard: {error}"),
        )
    })?;
    if !single.is_single() {
        return Err(startup_error(
            "CuePool is already running",
            "Another instance of CuePool is already running; the project was not opened.".into(),
        ));
    }

    let cuepool = CuePoolApp::new();
    let settings = load_settings(&profile);
    {
        let mut state = cuepool.state().lock_unpoisoned();
        state.recent_files = settings.recent_files;
        state.last_seen_release_notes = settings.last_seen_release_notes;
    }
    if let Some(path) = &project_path {
        load_startup_project(&cuepool, path).map_err(|message| {
            startup_error(
                "Could not open project",
                format!("CuePool startup project load result=failure: {message}"),
            )
        })?;
        log::info!(
            target: PERSIST_TARGET,
            "CuePool startup project load result=success: path={}",
            path.display()
        );
    } else {
        log::info!(target: PERSIST_TARGET, "CuePool startup project load result=not_requested");
    }
    cuepool.state().lock_unpoisoned().diagnostics.log_file = log_file;

    // 1 ms timer resolution so WaitUntil/sleep don't quantize to 15.6 ms.
    #[cfg(windows)]
    win_timer::raise();

    let event_loop = EventLoop::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let proxy = event_loop.create_proxy();

    let make_instance = |backends| {
        wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        })
    };
    // Create a headless adapter first (we'll create surfaces after windows exist)
    let request_headless_adapter = |instance: &wgpu::Instance| {
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
    };
    let zero_copy_preference =
        resolve_zero_copy_preference(cli.zero_copy, ZeroCopyPreference::from_env());
    // Prefer DX12 on Windows because Vulkan FIFO presentation can stall shared
    // GPU work and make otherwise-fast video frames arrive late. Zero-copy also
    // requires DX12 so FFmpeg and wgpu can share the same ID3D12Device.
    let (instance, adapter) = {
        let mut selected = None;
        if prefer_dx12_backend(zero_copy_preference.enabled()) {
            let dx12 = make_instance(wgpu::Backends::DX12);
            match request_headless_adapter(&dx12) {
                Ok(adapter) => selected = Some((dx12, adapter)),
                Err(error) => {
                    log::warn!("No DX12 adapter ({error}); using stock GPU backend selection");
                }
            }
        }
        match selected {
            Some(selected) => selected,
            None => {
                let instance = make_instance(wgpu::Backends::all());
                let adapter = request_headless_adapter(&instance)
                    .map_err(|e| anyhow::anyhow!("no wgpu adapter: {e}"))?;
                (instance, adapter)
            }
        }
    };
    let zero_copy_features =
        ZeroCopyAvailability::required_features(&adapter, zero_copy_preference);
    let hap_features = if std::env::var("QPLAYER_NO_HWACCEL").as_deref() == Ok("1") {
        wgpu::Features::empty()
    } else {
        adapter.features() & wgpu::Features::TEXTURE_COMPRESSION_BC
    };
    let request_device = |features| {
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("cuepool-device"),
            // Required for the 10-bit planar (p10le) GPU path.
            required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM | features,
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        }))
    };
    let mut failures = Vec::new();
    let mut selected = None;
    for features in optional_feature_candidates(zero_copy_features, hap_features) {
        match request_device(features) {
            Ok((device, queue)) => {
                selected = Some((device, queue, features));
                break;
            }
            Err(error) => failures.push((features, error.to_string())),
        }
    }
    let Some((device, raw_queue, enabled_optional_features)) = selected else {
        let reason = failures
            .last()
            .map(|(_, reason)| reason.as_str())
            .unwrap_or("no device candidates were attempted");
        return Err(anyhow::anyhow!("GPU device request failed: {reason}"));
    };
    if !failures.is_empty() {
        log::warn!(
            "Video acceleration device negotiation selected {enabled_optional_features:?} after {} failed request(s)",
            failures.len()
        );
    }
    let negotiation_reason = |label: &str| {
        let attempted = failures
            .iter()
            .map(|(features, reason)| format!("{features:?}: {reason}"))
            .collect::<Vec<_>>()
            .join("; ");
        format!("{label} device feature request failed ({attempted})")
    };
    let queue = Arc::new(cuepool_video::SharedQueue::new(raw_queue));
    let zero_copy = if zero_copy_features.is_empty()
        || enabled_optional_features.contains(zero_copy_features)
    {
        ZeroCopyAvailability::finish(&adapter, &device, queue.queue(), zero_copy_preference)
    } else {
        ZeroCopyAvailability::declined(negotiation_reason("zero-copy"))
    };
    if zero_copy_preference.enabled()
        && let Some(reason) = zero_copy.fallback_reason()
    {
        log::warn!("Video zero-copy fallback: {reason}; using the stock readback path");
    }
    let hap_acceleration =
        if !hap_features.is_empty() && enabled_optional_features.contains(hap_features) {
            HapAcceleration::available(device.limits().max_texture_dimension_2d)
        } else if !hap_features.is_empty() {
            HapAcceleration::unavailable(negotiation_reason("GPU-native HAP"))
        } else {
            HapAcceleration::unavailable(
                "GPU-native HAP unavailable: GPU device lacks BC texture compression",
            )
        };
    let device_lost_proxy = proxy.clone();
    device.set_device_lost_callback(move |reason, message| {
        log::error!("GPU device lost ({reason:?}): {message}");
        if device_lost_proxy.send_event(AppEvent::DeviceLost).is_err() {
            log::error!("Cannot report GPU device loss: event loop closed");
        }
    });

    let mut app = App::new(
        instance,
        adapter,
        device,
        queue,
        proxy,
        zero_copy,
        hap_acceleration,
        cuepool,
        profile,
    );

    // Ctrl-C / SIGTERM handler for graceful emergency save
    {
        let state = Arc::clone(app.cuepool.state());
        let profile = app.profile.clone();
        ctrlc::set_handler(move || {
            log::info!(target: PERSIST_TARGET, "Shutdown requested: termination signal");
            emergency_save(&state, "termination signal");
            save_settings_from_state(&profile, &state);
            log::info!(target: PERSIST_TARGET, "Shutdown complete: termination signal");
            log::logger().flush();
            std::process::exit(0);
        })?;
    }

    event_loop.run_app(&mut app)?;
    if let Some(api) = app.api.as_ref() {
        api.mark_stopping();
    }

    // Save persisted settings
    save_settings_from_state(&app.profile, app.cuepool.state());

    // Graceful exit (never reached via hard_exit, which process::exit()s):
    // stop and join the consume thread like the render threads.
    app.video_stop_flag.store(true, Ordering::Relaxed);
    app.pending_video_decode = None;
    if let Some(join) = app.video_decode_join.take() {
        let _ = join.join();
    }
    app.consume_stop.store(true, Ordering::Relaxed);
    if let Some(join) = app.consume_join.take() {
        let _ = join.join();
    }
    // Signal autosave thread to stop
    app.autosave_running.store(false, Ordering::Relaxed);

    #[cfg(windows)]
    win_timer::release();

    if let Some(error) = app.terminal_error {
        match error {
            TerminalError::Startup(message) => {
                return Err(startup_error("Could not start CuePool", message));
            }
            TerminalError::Runtime(message) => anyhow::bail!("{message}"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A timecode source runs to its own length, which is routinely longer
    /// than the clip being chased. Seeking past the media puts the demuxer off
    /// the end of the file, so the chase target is clamped like every other
    /// seek path.
    #[test]
    fn seeks_clamp_to_the_media_length() {
        let length = Some(230.02);
        assert_eq!(clamp_video_seek_secs(12.0, length), 12.0);
        assert!(clamp_video_seek_secs(500.0, length) < 230.02);
        assert!(clamp_video_seek_secs(230.02, length) < 230.02);
        // Unknown duration cannot be clamped, only sanitised.
        assert_eq!(clamp_video_seek_secs(500.0, None), 500.0);
        assert_eq!(clamp_video_seek_secs(f64::INFINITY, None), 0.0);
    }

    #[test]
    fn control_surface_retry_backoff_is_capped() {
        let delays: Vec<_> = (1..=9)
            .map(|failures| control_surface_retry_delay(failures).as_millis())
            .collect();
        assert_eq!(delays, [100, 200, 400, 800, 1600, 3200, 5000, 5000, 5000]);
    }

    #[test]
    fn optional_gpu_features_retry_independent_acceleration_paths() {
        let zero_copy = wgpu::Features::TIMESTAMP_QUERY;
        let hap = wgpu::Features::TEXTURE_COMPRESSION_BC;

        assert_eq!(
            optional_feature_candidates(zero_copy, hap),
            vec![zero_copy | hap, zero_copy, hap, wgpu::Features::empty()]
        );
        assert_eq!(
            optional_feature_candidates(wgpu::Features::empty(), hap),
            vec![hap, wgpu::Features::empty()]
        );
    }

    #[test]
    fn winit_drain_requeues_gui_file_commands() {
        let state = Arc::new(Mutex::new(cuepool_gui::SharedState::default()));
        state.lock().unwrap().command_queue.extend([
            AppCommand::OpenProject {
                path: "open.qproj".into(),
            },
            AppCommand::SaveProject,
            AppCommand::SaveProjectAs {
                path: "save-as.qproj".into(),
            },
            AppCommand::Go,
        ]);

        let mut handled = 0;
        drain_app_commands(&state, |command| match command {
            AppCommand::Go => {
                handled += 1;
                Ok(())
            }
            other => Err(other),
        });

        assert_eq!(handled, 1);
        let state = state.lock().unwrap();
        assert!(matches!(
            state.command_queue.as_slice(),
            [
                AppCommand::OpenProject { .. },
                AppCommand::SaveProject,
                AppCommand::SaveProjectAs { .. }
            ]
        ));
    }

    #[test]
    fn remote_discovery_requires_remote_control() {
        let disabled = cuepool_core::ShowSettings {
            node_name: "stage-left".into(),
            ..Default::default()
        };
        assert!(remote_discovery_message(&disabled).is_none());

        let enabled = cuepool_core::ShowSettings {
            enable_remote_control: true,
            ..disabled
        };
        let message = remote_discovery_message(&enabled).unwrap();
        assert_eq!(message.addr, "/qplayer/remote/discovery");
        assert_eq!(
            message.args,
            vec![rosc::OscType::String("stage-left".into())]
        );
    }

    #[test]
    fn unattended_shutdown_requires_an_idle_clean_project() {
        assert_eq!(shutdown_rejection(false, false), None);
        assert_eq!(
            shutdown_rejection(false, true),
            Some("cues are active; stop playback before shutting down")
        );
        assert_eq!(
            shutdown_rejection(true, false),
            Some("the project has unsaved changes; save or discard them before shutting down")
        );
    }

    fn cli_project_test_dir() -> PathBuf {
        static NEXT_TEST_DIR: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let unique = NEXT_TEST_DIR.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("cuepool-cli-path-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn cli_accepts_explicit_and_legacy_project_paths() {
        let path = PathBuf::from("Opening Night.qproj");
        assert_eq!(
            parse_cli([OsString::from("--project"), path.clone().into_os_string()]),
            Ok(CliOptions {
                project: Some(path.clone()),
                zero_copy: None,
            })
        );
        assert_eq!(
            parse_cli([path.clone().into_os_string()]),
            Ok(CliOptions {
                project: Some(path),
                zero_copy: None,
            })
        );
        assert_eq!(
            parse_cli([]),
            Ok(CliOptions {
                project: None,
                zero_copy: None,
            })
        );
    }

    #[test]
    fn cli_rejects_invalid_argument_shapes() {
        let missing = parse_cli([OsString::from("--project")]).unwrap_err();
        assert!(missing.contains("Missing path"), "{missing}");

        let unknown = parse_cli([OsString::from("--wat")]).unwrap_err();
        assert!(unknown.contains("Unknown option"), "{unknown}");

        let duplicate = parse_cli([
            OsString::from("one.qproj"),
            OsString::from("--project"),
            OsString::from("two.qproj"),
        ])
        .unwrap_err();
        assert!(duplicate.contains("Only one project"), "{duplicate}");

        let extra =
            parse_cli([OsString::from("one.qproj"), OsString::from("two.qproj")]).unwrap_err();
        assert!(extra.contains("Only one project"), "{extra}");
    }

    #[test]
    fn cli_zero_copy_controls_override_the_environment_fallback() {
        assert_eq!(
            parse_cli([OsString::from("--zero-copy")])
                .unwrap()
                .zero_copy,
            Some(true)
        );
        assert_eq!(
            parse_cli([OsString::from("--no-zero-copy")])
                .unwrap()
                .zero_copy,
            Some(false)
        );
        assert_eq!(
            resolve_zero_copy_preference(Some(true), ZeroCopyPreference::Disabled),
            ZeroCopyPreference::Enabled
        );
        assert_eq!(
            resolve_zero_copy_preference(Some(false), ZeroCopyPreference::Enabled),
            ZeroCopyPreference::Disabled
        );
        assert_eq!(
            resolve_zero_copy_preference(None, ZeroCopyPreference::Enabled),
            ZeroCopyPreference::Enabled
        );

        let conflict = parse_cli([
            OsString::from("--zero-copy"),
            OsString::from("--no-zero-copy"),
        ])
        .unwrap_err();
        assert!(conflict.contains("Only one of"), "{conflict}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_prefers_dx12_without_enabling_zero_copy() {
        assert!(prefer_dx12_backend(false));
    }

    #[test]
    fn cli_project_path_preserves_absolute_and_resolves_relative_paths() {
        let cwd = cli_project_test_dir();
        let relative = PathBuf::from("show files").join("Opening Night.QPROJ");
        let absolute = cwd.join(&relative);
        std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        std::fs::write(&absolute, b"{}").unwrap();

        assert!(absolute.is_absolute());
        assert_eq!(
            resolve_cli_project_path(&absolute, &cwd),
            Ok(absolute.clone())
        );
        assert_eq!(resolve_cli_project_path(&relative, &cwd), Ok(absolute));

        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn missing_cli_project_error_names_resolved_path() {
        let cwd = cli_project_test_dir();
        let resolved = cwd.join("missing project.qproj");

        let error = resolve_cli_project_path(Path::new("missing project.qproj"), &cwd).unwrap_err();

        assert!(error.contains(&resolved.display().to_string()), "{error}");
        std::fs::remove_dir_all(cwd).unwrap();
    }

    #[test]
    fn startup_project_load_populates_project_path() {
        let dir = cli_project_test_dir();
        let path = dir.join("startup.qproj");
        let json = serde_json::to_string(&cuepool_core::ShowFile::default()).unwrap();
        std::fs::write(&path, json).unwrap();
        let cuepool = CuePoolApp::new();

        load_startup_project(&cuepool, &path).unwrap();

        let state = cuepool.state().lock_unpoisoned();
        assert_eq!(state.project_path.as_deref(), Some(path.as_path()));
        assert_eq!(state.recent_files.first(), Some(&path));
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn corrupt_startup_project_fails_without_populating_project_path() {
        let dir = cli_project_test_dir();
        let path = dir.join("corrupt.qproj");
        std::fs::write(&path, "{").unwrap();
        let cuepool = CuePoolApp::new();

        let error = load_startup_project(&cuepool, &path).unwrap_err();

        assert!(error.contains("Cannot parse startup project"), "{error}");
        assert!(cuepool.state().lock_unpoisoned().project_path.is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn video_seek_clock_freezes_only_when_paused() {
        let now = Instant::now();
        let target = 3.25;

        let (clock, pause_started) = video_seek_clock(now, target, false).unwrap();
        assert_eq!(pause_started, None);
        assert_eq!(now.duration_since(clock), Duration::from_secs_f64(target));

        let (clock, pause_started) = video_seek_clock(now, target, true).unwrap();
        assert_eq!(pause_started, Some(now));
        assert_eq!(now.duration_since(clock), Duration::from_secs_f64(target));
    }

    #[test]
    fn video_start_clock_keeps_audio_anchor_after_output_setup() {
        let now = Instant::now();
        let clock_origin = Duration::from_secs(10);
        let engine_now = clock_origin + Duration::from_millis(275);
        let media_offset = 3.25;

        let clock = video_start_clock(now, media_offset, engine_now, clock_origin).unwrap();

        assert_eq!(
            now.duration_since(clock),
            Duration::from_secs_f64(media_offset) + Duration::from_millis(275)
        );
    }

    #[test]
    fn video_seek_translates_between_cue_and_media_timelines() {
        assert_eq!(video_media_secs(3.25, 10.0), 13.25);
        assert_eq!(video_timeline_secs(13.25, 10.0), 3.25);
        assert_eq!(video_timeline_secs(5.0, 10.0), 0.0);
        assert_eq!(video_timeline_secs(40.0, 10.0), 30.0);
    }

    #[test]
    fn video_decode_gate_keeps_only_the_latest_request_and_one_worker() {
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut join = Some(std::thread::spawn(move || {
            let _ = release_rx.recv();
        }));
        let mut pending = None;
        queue_latest_video_decode(
            &mut pending,
            VideoDecodeRequest {
                path: "first.mp4".into(),
                start_before: Some(1.0),
                seek_frame: None,
                clamp_to_media: true,
                hap_fallback_session: HapFallbackSession::default(),
            },
        );
        queue_latest_video_decode(
            &mut pending,
            VideoDecodeRequest {
                path: "latest.mp4".into(),
                start_before: Some(2.0),
                seek_frame: None,
                clamp_to_media: true,
                hap_fallback_session: HapFallbackSession::default(),
            },
        );

        assert!(take_ready_video_decode(&mut join, &mut pending).is_none());
        assert_eq!(pending.as_ref().unwrap().path, "latest.mp4");

        release_tx.send(()).unwrap();
        while !join.as_ref().unwrap().is_finished() {
            std::thread::yield_now();
        }
        let request = take_ready_video_decode(&mut join, &mut pending).unwrap();
        assert_eq!(request.path, "latest.mp4");
        assert!(join.is_none());
    }
}
