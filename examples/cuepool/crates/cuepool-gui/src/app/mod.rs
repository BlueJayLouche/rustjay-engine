//! Main application state and eframe integration.

use cuepool_core::{Cue, ShowFile};
use rust_decimal::Decimal;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

mod identity_card;

pub use identity_card::torus_colour;
use identity_card::{CardPresentation, LAUNCH_HOLD, identity_card, launch_opacity};

const OPERATOR_ALERT_DURATION: Duration = Duration::from_secs(10);
const MAX_AUTOMATION_PROJECT_BYTES: u64 = 16 * 1024 * 1024;
/// The generation of "What's new" copy the modal currently shows, matched
/// against each operator's stored `last_seen_release_notes` so the modal
/// appears once per generation.
///
/// Hand-written rather than derived from `CARGO_PKG_VERSION` on purpose: a
/// patch bump must not re-fire the modal, and a minor bump must not relabel
/// the previous release's copy with the new version. `release_notes_match_the_release`
/// fails while this trails the package minor, which is the reminder to rewrite
/// the modal body before bumping it (see AGENTS.md).
pub const RELEASE_NOTES_VERSION: &str = "0.11";

/// A full snapshot of editable state for undo/redo.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub show_file: ShowFile,
    pub project_path: Option<PathBuf>,
    pub selected_cue_id: Option<Decimal>,
    pub dirty: bool,
    /// If set, consecutive snapshots with the same key are merged into one.
    pub merge_key: Option<String>,
}

/// A validated project prepared off the show-control event loop.
#[derive(Debug)]
pub struct PreparedProject {
    pub path: PathBuf,
    show: ShowFile,
    file_len: u64,
    modified: Option<std::time::SystemTime>,
}

impl Snapshot {
    pub fn from_state(state: &SharedState) -> Self {
        Self {
            show_file: state.show_file.clone(),
            project_path: state.project_path.clone(),
            selected_cue_id: state.selected_cue_id,
            dirty: state.dirty,
            merge_key: None,
        }
    }

    pub fn with_merge_key(mut self, key: impl Into<String>) -> Self {
        self.merge_key = Some(key.into());
        self
    }

    pub fn apply(self, state: &mut SharedState) {
        let audio_changed = state.show_file.show_settings.audio_output_driver
            != self.show_file.show_settings.audio_output_driver
            || state.show_file.show_settings.audio_output_device
                != self.show_file.show_settings.audio_output_device;
        state.show_file = self.show_file;
        state.project_path = self.project_path;
        state.selected_cue_id = self.selected_cue_id;
        state.dirty = self.dirty;
        if audio_changed {
            queue_audio_settings_apply(state);
        }
    }
}

/// Note what a snapshot deliberately leaves out: `show_mode`. It is an operator
/// stance, not project content — it is never saved — so restoring it with an
/// edit would let Cmd+Z drop the operator out of Show mode mid-show, silently
/// unlocking every editing surface the mode exists to lock.
///
/// Undo/redo history with a configurable max depth.
#[derive(Debug, Clone)]
pub struct UndoRedo {
    undo_stack: Vec<Snapshot>,
    redo_stack: Vec<Snapshot>,
    max_depth: usize,
    /// When true, snapshot capture is suppressed (used during undo/redo itself)
    pub suppress: bool,
}

impl UndoRedo {
    pub fn new(max_depth: usize) -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            max_depth,
            suppress: false,
        }
    }

    /// Push a snapshot onto the undo stack, clearing the redo stack.
    ///
    /// Snapshots hold the state *before* the edit they accompany. So when a run
    /// of edits shares a merge key — every keystroke of a rename, every step of a
    /// drag — the one worth keeping is the run's **first**, which rewinds the
    /// whole run; the later ones only describe intermediate states nobody wants
    /// to land on. Keeping the last instead made an undo after typing a name give
    /// back the name minus its final keystroke.
    pub fn push(&mut self, snapshot: Snapshot) {
        if self.suppress {
            return;
        }
        if let Some(ref key) = snapshot.merge_key
            && let Some(top) = self.undo_stack.last()
            && top.merge_key.as_ref() == Some(key)
        {
            // Already recording this run — keep the snapshot that started it and
            // drop this one. The redo stack was cleared by the run's first push.
            self.redo_stack.clear();
            return;
        }
        self.undo_stack.push(snapshot);
        if self.undo_stack.len() > self.max_depth {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    /// Pop the most recent snapshot and return it, pushing current state to redo.
    pub fn undo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let prev = self.undo_stack.pop()?;
        self.redo_stack.push(current);
        Some(prev)
    }

    /// Pop the most recent redo snapshot and return it, pushing current state to undo.
    pub fn redo(&mut self, current: Snapshot) -> Option<Snapshot> {
        let next = self.redo_stack.pop()?;
        self.undo_stack.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }
}

impl Default for UndoRedo {
    fn default() -> Self {
        Self::new(50)
    }
}

/// Runtime state of an active cue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CueState {
    #[default]
    Ready,
    Delay,
    Playing,
    PlayingLooped,
    Paused,
    Done,
}

/// Lightweight info about a cue currently playing, synced from the audio engine.
#[derive(Debug, Clone, Default)]
pub struct ActiveCueInfo {
    /// Unique runtime playback instance; QIDs may be retriggered concurrently.
    pub instance_id: u64,
    pub qid: Decimal,
    pub name: String,
    /// True if the cue is currently paused.
    pub paused: bool,
    /// Current playback position in seconds.
    pub position_secs: f32,
    /// Total length in seconds, if known.
    pub length_secs: Option<f32>,
    /// Runtime state.
    pub state: CueState,
}

/// Master meter data synced from the audio engine.
#[derive(Debug, Clone, Copy, Default)]
pub struct GuiMeterData {
    pub peak_l_db: f32,
    pub peak_r_db: f32,
    pub rms_l_db: f32,
    pub rms_r_db: f32,
    pub clipped: bool,
    /// Master limiter gain reduction in dB (0 = no reduction, negative = active).
    pub limiter_gr_db: f32,
}

/// One output window's diagnostics snapshot (refreshed ~1 Hz by the engine).
#[derive(Debug, Clone)]
pub struct OutputDiagnostics {
    pub name: String,
    pub size: (u32, u32),
    pub present_mode: String,
    pub format: String,
    /// Monitor refresh, e.g. "59.94 Hz" ("?" when the monitor won't say).
    pub refresh: String,
    pub fullscreen: bool,
    /// Presents completed per second by this output's render thread. The field
    /// diagnostic for vsync starvation: an output pinned below its monitor
    /// refresh is the one stuttering.
    pub presented_per_sec: f64,
}

/// Per-stream decode timing updated without taking the shared GUI/project lock.
#[derive(Debug, Clone, Default)]
pub struct DecodeTiming(std::sync::Arc<std::sync::atomic::AtomicU64>);

impl DecodeTiming {
    pub fn from_ms(ms: f64) -> Self {
        Self(std::sync::Arc::new(std::sync::atomic::AtomicU64::new(
            ms.to_bits(),
        )))
    }

