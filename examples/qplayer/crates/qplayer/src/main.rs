//! QPlayer binary — custom winit event loop with dual windows.
//!
//! - Control window: egui UI (replaces eframe)
//! - Video output window: wgpu fullscreen blit (lazy-created on first video)
//! - Audio engine: cpal output with master clock for A/V sync
//! - Video decode: background thread that sleeps until frame PTS, then sends
//!   frame to main thread via winit user event.

use qplayer_audio::{AudioEngine, FileDecoder, SampleProvider};
use qplayer_gui::{AppCommand, QPlayerApp, SharedStateHandle};
use qplayer_gui::app::CueState;
use qplayer_protocols::msc::{MscCommandFlags, MscEvent, MscManager};
use qplayer_protocols::osc::{OscEvent, OscManager};
use qplayer_video::{Renderer, Texture, VideoFrame, VideoSource};
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use human_panic::Metadata;

mod plugin_manager;

/// User events sent to the main event loop from background threads.
#[derive(Debug)]
enum AppEvent {
    /// A decoded video frame ready for display.
    VideoFrame(VideoFrame),
    /// Video stream reached EOF.
    VideoEof,
}

/// Per-window identifiers so we can route events.
struct WindowIds {
    control: WindowId,
    video: Option<WindowId>,
}

#[derive(Clone)]
struct ActiveCue {
    qid: rust_decimal::Decimal,
    name: String,
    input: std::sync::Arc<qplayer_audio::MixerInput>,
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
    fade_type: qplayer_core::FadeType,
    fade_out_started: bool,
}

/// A cue that is waiting for its delay timer to expire before playing.
struct DelayedCue {
    cue: qplayer_core::Cue,
    start_at: std::time::Instant,
}

struct App {
    // ── wgpu core ──
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,

    // ── control window (egui) ──
    control_window: Option<Arc<Window>>,
    control_surface: Option<wgpu::Surface<'static>>,
    control_config: Option<wgpu::SurfaceConfiguration>,

    // ── video window (wgpu blit) ──
    video_window: Option<Arc<Window>>,
    video_surface: Option<wgpu::Surface<'static>>,
    video_config: Option<wgpu::SurfaceConfiguration>,

    // ── egui ──
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    egui_renderer: Option<egui_wgpu::Renderer>,

    // ── app state ──
    qplayer: QPlayerApp,
    window_ids: Option<WindowIds>,

    // ── audio ──
    audio_engine: AudioEngine,
    active_cues: Vec<ActiveCue>,
    delayed_cues: Vec<DelayedCue>,
    paused: bool,
    show_start_time: Option<Instant>,
    triggered_timecodes: Vec<rust_decimal::Decimal>,

    // ── video playback ──
    event_loop_proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    video_texture: Option<Texture>,
    video_renderer: Option<Renderer>,
    latest_video_frame: Option<VideoFrame>,
    video_frame_dirty: bool,
    video_start_clock: Option<Duration>,
    video_stop_flag: Arc<AtomicBool>,
    video_pause_flag: Arc<AtomicBool>,
    /// QID of the cue whose video is currently playing (for loop sync).
    current_video_qid: Option<rust_decimal::Decimal>,

    // ── protocols ──
    osc_manager: Option<OscManager>,
    osc_rx: Option<std::sync::mpsc::Receiver<OscEvent>>,
    #[allow(dead_code)]
    msc_manager: Option<MscManager>,
    msc_rx: Option<std::sync::mpsc::Receiver<MscEvent>>,
    last_discovery: Instant,

    // ── polish ──
    last_window_title: String,
    autosave_running: Arc<AtomicBool>,
    modifiers: winit::keyboard::ModifiersState,

    // ── plugins ──
    plugin_manager: Option<plugin_manager::PluginManager>,
    last_slow_update: Instant,
}

impl App {
    fn new(
        instance: wgpu::Instance,
        adapter: wgpu::Adapter,
        device: wgpu::Device,
        queue: wgpu::Queue,
        proxy: winit::event_loop::EventLoopProxy<AppEvent>,
    ) -> Self {
        let audio_engine = AudioEngine::new_default().expect("audio engine init failed");
        let qplayer = QPlayerApp::new();

        // Sync audio device info into GUI state
        {
            let devices: Vec<String> = AudioEngine::list_devices().into_iter().map(|(n, _)| n).collect();
            let device_name = audio_engine.device_name().to_string();
            if let Ok(mut state) = qplayer.state().lock() {
                state.audio_devices = devices;
                state.audio_device_name = device_name;
            }
        }

        // Protocol settings from project settings (fallback to defaults)
        let (nic, subnet, osc_rx_port, osc_tx_port, is_remote_host, enable_remote_control) = {
            match qplayer.state().lock() {
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
                            qplayer_protocols::msc::MscData::Go { qid, executor, page } => {
                                Some(MscEvent::Go { qid: qid.clone(), executor: *executor, page: *page })
                            }
                            qplayer_protocols::msc::MscData::TimedGo { qid, executor, page, time } => {
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

        let autosave_running = Arc::new(AtomicBool::new(true));
        spawn_autosave_thread(Arc::clone(&qplayer.state()), Arc::clone(&autosave_running));

        let mut plugin_manager = plugin_manager::PluginManager::new().ok();
        if let Some(pm) = plugin_manager.as_mut() {
            let exe_dir = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            pm.load_from_dir(&exe_dir.join("plugins"));
        }

        Self {
            instance,
            adapter,
            device,
            queue,
            control_window: None,
            control_surface: None,
            control_config: None,
            video_window: None,
            video_surface: None,
            video_config: None,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            egui_renderer: None,
            qplayer,
            window_ids: None,
            audio_engine,
            event_loop_proxy: proxy,
            video_texture: None,
            video_renderer: None,
            latest_video_frame: None,
            video_frame_dirty: false,
            video_start_clock: None,
            video_stop_flag: Arc::new(AtomicBool::new(false)),
            video_pause_flag: Arc::new(AtomicBool::new(false)),
            current_video_qid: None,
            osc_manager,
            osc_rx,
            msc_manager,
            msc_rx,
            last_discovery: Instant::now(),
            last_window_title: String::new(),
            autosave_running,
            plugin_manager,
            last_slow_update: Instant::now(),
            active_cues: Vec::new(),
            delayed_cues: Vec::new(),
            paused: false,
            show_start_time: None,
            triggered_timecodes: Vec::new(),
            modifiers: winit::keyboard::ModifiersState::empty(),
        }
    }

    /// Create the control window + surface + egui state.
    fn create_control_window(&mut self, event_loop: &ActiveEventLoop) {
        let window = Arc::new(
            event_loop
                .create_window(
                    winit::window::WindowAttributes::default()
                        .with_title("QPlayer")
                        .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0)),
                )
                .expect("create control window"),
        );

        let surface = self
            .instance
            .create_surface(Arc::clone(&window))
            .expect("create control surface");

        let size = window.inner_size();
        let config = surface
            .get_default_config(&self.adapter, size.width, size.height)
            .expect("control surface config");
        surface.configure(&self.device, &config);

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
            None,
            1,
            false,
        );

        let control_id = window.id();
        self.control_window = Some(window);
        self.control_surface = Some(surface);
        self.control_config = Some(config);
        self.egui_state = Some(egui_state);
        self.egui_renderer = Some(egui_renderer);

        let video_id = self.video_window.as_ref().map(|w| w.id());
        self.window_ids = Some(WindowIds {
            control: control_id,
            video: video_id,
        });
    }