    pub fn set_ms(&self, ms: f64) {
        self.0
            .store(ms.to_bits(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn get_ms(&self) -> f64 {
        f64::from_bits(self.0.load(std::sync::atomic::Ordering::Relaxed))
    }
}

#[derive(Debug, Clone, Default)]
pub struct VideoTimings {
    pub decode: DecodeTiming,
    pub hw_transfer: DecodeTiming,
    pub plane_copy: DecodeTiming,
    pub upload: DecodeTiming,
    pub conversion_submit: DecodeTiming,
}

/// The currently-decoding video source, published by the decode thread.
#[derive(Debug, Clone)]
pub struct VideoDiagnostics {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// The active decode path from `VideoSource::decode_path()`: `software`,
    /// `hap gpu-native`, `d3d12va zero-copy (<adapter>)`, `d3d11va readback`,
    /// or `hardware (<api>)`. Classify it with [`Self::accelerated`] rather
    /// than matching the text.
    pub decode_path: String,
    pub fallback_reason: Option<String>,
    pub timings: VideoTimings,
}

impl VideoDiagnostics {
    /// Whether the GPU is doing the decode work.
    ///
    /// `software` is the only unaccelerated value — every other path names its
    /// API instead. Matching on `"hardware"` looks equivalent and isn't: it
    /// mis-reports the two *fastest* paths (`hap gpu-native`,
    /// `d3d12va zero-copy`) as software.
    pub fn accelerated(&self) -> bool {
        self.decode_path != "software"
    }
}

/// The status-bar video badge: label, colour, and hover detail.
///
/// Green only when the GPU is decoding *and* nothing was given up getting
/// there — a hardware path that fell back from zero-copy is still degraded, so
/// it reads amber alongside plain software decode.
fn video_status_badge(video: Option<&VideoDiagnostics>) -> (String, egui::Color32, String) {
    const IDLE: egui::Color32 = egui::Color32::from_rgb(120, 120, 120);
    const HEALTHY: egui::Color32 = egui::Color32::from_rgb(100, 220, 100);
    const DEGRADED: egui::Color32 = egui::Color32::from_rgb(240, 190, 90);

    let Some(v) = video else {
        return (
            "Video: idle".into(),
            IDLE,
            "No video decoding right now.".into(),
        );
    };

    let healthy = v.accelerated() && v.fallback_reason.is_none();
    let label = format!(
        "Video: {}{}",
        v.decode_path,
        if healthy { "" } else { " ⚠" }
    );

    let mut tip = format!(
        "Decode path: {}\nSource: {}x{} — {}",
        v.decode_path, v.width, v.height, v.path
    );
    if let Some(reason) = &v.fallback_reason {
        tip.push_str(&format!("\n\nFell back: {reason}"));
    }
    if !v.accelerated() {
        // State, not cause: a missing hwaccel, a rejected pool and a hw device
        // whose first frame hasn't landed yet all read "software" here.
        tip.push_str("\n\nDecoding on the CPU — expect higher load than a hardware path.");
    }
    // ">" not "→": the bundled egui fonts have no U+2192 and render it as tofu.
    tip.push_str("\n\nFull detail in Help > Status…");

    (label, if healthy { HEALTHY } else { DEGRADED }, tip)
}

/// Plain-data snapshot behind the Status window (Help → Status…): what a
/// designer copies into a bug report. The engine fills the static fields once
/// at startup and refreshes the live counters ~once per second; the GUI only
/// reads it.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub log_file: String,
    pub gpu_name: String,
    pub gpu_backend: String,
    pub gpu_driver: String,
    pub gpu_driver_info: String,
    pub ffmpeg_version: String,
    /// Set `QPLAYER_*` overrides as (name, value); empty when none are set.
    pub env_overrides: Vec<(String, String)>,
    pub outputs: Vec<OutputDiagnostics>,
    /// Sum of all outputs' presented/s.
    pub presented_per_sec: f64,
    pub starved_per_sec: f64,
    pub uploads_per_sec: f64,
    pub dropped_per_sec: f64,
    /// Main event-loop iterations per second. The field diagnostic for a
    /// GPU-stalled winit loop (healthy ≈ 250, the Windows WSI stall showed 10).
    pub event_loop_per_sec: f64,
    /// Set once if the video consume thread exits while the app is still running.
    pub consumer_error: Option<String>,
    pub video: Option<VideoDiagnostics>,
    /// wgpu fence-lock timing rows (thread bucket → formatted stats), present
    /// only when the fork's `frame-pacing-diag` feature is compiled in.
    pub frame_pacing: Vec<(String, String)>,
}

impl Diagnostics {
    /// Sectioned key/value rows — the single source for both the on-screen
    /// layout and the clipboard text, so the two can't drift.
    pub fn sections(&self) -> Vec<(&'static str, Vec<(String, String)>)> {
        let mut sections = Vec::new();

        sections.push((
            "System",
            vec![
                ("App Version".into(), self.app_version.clone()),
                ("OS".into(), self.os.clone()),
                ("Arch".into(), self.arch.clone()),
                ("Log File".into(), self.log_file.clone()),
            ],
        ));

        sections.push((
            "GPU",
            vec![
                ("Name".into(), self.gpu_name.clone()),
                ("Backend".into(), self.gpu_backend.clone()),
                ("Driver".into(), self.gpu_driver.clone()),
                ("Driver Info".into(), self.gpu_driver_info.clone()),
            ],
        ));

        let mut outputs = Vec::new();
        if self.outputs.is_empty() {
            outputs.push(("Outputs".into(), "none open".into()));
        }
        for (i, out) in self.outputs.iter().enumerate() {
            let p = format!("Output {}", i + 1);
            outputs.push((format!("{p} Name"), out.name.clone()));
            outputs.push((
                format!("{p} Size"),
                format!("{}x{}", out.size.0, out.size.1),
            ));
            outputs.push((format!("{p} Present Mode"), out.present_mode.clone()));
            outputs.push((format!("{p} Surface Format"), out.format.clone()));
            outputs.push((format!("{p} Monitor Refresh"), out.refresh.clone()));
            outputs.push((
                format!("{p} Fullscreen"),
                if out.fullscreen { "yes" } else { "no" }.into(),
            ));
            outputs.push((
                format!("{p} Presented/s"),
                format!("{:.0}", out.presented_per_sec),
            ));
        }
        sections.push(("Outputs", outputs));

        let video = match &self.video {
            Some(v) => vec![
                ("File".into(), v.path.clone()),
                ("Source Size".into(), format!("{}x{}", v.width, v.height)),
                ("Decode Path".into(), v.decode_path.clone()),
                (
                    "Fallback Reason".into(),
                    v.fallback_reason.clone().unwrap_or_else(|| "none".into()),
                ),
                (
                    "Decode ms/frame".into(),
                    format!("{:.2}", v.timings.decode.get_ms()),
                ),
                (
                    "HW transfer ms/frame".into(),
                    format!("{:.2}", v.timings.hw_transfer.get_ms()),
                ),
                (
                    "Plane copy ms/frame".into(),
                    format!("{:.2}", v.timings.plane_copy.get_ms()),
                ),
                (
                    "Upload ms/frame".into(),
                    format!("{:.2}", v.timings.upload.get_ms()),
                ),
                (
                    "Conversion submit ms/frame".into(),
                    format!("{:.2}", v.timings.conversion_submit.get_ms()),
                ),
            ],
            None => vec![("Status".into(), "no video playing".into())],
        };
        sections.push(("Video Decode", video));

        sections.push((
            "Pacing",
            vec![
                ("Output Count".into(), self.outputs.len().to_string()),
                (
                    "Event Loop/s".into(),
                    format!("{:.0}", self.event_loop_per_sec),
                ),
                (
                    "Presented/s (all outputs)".into(),
                    format!("{:.0}", self.presented_per_sec),
                ),
                ("Uploads/s".into(), format!("{:.0}", self.uploads_per_sec)),
                ("Dropped/s".into(), format!("{:.0}", self.dropped_per_sec)),
                ("Starved/s".into(), format!("{:.0}", self.starved_per_sec)),
                (
                    "Video Consumer".into(),
                    self.consumer_error
                        .clone()
                        .unwrap_or_else(|| "running".into()),
                ),
            ],
        ));

        if !self.frame_pacing.is_empty() {
            sections.push(("Frame Pacing (wgpu)", self.frame_pacing.clone()));
        }

        let env = if self.env_overrides.is_empty() {
            vec![("Overrides".into(), "none set".into())]
        } else {
            self.env_overrides.clone()
        };
        sections.push(("Environment Overrides", env));

        sections
    }

    /// Plain-text rendering of every section, for the clipboard.
    pub fn to_text(&self) -> String {
        let mut text = String::new();
        for (title, rows) in self.sections() {
            text.push_str(title);
            text.push('\n');
            for (key, value) in rows {
                text.push_str(&format!("  {key}: {value}\n"));
            }
            text.push('\n');
        }
        text
    }
}

/// Central mutable state shared between GUI and audio/control threads.
#[derive(Debug)]
pub struct SharedState {
    pub show_file: ShowFile,
    pub project_path: Option<PathBuf>,
    pub selected_cue_id: Option<Decimal>,
    pub command_queue: Vec<AppCommand>,
    pub show_mode: ShowMode,
    pub dirty: bool,
    pub undo_redo: UndoRedo,
    pub active_cues: Vec<ActiveCueInfo>,
    pub meter_data: GuiMeterData,
    /// Recently opened/saved project paths (most recent first, max 10).
    pub recent_files: Vec<PathBuf>,
    /// Release-notes series acknowledged by the operator, persisted by the binary.
    pub last_seen_release_notes: Option<String>,
    /// Whether the project settings window is open.
    pub show_settings_window: bool,
    /// Current audio output device name.
    pub audio_device_name: String,
    /// Why audio playback is disabled, if output configuration failed.
    pub audio_error: Option<String>,
    /// Cached whole-media waveform data by path.
    pub waveform_cache: std::collections::HashMap<String, crate::waveform::WaveformData>,
    /// Paths currently being processed for waveform generation.
    pub pending_waveforms: std::collections::HashSet<String>,
    /// Waveform zoom level (1.0 = fit to width, >1.0 = zoomed in).
    pub waveform_zoom: f32,
    /// Waveform scroll offset in bars.
    pub waveform_scroll: f32,
    /// Available audio output devices (populated at startup).
    pub audio_devices: Vec<cuepool_audio::AudioDeviceInfo>,
    /// Whether the log window is open.
    pub show_log_window: bool,
    /// Whether Help → About has asked for the identity card.
    pub show_about_window: bool,
    /// Whether the Status diagnostics window is open.
    pub show_status_window: bool,
    /// Live snapshot behind the Status window, published by the engine.
    pub diagnostics: Diagnostics,
    /// Whether the Waveform pop-out window is open.
    pub show_waveform_window: bool,
    /// Waveform window zoom level (independent from inspector).
    pub waveform_window_zoom: f32,
    /// Waveform window scroll offset in bars.
    pub waveform_window_scroll: f32,
    /// Whether the Video Output window is open.
    pub show_video_window: bool,
    /// Whether the Projection Mapping window is open.
    pub show_projection_window: bool,
    pub show_lighting_window: bool,
    /// Whether the DMX Recorder window is open.
    pub show_recorder_window: bool,
    /// Recorder target `.dmxrec` path (GUI-owned, session-only).
    pub recorder_file: String,
    /// Monitor toggle mirror (engine applies it via RecorderSetMonitor).
    pub recorder_monitor: bool,
    /// MIDI CC → DMX bridge: enabled + target universe (CC# = channel).
    pub recorder_midi_enabled: bool,
    pub recorder_midi_universe: u16,
    /// Live status published by the engine.
    pub recorder_status: RecorderStatus,
    /// One-shot request to open the Take Editor on this file.
    pub open_take_editor: Option<String>,
    /// Show-clock elapsed seconds (None until the first Go); frozen while paused.
    pub show_time: Option<f64>,
    pub show_paused: bool,
    /// MTC receive status, published every tick by the control binary.
    pub mtc_running: bool,
    /// `true` while MTC quarter-frames are streaming (transport playing).
    pub mtc_playing: bool,
    /// Latest MTC position in seconds (for the transport readout).
    pub mtc_timecode_secs: f64,
    /// Frame rate reported by the MTC source.
    pub mtc_fps: f64,
    /// Name of the MIDI port sending MTC (empty until one is seen).
    pub mtc_source: String,
    /// Drift (target − video position) in ms — Some only while a follow cue runs.
    pub mtc_drift_ms: Option<f64>,
    /// Available audio input devices for the LTC chase source (published by
    /// the control binary on a slow scan, scoped to the selected driver).
    pub ltc_input_devices: Vec<String>,
    /// Available audio output devices for LTC generate (same scan, scoped to
    /// the LTC output driver — the programme device list belongs to the
    /// programme driver and may differ).
    pub ltc_output_devices: Vec<String>,
    /// Next armed timecode trigger: (cue qid, trigger seconds).
    pub next_timecode: Option<(Decimal, f64)>,
    /// Latest pixel-map sample per segment: id → (cols, rows, RGBA bytes).
    /// Published by the control binary; painted by the lighting panel preview
    /// and streamed by the pixel feed. `Arc` so readers snapshot a handle
    /// instead of copying megabytes under this lock.
    pub lighting_preview: std::collections::HashMap<u32, (u32, u32, std::sync::Arc<Vec<u8>>)>,
    /// Set on window-close with unsaved changes / running cues — shows the in-app
    /// quit-confirm modal (a native modal deadlocks the loop).
    pub pending_close_confirm: bool,
    /// A destructive command (New / Open) parked on the in-app discard modal,
    /// for the same reason as `pending_close_confirm`. Holds the command so it
    /// can be re-queued once the operator confirms.
    pub pending_discard_confirm: Option<AppCommand>,
    /// Set by the discard modal so the re-queued command runs without asking
    /// again. One flag is enough: only New and Open consult it, and the modal
    /// is exclusive, so two of them cannot be in flight at once.
    pub discard_confirmed: bool,
    /// Set by the quit-confirm modal; main.rs hard-exits on the next tick.
    pub quit: bool,
    /// Progress overlay: if Some, shows a blocking modal with message + progress.
    pub progress_overlay: Option<ProgressOverlay>,
    /// Latest non-blocking runtime error for the operator. Session-only.
    pub operator_alert: Option<OperatorAlert>,
    /// If Some, the next received MIDI event should be stored as this cue's MIDI trigger.
    pub pending_midi_learn: Option<Decimal>,
    /// If Some, the current show time should be stored as this cue's timecode trigger.
    pub pending_timecode_capture: Option<Decimal>,
    /// Bumped each time a project is loaded. The control binary watches this and
    /// rebuilds its output/projection windows from the new projection settings —
    /// without it, the windows keep the previous project's mapping on a switch.
    pub project_generation: u64,
    /// Current physical monitors, published by the control binary so the projection
    /// panel can offer a real monitor dropdown (not a blind index).
    pub available_monitors: Vec<cuepool_core::MonitorId>,
    /// When set, the control binary flashes each output a distinct colour so the
    /// operator can see which window is on which projector. Cleared after a timeout.
    pub identify_outputs: bool,
    /// Live DMX programming: while on, edits to a Lighting cue's looks in the
    /// inspector stream straight to the fixtures (session-only, not saved).
    pub lighting_live: bool,
    /// Pending "Import from Project…" modal: source show + chosen sections.
    pub import_request: Option<ImportRequest>,
}

/// State of the "Import from <file>" modal (File → Import from Project…).
#[derive(Debug, Clone)]
pub struct ImportRequest {
    /// Source file display name (window title).
    pub name: String,
    /// Parsed source show — only the checked sections are copied.
    pub show: ShowFile,
    pub sections: cuepool_core::ImportSections,
}

/// State for the progress overlay modal.
#[derive(Debug, Clone)]
pub struct ProgressOverlay {
    pub message: String,
    pub progress: f32, // 0.0 to 1.0
}

/// What the operator picked in the quit-confirm modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitChoice {
    Save,
    Discard,
    Cancel,
}

/// Shared by the modal and the `move_to_top` that keeps it reachable, so the
/// two cannot drift apart.
fn quit_modal_id() -> egui::Id {
    egui::Id::new("quit_confirm")
}

/// Where a project that has never been saved goes on "Save & Quit": next to the
/// crash-recovery file, stamped so two quits cannot collide.
fn unsaved_show_path() -> Result<std::path::PathBuf, String> {
    let dir = dirs::data_dir()
        .ok_or("no user data directory")?
        .join("CuePool");
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H%M%S");
    Ok(dir.join(format!("Unsaved Show {stamp}.qproj")))
}

/// A short-lived, non-blocking runtime error shown above the status bar.
#[derive(Debug, Clone)]
pub struct OperatorAlert {
    pub message: String,
    pub expires_at: Instant,
}

impl Default for SharedState {
    fn default() -> Self {
        Self {
            show_file: ShowFile::default(),
            project_path: None,
            selected_cue_id: None,
            command_queue: Vec::new(),
            show_mode: ShowMode::Edit,
            dirty: false,
            undo_redo: UndoRedo::default(),
            active_cues: Vec::new(),
            meter_data: GuiMeterData::default(),
            recent_files: Vec::new(),
            last_seen_release_notes: None,
            show_settings_window: false,
            audio_device_name: String::new(),
            audio_error: None,
            waveform_cache: std::collections::HashMap::new(),
            pending_waveforms: std::collections::HashSet::new(),
            waveform_zoom: 1.0,
            waveform_scroll: 0.0,
            audio_devices: Vec::new(),
            show_log_window: false,
            show_about_window: false,
            show_status_window: false,
            diagnostics: Diagnostics::default(),
            show_waveform_window: false,
            waveform_window_zoom: 1.0,
            waveform_window_scroll: 0.0,
            show_video_window: false,
            show_projection_window: false,
            show_lighting_window: false,
            show_recorder_window: false,
            recorder_file: String::new(),
            recorder_monitor: true,
            recorder_midi_enabled: false,
            recorder_midi_universe: 1,
            open_take_editor: None,
            show_time: None,
            show_paused: false,
            mtc_running: false,
            mtc_playing: false,
            mtc_timecode_secs: 0.0,
            mtc_fps: 25.0,
            mtc_source: String::new(),
            mtc_drift_ms: None,
            ltc_input_devices: Vec::new(),
            ltc_output_devices: Vec::new(),
            next_timecode: None,
            recorder_status: RecorderStatus::default(),
            lighting_preview: std::collections::HashMap::new(),
            pending_close_confirm: false,
            pending_discard_confirm: None,
            discard_confirmed: false,
            quit: false,
            progress_overlay: None,
            operator_alert: None,
            pending_midi_learn: None,
            pending_timecode_capture: None,
            project_generation: 0,
            available_monitors: Vec::new(),
            identify_outputs: false,
            lighting_live: false,
            import_request: None,
        }
    }
}

impl SharedState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn report_operator_error(&mut self, message: impl Into<String>) {
        self.operator_alert = Some(OperatorAlert {
            message: message.into(),
            expires_at: Instant::now() + OPERATOR_ALERT_DURATION,
        });
    }

    /// Add a path to the recent files list, moving it to the front if it already exists.
    pub fn push_recent_file(&mut self, path: &std::path::Path) {
        let path_buf = path.to_path_buf();
        self.recent_files.retain(|p| p != &path_buf);
        self.recent_files.insert(0, path_buf);
        self.recent_files.truncate(10);
    }

    pub fn load_show_file(
        &mut self,
        path: &std::path::Path,
        data: &str,
    ) -> Result<(), serde_json::Error> {
        let show: ShowFile = serde_json::from_str(data)?;
        self.apply_show_file(path, show);
        Ok(())
    }

    fn apply_show_file(&mut self, path: &std::path::Path, show: ShowFile) {
        self.show_file = show;
        self.project_path = Some(path.to_path_buf());
        self.selected_cue_id = None;
        self.dirty = false;
        // Signal the control binary to rebuild output windows for the new projection.
        self.project_generation = self.project_generation.wrapping_add(1);
    }

    pub fn selected_cue(&self) -> Option<&Cue> {
        let id = self.selected_cue_id?;
        self.show_file.cues.iter().find(|c| c.base().qid == id)
    }

    pub fn selected_cue_mut(&mut self) -> Option<&mut Cue> {
        let id = self.selected_cue_id?;
        self.show_file.cues.iter_mut().find(|c| c.base().qid == id)
    }
}

pub type SharedStateHandle = Arc<Mutex<SharedState>>;

#[derive(Debug, Clone)]
pub enum AppCommand {
    NewProject,
    OpenProject {
        path: PathBuf,
    },
    SaveProject,
    SaveProjectAs {
        path: PathBuf,
    },
    PackProject {
        path: PathBuf,
    },
    Go,
    Stop,
    Pause,
    SelectCue(Decimal),
    SelectPreviousCue,
    SelectNextCue,
    SelectFirstCue,
    SelectLastCue,
    Undo,
    Redo,
    AddCue {
        cue_type: CueType,
    },
    DeleteSelectedCue,
    DuplicateSelectedCue,
    MoveSelectedCueUp,
    MoveSelectedCueDown,
    MoveCue {
        from_idx: usize,
        to_idx: usize,
        parent: Option<Decimal>,
    },
    SetLimiterThreshold(f32),
    SetAudioDriver(cuepool_core::AudioOutputDriver),
    SetAudioDevice(String),
    ApplyAudioSettings,
    ToggleVideoWindow,
    ToggleVideoFullscreen,
    ToggleProjectionWindow,
    OpenProjectionOutputs,
    Preload,
    UpdateCueQid {
        qid: Decimal,
        new_qid: Decimal,
    },
    UpdateCueName {
        qid: Decimal,
        name: String,
    },
    UpdateCueTrigger {
        qid: Decimal,
        trigger: cuepool_core::TriggerMode,
    },
    LearnMidiTrigger {
        qid: Decimal,
    },
    CaptureTimecodeTrigger {
        qid: Decimal,
    },
    /// Snap the lighting engine's live state to these looks (inspector live mode).
    LightingLivePush {
        snapshot: std::collections::BTreeMap<u32, cuepool_core::FixtureLook>,
    },
    /// Start a recording pass on `file`, or stop-and-keep the running one.
    RecorderRecord {
        file: String,
    },
    /// Stop and throw away the in-flight pass.
    RecorderDiscard,
    /// Swap `file` with its `.prev` (undo the last kept pass).
    RecorderRevert {
        file: String,
    },
    RecorderSetMonitor(bool),
    /// Preview the take through the lighting output (no cue involved).
    RecorderPreview {
        file: String,
    },
    RecorderStopPreview,
    /// Release every channel the OSC/MIDI live bridge holds.
    RecorderClearLive,
    /// Take-editor scrub: hold this frame on the lighting output (None = release).
    RecorderScrub {
        frame: Option<rustjay_lighting::MaskedFrame>,
    },
    /// Step one video frame forward while paused (show clock follows).
    FrameStep,
    /// Step one video frame back while paused (show clock follows).
    FrameStepBack,
    /// Seek an active Sound or Video cue in the `ActiveCueInfo` timeline.
    /// Looped cues use seconds relative to the loop region; targets outside
    /// that region clamp to its final frame.
    SeekCue {
        instance_id: u64,
        secs: f32,
    },
}

impl AppCommand {
    /// Commands that change the cue list, and so are refused in Show mode.
    ///
    /// The queue window already hides the widgets that raise these, but the same
    /// commands arrive from the keyboard shortcuts (and the right-click menu),
    /// which the mode never gated — so the lock has to be enforced where the
    /// commands are executed, not where they are raised. Selection, transport and
    /// project I/O are deliberately absent: none of them change the cue list.
    ///
    /// Undo and redo *are* here. Nothing in Show mode can create an edit to undo,
    /// so a Cmd+Z during a show can only rewind work done before it — which is
    /// exactly what the mode promises will not happen.
    fn edits_cues(&self) -> bool {
        matches!(
            self,
            Self::Undo
                | Self::Redo
                | Self::AddCue { .. }
                | Self::DeleteSelectedCue
                | Self::DuplicateSelectedCue
                | Self::MoveSelectedCueUp
                | Self::MoveSelectedCueDown
                | Self::MoveCue { .. }
                | Self::UpdateCueQid { .. }
                | Self::UpdateCueName { .. }
                | Self::UpdateCueTrigger { .. }
        )
    }
}

#[derive(Clone, Copy)]
enum SelectionStep {
    Previous,
    Next,
    First,
    Last,
}

fn step_selection(cues: &[Cue], current: Option<Decimal>, step: SelectionStep) -> Option<Decimal> {
    if cues.is_empty() {
        return None;
    }
    let current_idx = current.and_then(|qid| cues.iter().position(|cue| cue.base().qid == qid));
    let idx = match step {
        SelectionStep::Previous => current_idx.map_or(cues.len() - 1, |idx| idx.saturating_sub(1)),
        SelectionStep::Next => current_idx.map_or(0, |idx| (idx + 1).min(cues.len() - 1)),
        SelectionStep::First => 0,
        SelectionStep::Last => cues.len() - 1,
    };
    Some(cues[idx].base().qid)
}

fn audio_driver_command(
    current: cuepool_core::AudioOutputDriver,
    selected: cuepool_core::AudioOutputDriver,
) -> Option<AppCommand> {
    (current != selected).then_some(AppCommand::SetAudioDriver(selected))
}

/// Driver combo for the LTC in/out settings — same options as the programme
/// Output Driver, but a plain settings write: the control binary rebuilds
/// the LTC streams on change, so no AppCommand is needed. Returns true on
/// change.
fn ltc_driver_combo(
    ui: &mut egui::Ui,
    id_salt: &str,
    current: &mut cuepool_core::AudioOutputDriver,
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(current.to_string())
        .show_ui(ui, |ui| {
            for driver in [
                cuepool_core::AudioOutputDriver::WASAPI,
                cuepool_core::AudioOutputDriver::Wave,
                cuepool_core::AudioOutputDriver::DirectSound,
                cuepool_core::AudioOutputDriver::ASIO,
            ] {
                if ui
                    .selectable_label(*current == driver, driver.to_string())
                    .clicked()
                {
                    *current = driver;
                    changed = true;
                }
            }
        });
    changed
}

fn queue_audio_settings_apply(state: &mut SharedState) {
    if !state
        .command_queue
        .iter()
        .any(|command| matches!(command, AppCommand::ApplyAudioSettings))
    {
        state.command_queue.push(AppCommand::ApplyAudioSettings);
    }
}

fn apply_project_import(
    state: &mut SharedState,
    source: &ShowFile,
    sections: cuepool_core::ImportSections,
) {
    let snapshot = Snapshot::from_state(state);
    state.undo_redo.push(snapshot);
    cuepool_core::apply_import(&mut state.show_file, source, sections);
    state.dirty = true;
    if sections.projection || sections.lighting {
        state.project_generation = state.project_generation.wrapping_add(1);
    }
    if sections.show_settings {
        queue_audio_settings_apply(state);
    }
}

/// DMX recorder status, published by the engine each tick.
#[derive(Debug, Clone, Default)]
pub struct RecorderStatus {
    pub recording: bool,
    pub elapsed_s: f32,
    pub event_count: usize,
    pub punched_count: usize,
    pub rx_packets: u64,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CueType {
    Sound,
    Video,
    Stop,
    Volume,
    Group,
    Dummy,
    TimeCode,
    Osc,
    Text,
    Image,
    Goto,
    Lighting,
    DmxShow,
    PixelMap,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowMode {
    Edit,
    Show,
}

/// The main egui application.
pub struct CuePoolApp {
    state: SharedStateHandle,
    take_editor: crate::take_editor::TakeEditor,
    /// Status window "Copied!" feedback: when the clipboard copy happened.
    status_copied_at: Option<Instant>,
    /// Starts on the first rendered frame, so slow initialization cannot consume it.
    launch_card_pending: bool,
    launch_card_started_at: Option<f64>,
    /// When Help → About opened the card, so its torus animates from zero.
    invoked_card_opened_at: Option<f64>,
    /// Did a widget hold keyboard focus when the last frame ended?
    ///
    /// egui drops focus in `Focus::begin_pass` when Escape arrives, so by the
    /// time the shortcut handler runs, the field that Escape is cancelling
    /// already reads as unfocused. This remembers the frame before.
    keyboard_focus_at_frame_end: bool,
}

impl Default for CuePoolApp {
    fn default() -> Self {
        Self::new()
    }
}

impl CuePoolApp {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(SharedState::new())),
            take_editor: Default::default(),
            status_copied_at: None,
            launch_card_pending: true,
            launch_card_started_at: None,
            invoked_card_opened_at: None,
            keyboard_focus_at_frame_end: false,
        }
    }

    pub fn with_show_file(show: ShowFile, path: Option<PathBuf>) -> Self {
        Self {
            state: Arc::new(Mutex::new(SharedState {
                show_file: show,
                project_path: path,
                last_seen_release_notes: Some(RELEASE_NOTES_VERSION.into()),
                ..SharedState::default()
            })),
            take_editor: Default::default(),
            status_copied_at: None,
            launch_card_pending: false,
            launch_card_started_at: None,
            invoked_card_opened_at: None,
            keyboard_focus_at_frame_end: false,
        }
    }

    pub fn state(&self) -> &SharedStateHandle {
        &self.state
    }

    /// Render the contents of the native Status window.
    pub fn show_status(&mut self, ui: &mut egui::Ui) {
        crate::status_panel::show(ui, &self.state, &mut self.status_copied_at);
    }

    /// Open a project without showing native confirmation dialogs.
    ///
    /// Remote automation may only replace a clean project with a bounded,
    /// regular local file. The playback owner separately enforces idleness.
    pub fn open_project_unattended(&mut self, path: &std::path::Path) -> Result<(), String> {
        if self
            .state
            .lock()
            .map_err(|_| "project state lock poisoned".to_string())?
            .dirty
        {
            return Err("current project has unsaved changes".into());
        }
        let project = prepare_unattended_project(path)?;
        self.apply_unattended_project(project)
    }

    pub fn apply_unattended_project(&mut self, project: PreparedProject) -> Result<(), String> {
        {
            let state = self
                .state
                .lock()
                .map_err(|_| "project state lock poisoned".to_string())?;
            if state.dirty {
                return Err("current project has unsaved changes".into());
            }
        }
        let metadata = project.path.metadata().map_err(|error| {
            format!(
                "project changed after validation; failed to inspect '{}': {error}",
                project.path.display()
            )
        })?;
        if metadata.len() != project.file_len || metadata.modified().ok() != project.modified {
            return Err("project changed after validation; submit the command again".into());
        }
        self.apply_project_show(&project.path, project.show)
    }

    pub fn select_cue(&mut self, id: Decimal) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "project state lock poisoned".to_string())?;
        if !state.show_file.cues.iter().any(|cue| cue.base().qid == id) {
            return Err(format!("cue Q{id} not found"));
        }
        // Not undoable — see `step_selection`'s caller for why.
        state.selected_cue_id = Some(id);
        Ok(())
    }

    fn open_project_path(&mut self, path: &std::path::Path) -> Result<(), String> {
        let data = std::fs::read_to_string(path)
            .map_err(|error| format!("failed to read '{}': {error}", path.display()))?;
        self.apply_project_text(path, &data)
    }

    fn apply_project_text(&mut self, path: &std::path::Path, data: &str) -> Result<(), String> {
        let show = serde_json::from_str(data)
            .map_err(|error| format!("failed to parse '{}': {error}", path.display()))?;
        self.apply_project_show(path, show)
    }

    fn apply_project_show(&mut self, path: &std::path::Path, show: ShowFile) -> Result<(), String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "project state lock poisoned".to_string())?;
        let snapshot = Snapshot::from_state(&state);
        state.undo_redo.push(snapshot);
        state.apply_show_file(path, show);
        state.push_recent_file(path);
        log::info!("Open project: {:?}", path);
        Ok(())
    }
}