    /// Toggle fullscreen on the video window and update cursor visibility.
    fn toggle_video_fullscreen(&self) {
        if let Some(window) = self.video_window.as_ref() {
            let currently_fullscreen = window.fullscreen().is_some();
            if currently_fullscreen {
                window.set_fullscreen(None);
                window.set_cursor_visible(true);
            } else {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
                window.set_cursor_visible(false);
            }
        }
    }

    /// Create (or recreate) the video output window (starts windowed).
    fn create_video_window(&mut self, event_loop: &ActiveEventLoop) {
        if self.video_window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    winit::window::WindowAttributes::default()
                        .with_title("QPlayer Video Output")
                        .with_visible(true),
                )
                .expect("create video window"),
        );

        let surface = self
            .instance
            .create_surface(Arc::clone(&window))
            .expect("create video surface");

        let size = window.inner_size();
        let config = surface
            .get_default_config(&self.adapter, size.width, size.height)
            .expect("video surface config");
        surface.configure(&self.device, &config);

        let video_id = window.id();
        self.video_window = Some(window);
        self.video_surface = Some(surface);
        self.video_config = Some(config);

        if let Some(ids) = self.window_ids.as_mut() {
            ids.video = Some(video_id);
        }
    }



    /// Handle a `Go` command: start audio (and video if cue is VideoCue).
    /// Also handles `WithLast` trigger mode for subsequent cues.
    fn handle_go(&mut self, event_loop: &ActiveEventLoop) {
        // Start the show clock on first Go
        if self.show_start_time.is_none() {
            self.show_start_time = Some(Instant::now());
            self.triggered_timecodes.clear();
        }

        let (start_qid, start_idx) = {
            let state = self.qplayer.state().lock().unwrap();
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

        let qid_i32: i32 = start_qid.try_into().unwrap_or(0);
        if let Some(pm) = self.plugin_manager.as_mut() {
            pm.on_go(qid_i32);
        }

        // Play the selected cue and all consecutive WithLast followers
        let cues_to_play = {
            let state = self.qplayer.state().lock().unwrap();
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
                if i == start_idx || cue.base().trigger == qplayer_core::TriggerMode::WithLast {
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

        // Check for AfterLast cues and schedule them
        let after_last = {
            let state = self.qplayer.state().lock().unwrap();
            let mut after_last_qids = Vec::new();
            for i in (start_idx + 1)..state.show_file.cues.len() {
                let cue = &state.show_file.cues[i];
                if cue.base().trigger == qplayer_core::TriggerMode::AfterLast {
                    after_last_qids.push(cue.base().qid);
                } else {
                    break;
                }
            }
            after_last_qids
        };
        for qid in after_last {
            log::info!("AfterLast cue Q{} scheduled (TODO: auto-trigger when previous finishes)", qid);
        }
    }

    fn play_cue(&mut self, cue: &qplayer_core::Cue, event_loop: &ActiveEventLoop) {
        if !cue.enabled() {
            log::info!("Skipping disabled cue Q{}", cue.base().qid);
            return;
        }

        let qid = cue.base().qid;
        let name = cue.base().name.clone();
        let delay = cue.base().delay;

        // Remote cue delegation: if remote_node is set and not local, send OSC instead
        let remote_node = cue.base().remote_node.clone();
        if !remote_node.is_empty() {
            let (enable_remote, local_name) = {
                let Ok(state) = self.qplayer.state().lock() else { return; };
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

        // If cue has a delay, schedule it instead of playing immediately
        if delay.as_secs_f64() > 0.0 {
            log::info!("Delaying cue Q{} by {:.2}s", qid, delay.as_secs_f64());
            self.delayed_cues.push(DelayedCue {
                cue: cue.clone(),
                start_at: std::time::Instant::now() + std::time::Duration::from_secs_f64(delay.as_secs_f64()),
            });
            return;
        }

        // Check if cue is already preloaded — if so, just activate it
        if let Some(idx) = self.active_cues.iter().position(|ac| ac.qid == qid && ac.state == CueState::Ready) {
            let ac = &mut self.active_cues[idx];
            ac.input.set_active(true);
            let new_state = if cue.base().loop_mode == qplayer_core::LoopMode::Looped || cue.base().loop_mode == qplayer_core::LoopMode::LoopedInfinite {
                CueState::PlayingLooped
            } else {
                CueState::Playing
            };
            ac.state = new_state;
            log::info!("Activated preloaded cue Q{}", qid);
            return;
        }

        match cue {
            qplayer_core::Cue::Sound { path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, routing, .. } => {
                log::info!("Go SoundCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, *start_time, *duration, *volume, *fade_in, *fade_out, *fade_type, *eq, *pan, routing.clone(), false);
            }
            qplayer_core::Cue::Video { path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, routing, .. } => {
                log::info!("Go VideoCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, *start_time, *duration, *volume, *fade_in, *fade_out, *fade_type, *eq, *pan, routing.clone(), false);
                self.play_video(path, qid, event_loop);
            }
            qplayer_core::Cue::Stop { stop_qid, fade_out_time, fade_type, .. } => {
                log::info!("Go StopCue -> stop Q{}", stop_qid);
                self.handle_stop_cue(*stop_qid, *fade_out_time, *fade_type);
            }
            qplayer_core::Cue::Volume { sound_qid, volume, fade_time, fade_type, .. } => {
                log::info!("Go VolumeCue -> adjust Q{} to {:.1} dB", sound_qid, 20.0 * volume.log10());
                self.handle_volume_cue(*sound_qid, *volume, *fade_time, *fade_type);
            }
            qplayer_core::Cue::Osc { command, .. } => {
                log::info!("Go OSCCue: {}", command);
                if let Some(osc) = &self.osc_manager {
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
            qplayer_core::Cue::Group { .. } => {
                // A group "owns" the consecutive cues after it, up to the next group.
                // Going the group fires that whole block (each via the normal play
                // path, so per-cue delay and the enabled flag still apply).
                let members: Vec<qplayer_core::Cue> = {
                    let state = self.qplayer.state().lock().unwrap();
                    match state.show_file.cues.iter().position(|c| c.base().qid == qid) {
                        Some(idx) => state.show_file.cues[idx + 1..]
                            .iter()
                            .take_while(|c| !matches!(c, qplayer_core::Cue::Group { .. }))
                            .cloned()
                            .collect(),
                        None => Vec::new(),
                    }
                };
                log::info!("Go GroupCue Q{} — firing {} member(s)", qid, members.len());
                for member in members {
                    self.play_cue(&member, event_loop);
                }
            }
            other => {
                log::info!("Go on unsupported cue type: {:?}", std::mem::discriminant(other));
            }
        }
    }

    /// Resolve a file path: try absolute, then relative to project, then search project tree.
    fn resolve_path(&self, path: &str) -> Option<String> {
        let p = std::path::Path::new(path);
        if p.is_absolute() && p.exists() {
            return Some(path.to_string());
        }
        // Try relative to project directory
        let project_dir = self.qplayer.state().lock().ok()?.project_path
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
        loop_mode: qplayer_core::LoopMode,
        loop_count: i32,
        start_time: qplayer_core::Timespan,
        duration: qplayer_core::Timespan,
        volume: f32,
        fade_in: f32,
        fade_out: f32,
        fade_type: qplayer_core::FadeType,
        eq: Option<qplayer_core::EQSettings>,
        pan: f32,
        routing: qplayer_core::AudioRouting,
        preload_only: bool,
    ) {
        let resolved = self.resolve_path(path).unwrap_or_else(|| path.to_string());
        if resolved != path {
            log::info!("Resolved path '{}' -> '{}'", path, resolved);
        }
        match FileDecoder::open(&resolved) {
            Ok(decoder) => {
                let sample_rate = decoder.sample_rate();
                // input.position()/length() are reported in device-rate samples (post-resample),
                // so anything compared against them (loop bounds, fade trigger) must scale too.
                let out_scale = self.audio_engine.sample_rate() as f64 / sample_rate as f64;
                let start_frame = (start_time.as_secs_f64() * sample_rate as f64) as u64;
                let end_frame = if duration.as_secs_f64() > 0.0 {
                    start_frame + (duration.as_secs_f64() * sample_rate as f64) as u64
                } else {
                    0 // auto-detect from source length
                };

                // Create a shared loop counter so the main thread can detect loop boundaries
                // and synchronise video restarts + progress-bar resets.
                let is_looped = loop_mode == qplayer_core::LoopMode::Looped
                    || loop_mode == qplayer_core::LoopMode::LoopedInfinite;
                let loop_counter: Option<std::sync::Arc<std::sync::atomic::AtomicU32>> = if is_looped {
                    Some(std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0)))
                } else {
                    None
                };

                let loop_proc = {
                    let proc = qplayer_audio::LoopProcessor::new(Box::new(decoder));
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
                    source = Box::new(qplayer_audio::EqProcessor::new(source, eq_settings));
                }

                // Wire fade processor for fade-in
                if fade_in > 0.0 {
                    let fade_proc = qplayer_audio::FadeProcessor::new(source, 0.0);
                    let fade_in_frames = (fade_in * sample_rate as f32) as u32;
                    fade_proc.start_fade(1.0, fade_in_frames, fade_type);
                    source = Box::new(fade_proc);
                }

                let input = self.audio_engine.play(source);
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
                });
            }
            Err(e) => {
                if let qplayer_audio::DecodeError::NoAudioTrack = e {
                    log::info!("No audio stream in {} — playing silent", path);
                } else {
                    log::error!("Failed to open audio for {}: {}", path, e);
                }
            }
        }
    }

    fn handle_stop_cue(&mut self, stop_qid: rust_decimal::Decimal, fade_out_time: f32, fade_type: qplayer_core::FadeType) {
        let idx = self.active_cues.iter().position(|ac| ac.qid == stop_qid);
        if let Some(idx) = idx {
            let input = &self.active_cues[idx].input;
            if fade_out_time > 0.0 {
                let sample_rate = self.audio_engine.sample_rate();
                let fade_frames = (fade_out_time * sample_rate as f32) as u32;
                input.start_fade(0.0, fade_frames.max(1), fade_type);
                log::info!("Fade-out Q{} over {} frames", stop_qid, fade_frames);
            } else {
                input.set_active(false);
                input.set_volume(0.0);
                self.active_cues[idx].state = CueState::Done;
            }
        } else {
            log::warn!("StopCue target Q{} not found in active cues", stop_qid);
        }
    }

    /// Check if an incoming remote OSC command targets this node.
    fn is_remote_target_match(&self, target: &str) -> bool {
        let local_name = {
            let Ok(state) = self.qplayer.state().lock() else { return false; };
            state.show_file.show_settings.node_name.clone()
        };
        target == local_name || target == "*"
    }

    /// Preload the selected cue: decode and add to mixer as inactive (Ready state).
    fn handle_preload(&mut self, _event_loop: &ActiveEventLoop) {
        let cue = {
            let state = self.qplayer.state().lock().unwrap();
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
            qplayer_core::Cue::Sound { ref path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, ref routing, .. } => {
                log::info!("Preload SoundCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, start_time, duration, volume, fade_in, fade_out, fade_type, eq, pan, routing.clone(), true);
            }
            qplayer_core::Cue::Video { ref path, start_time, duration, volume, pan, fade_in, fade_out, fade_type, eq, ref routing, .. } => {
                log::info!("Preload VideoCue: {}", path);
                self.play_audio(path, qid, &name, cue.base().loop_mode, cue.base().loop_count, start_time, duration, volume, fade_in, fade_out, fade_type, eq, pan, routing.clone(), true);
            }
            other => {
                log::info!("Preload not supported for cue type: {:?}", std::mem::discriminant(&other));
            }
        }
    }

    /// Restart the audio engine with a specific device.
    fn restart_audio_engine(&mut self, device: &cpal::Device) {
        self.stop_all();
        match AudioEngine::new(device) {
            Ok(new_engine) => {
                let name = new_engine.device_name().to_string();
                self.audio_engine = new_engine;
                if let Ok(mut state) = self.qplayer.state().lock() {
                    state.audio_device_name = name;
                }
                log::info!("Switched audio output device");
            }
            Err(e) => {
                log::error!("Failed to switch audio device: {}. Attempting fallback to default.", e);
                if let Ok(fallback) = AudioEngine::new_default() {
                    let name = fallback.device_name().to_string();
                    self.audio_engine = fallback;
                    if let Ok(mut state) = self.qplayer.state().lock() {
                        state.audio_device_name = name;
                    }
                }
            }
        }
    }

    /// Start a cue's tail fade-out when playback reaches `fade_out` seconds before
    /// its end. Mirrors C# SoundCue, where FadeOut begins (Duration - FadeOut)
    /// before the natural end. Looping cues are skipped (state != Playing).
    fn check_fade_outs(&mut self) {
        let sr = self.audio_engine.sample_rate();
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
        // Mark finished cues as Done and collect their QIDs
        let finished_qids: Vec<rust_decimal::Decimal> = {
            let mut qids = Vec::new();
            for ac in &mut self.active_cues {
                if ac.input.is_finished() {
                    ac.state = CueState::Done;
                    qids.push(ac.qid);
                }
            }
            qids
        };

        for qid in finished_qids {
            // Remove finished cue from active list
            self.active_cues.retain(|ac| ac.qid != qid);
            log::info!("Cue Q{} finished naturally — checking AfterLast chain", qid);

            // Find the cue's position in the show file
            let state = self.qplayer.state().lock().unwrap();
            let Some(idx) = state.show_file.cues.iter().position(|c| c.base().qid == qid) else {
                continue;
            };

            // Collect consecutive AfterLast cues after this one
            let mut after_last_cues = Vec::new();
            for i in (idx + 1)..state.show_file.cues.len() {
                let cue = &state.show_file.cues[i];
                if cue.base().trigger == qplayer_core::TriggerMode::AfterLast {
                    after_last_cues.push(cue.clone());
                } else {
                    break;
                }
            }
            drop(state);

            // Play AfterLast chain: non-audio cues fire immediately in a burst,
            // then the first audio cue starts and will trigger its own chain when it finishes.
            for cue in after_last_cues {
                let is_audio = matches!(cue, qplayer_core::Cue::Sound { .. } | qplayer_core::Cue::Video { .. });
                self.play_cue(&cue, event_loop);
                if is_audio {
                    break; // wait for this audio cue to finish before continuing the chain
                }
            }
        }
    }

    fn handle_volume_cue(&mut self, sound_qid: rust_decimal::Decimal, target_volume: f32, fade_time: f32, fade_type: qplayer_core::FadeType) {
        let target = self.active_cues.iter().find(|ac| ac.qid == sound_qid);
        if let Some(ac) = target {
            let input = &ac.input;
            if fade_time > 0.0 {
                let sample_rate = self.audio_engine.sample_rate();
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
        self.create_video_window(event_loop);
        self.video_stop_flag.store(false, Ordering::Relaxed);
        // A newly-started video should always play, even if the system was paused.
        self.video_pause_flag.store(false, Ordering::Relaxed);
        self.video_start_clock = Some(self.audio_engine.playback_time());
        self.current_video_qid = Some(qid);
        self.latest_video_frame = None;
        self.video_frame_dirty = false;

        // Create video texture/renderer if not yet created
        if self.video_texture.is_none() {
            let texture = Texture::new(&self.device, 1920, 1080);
            let renderer = Renderer::new(&self.device, texture.bind_group_layout());
            self.video_texture = Some(texture);
            self.video_renderer = Some(renderer);
        }

        // Spawn decode thread
        let path = path.to_string();
        let clock = {
            let mixer = Arc::clone(self.audio_engine.mixer());
            Arc::new(move || mixer.playback_time()) as Arc<dyn Fn() -> Duration + Send + Sync>
        };
        let start = self.video_start_clock.unwrap();
        let stop_flag = Arc::clone(&self.video_stop_flag);
        let pause_flag = Arc::clone(&self.video_pause_flag);
        let proxy = self.event_loop_proxy.clone();

        std::thread::Builder::new()
            .name("video-decode".into())
            .spawn(move || {
                video_decode_thread(&path, clock, start, stop_flag, pause_flag, proxy);
            })
            .expect("spawn video decode thread");
    }

    /// Restart the current video decode thread (used when audio loops).
    fn restart_video(&mut self, path: &str, qid: rust_decimal::Decimal, event_loop: &ActiveEventLoop) {
        self.video_stop_flag.store(true, Ordering::Relaxed);
        // Brief yield to let the old thread exit its read loop
        std::thread::sleep(std::time::Duration::from_millis(20));
        self.play_video(path, qid, event_loop);
        log::info!("Restarted video for Q{} on loop", qid);
    }

    fn stop_all(&mut self) {
        self.video_stop_flag.store(true, Ordering::Relaxed);
        self.video_pause_flag.store(false, Ordering::Relaxed);
        self.latest_video_frame = None;
        self.video_frame_dirty = false;
        self.video_start_clock = None;
        self.current_video_qid = None;
        self.audio_engine.stop_all();
        self.active_cues.clear();
        self.delayed_cues.clear();
        self.paused = false;
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
        log::info!("Resumed {} cue(s)", self.active_cues.len());
    }

    fn handle_dropped_file(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase());

        // Open project files directly
        if ext.as_deref() == Some("qproj") {
            if let Ok(mut state) = self.qplayer.state().lock() {
                state.command_queue.push(qplayer_gui::AppCommand::OpenProject {
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

        if let Ok(mut state) = self.qplayer.state().lock() {
            let snapshot = qplayer_gui::app::Snapshot::from_state(&state);
            state.undo_redo.push(snapshot);

            let next_qid = state.show_file.choose_qid(state.selected_cue_id);

            let base = qplayer_core::CueBase {
                qid: next_qid,
                name,
                ..Default::default()
            };

            let cue = if is_video {
                qplayer_core::Cue::Video {
                    base,
                    path: path_str,
                    start_time: qplayer_core::Timespan::ZERO,
                    duration: qplayer_core::Timespan::ZERO,
                    volume: 1.0,
                    pan: 0.0,
                    fade_in: 0.0,
                    fade_out: 0.0,
                    fade_type: qplayer_core::FadeType::Linear,
                    eq: None,
                    routing: qplayer_core::AudioRouting::default(),
                }
            } else {
                qplayer_core::Cue::Sound {
                    base,
                    path: path_str,
                    start_time: qplayer_core::Timespan::ZERO,
                    duration: qplayer_core::Timespan::ZERO,
                    volume: 1.0,
                    pan: 0.0,
                    fade_in: 0.0,
                    fade_out: 0.0,
                    fade_type: qplayer_core::FadeType::Linear,
                    eq: None,
                    routing: qplayer_core::AudioRouting::default(),
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
            let Ok(mut state) = self.qplayer.state().lock() else { return };
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
                    self.audio_engine.set_limiter_threshold(threshold);
                    log::info!("Set master limiter threshold to {:.2} dB", 20.0 * threshold.log10());
                }
                AppCommand::SetAudioDevice(name) => {
                    let devices = AudioEngine::list_devices();
                    if let Some((_, device)) = devices.into_iter().find(|(n, _)| n == &name) {
                        self.restart_audio_engine(&device);
                    } else {
                        log::warn!("Audio device '{}' not found", name);
                    }
                }
                AppCommand::Preload => {
                    self.handle_preload(event_loop);
                }
                AppCommand::ToggleVideoWindow => {
                    if self.video_window.is_some() {
                        // Hide/destroy
                        self.video_window = None;
                        self.video_surface = None;
                        self.video_config = None;
                        if let Some(ids) = self.window_ids.as_mut() {
                            ids.video = None;
                        }
                    } else {
                        // Show/create (even if no video is playing, show a black window)
                        self.create_video_window(event_loop);
                    }
                }
                AppCommand::ToggleVideoFullscreen => {
                    self.toggle_video_fullscreen();
                }
                AppCommand::SaveProject | AppCommand::SaveProjectAs { .. } => {
                    if let Some(pm) = self.plugin_manager.as_mut() {
                        pm.on_save();
                    }
                }
                _ => {}
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
                                let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                        }
                        if let Ok(mut state) = self.qplayer.state().lock() {
                            state.command_queue.push(AppCommand::Go);
                        }
                    }
                    OscEvent::Stop { qid: _ } => {
                        if let Ok(mut state) = self.qplayer.state().lock() {
                            state.command_queue.push(AppCommand::Stop);
                        }
                    }
                    OscEvent::Pause { .. } => {
                        if let Ok(mut state) = self.qplayer.state().lock() {
                            state.command_queue.push(AppCommand::Pause);
                        }
                    }
                    OscEvent::Unpause { .. } => {
                        if self.paused {
                            if let Ok(mut state) = self.qplayer.state().lock() {
                                state.command_queue.push(AppCommand::Pause);
                            }
                        }
                    }
                    OscEvent::Select { qid } => {
                        if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                            let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                        }
                    }
                    OscEvent::Up => {}
                    OscEvent::Down => {}
                    OscEvent::Save => {
                        if let Ok(mut state) = self.qplayer.state().lock() {
                            state.command_queue.push(AppCommand::SaveProject);
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
                        if let Ok(mut state) = self.qplayer.state().lock() {
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
                                    nodes.push(qplayer_core::RemoteNode {
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
                                let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.qplayer.state().lock() {
                                state.command_queue.push(AppCommand::Go);
                            }
                        }
                    }
                    OscEvent::RemoteStop { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.qplayer.state().lock() {
                                state.command_queue.push(AppCommand::Stop);
                            }
                        }
                    }
                    OscEvent::RemotePause { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.qplayer.state().lock() {
                                state.command_queue.push(AppCommand::Pause);
                            }
                        }
                    }
                    OscEvent::RemoteUnpause { target, qid } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if self.paused {
                                if let Ok(mut state) = self.qplayer.state().lock() {
                                    state.command_queue.push(AppCommand::Pause);
                                }
                            }
                        }
                    }
                    OscEvent::RemotePreload { target, qid, time: _ } => {
                        if self.is_remote_target_match(&target) {
                            if let Ok(qid_dec) = qid.parse::<rust_decimal::Decimal>() {
                                let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                            }
                            if let Ok(mut state) = self.qplayer.state().lock() {
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
                            let _ = self.qplayer.state().lock().map(|mut s| s.selected_cue_id = Some(qid_dec));
                        }
                        if let Ok(mut state) = self.qplayer.state().lock() {
                            state.command_queue.push(AppCommand::Go);
                        }
                    }
                    MscEvent::Stop { .. } => {
                        if let Ok(mut state) = self.qplayer.state().lock() {
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
                let Ok(state) = self.qplayer.state().lock() else { return; };
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
            let Ok(mut state) = self.qplayer.state().lock() else { return; };
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
            let Ok(state) = self.qplayer.state().lock() else { return };
            (state.project_path.clone(), state.dirty)
        };
        let name = path
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled");
        let title = if dirty {
            format!("QPlayer — {} *", name)
        } else {
            format!("QPlayer — {}", name)
        };
        if self.last_window_title != title {
            self.last_window_title = title.clone();
            if let Some(window) = self.control_window.as_ref() {
                window.set_title(&title);
            }
        }
    }

    fn render_control(&mut self, event_loop: &ActiveEventLoop) {
        self.check_fade_outs();
        self.check_finished_cues(event_loop);

        // Check for video cues that have looped and restart their video threads.
        if let Some(video_qid) = self.current_video_qid {
            if let Some(ac) = self.active_cues.iter_mut().find(|ac| ac.qid == video_qid) {
                if let Some(ref counter) = ac.loop_counter {
                    let current = counter.load(Ordering::Relaxed);
                    if current > ac.video_loop_count {
                        ac.video_loop_count = current;
                        // Look up the cue's video path in the show file
                        let path = {
                            let Ok(state) = self.qplayer.state().lock() else { return };
                            state.show_file.cues.iter()
                                .find(|c| c.base().qid == video_qid)
                                .and_then(|cue| match cue {
                                    qplayer_core::Cue::Video { path, .. } => Some(path.clone()),
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
                let Ok(state) = self.qplayer.state().lock() else { return; };
                state.show_file.cues.iter()
                    .filter_map(|cue| match cue {
                        qplayer_core::Cue::TimeCode { base, start_time, .. } => {
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

        self.update_window_title();
        let Some(surface) = self.control_surface.as_ref() else { return };
        let Some(config) = self.control_config.as_ref() else { return };
        let Some(window) = self.control_window.as_ref() else { return };
        let Some(egui_state) = self.egui_state.as_mut() else { return };
        let Some(egui_renderer) = self.egui_renderer.as_mut() else { return };

        let output = match surface.get_current_texture() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Control surface acquire failed: {e}");
                return;
            }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let raw_input = egui_state.take_egui_input(window);
        // Sync active cue state into the GUI shared state
        {
            let gui_active: Vec<qplayer_gui::ActiveCueInfo> = self.active_cues.iter().map(|ac| {
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
                qplayer_gui::ActiveCueInfo {
                    qid: ac.qid,
                    name: ac.name.clone(),
                    volume: ac.input.volume(),
                    paused: !ac.input.is_active(),
                    position,
                    length,
                    state: ac.state,
                }
            }).collect();
            if let Ok(mut state) = self.qplayer.state().lock() {
                state.active_cues = gui_active;
            }
        }

        // Sync master meter data into the GUI shared state
        {
            let meters = self.audio_engine.read_meters();
            let peak_l_db = if meters.peak_l > 0.0 { 20.0 * meters.peak_l.log10() } else { -f32::INFINITY };
            let peak_r_db = if meters.peak_r > 0.0 { 20.0 * meters.peak_r.log10() } else { -f32::INFINITY };
            let rms_l_db = if meters.rms_l > 0.0 { 20.0 * meters.rms_l.log10() } else { -f32::INFINITY };
            let rms_r_db = if meters.rms_r > 0.0 { 20.0 * meters.rms_r.log10() } else { -f32::INFINITY };
            let limiter_gr_db = self.audio_engine.read_limiter_gr_db();
            if let Ok(mut state) = self.qplayer.state().lock() {
                state.meter_data = qplayer_gui::GuiMeterData {
                    peak_l_db,
                    peak_r_db,
                    rms_l_db,
                    rms_r_db,
                    clipped: false, // TODO: expose clip flag from MeteringProcessor
                    limiter_gr_db,
                };
            }
        }

        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            self.qplayer.update(ctx);
        });
        egui_state.handle_platform_output(window, full_output.platform_output);

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [config.width, config.height],
            pixels_per_point: window.scale_factor() as f32 * self.egui_ctx.zoom_factor(),
        };

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("control-encoder"),
        });

        let paint_jobs = self.egui_ctx.tessellate(full_output.shapes, full_output.pixels_per_point);
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
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            egui_renderer.render(&mut render_pass.forget_lifetime(), &paint_jobs, &screen_descriptor);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        // Process commands that were queued during the UI frame
        self.process_commands(event_loop);

        // Sync the mixer snapshot so newly-added inputs are visible to the audio callback.
        // Must be called after process_commands() so any play() calls from this frame
        // are reflected before the next callback fires.
        self.audio_engine.refresh();
    }

    /// Render the video output window.
    fn render_video(&mut self) {
        let Some(surface) = self.video_surface.as_ref() else { return };
        let Some(texture) = self.video_texture.as_mut() else { return };
        let Some(renderer) = self.video_renderer.as_ref() else { return };

        if self.video_frame_dirty {
            if let Some(frame) = self.latest_video_frame.as_ref() {
                texture.upload(&self.queue, frame);
            }
            self.video_frame_dirty = false;
        }

        let output = match surface.get_current_texture() {
            Ok(o) => o,
            Err(e) => {
                log::warn!("Video surface acquire failed: {e}");
                return;
            }
        };
        let view = output.texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("video-encoder"),
        });

        // If no video is active, just clear to black instead of drawing the last frame.
        let has_video = self.current_video_qid.is_some() || self.latest_video_frame.is_some();
        if has_video {
            renderer.render(&mut encoder, &view, texture.current_bind_group());
        } else {
            encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("video-clear-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();
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
            AppEvent::VideoFrame(frame) => {
                self.latest_video_frame = Some(frame);
                self.video_frame_dirty = true;
                if let Some(window) = self.video_window.as_ref() {
                    window.request_redraw();
                }
            }
            AppEvent::VideoEof => {
                log::info!("Video EOF");
                // What the output window shows after a clip ends:
                //   Looped/LoopedInfinite -> restart (video-only here; audio-backed
                //     clips restart via the audio loop_counter, so skip those),
                //   HoldLast -> keep the final frame on screen,
                //   OneShot (default) -> blank the window to black.
                if let Some(qid) = self.current_video_qid {
                    let has_audio_cue = self.active_cues.iter().any(|ac| ac.qid == qid);
                    let cue_info = {
                        let state = self.qplayer.state().lock().unwrap();
                        state.show_file.cues.iter()
                            .find(|c| c.base().qid == qid)
                            .and_then(|c| match c {
                                qplayer_core::Cue::Video { path, .. } => {
                                    Some((c.base().loop_mode, path.clone()))
                                }
                                _ => None,
                            })
                    };
                    match cue_info {
                        Some((
                            qplayer_core::LoopMode::Looped | qplayer_core::LoopMode::LoopedInfinite,
                            path,
                        )) => {
                            if !has_audio_cue {
                                self.restart_video(&path, qid, event_loop);
                            }
                        }
                        Some((qplayer_core::LoopMode::HoldLast, _)) => {
                            // Hold the last frame — leave the video state untouched.
                        }
                        _ => {
                            // OneShot (or cue gone): blank the output to black.
                            self.current_video_qid = None;
                            self.latest_video_frame = None;
                            self.video_frame_dirty = true;
                            if let Some(window) = self.video_window.as_ref() {
                                window.request_redraw();
                            }
                        }
                    }
                }
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
            .map(|ids| ids.video == Some(window_id))
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
                    let has_running = !self.active_cues.is_empty();
                    if has_running {
                        let choice = rfd::MessageDialog::new()
                            .set_title("Running Cues")
                            .set_description("There are cues currently playing. Stop them and exit?")
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show();
                        if !matches!(choice, rfd::MessageDialogResult::Ok) {
                            return;
                        }
                        self.stop_all();
                    }
                    let dirty = self.qplayer.state().lock().map(|s| s.dirty).unwrap_or(false);
                    if dirty {
                        let choice = rfd::MessageDialog::new()
                            .set_title("Unsaved Changes")
                            .set_description("You have unsaved changes. Discard them?")
                            .set_buttons(rfd::MessageButtons::OkCancel)
                            .show();
                        if !matches!(choice, rfd::MessageDialogResult::Ok) {
                            return;
                        }
                    }
                    event_loop.exit();
                }
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        if let Some(config) = self.control_config.as_mut() {
                            config.width = size.width;
                            config.height = size.height;
                        }
                        if let Some(surface) = self.control_surface.as_ref() {
                            if let Some(config) = self.control_config.as_ref() {
                                surface.configure(&self.device, config);
                            }
                        }
                    }
                }
                WindowEvent::DroppedFile(path) => {
                    self.handle_dropped_file(&path);
                }
                WindowEvent::RedrawRequested => {
                    self.render_control(event_loop);
                    if let Some(window) = self.control_window.as_ref() {
                        window.request_redraw();
                    }
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
                            self.create_video_window(event_loop);
                            self.toggle_video_fullscreen();
                        }
                    }
                }
                _ => {}
            }
        } else if is_video {
            match event {
                WindowEvent::CloseRequested => {
                    self.video_window = None;
                    self.video_surface = None;
                    self.video_config = None;
                    if let Some(ids) = self.window_ids.as_mut() {
                        ids.video = None;
                    }
                    if let Ok(mut state) = self.qplayer.state().lock() {
                        state.show_video_window = false;
                    }
                }
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state == winit::event::ElementState::Pressed {
                        use winit::keyboard::{Key, NamedKey, PhysicalKey};
                        let is_esc = event.logical_key == Key::Named(NamedKey::Escape);
                        let is_f11 = matches!(event.physical_key, PhysicalKey::Code(winit::keyboard::KeyCode::F11));
                        let is_f = event.logical_key == Key::Character("f".into());
                        let has_ctrl = self.modifiers.control_key() || self.modifiers.super_key();

                        // Esc always exits fullscreen
                        if is_esc {
                            if let Some(window) = self.video_window.as_ref() {
                                window.set_fullscreen(None);
                                window.set_cursor_visible(true);
                            }
                        }
                        // F11 toggles fullscreen
                        else if is_f11 {
                            self.toggle_video_fullscreen();
                        }
                        // Ctrl+F or Cmd+F toggles fullscreen
                        else if is_f && has_ctrl {
                            self.toggle_video_fullscreen();
                        }
                    }
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    self.modifiers = modifiers.state();
                }
                WindowEvent::Resized(size) => {
                    if size.width > 0 && size.height > 0 {
                        if let Some(config) = self.video_config.as_mut() {
                            config.width = size.width;
                            config.height = size.height;
                        }
                        if let Some(surface) = self.video_surface.as_ref() {
                            if let Some(config) = self.video_config.as_ref() {
                                surface.configure(&self.device, config);
                            }
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    self.render_video();
                    if let Some(window) = self.video_window.as_ref() {
                        window.request_redraw();
                    }
                }
                _ => {}
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.process_protocol_events();

        // Plugin slow update every 250 ms
        if self.last_slow_update.elapsed() >= Duration::from_millis(250) {
            self.last_slow_update = Instant::now();
            if let Some(pm) = self.plugin_manager.as_mut() {
                pm.on_slow_update();
            }
            // Sync plugin list to GUI state
            if let Some(pm) = self.plugin_manager.as_ref() {
                let plugins: Vec<(String, String)> = pm.list_plugins()
                    .iter()
                    .map(|p| (p.name.clone(), p.path.clone()))
                    .collect();
                if let Ok(mut state) = self.qplayer.state().lock() {
                    state.plugin_list = plugins;
                }
            }
        }

        // Continuously redraw both windows when active.
        if let Some(window) = self.control_window.as_ref() {
            window.request_redraw();
        }
        if let Some(window) = self.video_window.as_ref() {
            window.request_redraw();
        }
    }
}

/// Video decode thread: sleeps until each frame's PTS, then sends it to the main loop.
fn video_decode_thread(
    path: &str,
    clock: Arc<dyn Fn() -> Duration + Send + Sync>,
    start_clock: Duration,
    stop_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    proxy: winit::event_loop::EventLoopProxy<AppEvent>,
) {
    let mut source = match VideoSource::open(path, 1920, 1080) {
        Ok(s) => s,
        Err(e) => {
            log::error!("Failed to open video source {}: {e}", path);
            return;
        }
    };

    let mut paused_at: Option<Duration> = None;
    let mut total_pause = Duration::ZERO;

    while !stop_flag.load(Ordering::Relaxed) {
        // When paused, sleep without decoding so we don't read ahead.
        if pause_flag.load(Ordering::Relaxed) {
            if paused_at.is_none() {
                paused_at = Some(clock());
            }
            std::thread::sleep(Duration::from_millis(10));
            continue;
        }

        // Just resumed — accumulate the pause duration so we don't fast-forward.
        if let Some(start) = paused_at.take() {
            total_pause += clock().saturating_sub(start);
        }

        match source.read_frame() {
            Some(frame) => {
                let elapsed = clock().saturating_sub(start_clock).saturating_sub(total_pause);
                let frame_due = Duration::from_secs_f64(frame.pts.max(0.0));

                if frame_due > elapsed {
                    let sleep_for = frame_due - elapsed;
                    // Cap sleep to avoid missing stop signals for too long
                    std::thread::sleep(sleep_for.min(Duration::from_millis(50)));
                }

                if stop_flag.load(Ordering::Relaxed) {
                    break;
                }
                if pause_flag.load(Ordering::Relaxed) {
                    continue;
                }

                if proxy.send_event(AppEvent::VideoFrame(frame)).is_err() {
                    break;
                }
            }
            None => {
                let _ = proxy.send_event(AppEvent::VideoEof);
                break;
            }
        }
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
                .join("QPlayer");
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
    dirs::config_dir().map(|p| p.join("QPlayer").join("settings.json"))
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
        .join("QPlayer");
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
    // Single instance guard
    let single = single_instance::SingleInstance::new("QPlayer_rust_port").unwrap();
    if !single.is_single() {
        log::warn!("Another instance of QPlayer is already running. Exiting.");
        return Ok(());
    }

    human_panic::setup_panic!(
        Metadata::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"))
            .authors("QPlayer Contributors")
            .homepage("https://github.com/BlueJayLouche/QPlayer")
    );

    qplayer_gui::logging::init_logger();

    let event_loop = EventLoop::with_user_event().build()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let proxy = event_loop.create_proxy();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
        backends: wgpu::Backends::all(),
        ..Default::default()
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
            label: Some("qplayer-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default(),
            ..Default::default()
        },
    ))?;

    let mut app = App::new(instance, adapter, device, queue, proxy);

    // Load persisted settings and sync audio device name
    let settings = load_settings();
    let device_name = app.audio_engine.device_name().to_string();
    if let Ok(mut state) = app.qplayer.state().lock() {
        state.recent_files = settings.recent_files;
        state.audio_device_name = device_name;
    }

    // Ctrl-C / SIGTERM handler for graceful emergency save
    {
        let state = Arc::clone(app.qplayer.state());
        ctrlc::set_handler(move || {
            log::info!("SIGINT received, performing emergency save...");
            emergency_save(&state);
            std::process::exit(0);
        })?;
    }

    event_loop.run_app(&mut app)?;

    // Save persisted settings
    let recent_files = app.qplayer.state().lock().map(|s| s.recent_files.clone()).unwrap_or_default();
    save_settings(&AppSettings { recent_files });

    // Notify plugins before shutdown
    if let Some(pm) = app.plugin_manager.as_mut() {
        pm.on_unload();
    }

    // Signal autosave thread to stop
    app.autosave_running.store(false, Ordering::Relaxed);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let err = parse_osc_command("qplayer/go");
        assert!(err.is_err());
    }

    #[test]
    fn test_parse_osc_command_empty() {
        let err = parse_osc_command("");
        assert!(err.is_err());
    }
}