pub fn prepare_unattended_project(path: &std::path::Path) -> Result<PreparedProject, String> {
    if !path.is_absolute() {
        return Err("project path must be absolute".into());
    }
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("qproj"))
    {
        return Err("project path must name a .qproj file".into());
    }
    #[cfg(windows)]
    if !matches!(
        path.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(prefix.kind(), std::path::Prefix::Disk(_))
    ) {
        return Err("project path must use a local drive".into());
    }
    let canonical = path
        .canonicalize()
        .map_err(|error| format!("failed to resolve '{}': {error}", path.display()))?;
    let metadata = canonical
        .metadata()
        .map_err(|error| format!("failed to inspect '{}': {error}", canonical.display()))?;
    if !metadata.is_file() {
        return Err("project path must name a regular file".into());
    }
    if metadata.len() > MAX_AUTOMATION_PROJECT_BYTES {
        return Err(format!(
            "project exceeds the {} MiB automation limit",
            MAX_AUTOMATION_PROJECT_BYTES / 1024 / 1024
        ));
    }
    let file = std::fs::File::open(&canonical)
        .map_err(|error| format!("failed to open '{}': {error}", canonical.display()))?;
    let mut data = String::new();
    file.take(MAX_AUTOMATION_PROJECT_BYTES + 1)
        .read_to_string(&mut data)
        .map_err(|error| format!("failed to read '{}': {error}", canonical.display()))?;
    if data.len() as u64 > MAX_AUTOMATION_PROJECT_BYTES {
        return Err(format!(
            "project exceeds the {} MiB automation limit",
            MAX_AUTOMATION_PROJECT_BYTES / 1024 / 1024
        ));
    }
    let show = serde_json::from_str(&data)
        .map_err(|error| format!("failed to parse '{}': {error}", canonical.display()))?;
    Ok(PreparedProject {
        path: canonical,
        show,
        file_len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

impl CuePoolApp {
    pub fn update(&mut self, ui: &mut egui::Ui) {
        // Panels lay out in the root `ui`; windows/areas/input still go through
        // the context.
        let ctx = &ui.ctx().clone();
        let frame_time = ctx.input(|i| i.time);
        let launch_card_timing = self.launch_card_timing(frame_time);
        let show_release_notes = launch_card_timing.is_none()
            && self
                .state
                .lock()
                .map(|state| {
                    state.last_seen_release_notes.as_deref() != Some(RELEASE_NOTES_VERSION)
                })
                .unwrap_or(false);
        // Keyboard shortcuts. A focused field owns every keystroke that a text
        // caret could claim — the bare keys it is being typed into, plus Cmd+Z
        // (the field runs its own undo) and Cmd+arrows (start/end of text on
        // macOS). Only the shortcuts that collide with nothing stay live, as
        // menu shortcuts do in every other app. Startup overlays block the lot,
        // so the keypress that dismisses the launch card cannot also operate
        // the show behind it.
        //
        // The focus test has to reach back a frame: egui drops focus in
        // `Focus::begin_pass` when Escape arrives, so on the frame that cancels
        // an edit the field already reads as unfocused, and Escape would stop
        // the show instead of just closing the editor.
        let editing_text = ctx.egui_wants_keyboard_input() || self.keyboard_focus_at_frame_end;
        ctx.input(|i| {
            if launch_card_timing.is_some() || show_release_notes {
                return;
            }
            let modifiers = i.modifiers;

            // New / Open / Save
            if modifiers.command
                && i.key_pressed(egui::Key::N)
                && let Ok(mut state) = self.state.lock()
            {
                state.command_queue.push(AppCommand::NewProject);
            }
            if modifiers.command
                && i.key_pressed(egui::Key::O)
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("CuePool project", &["qproj"])
                    .pick_file()
                && let Ok(mut state) = self.state.lock()
            {
                state.command_queue.push(AppCommand::OpenProject { path });
            }
            if modifiers.command
                && i.key_pressed(egui::Key::S)
                && let Ok(mut state) = self.state.lock()
            {
                state.command_queue.push(AppCommand::SaveProject);
            }

            // Duplicate selected cue
            if modifiers.command
                && i.key_pressed(egui::Key::D)
                && let Ok(mut state) = self.state.lock()
            {
                state.command_queue.push(AppCommand::DuplicateSelectedCue);
            }

            // Add new sound cue
            if modifiers.command
                && i.key_pressed(egui::Key::T)
                && let Ok(mut state) = self.state.lock()
            {
                state.command_queue.push(AppCommand::AddCue {
                    cue_type: CueType::Sound,
                });
            }

            // Everything past here is the field's while one is being edited.
            // One gate, so a shortcut added later cannot quietly default to
            // hijacking whatever the operator is typing.
            if editing_text {
                return;
            }

            // Undo / Redo
            if modifiers.command && i.key_pressed(egui::Key::Z) {
                let cmd = if modifiers.shift {
                    AppCommand::Redo
                } else {
                    AppCommand::Undo
                };
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(cmd);
                }
            }

            // Delete selected cue (Delete, or Backspace which is the Mac "delete").
            if (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
                && let Ok(mut state) = self.state.lock()
            {
                state.command_queue.push(AppCommand::DeleteSelectedCue);
            }

            // Move selected cue up/down
            if modifiers.command {
                if i.key_pressed(egui::Key::ArrowUp)
                    && let Ok(mut state) = self.state.lock()
                {
                    state.command_queue.push(AppCommand::MoveSelectedCueUp);
                }
                if i.key_pressed(egui::Key::ArrowDown)
                    && let Ok(mut state) = self.state.lock()
                {
                    state.command_queue.push(AppCommand::MoveSelectedCueDown);
                }
            }

            // Walk the standby playhead in list order.
            if modifiers.is_none() {
                let command = if i.key_pressed(egui::Key::ArrowUp) {
                    Some(AppCommand::SelectPreviousCue)
                } else if i.key_pressed(egui::Key::ArrowDown) {
                    Some(AppCommand::SelectNextCue)
                } else if i.key_pressed(egui::Key::Home) {
                    Some(AppCommand::SelectFirstCue)
                } else if i.key_pressed(egui::Key::End) {
                    Some(AppCommand::SelectLastCue)
                } else {
                    None
                };
                if let Some(command) = command
                    && let Ok(mut state) = self.state.lock()
                {
                    state.command_queue.push(command);
                }
            }

            // Go / Stop / Pause (transport shortcuts). `editing_text` gates these
            // for the same reason it gates the keys above: Space is an ordinary
            // character in a cue name, and Escape is the cancel key for the Q#
            // cell and the inspector's timecode fields. Both reach here before any
            // widget consumes them, so ungated the one keypress edited the cue
            // *and* ran the show.
            //
            // Gating on any focused widget rather than only a text field costs
            // nothing: egui does not focus a widget on a mouse click (only text
            // fields, on click, and anything reached by Tab), so this cannot
            // silence Space after the operator clicks Go. And where Tab has left
            // a button focused, egui already treats Space as a click on it —
            // firing the shortcut too would double up.
            if !editing_text && !modifiers.command && !modifiers.alt {
                if i.key_pressed(egui::Key::Space)
                    && let Ok(mut state) = self.state.lock()
                {
                    state.command_queue.push(AppCommand::Go);
                }
                if i.key_pressed(egui::Key::Escape)
                    && let Ok(mut state) = self.state.lock()
                {
                    state.command_queue.push(AppCommand::Stop);
                }
            }
        });

        // Top menu bar
        egui::Panel::top("menu_bar").show(ui, |ui| {
            self.menu_bar(ui);
        });

        // Transport controls
        egui::Panel::top("transport").show(ui, |ui| {
            crate::transport::show(ui, &self.state);
        });

        // Status bar
        egui::Panel::bottom("status_bar").show(ui, |ui| {
            self.status_bar(ui);
        });

        let now = Instant::now();
        let alert = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            if state
                .operator_alert
                .as_ref()
                .is_some_and(|alert| alert.expires_at <= now)
            {
                state.operator_alert = None;
            }
            state.operator_alert.clone()
        };
        if let Some(alert) = alert {
            ctx.request_repaint_after(alert.expires_at.saturating_duration_since(now));
            let mut dismiss = false;
            egui::Panel::bottom("operator_alert")
                .frame(
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(70, 28, 28))
                        .stroke(egui::Stroke::new(
                            1.0_f32,
                            egui::Color32::from_rgb(220, 100, 100),
                        ))
                        .inner_margin(8),
                )
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Neutral tag, not a cause: the alert channel carries
                        // failed projection outputs, unreadable video, failed
                        // saves and refused cue edits, and every caller writes a
                        // complete sentence. Naming one cause here mislabelled
                        // all the others.
                        ui.label(
                            egui::RichText::new("Alert")
                                .strong()
                                .color(egui::Color32::LIGHT_RED),
                        );
                        ui.separator();
                        ui.add(egui::Label::new(&alert.message).wrap());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            dismiss = ui.button("Dismiss").clicked()
                        });
                    });
                });
            if dismiss && let Ok(mut state) = self.state.lock() {
                state.operator_alert = None;
            }
        }

        // Active cues panel (left side)
        egui::Panel::left("active_cues")
            .default_size(220.0)
            .show(ui, |ui| {
                crate::active_cues::show(ui, &self.state);
            });

        // Cue inspector (right side)
        egui::Panel::right("inspector")
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    crate::inspector::show(ui, &self.state);
                });
            });

        // Main cue list (central: fills what the panels above left over).
        egui::CentralPanel::default().show(ui, |ui| {
            crate::cue_list::show(ui, &self.state);
        });

        // Progress overlay
        let overlay = {
            let Ok(state) = self.state.lock() else {
                return;
            };
            state.progress_overlay.clone()
        };
        if let Some(overlay) = overlay {
            egui::Area::new(egui::Id::new("progress_overlay"))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let screen_rect = ctx.content_rect();
                    ui.painter().rect_filled(
                        screen_rect,
                        0.0,
                        egui::Color32::from_rgba_premultiplied(0, 0, 0, 180),
                    );

                    let modal_size = egui::vec2(320.0, 120.0);
                    let modal_rect = egui::Rect::from_center_size(screen_rect.center(), modal_size);
                    ui.painter()
                        .rect_filled(modal_rect, 8.0, ui.visuals().panel_fill);
                    ui.painter().rect_stroke(
                        modal_rect,
                        8.0,
                        egui::Stroke::new(
                            1.0_f32,
                            ui.visuals().widgets.noninteractive.bg_stroke.color,
                        ),
                        egui::StrokeKind::Inside,
                    );

                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(modal_rect.shrink(16.0)),
                        |ui| {
                            ui.vertical_centered(|ui| {
                                ui.heading("Please Wait");
                                ui.add_space(8.0);
                                ui.label(&overlay.message);
                                ui.add_space(8.0);
                                let progress = overlay.progress.clamp(0.0, 1.0);
                                ui.add(egui::ProgressBar::new(progress).show_percentage());
                            });
                        },
                    );
                });
        }

        // Project settings window
        let mut show_settings = if let Ok(state) = self.state.lock() {
            state.show_settings_window
        } else {
            false
        };
        if show_settings {
            let mut settings_changed = false;
            let mut limiter_cmd: Option<AppCommand> = None;
            let mut audio_driver_cmd: Option<AppCommand> = None;
            let mut audio_device_cmd: Option<AppCommand> = None;
            egui::Window::new("Project Settings")
                .collapsible(false)
                .resizable(true)
                .default_size([380.0, 520.0])
                .open(&mut show_settings)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Ok(mut state) = self.state.lock() {
                        let devices = state.audio_devices.clone();
                        let current_device = state.audio_device_name.clone();
                        let ltc_inputs = state.ltc_input_devices.clone();
                        let ltc_outputs = state.ltc_output_devices.clone();
                        let audio_error = state.audio_error.clone();
                        let threshold = state.command_queue.iter().rev().find_map(|cmd| {
                            if let AppCommand::SetLimiterThreshold(t) = cmd { Some(*t) } else { None }
                        }).unwrap_or(0.95);
                        let settings = &mut state.show_file.show_settings;

                        egui::CollapsingHeader::new("Show Info").default_open(true).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Title:");
                                settings_changed |= ui.text_edit_singleline(&mut settings.title).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Author:");
                                settings_changed |= ui.text_edit_singleline(&mut settings.author).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Description:");
                                settings_changed |= ui.text_edit_singleline(&mut settings.description).changed();
                            });
                        });
                        ui.separator();

                        egui::CollapsingHeader::new("Audio").default_open(true).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Latency (ms):");
                                settings_changed |= ui.add(egui::DragValue::new(&mut settings.audio_latency).speed(1).range(10..=500)).changed();
                            });
                            settings_changed |= ui.checkbox(&mut settings.exclusive_mode, "Exclusive Mode").changed();

                            ui.horizontal(|ui| {
                                ui.label("Output Driver:");
                                egui::ComboBox::from_id_salt("audio_driver")
                                    .selected_text(settings.audio_output_driver.to_string())
                                    .show_ui(ui, |ui| {
                                        for driver in [
                                            cuepool_core::AudioOutputDriver::WASAPI,
                                            cuepool_core::AudioOutputDriver::Wave,
                                            cuepool_core::AudioOutputDriver::DirectSound,
                                            cuepool_core::AudioOutputDriver::ASIO,
                                        ] {
                                            if ui
                                                .selectable_label(
                                                    driver == settings.audio_output_driver,
                                                    driver.to_string(),
                                                )
                                                .clicked()
                                            {
                                                audio_driver_cmd = audio_driver_command(
                                                    settings.audio_output_driver,
                                                    driver,
                                                );
                                            }
                                        }
                                    });
                            });

                            ui.horizontal(|ui| {
                                ui.label("Output Device:");
                                egui::ComboBox::from_id_salt("audio_device")
                                    .selected_text(&current_device)
                                    .width(200.0)
                                    .show_ui(ui, |ui| {
                                        for device in &devices {
                                            let label = if device.is_available() {
                                                device.name.clone()
                                            } else {
                                                format!("{} (unavailable)", device.name)
                                            };
                                            let response = ui.add_enabled(
                                                device.is_available(),
                                                egui::Button::selectable(
                                                    device.name == current_device,
                                                    label,
                                                ),
                                            );
                                            let response = if let Some(error) = &device.probe_error {
                                                response.on_hover_text(format!(
                                                    "Configuration probe failed: {error}"
                                                ))
                                            } else {
                                                response
                                            };
                                            if response.clicked() {
                                                audio_device_cmd = Some(AppCommand::SetAudioDevice(
                                                    device.name.clone(),
                                                ));
                                            }
                                        }
                                    });
                            });

                            if let Some(error) = &audio_error {
                                ui.colored_label(
                                    egui::Color32::LIGHT_RED,
                                    format!("Audio playback disabled: {error}"),
                                );
                            }

                            ui.label("Master Limiter Threshold:");
                            let mut db = 20.0 * threshold.log10();
                            let response = ui.add(egui::Slider::new(&mut db, -24.0..=0.0).text("dB"));
                            if response.changed() {
                                let linear = 10.0f32.powf(db / 20.0);
                                limiter_cmd = Some(AppCommand::SetLimiterThreshold(linear));
                            }
                        });
                        ui.separator();

                        egui::CollapsingHeader::new("Timecode").default_open(false).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Display fps:")
                                    .on_hover_text("Frame rate of the transport clock readout only — triggers are stored in seconds");
                                settings_changed |= ui
                                    .add(egui::DragValue::new(&mut settings.timecode_fps).speed(1).range(1.0..=120.0))
                                    .changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("Chase source:")
                                    .on_hover_text("External timecode the show chases — video follow cues and the transport readout");
                                egui::ComboBox::from_id_salt("timecode_source")
                                    .selected_text(settings.timecode_source.to_string())
                                    .show_ui(ui, |ui| {
                                        for source in [
                                            cuepool_core::TimecodeSourceKind::Mtc,
                                            cuepool_core::TimecodeSourceKind::Ltc,
                                        ] {
                                            if ui
                                                .selectable_label(
                                                    source == settings.timecode_source,
                                                    source.to_string(),
                                                )
                                                .clicked()
                                            {
                                                settings.timecode_source = source;
                                                settings_changed = true;
                                            }
                                        }
                                    });
                            });
                            if settings.timecode_source == cuepool_core::TimecodeSourceKind::Ltc {
                                ui.horizontal(|ui| {
                                    ui.label("LTC driver:")
                                        .on_hover_text("Audio driver hosting the LTC input — pick ASIO for multichannel interfaces on Windows");
                                    settings_changed |=
                                        ltc_driver_combo(ui, "ltc_input_driver", &mut settings.ltc_input_driver);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("LTC input:")
                                        .on_hover_text("Audio input carrying linear timecode");
                                    egui::ComboBox::from_id_salt("ltc_input_device")
                                        .selected_text(if settings.ltc_input_device.is_empty() {
                                            "System Default".to_string()
                                        } else {
                                            settings.ltc_input_device.clone()
                                        })
                                        .width(200.0)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    settings.ltc_input_device.is_empty(),
                                                    "System Default",
                                                )
                                                .clicked()
                                            {
                                                settings.ltc_input_device.clear();
                                                settings_changed = true;
                                            }
                                            for device in &ltc_inputs {
                                                if ui
                                                    .selectable_label(
                                                        settings.ltc_input_device == *device,
                                                        device,
                                                    )
                                                    .clicked()
                                                {
                                                    settings.ltc_input_device = device.clone();
                                                    settings_changed = true;
                                                }
                                            }
                                        });
                                });
                                ui.horizontal(|ui| {
                                    ui.label("LTC channel:")
                                        .on_hover_text("Channel of the input device carrying timecode (clamped to the device's channel count)");
                                    settings_changed |= ui
                                        .add(egui::DragValue::new(&mut settings.ltc_input_channel).speed(1).range(1..=64))
                                        .changed();
                                });
                            }

                            settings_changed |= ui
                                .checkbox(&mut settings.ltc_output_enabled, "Generate LTC out")
                                .on_hover_text(
                                    "Encode the show clock as LTC on one channel of a dedicated \
                                     output, so external gear can chase CuePool — never mixed \
                                     into programme audio",
                                )
                                .changed();
                            if settings.ltc_output_enabled {
                                ui.horizontal(|ui| {
                                    ui.label("LTC out driver:")
                                        .on_hover_text("Audio driver hosting the LTC output — pick ASIO for multichannel interfaces on Windows");
                                    settings_changed |=
                                        ltc_driver_combo(ui, "ltc_output_driver", &mut settings.ltc_output_driver);
                                });
                                ui.horizontal(|ui| {
                                    ui.label("LTC output:");
                                    egui::ComboBox::from_id_salt("ltc_output_device")
                                        .selected_text(if settings.ltc_output_device.is_empty() {
                                            "System Default".to_string()
                                        } else {
                                            settings.ltc_output_device.clone()
                                        })
                                        .width(200.0)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    settings.ltc_output_device.is_empty(),
                                                    "System Default",
                                                )
                                                .clicked()
                                            {
                                                settings.ltc_output_device.clear();
                                                settings_changed = true;
                                            }
                                            for device in &ltc_outputs {
                                                if ui
                                                    .selectable_label(
                                                        settings.ltc_output_device == *device,
                                                        device,
                                                    )
                                                    .clicked()
                                                {
                                                    settings.ltc_output_device = device.clone();
                                                    settings_changed = true;
                                                }
                                            }
                                        });
                                });
                                ui.horizontal(|ui| {
                                    ui.label("LTC out channel:")
                                        .on_hover_text("Channel of the output device the timecode plays on (clamped to the device's channel count)");
                                    settings_changed |= ui
                                        .add(egui::DragValue::new(&mut settings.ltc_output_channel).speed(1).range(1..=64))
                                        .changed();
                                });
                                ui.horizontal(|ui| {
                                    ui.label("LTC fps:");
                                    egui::ComboBox::from_id_salt("ltc_output_fps")
                                        .selected_text(settings.ltc_output_fps.to_string())
                                        .show_ui(ui, |ui| {
                                            for rate in cuepool_core::TimecodeFrameRate::ALL {
                                                if ui
                                                    .selectable_label(
                                                        rate == settings.ltc_output_fps,
                                                        rate.to_string(),
                                                    )
                                                    .clicked()
                                                {
                                                    settings.ltc_output_fps = rate;
                                                    settings_changed = true;
                                                }
                                            }
                                        });
                                });
                                ui.horizontal(|ui| {
                                    ui.label("LTC start:").on_hover_text(
                                        "Timecode label at show position 0 (Pro Tools convention: 01:00:00:00)",
                                    );
                                    let mut secs = settings.ltc_output_start.as_secs_f64();
                                    if crate::inspector::timecode_edit(
                                        ui,
                                        "ltc_output_start",
                                        &mut secs,
                                        settings.ltc_output_fps.fps(),
                                    ) {
                                        settings.ltc_output_start =
                                            cuepool_core::Timespan::from_secs_f64(secs);
                                        settings_changed = true;
                                    }
                                });
                            }
                        });
                        ui.separator();

                        egui::CollapsingHeader::new("OSC / Remote").default_open(false).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("Destination:")
                                    .on_hover_text(
                                        "Where outbound OSC and MSC are sent. Pick an interface to \
                                         use its broadcast address, or type a unicast address. \
                                         Applies on restart.",
                                    );
                                settings_changed |= ui.text_edit_singleline(&mut settings.osc_tx_host).changed();
                                egui::ComboBox::from_id_salt("osc_tx_host_pick")
                                    .selected_text("Pick…")
                                    .show_ui(ui, |ui| {
                                        // ponytail: Enumerate on open rather than caching. The
                                        // ceiling is one getifaddrs per frame while the dropdown
                                        // is held open; cache on the panel struct if that ever
                                        // shows up in a profile.
                                        let nics = crate::osc_destination::local_nics();
                                        for choice in crate::osc_destination::destination_choices(&nics) {
                                            let picked = settings.osc_tx_host == choice.address;
                                            if ui.selectable_label(picked, &choice.label).clicked() {
                                                settings.osc_tx_host = choice.address;
                                                settings_changed = true;
                                            }
                                        }
                                    });
                            });
                            ui.horizontal(|ui| {
                                ui.label("RX Port:");
                                settings_changed |= ui.add(egui::DragValue::new(&mut settings.osc_rx_port).speed(1)).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("TX Port:");
                                settings_changed |= ui.add(egui::DragValue::new(&mut settings.osc_tx_port).speed(1)).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("UDP default target host (255.255.255.255 = broadcast):")
                                    .on_hover_text("Destination for `udp:` cue commands without a target prefix — set a specific IP for unicast");
                                settings_changed |= ui.text_edit_singleline(&mut settings.udp_tx_host).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("UDP target port:");
                                settings_changed |= ui.add(egui::DragValue::new(&mut settings.udp_tx_port).speed(1)).changed();
                            });

                            // Named UDP targets (per-cue `udp:name:payload` addressing)
                            ui.separator();
                            ui.label("UDP Named Targets (udp:name:payload):");
                            let mut target_to_remove = None;
                            for (idx, target) in settings.udp_targets.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    ui.label("Name:");
                                    settings_changed |= ui.text_edit_singleline(&mut target.name).changed();
                                    ui.label("Host:");
                                    settings_changed |= ui.text_edit_singleline(&mut target.host).changed();
                                    if ui.button("×").clicked() {
                                        target_to_remove = Some(idx);
                                    }
                                });
                            }
                            if let Some(idx) = target_to_remove {
                                settings.udp_targets.remove(idx);
                                settings_changed = true;
                            }
                            if ui.button("Add target").clicked() {
                                settings.udp_targets.push(cuepool_core::UdpTarget::default());
                                settings_changed = true;
                            }
                            settings_changed |= ui.checkbox(&mut settings.enable_remote_control, "Enable Remote Control").changed();
                            settings_changed |= ui.checkbox(&mut settings.is_remote_host, "Is Remote Host").changed();
                            settings_changed |= ui.checkbox(&mut settings.sync_show_file_on_save, "Sync Showfile On Save").changed();
                            ui.horizontal(|ui| {
                                ui.label("Node Name:");
                                settings_changed |= ui.text_edit_singleline(&mut settings.node_name).changed();
                            });

                            // Detected remote nodes
                            ui.separator();
                            ui.label("Detected Remote Nodes:");
                            let now = std::time::Instant::now();
                            let mut to_remove = Vec::new();
                            for (idx, node) in settings.remote_nodes.iter().enumerate() {
                                let is_active = node.is_live(now);
                                let color = if is_active {
                                    egui::Color32::from_rgb(100, 220, 100)
                                } else {
                                    egui::Color32::from_rgb(220, 100, 100)
                                };
                                ui.horizontal(|ui| {
                                    ui.colored_label(color, if is_active { "●" } else { "○" });
                                    ui.label(format!("{} @ {}", node.name, node.address));
                                    if ui.button("×").clicked() {
                                        to_remove.push(idx);
                                    }
                                });
                            }
                            for idx in to_remove.into_iter().rev() {
                                settings.remote_nodes.remove(idx);
                                settings_changed = true;
                            }
                        });
                        ui.separator();

                        egui::CollapsingHeader::new("MSC").default_open(false).show(ui, |ui| {
                            settings_changed |= ui.checkbox(&mut settings.enable_msc, "Enable MSC").changed();
                            ui.horizontal(|ui| {
                                ui.label("RX Port:");
                                settings_changed |= ui.add(egui::DragValue::new(&mut settings.msc_rx_port).speed(1)).changed();
                            });
                            ui.horizontal(|ui| {
                                ui.label("TX Port:");
                                settings_changed |= ui.add(egui::DragValue::new(&mut settings.msc_tx_port).speed(1)).changed();
                            });
                        });
                    }
                    });
                });
            if let Ok(mut state) = self.state.lock() {
                state.show_settings_window = show_settings;
                if settings_changed {
                    state.dirty = true;
                }
                if let Some(cmd) = limiter_cmd {
                    state.command_queue.push(cmd);
                }
                if let Some(cmd) = audio_driver_cmd {
                    state.command_queue.push(cmd);
                }
                if let Some(cmd) = audio_device_cmd {
                    state.command_queue.push(cmd);
                }
            }
        }

        // Log window
        let mut show_log = if let Ok(state) = self.state.lock() {
            state.show_log_window
        } else {
            false
        };
        if show_log {
            egui::Window::new("Log")
                .collapsible(false)
                .resizable(true)
                .default_size([600.0, 400.0])
                .open(&mut show_log)
                .show(ctx, |ui| {
                    crate::log_window::show(ui, &self.state);
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_log_window = show_log;
        }

        // Waveform pop-out window
        let mut show_waveform = if let Ok(state) = self.state.lock() {
            state.show_waveform_window
        } else {
            false
        };
        if show_waveform {
            let (selected_path, peaks, zoom, scroll) = if let Ok(state) = self.state.lock() {
                let path = state
                    .selected_cue()
                    .and_then(|cue| match cue {
                        cuepool_core::Cue::Sound { path, .. }
                        | cuepool_core::Cue::Video { path, .. } => Some(path.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let peaks = state.waveform_cache.get(&path).cloned();
                (
                    path,
                    peaks,
                    state.waveform_window_zoom,
                    state.waveform_window_scroll,
                )
            } else {
                show_waveform = false;
                (String::new(), None, 1.0, 0.0)
            };
            egui::Window::new("Waveform")
                .collapsible(false)
                .resizable(true)
                .default_size([800.0, 300.0])
                .open(&mut show_waveform)
                .show(ctx, |ui| {
                    if selected_path.is_empty() {
                        ui.label("Select a Sound or Video cue to view its waveform.");
                    } else if let Some(waveform) = peaks {
                        ui.label(
                            std::path::Path::new(&selected_path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&selected_path)
                                .to_string(),
                        );
                        let response = crate::waveform::draw(
                            ui,
                            &waveform,
                            zoom,
                            scroll,
                            200.0,
                            crate::waveform::Interaction::Pan,
                            None,
                        );
                        if let Ok(mut state) = self.state.lock() {
                            state.waveform_window_zoom = response.zoom;
                            state.waveform_window_scroll = response.scroll_offset;
                        }
                    } else {
                        ui.label("Generating waveform…");
                    }
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_waveform_window = show_waveform;
        }

        // Projection mapping window
        let mut show_projection = if let Ok(state) = self.state.lock() {
            state.show_projection_window
        } else {
            false
        };
        if show_projection {
            egui::Window::new("Projection Mapping")
                .collapsible(false)
                .resizable(true)
                .default_size([520.0, 640.0])
                .open(&mut show_projection)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        crate::projection_panel::show(ui, &self.state);
                    });
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_projection_window = show_projection;
        }

        // Lighting window (DMX output + fixture patch)
        let mut show_lighting = if let Ok(state) = self.state.lock() {
            state.show_lighting_window
        } else {
            false
        };
        if show_lighting {
            egui::Window::new("Lighting")
                .collapsible(false)
                .resizable(true)
                .default_size([560.0, 480.0])
                .open(&mut show_lighting)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        crate::lighting_panel::show(ui, &self.state);
                    });
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_lighting_window = show_lighting;
        }

        // DMX Recorder window (wire capture → .dmxrec takes)
        let mut show_recorder = if let Ok(state) = self.state.lock() {
            state.show_recorder_window
        } else {
            false
        };
        if show_recorder {
            egui::Window::new("DMX Recorder")
                .collapsible(false)
                .resizable(true)
                .default_size([420.0, 220.0])
                .open(&mut show_recorder)
                .show(ctx, |ui| {
                    crate::recorder_panel::show(ui, &self.state);
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_recorder_window = show_recorder;
        }

        // Take Editor (curve editing for .dmxrec recordings).
        let editor_request = self
            .state
            .lock()
            .ok()
            .and_then(|mut s| s.open_take_editor.take());
        if let Some(path) = editor_request {
            self.take_editor.open_file(path);
        }
        self.take_editor.show(ctx, &self.state);

        // Quit-confirm modal (in-app; a native dialog deadlocks the winit loop).
        let pending_close = self
            .state
            .lock()
            .map(|s| s.pending_close_confirm)
            .unwrap_or(false);
        if pending_close {
            // egui hands the modal layer to the most recently created modal area
            // and blocks input to every layer below it, so a modal opening after
            // this one left a visible but unclickable "Quit CuePool?" on screen
            // with no way out (#173). Claim the top each frame: a pending quit
            // outranks anything else on screen, and whatever it covers is
            // reachable again after Cancel.
            ctx.move_to_top(egui::LayerId::new(egui::Order::Foreground, quit_modal_id()));
            let response = egui::Modal::new(quit_modal_id()).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Quit CuePool?");
                ui.add_space(4.0);
                ui.label("You have unsaved changes or running cues.");
                ui.add_space(12.0);
                let mut choice = None;
                ui.horizontal(|ui| {
                    if ui.button("Save & Quit").clicked() {
                        choice = Some(QuitChoice::Save);
                    }
                    if ui.button("Discard & Quit").clicked() {
                        choice = Some(QuitChoice::Discard);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Some(QuitChoice::Cancel);
                    }
                });
                choice
            });
            // Escape and a backdrop click mean cancel. Dropping the ModalResponse
            // left both doing nothing, which is most of what "the app soft-locks"
            // looked like to an operator whose reflex is Escape.
            let choice = response
                .inner
                .or_else(|| response.should_close().then_some(QuitChoice::Cancel));
            if !response.is_top_modal {
                // move_to_top only takes effect at end of frame, so this can be
                // false for a frame after another modal appears. Ask for the
                // repaint that resolves it rather than waiting on the next event.
                ctx.request_repaint();
            }
            match choice {
                Some(QuitChoice::Save) => self.save_and_quit(),
                Some(QuitChoice::Discard) => {
                    if let Ok(mut s) = self.state.lock() {
                        s.pending_close_confirm = false;
                        s.quit = true;
                    }
                }
                Some(QuitChoice::Cancel) => {
                    if let Ok(mut s) = self.state.lock() {
                        s.pending_close_confirm = false;
                    }
                }
                None => {}
            }
        }

        // Discard-confirm modal for New / Open, in-app for the same reason.
        let pending_discard = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.pending_discard_confirm.clone());
        if let Some(command) = pending_discard {
            let (dirty, running) = self
                .state
                .lock()
                .map(|s| (s.dirty, !s.active_cues.is_empty()))
                .unwrap_or((false, false));
            let message = match (dirty, running) {
                (true, true) => "There are cues playing and you have unsaved changes.",
                (true, false) => "You have unsaved changes.",
                (false, true) => "There are cues currently playing.",
                (false, false) => "Discard the current project?",
            };
            egui::Modal::new(egui::Id::new("discard_confirm")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading(match command {
                    AppCommand::NewProject => "New Project",
                    _ => "Open Project",
                });
                ui.add_space(4.0);
                ui.label(format!("{message} Discard and continue?"));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard & Continue").clicked()
                        && let Ok(mut s) = self.state.lock()
                    {
                        s.pending_discard_confirm = None;
                        s.discard_confirmed = true;
                        s.command_queue.push(command.clone());
                    }
                    if ui.button("Cancel").clicked()
                        && let Ok(mut s) = self.state.lock()
                    {
                        s.pending_discard_confirm = None;
                    }
                });
            });
        }

        // Import-from-project modal (File → Import from Project…)
        let import = self
            .state
            .lock()
            .ok()
            .and_then(|s| s.import_request.clone());
        if let Some(mut request) = import {
            egui::Window::new(format!("Import from {}", request.name))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("Replace these sections of the current project:");
                    ui.checkbox(&mut request.sections.projection, "Projection mapping");
                    ui.checkbox(&mut request.sections.lighting, "Lighting patch");
                    ui.checkbox(&mut request.sections.show_settings, "Show settings");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let any_checked = request.sections.projection
                            || request.sections.lighting
                            || request.sections.show_settings;
                        if ui
                            .add_enabled(any_checked, egui::Button::new("Import"))
                            .clicked()
                            && let Ok(mut state) = self.state.lock()
                        {
                            apply_project_import(&mut state, &request.show, request.sections);
                            state.import_request = None;
                        }
                        if ui.button("Cancel").clicked()
                            && let Ok(mut state) = self.state.lock()
                        {
                            state.import_request = None;
                        }
                    });
                });
            // Persist checkbox edits unless Import/Cancel cleared the request.
            if let Ok(mut state) = self.state.lock()
                && state.import_request.is_some()
            {
                state.import_request = Some(request);
            }
        }

        if show_release_notes {
            egui::Modal::new(egui::Id::new("release_notes")).show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(
                    egui::RichText::new(format!("What's new · {RELEASE_NOTES_VERSION}"))
                        .monospace()
                        .strong()
                        .color(egui::Color32::from_rgb(255, 184, 92)),
                );
                ui.add_space(4.0);
                ui.heading("A live feed of your pixel map");
                ui.label(
                    "The samples driving your LED segments can now be streamed out over a WebSocket, so a web page can show what the pixel map is doing on any screen you point at it.",
                );
                ui.add_space(12.0);
                ui.separator();
                ui.add_space(8.0);

                for (title, detail) in [
                    (
                        "Off until you ask for it",
                        "Set CUEPOOL_PIXELS_BIND to an address and CuePool serves the feed there. Leave it unset and no extra port opens.",
                    ),
                    (
                        "Every segment, as it plays",
                        "Each enabled segment streams its own grid at up to 60 frames a second. Frames go out only when the pixels change, so a held look costs nothing.",
                    ),
                    (
                        "The page works it out for itself",
                        "The stream opens by describing every segment: grid size, region, scan order and patch. A visualiser can lay itself out without being handed your show file.",
                    ),
                ] {
                    ui.label(
                        egui::RichText::new(title)
                            .strong()
                            .color(egui::Color32::from_rgb(92, 168, 255)),
                    );
                    ui.label(detail);
                    ui.add_space(6.0);
                }

                ui.label(
                    egui::RichText::new(
                        "Also: the quit-confirm dialog is reachable again and offers Save, OSC now logs messages that match no address instead of dropping them in silence, and the unimplemented /qplayer/up and /qplayer/down are gone.",
                    )
                    .small()
                    .color(egui::Color32::from_gray(180)),
                );
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_sized([104.0, 40.0], egui::Button::new("Continue"))
                        .clicked()
                        && let Ok(mut state) = self.state.lock()
                    {
                        state.last_seen_release_notes = Some(RELEASE_NOTES_VERSION.into());
                    }
                });
            });
        }

        // The identity card, in whichever presentation is called for. The launch
        // one wins if both are somehow armed, since Help is unreachable behind it.
        if let Some((elapsed, remaining)) = launch_card_timing {
            let opacity = launch_opacity(remaining);
            ctx.request_repaint_after(remaining.min(identity_card::FRAME_INTERVAL));
            let card = egui::Modal::new(egui::Id::new("identity_card"))
                .frame(egui::Frame::NONE)
                .backdrop_color(CardPresentation::Launch.backdrop().gamma_multiply(opacity))
                .show(ctx, |ui| {
                    ui.set_opacity(opacity);
                    identity_card(ui, elapsed, CardPresentation::Launch);
                });
            // Any click or keypress skips the rest of the hold — the modal's own
            // `should_close` only covers Escape and the backdrop, not the card
            // itself. The shortcut handler above has already ignored this frame's
            // input, so whatever ends the card can't also operate the show.
            let skipped = ctx.input(|i| {
                i.pointer.any_pressed()
                    || i.events
                        .iter()
                        .any(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
            });
            if skipped || card.should_close() {
                self.launch_card_started_at = None;
            }
        } else if !show_release_notes && self.about_requested() {
            let opened_at = *self.invoked_card_opened_at.get_or_insert(frame_time);
            let elapsed = (frame_time - opened_at).max(0.0) as f32;
            ctx.request_repaint_after(identity_card::FRAME_INTERVAL);
            let card = egui::Modal::new(egui::Id::new("identity_card"))
                .frame(egui::Frame::NONE)
                .backdrop_color(CardPresentation::Invoked.backdrop())
                .show(ctx, |ui| {
                    identity_card(ui, elapsed, CardPresentation::Invoked)
                });
            if card.inner || card.should_close() {
                self.invoked_card_opened_at = None;
                if let Ok(mut state) = self.state.lock() {
                    state.show_about_window = false;
                }
            }
        }

        // Sampled after the panels have laid out, so the next frame can tell an
        // Escape that cancels an edit from one that stops the show.
        self.keyboard_focus_at_frame_end = ctx.egui_wants_keyboard_input();

        // Process any commands queued during the frame
        self.process_commands(ctx);
    }

    fn about_requested(&self) -> bool {
        self.state
            .lock()
            .map(|state| state.show_about_window)
            .unwrap_or(false)
    }

    fn launch_card_timing(&mut self, now: f64) -> Option<(f32, Duration)> {
        if self.launch_card_pending {
            self.launch_card_pending = false;
            self.launch_card_started_at = Some(now);
        }
        let elapsed = (now - self.launch_card_started_at?).max(0.0);
        if elapsed >= LAUNCH_HOLD.as_secs_f64() {
            self.launch_card_started_at = None;
            None
        } else {
            Some((
                elapsed as f32,
                Duration::from_secs_f64(LAUNCH_HOLD.as_secs_f64() - elapsed),
            ))
        }
    }
}

/// The CuePool mark as flat geometry: a ring bowl with a tapered tail on the
/// 45° axis, so it reads as a Q and as a cue ball being struck.
///
/// Coordinates are fractions of the mark's own box, lifted from
/// `packaging/cuepool-02-cue.svg`, which stays the source of truth. Painting it
/// rather than shipping a bitmap keeps it crisp at any DPI.
const CUE_MARK_BOWL_CENTRE: f32 = 0.446;
const CUE_MARK_BOWL_RADIUS: f32 = 0.259;
const CUE_MARK_BOWL_STROKE: f32 = 0.194;
const CUE_MARK_TAIL: [(f32, f32); 4] = [
    (0.541, 0.655),
    (0.655, 0.541),
    (0.910, 0.773),
    (0.773, 0.910),
];

fn paint_cue_mark(ui: &mut egui::Ui, size: f32) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Image, ui.is_enabled(), "CuePool")
    });
    if !ui.is_rect_visible(rect) {
        return;
    }
    let colour = ui.visuals().text_color();
    let painter = ui.painter();
    let origin = rect.left_top();
    painter.circle_stroke(
        origin + egui::Vec2::splat(CUE_MARK_BOWL_CENTRE * size),
        CUE_MARK_BOWL_RADIUS * size,
        egui::Stroke::new(CUE_MARK_BOWL_STROKE * size, colour),
    );
    painter.add(egui::Shape::convex_polygon(
        CUE_MARK_TAIL
            .iter()
            .map(|(x, y)| origin + egui::vec2(x * size, y * size))
            .collect(),
        colour,
        egui::Stroke::NONE,
    ));
}

impl CuePoolApp {
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            paint_cue_mark(ui, ui.text_style_height(&egui::TextStyle::Body));
            ui.menu_button("File", |ui| {
                if ui.button("New").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::NewProject);
                    }
                    ui.close();
                }
                if ui.button("Open…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CuePool project", &["qproj"])
                        .pick_file()
                        && let Ok(mut state) = self.state.lock() {
                            state.command_queue.push(AppCommand::OpenProject { path });
                        }
                    ui.close();
                }
                if ui
                    .button("Import from Project…")
                    .on_hover_text("Copy projection, lighting patch and/or show settings from another .qproj (cues are never imported)")
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CuePool project", &["qproj"])
                        .pick_file()
                    {
                        // Same deserialize path as OpenProject, minus the replace.
                        match std::fs::read_to_string(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|data| serde_json::from_str::<ShowFile>(&data).map_err(|e| e.to_string()))
                        {
                            Ok(show) => {
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_else(|| path.display().to_string());
                                if let Ok(mut state) = self.state.lock() {
                                    state.import_request = Some(ImportRequest {
                                        name,
                                        show,
                                        sections: cuepool_core::ImportSections {
                                            projection: true,
                                            lighting: true,
                                            show_settings: true,
                                        },
                                    });
                                }
                            }
                            Err(e) => log::error!("Import from {} failed: {}", path.display(), e),
                        }
                    }
                    ui.close();
                }
                if ui.button("Save").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::SaveProject);
                    }
                    ui.close();
                }
                if ui.button("Save As…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CuePool project", &["qproj"])
                        .save_file()
                        && let Ok(mut state) = self.state.lock() {
                            state.command_queue.push(AppCommand::SaveProjectAs { path });
                        }
                    ui.close();
                }

                ui.separator();
                let mut autosave = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_file.show_settings.autosave_enabled
                };
                if ui.checkbox(&mut autosave, "Autosave").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_file.show_settings.autosave_enabled = autosave;
                        state.dirty = true;
                    }
                    ui.close();
                }

                ui.separator();
                if ui.button("Pack…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CuePool project", &["qproj"])
                        .save_file()
                    {
                        // Strip extension to get target folder (matches C# behavior)
                        let folder = path.with_extension("");
                        if let Ok(mut state) = self.state.lock() {
                            state.command_queue.push(AppCommand::PackProject { path: folder });
                        }
                    }
                    ui.close();
                }

                ui.separator();
                if ui.button("Project Settings…").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_settings_window = true;
                    }
                    ui.close();
                }

                // Recent files
                let recent = {
                    let Ok(state) = self.state.lock() else { return };
                    state.recent_files.clone()
                };
                if !recent.is_empty() {
                    ui.separator();
                    ui.label("Recent Files:");
                    for path in &recent {
                        let label = path.file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("Untitled");
                        if ui.button(label).clicked() {
                            if let Ok(mut state) = self.state.lock() {
                                state.command_queue.push(AppCommand::OpenProject { path: path.clone() });
                            }
                            ui.close();
                        }
                    }
                }
            });

            ui.menu_button("Edit", |ui| {
                let (can_undo, can_redo) = {
                    let Ok(state) = self.state.lock() else { return };
                    (state.undo_redo.can_undo(), state.undo_redo.can_redo())
                };
                if ui.add_enabled(can_undo, egui::Button::new("Undo")).clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::Undo);
                    }
                    ui.close();
                }
                if ui.add_enabled(can_redo, egui::Button::new("Redo")).clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::Redo);
                    }
                    ui.close();
                }
            });

            ui.menu_button("Window", |ui| {
                let mut show_log = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_log_window
                };
                if ui.checkbox(&mut show_log, "Log").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_log_window = show_log;
                    }
                    ui.close();
                }
                let mut show_waveform = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_waveform_window
                };
                if ui.checkbox(&mut show_waveform, "Waveform").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_waveform_window = show_waveform;
                    }
                    ui.close();
                }
                let mut show_video = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_video_window
                };
                if ui.checkbox(&mut show_video, "Video Output").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_video_window = show_video;
                        state.command_queue.push(AppCommand::ToggleVideoWindow);
                    }
                    ui.close();
                }
                let mut show_projection = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_projection_window
                };
                if ui.checkbox(&mut show_projection, "Projection Mapping").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_projection_window = show_projection;
                    }
                    ui.close();
                }
                let mut show_lighting = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_lighting_window
                };
                if ui.checkbox(&mut show_lighting, "Lighting").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_lighting_window = show_lighting;
                    }
                    ui.close();
                }
                let mut show_recorder = {
                    let Ok(state) = self.state.lock() else { return; };
                    state.show_recorder_window
                };
                if ui.checkbox(&mut show_recorder, "DMX Recorder").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_recorder_window = show_recorder;
                    }
                    ui.close();
                }
            });

            ui.menu_button("Help", |ui| {
                if ui.button("Status…").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_status_window = true;
                    }
                    ui.close();
                }
                if ui.button("About CuePool").clicked() {
                    if let Ok(mut state) = self.state.lock() {
                        state.show_about_window = true;
                    }
                    ui.close();
                }
            });
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let (active_count, cue_count, show_mode, dirty, video) = {
            let Ok(state) = self.state.lock() else {
                return;
            };
            (
                state.active_cues.len(),
                state.show_file.cues.len(),
                state.show_mode,
                state.dirty,
                state.diagnostics.video.clone(),
            )
        };

        ui.horizontal(|ui| {
            // Status text
            let status = if active_count > 0 {
                format!(
                    "▶ Playing {} cue{}",
                    active_count,
                    if active_count == 1 { "" } else { "s" }
                )
            } else {
                "Ready".to_string()
            };
            ui.label(egui::RichText::new(status).small());

            ui.separator();

            // Show mode indicator
            let mode_text = match show_mode {
                ShowMode::Edit => "🖊 Edit",
                ShowMode::Show => "▶ Show",
            };
            let mode_color = match show_mode {
                ShowMode::Edit => egui::Color32::from_rgb(120, 180, 255),
                ShowMode::Show => egui::Color32::from_rgb(100, 220, 100),
            };
            ui.label(egui::RichText::new(mode_text).small().color(mode_color));

            ui.separator();

            // Cue count
            ui.label(egui::RichText::new(format!("{} cues", cue_count)).small());

            ui.separator();

            // Dirty indicator
            if dirty {
                ui.label(
                    egui::RichText::new("● Unsaved changes")
                        .small()
                        .color(egui::Color32::YELLOW),
                );
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Audio active indicator
                let audio_color = if active_count > 0 {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(120, 120, 120)
                };
                let audio_text = if active_count > 0 {
                    "Audio: On"
                } else {
                    "Audio: Off"
                };
                ui.label(egui::RichText::new(audio_text).small().color(audio_color));

                ui.separator();

                // Video decode path — the same diagnostic Help → Status carries,
                // put where an operator will actually notice acceleration going
                // away. A silent fall back to CPU decode is the difference
                // between a show that holds frame rate and one that doesn't.
                let (video_text, video_color, video_tip) = video_status_badge(video.as_ref());
                ui.label(egui::RichText::new(video_text).small().color(video_color))
                    .on_hover_text(video_tip);
            });
        });
    }

    /// Gate a destructive command (New / Open) behind the in-app discard modal.
    ///
    /// Returns true when the command may run now. Otherwise the command is
    /// parked in `pending_discard_confirm` and re-queued once the operator
    /// confirms. The modal is in-app for the same reason the quit-confirm one
    /// is: a native dialog here deadlocks the winit loop, and with fullscreen
    /// output windows up it can open behind them — the operator then sees a
    /// frozen app waiting on a dialog they cannot find.
    fn confirm_discard(
        state: &SharedStateHandle,
        command: &AppCommand,
        ctx: &egui::Context,
    ) -> bool {
        let Ok(mut state) = state.lock() else {
            return false;
        };
        if state.discard_confirmed {
            state.discard_confirmed = false;
            return true;
        }
        if !state.dirty && state.active_cues.is_empty() {
            return true;
        }
        state.pending_discard_confirm = Some(command.clone());
        // The modal is drawn earlier in this same update, so ask for another
        // pass rather than leaving the operator on a stale frame.
        ctx.request_repaint();
        false
    }

    fn process_commands(&mut self, ctx: &egui::Context) {
        let commands = {
            let Ok(mut state) = self.state.lock() else {
                return;
            };
            let cmds: Vec<_> = state.command_queue.drain(..).collect();
            cmds
        };

        let mut unhandled = Vec::new();
        for cmd in commands {
            // Re-read the mode per command rather than once per drain: `Undo` can
            // restore a different mode part-way through a batch. Only cue edits
            // pay for the lock.
            if cmd.edits_cues()
                && self
                    .state
                    .lock()
                    .is_ok_and(|state| state.show_mode == ShowMode::Show)
            {
                log::warn!("Ignored {cmd:?} — cue editing is locked in Show mode");
                continue;
            }
            match cmd {
                AppCommand::NewProject => {
                    if !Self::confirm_discard(&self.state, &AppCommand::NewProject, ctx) {
                        continue;
                    }
                    if let Ok(mut state) = self.state.lock() {
                        let snapshot = Snapshot::from_state(&state);
                        state.undo_redo.push(snapshot);
                        state.show_file = ShowFile::default();
                        state.project_path = None;
                        state.selected_cue_id = None;
                        state.dirty = false;
                        // Signal the control binary to stop cues + close output windows.
                        state.project_generation = state.project_generation.wrapping_add(1);
                    }
                }
                AppCommand::OpenProject { path } => {
                    if !Self::confirm_discard(
                        &self.state,
                        &AppCommand::OpenProject { path: path.clone() },
                        ctx,
                    ) {
                        continue;
                    }
                    log::info!(
                        target: crate::logging::PERSIST_TARGET,
                        "Project load requested: {}",
                        path.display()
                    );
                    match self.open_project_path(&path) {
                        Ok(()) => log::info!(
                            target: crate::logging::PERSIST_TARGET,
                            "Project loaded: {}",
                            path.display()
                        ),
                        Err(error) => {
                            log::error!("Project load failed for '{}': {error}", path.display())
                        }
                    }
                }
                AppCommand::SaveProject => {
                    let path = {
                        let Ok(state) = self.state.lock() else {
                            continue;
                        };
                        state.project_path.clone()
                    };
                    if let Some(path) = path {
                        if let Err(e) = self.save_to_path(&path) {
                            log::error!("Failed to save project: {}", e);
                        }
                    } else {
                        // No path yet — prompt Save As
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("CuePool project", &["qproj"])
                            .save_file()
                            && let Err(e) = self.save_to_path(&path)
                        {
                            log::error!("Failed to save project: {}", e);
                        }
                    }
                }
                AppCommand::SaveProjectAs { path } => {
                    if let Err(e) = self.save_to_path(&path) {
                        log::error!("Failed to save project: {}", e);
                    }
                }
                AppCommand::PackProject { path } => {
                    if let Err(e) = self.pack_project(&path) {
                        log::error!("Failed to pack project: {}", e);
                    }
                }
                AppCommand::SelectCue(id) => {
                    if let Err(error) = self.select_cue(id) {
                        log::warn!("{error}");
                    }
                }
                navigation @ (AppCommand::SelectPreviousCue
                | AppCommand::SelectNextCue
                | AppCommand::SelectFirstCue
                | AppCommand::SelectLastCue) => {
                    let step = match navigation {
                        AppCommand::SelectPreviousCue => SelectionStep::Previous,
                        AppCommand::SelectNextCue => SelectionStep::Next,
                        AppCommand::SelectFirstCue => SelectionStep::First,
                        AppCommand::SelectLastCue => SelectionStep::Last,
                        _ => unreachable!(),
                    };
                    // Moving the standby playhead is not an edit, so it takes no
                    // undo entry. It used to take one per mouse click (keyboard
                    // navigation merged, mouse did not), which both buried real
                    // edits under a 50-deep stack of selections and — because a
                    // push clears redo — threw redo away just for clicking a cue.
                    // Undo still *restores* a selection, since a snapshot carries
                    // the one in force when the edit was made.
                    if let Ok(mut state) = self.state.lock()
                        && let Some(next) =
                            step_selection(&state.show_file.cues, state.selected_cue_id, step)
                    {
                        state.selected_cue_id = Some(next);
                    }
                }
                AppCommand::Undo => {
                    if let Ok(mut state) = self.state.lock() {
                        let current = Snapshot::from_state(&state);
                        if let Some(prev) = state.undo_redo.undo(current) {
                            state.undo_redo.suppress = true;
                            prev.apply(&mut state);
                            state.undo_redo.suppress = false;
                            log::info!("Undo");
                        }
                    }
                }
                AppCommand::Redo => {
                    if let Ok(mut state) = self.state.lock() {
                        let current = Snapshot::from_state(&state);
                        if let Some(next) = state.undo_redo.redo(current) {
                            state.undo_redo.suppress = true;
                            next.apply(&mut state);
                            state.undo_redo.suppress = false;
                            log::info!("Redo");
                        }
                    }
                }
                AppCommand::AddCue { cue_type } => {
                    if let Ok(mut state) = self.state.lock() {
                        let snapshot = Snapshot::from_state(&state);
                        state.undo_redo.push(snapshot);

                        let next_qid = state.show_file.choose_qid(state.selected_cue_id);
                        // Position and group come from the same anchor the number
                        // does, so a cue numbered "after Q1.1" is not appended to
                        // the end of the show reading Q1, Q2, Q3, Q1.11.
                        let (insert_at, parent) = crate::cue_order::insertion(
                            &state.show_file.cues,
                            state.selected_cue_id,
                        );

                        let mut base = cuepool_core::CueBase {
                            qid: next_qid,
                            parent,
                            name: format!("New {:?} Cue", cue_type),
                            ..Default::default()
                        };
                        // Show clock running (or paused mid-step): pre-fill the
                        // timecode trigger with the current time, so "pause at
                        // the moment, add cue" lands armed at the right spot.
                        if let Some(t) = state.show_time {
                            base.triggers.timecode = Some(cuepool_core::TimecodeTrigger {
                                time: cuepool_core::Timespan::from_secs_f64(t),
                            });
                        }

                        let cue = match cue_type {
                            CueType::Sound => cuepool_core::Cue::Sound {
                                base,
                                path: String::new(),
                                start_time: cuepool_core::Timespan::ZERO,
                                duration: cuepool_core::Timespan::ZERO,
                                volume: 1.0,
                                pan: 0.0,
                                fade_in: 0.0,
                                fade_out: 0.0,
                                fade_type: cuepool_core::FadeType::Linear,
                                eq: None,
                                routing: cuepool_core::AudioRouting::default(),
                            },
                            CueType::Video => cuepool_core::Cue::Video {
                                base,
                                path: String::new(),
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
                            },
                            CueType::Stop => cuepool_core::Cue::Stop {
                                base,
                                stop_qid: Decimal::ZERO,
                                stop_mode: cuepool_core::StopMode::Immediate,
                                fade_out_time: 0.0,
                                fade_type: cuepool_core::FadeType::Linear,
                                stop_all: false,
                            },
                            CueType::Volume => cuepool_core::Cue::Volume {
                                base,
                                sound_qid: Decimal::ZERO,
                                fade_time: 0.0,
                                volume: 0.0,
                                fade_type: cuepool_core::FadeType::Linear,
                            },
                            CueType::Group => cuepool_core::Cue::Group { base },
                            CueType::Dummy => cuepool_core::Cue::Dummy { base },
                            CueType::TimeCode => cuepool_core::Cue::TimeCode {
                                base,
                                start_time: cuepool_core::Timespan::ZERO,
                                duration: cuepool_core::Timespan::ZERO,
                            },
                            CueType::Osc => cuepool_core::Cue::Osc {
                                base,
                                command: String::new(),
                            },
                            CueType::Text => cuepool_core::Cue::Text {
                                base,
                                text: String::new(),
                                font_size: 48.0,
                                font_colour: cuepool_core::SerializedColour::WHITE,
                                fit: cuepool_core::CanvasFit::Fit,
                                font: String::new(),
                            },
                            CueType::Image => cuepool_core::Cue::Image {
                                base,
                                path: String::new(),
                                fit: cuepool_core::CanvasFit::Fit,
                            },
                            CueType::Goto => cuepool_core::Cue::Goto {
                                base,
                                target_qid: Decimal::ZERO,
                            },
                            CueType::Lighting => cuepool_core::Cue::Lighting {
                                base,
                                snapshot: Default::default(),
                                fade_time: 2.0,
                                fade_type: cuepool_core::FadeType::Linear,
                            },
                            CueType::DmxShow => cuepool_core::Cue::DmxShow {
                                base,
                                path: String::new(),
                                fade_in: 0.0,
                                fade_out: 0.0,
                                fade_type: cuepool_core::FadeType::Linear,
                                priority: 100,
                            },
                            CueType::PixelMap => cuepool_core::Cue::PixelMap {
                                base,
                                path: String::new(),
                            },
                        };
                        state.show_file.cues.insert(insert_at, cue);
                        // Select it: otherwise the operator has to go and find the
                        // cue they just made.
                        state.selected_cue_id = Some(next_qid);
                        state.dirty = true;
                    }
                }
                AppCommand::DeleteSelectedCue => {
                    if let Ok(mut state) = self.state.lock()
                        && let Some(id) = state.selected_cue_id
                        && let Some(idx) =
                            state.show_file.cues.iter().position(|c| c.base().qid == id)
                    {
                        let snapshot = Snapshot::from_state(&state);
                        state.undo_redo.push(snapshot);
                        // A group deletes with its members: they are drawn inside
                        // it, and removing the header alone left them orphaned —
                        // pointing at a cue that no longer exists, indented for
                        // ever, and impossible to fire as a group again.
                        let block = crate::cue_order::span(&state.show_file.cues, idx);
                        state.selected_cue_id = crate::cue_order::selection_after_removal(
                            &state.show_file.cues,
                            block.clone(),
                        );
                        state.show_file.cues.drain(block);
                        state.dirty = true;
                    }
                }
                AppCommand::DuplicateSelectedCue => {
                    if let Ok(mut state) = self.state.lock()
                        && let Some(id) = state.selected_cue_id
                        && let Some(idx) =
                            state.show_file.cues.iter().position(|c| c.base().qid == id)
                    {
                        let snapshot = Snapshot::from_state(&state);
                        state.undo_redo.push(snapshot);

                        // A group copies with its members, each renumbered and
                        // re-pointed at the copied header — a lone header would
                        // look like a group and fire nothing.
                        let block = crate::cue_order::span(&state.show_file.cues, idx);
                        let copies = crate::cue_order::duplicate_span(
                            &state.show_file.cues,
                            block.clone(),
                            |cues, after| {
                                let mut probe = state.show_file.clone();
                                probe.cues = cues.to_vec();
                                probe.choose_qid(Some(after))
                            },
                        );
                        let selected = copies.first().map(|cue| cue.base().qid);
                        // Triggers are copied with everything else, which for a
                        // hotkey or MIDI note means two cues now answer to one
                        // key. Say so rather than silently clearing them: a
                        // trigger the operator wanted and lost is a cue that does
                        // not fire during the show, which is worse and quieter.
                        let shared: Vec<&str> = copies
                            .iter()
                            .flat_map(|cue| {
                                let triggers = &cue.base().triggers;
                                [
                                    triggers.hotkey.is_some().then_some("hotkey"),
                                    triggers.midi.is_some().then_some("MIDI"),
                                    triggers.wall_clock.is_some().then_some("wall clock"),
                                    triggers.timecode.is_some().then_some("timecode"),
                                ]
                            })
                            .flatten()
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .collect();
                        if !shared.is_empty() {
                            log::warn!(
                                "Duplicated Q{id} kept its {} trigger(s); both cues now fire on them",
                                shared.join(", ")
                            );
                            state.report_operator_error(format!(
                                "The copy of Q{id} shares its {} trigger — both cues will fire",
                                shared.join(" and ")
                            ));
                        }
                        state.show_file.cues.splice(block.end..block.end, copies);
                        state.selected_cue_id = selected;
                        state.dirty = true;
                    }
                }
                nudge @ (AppCommand::MoveSelectedCueUp | AppCommand::MoveSelectedCueDown) => {
                    let down = matches!(nudge, AppCommand::MoveSelectedCueDown);
                    if let Ok(mut state) = self.state.lock()
                        && let Some(id) = state.selected_cue_id
                        && let Some(idx) =
                            state.show_file.cues.iter().position(|c| c.base().qid == id)
                    {
                        // Merge key is per cue: nudging cue A then cue B used to
                        // collapse into one undo entry under a shared "move_cue".
                        let snapshot =
                            Snapshot::from_state(&state).with_merge_key(format!("cue:{id}:move"));
                        // A plain swap moved a group header off its members and
                        // stepped into the middle of a neighbouring group; the
                        // helper moves whole blocks and steps over them.
                        if crate::cue_order::nudge(&mut state.show_file.cues, idx, down) {
                            state.undo_redo.push(snapshot);
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::MoveCue {
                    from_idx,
                    to_idx,
                    parent,
                } => {
                    if let Ok(mut state) = self.state.lock()
                        && from_idx < state.show_file.cues.len()
                    {
                        let cues = &mut state.show_file.cues;
                        let block = crate::cue_order::span(cues, from_idx);
                        let dragged = &cues[from_idx];
                        let is_group = matches!(dragged, cuepool_core::Cue::Group { .. });
                        // Drag sets group membership: parent = the group the cue
                        // was dropped into, or None to free it. Groups are always
                        // top-level (no nesting / self-membership).
                        let parent = if is_group || parent == Some(dragged.base().qid) {
                            None
                        } else {
                            parent
                        };
                        // A group must land on a block boundary, or it would be
                        // dropped into the middle of another group's members. Past
                        // the end of the list is already a boundary — that is the
                        // trailing "move to end" strip — so leave it alone.
                        let to_idx = if is_group && to_idx < cues.len() {
                            crate::cue_order::enclosing_span(cues, to_idx).start
                        } else {
                            to_idx
                        };

                        // "No position change" is not "no change". Dropping a cue
                        // on the group header directly above it leaves it exactly
                        // where it is and *only* changes the parent — which the old
                        // `from_idx != to_idx` guard threw away, so the commonest
                        // way to put the first cue in a group did nothing at all.
                        let moves = !(block.start..=block.end).contains(&to_idx);
                        let regroups = dragged.base().parent != parent;
                        if moves || regroups {
                            // No merge key: one drop is one undo step.
                            let snapshot = Snapshot::from_state(&state);
                            state.undo_redo.push(snapshot);
                            let cues = &mut state.show_file.cues;
                            cues[block.start].base_mut().parent = parent;
                            if moves {
                                crate::cue_order::move_span(cues, block, to_idx);
                            }
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::UpdateCueQid { qid, new_qid } => {
                    if let Ok(mut state) = self.state.lock() {
                        // Qid is the cue's identity — refuse duplicates. Say so on
                        // screen as well as in the log: the field reverts to the old
                        // number on its own, which without this reads as the edit
                        // simply not having registered.
                        if state.show_file.cues.iter().any(|c| c.base().qid == new_qid) {
                            log::warn!("Cue {} already exists — keeping {}", new_qid, qid);
                            state.report_operator_error(format!(
                                "Q{new_qid} is already in use — Q{qid} keeps its number"
                            ));
                            continue;
                        }
                        let idx = state
                            .show_file
                            .cues
                            .iter()
                            .position(|c| c.base().qid == qid);
                        if let Some(i) = idx {
                            let snapshot = Snapshot::from_state(&state)
                                .with_merge_key(format!("cue:{}:qid", qid));
                            state.undo_redo.push(snapshot);
                            state.show_file.cues[i].base_mut().qid = new_qid;
                            // Follow the rename everywhere the old qid is referenced.
                            for c in &mut state.show_file.cues {
                                if c.base().parent == Some(qid) {
                                    c.base_mut().parent = Some(new_qid);
                                }
                                match c {
                                    cuepool_core::Cue::Stop { stop_qid, .. }
                                        if *stop_qid == qid =>
                                    {
                                        *stop_qid = new_qid
                                    }
                                    cuepool_core::Cue::Volume { sound_qid, .. }
                                        if *sound_qid == qid =>
                                    {
                                        *sound_qid = new_qid
                                    }
                                    cuepool_core::Cue::Goto { target_qid, .. }
                                        if *target_qid == qid =>
                                    {
                                        *target_qid = new_qid
                                    }
                                    _ => {}
                                }
                            }
                            if state.selected_cue_id == Some(qid) {
                                state.selected_cue_id = Some(new_qid);
                            }
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::UpdateCueName { qid, name } => {
                    if let Ok(mut state) = self.state.lock() {
                        let idx = state
                            .show_file
                            .cues
                            .iter()
                            .position(|c| c.base().qid == qid);
                        if let Some(i) = idx {
                            let snapshot = Snapshot::from_state(&state)
                                .with_merge_key(format!("cue:{}:name", qid));
                            state.undo_redo.push(snapshot);
                            state.show_file.cues[i].base_mut().name = name;
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::UpdateCueTrigger { qid, trigger } => {
                    if let Ok(mut state) = self.state.lock() {
                        let idx = state
                            .show_file
                            .cues
                            .iter()
                            .position(|c| c.base().qid == qid);
                        if let Some(i) = idx {
                            let snapshot = Snapshot::from_state(&state)
                                .with_merge_key(format!("cue:{}:trigger", qid));
                            state.undo_redo.push(snapshot);
                            state.show_file.cues[i].base_mut().trigger = trigger;
                            state.dirty = true;
                        }
                    }
                }
                // Transport and audio-output commands are handled by main.rs.
                other => {
                    unhandled.push(other);
                }
            }
        }

        // Put back commands that main.rs should handle
        if !unhandled.is_empty()
            && let Ok(mut state) = self.state.lock()
        {
            state.command_queue.extend(unhandled);
        }
    }

    /// Save, then quit only if the save landed. A project with no path would
    /// otherwise fall through to `AppCommand::SaveProject`'s native Save As
    /// dialog, which runs inside the egui frame: the exact thing this in-app
    /// modal exists to avoid. Those go to the CuePool data directory instead.
    fn save_and_quit(&self) {
        let existing = self
            .state
            .lock()
            .ok()
            .and_then(|state| state.project_path.clone());
        let path = match existing {
            Some(path) => path,
            None => match unsaved_show_path() {
                Ok(path) => path,
                Err(error) => {
                    log::error!("Save before quit failed: {error}");
                    if let Ok(mut state) = self.state.lock() {
                        state.report_operator_error(format!(
                            "Could not save before quitting: {error}"
                        ));
                    }
                    return;
                }
            },
        };
        if let Err(error) = self.save_to_path(&path) {
            log::error!("Save before quit failed for '{}': {error}", path.display());
            if let Ok(mut state) = self.state.lock() {
                state.report_operator_error(format!("Could not save before quitting: {error}"));
            }
            return;
        }
        log::info!(
            target: crate::logging::PERSIST_TARGET,
            "Saved before quit: {}",
            path.display()
        );
        if let Ok(mut state) = self.state.lock() {
            state.pending_close_confirm = false;
            state.quit = true;
        }
    }

    fn save_to_path(&self, path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        let json = {
            let Ok(state) = self.state.lock() else {
                return Err("failed to lock state".into());
            };
            serde_json::to_string_pretty(&state.show_file)?
        };
        std::fs::write(path, json)?;
        if let Ok(mut state) = self.state.lock() {
            state.project_path = Some(path.to_path_buf());
            state.dirty = false;
            state.push_recent_file(path);
        }
        log::info!("Project saved to {:?}", path);
        Ok(())
    }

    /// Pack project: copy all media into `Media/` folder, rewrite paths, save.
    fn pack_project(&self, folder: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
        std::fs::create_dir_all(folder)?;

        let media_dir = folder.join("Media");
        std::fs::create_dir_all(&media_dir)?;

        let folder_name = folder
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Packed");
        let proj_path = folder.join(format!("{}.qproj", folder_name));

        // Collect file paths and build path mapping
        let mut path_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        {
            let Ok(state) = self.state.lock() else {
                return Err("failed to lock state".into());
            };

            // Gather all referenced file paths from cues
            let mut file_paths: Vec<String> = Vec::new();
            for cue in &state.show_file.cues {
                match cue {
                    cuepool_core::Cue::Sound { path, .. }
                    | cuepool_core::Cue::Video { path, .. }
                    | cuepool_core::Cue::Image { path, .. }
                    | cuepool_core::Cue::PixelMap { path, .. } => {
                        if !path.is_empty() && !file_paths.contains(path) {
                            file_paths.push(path.clone());
                        }
                    }
                    cuepool_core::Cue::Text { font, .. }
                        if !font.is_empty() && !file_paths.contains(font) =>
                    {
                        file_paths.push(font.clone());
                    }
                    _ => {}
                }
            }

            // Build collision map: filename -> list of (original_path, absolute_path)
            let mut by_filename: std::collections::HashMap<
                String,
                Vec<(String, std::path::PathBuf)>,
            > = std::collections::HashMap::new();

            for original in &file_paths {
                let abs = if std::path::Path::new(original).is_absolute() {
                    std::path::PathBuf::from(original)
                } else if let Some(proj) = state.project_path.as_ref() {
                    proj.parent().unwrap_or(folder).join(original)
                } else {
                    folder.join(original)
                };
                let fname = std::path::Path::new(original)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                by_filename
                    .entry(fname)
                    .or_default()
                    .push((original.clone(), abs));
            }

            // Copy files and build path mapping
            for entries in by_filename.values() {
                if entries.len() > 1 {
                    // Name collision: preserve subdir structure by finding common prefix
                    let abs_paths: Vec<_> = entries.iter().map(|(_, abs)| abs.clone()).collect();
                    let common = common_path_prefix(&abs_paths);
                    for (original, abs) in entries {
                        let rel = abs
                            .strip_prefix(&common)
                            .unwrap_or(std::path::Path::new(abs.file_name().unwrap_or_default()));
                        let dst = media_dir.join(rel);
                        if let Some(parent) = dst.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if abs.exists() {
                            std::fs::copy(abs, &dst)?;
                        }
                        let new_rel =
                            pathdiff::diff_paths(&dst, folder).unwrap_or_else(|| dst.clone());
                        path_map.insert(original.clone(), new_rel.to_string_lossy().to_string());
                    }
                } else {
                    // Unique name: copy directly to Media/
                    let (original, abs) = &entries[0];
                    let dst = media_dir.join(abs.file_name().unwrap_or_default());
                    if abs.exists() {
                        std::fs::copy(abs, &dst)?;
                    }
                    let new_rel = pathdiff::diff_paths(&dst, folder).unwrap_or_else(|| dst.clone());
                    path_map.insert(original.clone(), new_rel.to_string_lossy().to_string());
                }
            }
        }

        // Rewrite paths in cues and save
        {
            let Ok(mut state) = self.state.lock() else {
                return Err("failed to lock state".into());
            };

            for cue in &mut state.show_file.cues {
                match cue {
                    cuepool_core::Cue::Sound { path, .. }
                    | cuepool_core::Cue::Video { path, .. }
                    | cuepool_core::Cue::Image { path, .. }
                    | cuepool_core::Cue::PixelMap { path, .. } => {
                        if let Some(new_path) = path_map.get(path) {
                            *path = new_path.clone();
                        }
                    }
                    cuepool_core::Cue::Text { font, .. } => {
                        if let Some(new_path) = path_map.get(font) {
                            *font = new_path.clone();
                        }
                    }
                    _ => {}
                }
            }

            let json = serde_json::to_string_pretty(&state.show_file)?;
            std::fs::write(&proj_path, json)?;
            state.project_path = Some(proj_path.clone());
            state.dirty = false;
            state.push_recent_file(&proj_path);
        }

        log::info!("Project packed to {:?}", proj_path);
        Ok(())
    }
}

/// Find the longest common directory prefix among a set of paths.
fn common_path_prefix(paths: &[std::path::PathBuf]) -> std::path::PathBuf {
    if paths.is_empty() {
        return std::path::PathBuf::new();
    }
    let mut prefix = paths[0].parent().unwrap_or(&paths[0]).to_path_buf();
    for path in &paths[1..] {
        let parent = path.parent().unwrap_or(path);
        while !parent.starts_with(&prefix) {
            if !prefix.pop() {
                break;
            }
        }
    }
    prefix
}

#[cfg(test)]
mod tests {
    use super::*;
    use cuepool_core::CueBase;

    /// A minor release ships refreshed "What's new" copy, or it ships a modal
    /// nobody sees. `RELEASE_NOTES_VERSION` gates the modal against each
    /// operator's stored `last_seen_release_notes`, so leaving it behind means
    /// returning operators never see the notes again while fresh installs read
    /// the *previous* release's copy. This caught it once already: the constant
    /// sat at "0.4" from 0.4.0 through 0.10.2.
    ///
    /// When this fails, rewrite the modal body in `CuePoolApp::update` for the
    /// current release, then bump the constant to match.
    #[test]
    fn release_notes_match_the_release() {
        const PACKAGE_MINOR: &str = concat!(
            env!("CARGO_PKG_VERSION_MAJOR"),
            ".",
            env!("CARGO_PKG_VERSION_MINOR")
        );
        assert_eq!(
            RELEASE_NOTES_VERSION, PACKAGE_MINOR,
            "the \"What's new\" copy still describes {RELEASE_NOTES_VERSION}; \
             rewrite it for {PACKAGE_MINOR}, then bump RELEASE_NOTES_VERSION"
        );
    }

    #[test]
    fn video_badge_separates_acceleration_from_fallback() {
        const HEALTHY: egui::Color32 = egui::Color32::from_rgb(100, 220, 100);
        const DEGRADED: egui::Color32 = egui::Color32::from_rgb(240, 190, 90);
        let diag = |decode_path: &str, fallback: Option<&str>| VideoDiagnostics {
            path: "clip.mov".into(),
            width: 1920,
            height: 1080,
            decode_path: decode_path.into(),
            fallback_reason: fallback.map(str::to_owned),
            timings: VideoTimings::default(),
        };

        let (label, _, tip) = video_status_badge(None);
        assert_eq!(label, "Video: idle");
        assert!(!tip.contains("Fell back"));

        // Every accelerated path reads green — including the two fastest, which
        // a `starts_with("hardware")` check would wrongly report as software.
        for path in [
            "hap gpu-native",
            "d3d12va zero-copy (Radeon Pro W5700)",
            "hardware (videotoolbox)",
        ] {
            let d = diag(path, None);
            assert!(d.accelerated(), "{path} should count as accelerated");
            let (label, colour, _) = video_status_badge(Some(&d));
            assert_eq!(label, format!("Video: {path}"));
            assert_eq!(colour, HEALTHY, "{path} should read healthy");
        }

        // Software decode is flagged even though nothing explicitly "failed".
        let software = diag("software", None);
        assert!(!software.accelerated());
        let (label, colour, tip) = video_status_badge(Some(&software));
        assert_eq!(label, "Video: software ⚠");
        assert_eq!(colour, DEGRADED);
        assert!(tip.contains("Decoding on the CPU"));

        // Still on the GPU, but a faster path was abandoned: degraded, not green.
        let fell_back = diag("d3d11va readback", Some("shareable D3D12VA pool rejected"));
        assert!(fell_back.accelerated());
        let (label, colour, tip) = video_status_badge(Some(&fell_back));
        assert_eq!(label, "Video: d3d11va readback ⚠");
        assert_eq!(colour, DEGRADED);
        assert!(tip.contains("Fell back: shareable D3D12VA pool rejected"));
        assert!(!tip.contains("Decoding on the CPU"));
    }

    #[test]
    fn decode_timing_clones_share_the_latest_sample() {
        let timing = DecodeTiming::default();
        let snapshot = timing.clone();
        timing.set_ms(4.2);

        assert_eq!(snapshot.get_ms(), 4.2);
    }

    #[test]
    fn test_shared_state_default() {
        let state = SharedState::new();
        assert!(state.show_file.cues.is_empty());
        assert_eq!(state.selected_cue_id, None);
    }

    #[test]
    fn unattended_open_refuses_to_discard_dirty_state() {
        let mut app = CuePoolApp::new();
        app.state().lock().unwrap().dirty = true;

        assert_eq!(
            app.open_project_unattended(std::path::Path::new("/missing.qproj")),
            Err("current project has unsaved changes".into())
        );
    }

    #[test]
    fn unattended_open_rejects_non_regular_paths() {
        let mut app = CuePoolApp::new();

        assert_eq!(
            app.open_project_unattended(&std::env::temp_dir()),
            Err("project path must name a .qproj file".into())
        );
    }

    #[test]
    fn cue_selection_reuses_the_validated_app_path() {
        let mut show = ShowFile::default();
        show.cues.push(Cue::Dummy {
            base: CueBase {
                qid: Decimal::new(15, 1),
                ..Default::default()
            },
        });
        let mut app = CuePoolApp::with_show_file(show, None);

        assert!(app.select_cue(Decimal::new(15, 1)).is_ok());
        assert_eq!(
            app.state().lock().unwrap().selected_cue_id,
            Some(Decimal::new(15, 1))
        );
        assert_eq!(
            app.select_cue(Decimal::from(2)),
            Err("cue Q2 not found".into())
        );
    }

    #[test]
    fn unattended_open_rejects_a_file_changed_after_preparation() {
        let path = std::env::temp_dir().join(format!(
            "cuepool-project-{}-{}.qproj",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let show = serde_json::to_string(&ShowFile::default()).unwrap();
        std::fs::write(&path, &show).unwrap();
        let prepared = prepare_unattended_project(&path).unwrap();
        std::fs::write(&path, format!("{show}\n")).unwrap();

        let mut app = CuePoolApp::new();
        assert_eq!(
            app.apply_unattended_project(prepared),
            Err("project changed after validation; submit the command again".into())
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn selection_steps_by_list_position_and_clamps() {
        let cue = |qid| Cue::Dummy {
            base: CueBase {
                qid,
                ..Default::default()
            },
        };
        let cues = vec![
            cue(Decimal::from(9)),
            cue(Decimal::new(15, 1)),
            cue(Decimal::from(4)),
        ];

        assert_eq!(step_selection(&[], None, SelectionStep::Next), None);
        assert_eq!(
            step_selection(&cues, None, SelectionStep::Next),
            Some(Decimal::from(9))
        );
        assert_eq!(
            step_selection(&cues, None, SelectionStep::Previous),
            Some(Decimal::from(4))
        );
        assert_eq!(
            step_selection(&cues, Some(Decimal::from(9)), SelectionStep::Previous),
            Some(Decimal::from(9))
        );
        assert_eq!(
            step_selection(&cues, Some(Decimal::from(4)), SelectionStep::Next),
            Some(Decimal::from(4))
        );
        assert_eq!(
            step_selection(&cues, Some(Decimal::from(9)), SelectionStep::Next),
            Some(Decimal::new(15, 1))
        );
        assert_eq!(
            step_selection(&cues, Some(Decimal::from(4)), SelectionStep::Previous),
            Some(Decimal::new(15, 1))
        );
        assert_eq!(
            step_selection(&cues, Some(Decimal::new(15, 1)), SelectionStep::First),
            Some(Decimal::from(9))
        );
        assert_eq!(
            step_selection(&cues, Some(Decimal::new(15, 1)), SelectionStep::Last),
            Some(Decimal::from(4))
        );
    }

    #[test]
    fn launch_card_runs_for_its_full_duration_from_first_render() {
        let mut app = CuePoolApp::new();

        assert_eq!(app.launch_card_timing(10.0), Some((0.0, LAUNCH_HOLD)));
        assert_eq!(app.launch_card_timing(12.4), None);
        assert!(app.launch_card_started_at.is_none());
    }

    #[test]
    fn a_loaded_project_skips_the_launch_card_but_keeps_about_available() {
        let mut app = CuePoolApp::with_show_file(ShowFile::default(), None);

        assert_eq!(app.launch_card_timing(10.0), None);
        assert!(!app.about_requested());
        app.state().lock().unwrap().show_about_window = true;
        assert!(app.about_requested());
    }

    #[test]
    fn test_generate_large_show_file() {
        let mut show = ShowFile::default();
        for i in 0..500 {
            show.cues.push(Cue::Sound {
                base: CueBase {
                    qid: Decimal::from(i + 1),
                    name: format!("Cue {}", i + 1),
                    ..Default::default()
                },
                path: format!("/audio/cue_{}.wav", i + 1),
                start_time: cuepool_core::Timespan::ZERO,
                duration: cuepool_core::Timespan::from_secs_f64(10.0),
                volume: 0.0,
                pan: 0.0,
                fade_in: 0.0,
                fade_out: 0.0,
                fade_type: cuepool_core::FadeType::Linear,
                eq: None,
                routing: cuepool_core::AudioRouting::default(),
            });
        }
        assert_eq!(show.cues.len(), 500);
    }

    #[test]
    fn selecting_current_audio_driver_is_a_no_op() {
        let current = cuepool_core::AudioOutputDriver::ASIO;
        assert!(audio_driver_command(current, current).is_none());
        assert!(matches!(
            audio_driver_command(current, cuepool_core::AudioOutputDriver::WASAPI),
            Some(AppCommand::SetAudioDriver(
                cuepool_core::AudioOutputDriver::WASAPI
            ))
        ));
    }

    #[test]
    fn diagnostics_include_the_persistent_log_path() {
        let diagnostics = Diagnostics {
            log_file: "C:/CuePool/cuepool.log".into(),
            ..Default::default()
        };

        assert!(
            diagnostics.sections()[0]
                .1
                .contains(&("Log File".into(), "C:/CuePool/cuepool.log".into()))
        );
        assert!(
            diagnostics
                .to_text()
                .contains("Log File: C:/CuePool/cuepool.log")
        );
    }

    #[test]
    fn settings_only_import_applies_audio_without_project_reset() {
        let mut state = SharedState::new();
        state.project_generation = 7;
        let mut source = ShowFile::default();
        source.show_settings.audio_output_driver = cuepool_core::AudioOutputDriver::ASIO;
        source.show_settings.audio_output_device = "Dante Virtual Soundcard (x64)".into();

        apply_project_import(
            &mut state,
            &source,
            cuepool_core::ImportSections {
                show_settings: true,
                ..Default::default()
            },
        );

        assert_eq!(state.project_generation, 7);
        assert!(
            state
                .command_queue
                .iter()
                .any(|command| matches!(command, AppCommand::ApplyAudioSettings))
        );
    }

    #[test]
    fn undo_queues_audio_apply_when_output_settings_change() {
        let mut state = SharedState::new();
        state.show_file.show_settings.audio_output_driver = cuepool_core::AudioOutputDriver::ASIO;
        state.show_file.show_settings.audio_output_device = "ASIO device".into();
        state.undo_redo.push(Snapshot::from_state(&state));
        state.show_file.show_settings.audio_output_driver = cuepool_core::AudioOutputDriver::WASAPI;
        state.show_file.show_settings.audio_output_device = "WASAPI device".into();

        let current = Snapshot::from_state(&state);
        state.undo_redo.undo(current).unwrap().apply(&mut state);

        assert_eq!(
            state.show_file.show_settings.audio_output_driver,
            cuepool_core::AudioOutputDriver::ASIO
        );
        assert!(
            state
                .command_queue
                .iter()
                .any(|command| matches!(command, AppCommand::ApplyAudioSettings))
        );
    }

    #[test]
    fn test_undo_redo() {
        let mut state = SharedState::new();
        state.show_file.cues.push(Cue::Sound {
            base: CueBase {
                qid: Decimal::ONE,
                name: "First".into(),
                ..Default::default()
            },
            path: "/audio/first.wav".into(),
            start_time: cuepool_core::Timespan::ZERO,
            duration: cuepool_core::Timespan::ZERO,
            volume: 0.0,
            pan: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_type: cuepool_core::FadeType::Linear,
            eq: None,
            routing: cuepool_core::AudioRouting::default(),
        });

        // Capture snapshot, then mutate
        let s1 = Snapshot::from_state(&state);
        state.undo_redo.push(s1);
        state.show_file.cues.push(Cue::Sound {
            base: CueBase {
                qid: Decimal::from(2),
                name: "Second".into(),
                ..Default::default()
            },
            path: "/audio/second.wav".into(),
            start_time: cuepool_core::Timespan::ZERO,
            duration: cuepool_core::Timespan::ZERO,
            volume: 0.0,
            pan: 0.0,
            fade_in: 0.0,
            fade_out: 0.0,
            fade_type: cuepool_core::FadeType::Linear,
            eq: None,
            routing: cuepool_core::AudioRouting::default(),
        });
        assert_eq!(state.show_file.cues.len(), 2);

        // Undo
        let current = Snapshot::from_state(&state);
        let prev = state.undo_redo.undo(current).unwrap();
        prev.apply(&mut state);
        assert_eq!(state.show_file.cues.len(), 1);
        assert_eq!(state.show_file.cues[0].base().name, "First");

        // Redo
        let current = Snapshot::from_state(&state);
        let next = state.undo_redo.redo(current).unwrap();
        next.apply(&mut state);
        assert_eq!(state.show_file.cues.len(), 2);
        assert_eq!(state.show_file.cues[1].base().name, "Second");
    }
}
