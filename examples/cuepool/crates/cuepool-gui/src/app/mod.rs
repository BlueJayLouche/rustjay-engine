//! Main application state and eframe integration.

use cuepool_core::{Cue, ShowFile};
use rust_decimal::Decimal;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A full snapshot of editable state for undo/redo.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub show_file: ShowFile,
    pub project_path: Option<PathBuf>,
    pub selected_cue_id: Option<Decimal>,
    pub show_mode: ShowMode,
    pub dirty: bool,
    /// If set, consecutive snapshots with the same key are merged into one.
    pub merge_key: Option<String>,
}

impl Snapshot {
    pub fn from_state(state: &SharedState) -> Self {
        Self {
            show_file: state.show_file.clone(),
            project_path: state.project_path.clone(),
            selected_cue_id: state.selected_cue_id,
            show_mode: state.show_mode,
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
        state.show_mode = self.show_mode;
        state.dirty = self.dirty;
        if audio_changed {
            queue_audio_settings_apply(state);
        }
    }
}

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
    /// If the new snapshot has the same merge_key as the top of the stack,
    /// the top snapshot is replaced instead of pushing a new one.
    pub fn push(&mut self, snapshot: Snapshot) {
        if self.suppress {
            return;
        }
        if let Some(ref key) = snapshot.merge_key {
            if let Some(top) = self.undo_stack.last() {
                if top.merge_key.as_ref() == Some(key) {
                    // Replace top snapshot with new one (merge consecutive edits)
                    *self.undo_stack.last_mut().unwrap() = snapshot;
                    self.redo_stack.clear();
                    return;
                }
            }
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
    pub qid: Decimal,
    pub name: String,
    /// Linear volume (0.0 – 1.0+).
    pub volume: f32,
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

/// The currently-decoding video source, published by the decode thread.
#[derive(Debug, Clone)]
pub struct VideoDiagnostics {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// `hardware (<api>)` or `software`, from `VideoSource::decode_path()`.
    pub decode_path: String,
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
    /// Main event-loop iterations per second. The field diagnostic for a
    /// GPU-stalled winit loop (healthy ≈ 250, the Windows WSI stall showed 10).
    pub event_loop_per_sec: f64,
    /// Set once if the video consume thread exits while the app is still running.
    pub consumer_error: Option<String>,
    pub video: Option<VideoDiagnostics>,
}

impl Diagnostics {
    /// Sectioned key/value rows — the single source for both the on-screen
    /// layout and the clipboard text, so the two can't drift.
    pub fn sections(&self) -> Vec<(&'static str, Vec<(String, String)>)> {
        let mut sections = Vec::new();

        sections.push(("System", vec![
            ("App Version".into(), self.app_version.clone()),
            ("OS".into(), self.os.clone()),
            ("Arch".into(), self.arch.clone()),
        ]));

        sections.push(("GPU", vec![
            ("Name".into(), self.gpu_name.clone()),
            ("Backend".into(), self.gpu_backend.clone()),
            ("Driver".into(), self.gpu_driver.clone()),
            ("Driver Info".into(), self.gpu_driver_info.clone()),
        ]));

        let mut outputs = Vec::new();
        if self.outputs.is_empty() {
            outputs.push(("Outputs".into(), "none open".into()));
        }
        for (i, out) in self.outputs.iter().enumerate() {
            let p = format!("Output {}", i + 1);
            outputs.push((format!("{p} Name"), out.name.clone()));
            outputs.push((format!("{p} Size"), format!("{}x{}", out.size.0, out.size.1)));
            outputs.push((format!("{p} Present Mode"), out.present_mode.clone()));
            outputs.push((format!("{p} Surface Format"), out.format.clone()));
            outputs.push((format!("{p} Monitor Refresh"), out.refresh.clone()));
            outputs.push((format!("{p} Fullscreen"), if out.fullscreen { "yes" } else { "no" }.into()));
            outputs.push((format!("{p} Presented/s"), format!("{:.0}", out.presented_per_sec)));
        }
        sections.push(("Outputs", outputs));

        let video = match &self.video {
            Some(v) => vec![
                ("File".into(), v.path.clone()),
                ("Source Size".into(), format!("{}x{}", v.width, v.height)),
                ("Decode Path".into(), v.decode_path.clone()),
            ],
            None => vec![("Status".into(), "no video playing".into())],
        };
        sections.push(("Video Decode", video));

        sections.push(("Render Loop", vec![
            ("Output Count".into(), self.outputs.len().to_string()),
            ("Event Loop/s".into(), format!("{:.0}", self.event_loop_per_sec)),
            ("Presented/s (all outputs)".into(), format!("{:.0}", self.presented_per_sec)),
            ("Starved/s".into(), format!("{:.0}", self.starved_per_sec)),
            ("Video Consumer".into(), self.consumer_error.clone().unwrap_or_else(|| "running".into())),
        ]));

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
    /// Whether the project settings window is open.
    pub show_settings_window: bool,
    /// Current audio output device name.
    pub audio_device_name: String,
    /// Why audio playback is disabled, if output configuration failed.
    pub audio_error: Option<String>,
    /// Cached waveform peaks: path → Vec<(min, max)>.
    pub waveform_cache: std::collections::HashMap<String, Vec<(f32, f32)>>,
    /// Paths currently being processed for waveform generation.
    pub pending_waveforms: std::collections::HashSet<String>,
    /// Waveform zoom level (1.0 = fit to width, >1.0 = zoomed in).
    pub waveform_zoom: f32,
    /// Waveform scroll offset in bars.
    pub waveform_scroll: f32,
    /// Available audio output device names (populated at startup).
    pub audio_devices: Vec<String>,
    /// Whether the log window is open.
    pub show_log_window: bool,
    /// Whether the About window is open.
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
    /// Next armed timecode trigger: (cue qid, trigger seconds).
    pub next_timecode: Option<(Decimal, f64)>,
    /// Latest pixel-map sample per segment: id → (cols, rows, RGBA bytes).
    /// Published by the control binary; painted by the lighting panel preview.
    pub lighting_preview: std::collections::HashMap<u32, (u32, u32, Vec<u8>)>,
    /// Set on window-close with unsaved changes / running cues — shows the in-app
    /// quit-confirm modal (a native modal deadlocks the loop).
    pub pending_close_confirm: bool,
    /// Set by the quit-confirm modal; main.rs hard-exits on the next tick.
    pub quit: bool,
    /// Progress overlay: if Some, shows a blocking modal with message + progress.
    pub progress_overlay: Option<ProgressOverlay>,
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
            next_timecode: None,
            recorder_status: RecorderStatus::default(),
            lighting_preview: std::collections::HashMap::new(),
            pending_close_confirm: false,
            quit: false,
            progress_overlay: None,
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

    /// Add a path to the recent files list, moving it to the front if it already exists.
    pub fn push_recent_file(&mut self, path: &std::path::Path) {
        let path_buf = path.to_path_buf();
        self.recent_files.retain(|p| p != &path_buf);
        self.recent_files.insert(0, path_buf);
        self.recent_files.truncate(10);
    }

    pub fn load_show_file(&mut self, path: &std::path::Path, data: &str) -> Result<(), serde_json::Error> {
        let show: ShowFile = serde_json::from_str(data)?;
        self.show_file = show;
        self.project_path = Some(path.to_path_buf());
        self.dirty = false;
        // Signal the control binary to rebuild output windows for the new projection.
        self.project_generation = self.project_generation.wrapping_add(1);
        Ok(())
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
    OpenProject { path: PathBuf },
    SaveProject,
    SaveProjectAs { path: PathBuf },
    PackProject { path: PathBuf },
    Go,
    Stop,
    Pause,
    SelectCue(Decimal),
    Undo,
    Redo,
    AddCue { cue_type: CueType },
    DeleteSelectedCue,
    DuplicateSelectedCue,
    MoveSelectedCueUp,
    MoveSelectedCueDown,
    MoveCue { from_idx: usize, to_idx: usize, parent: Option<Decimal> },
    SetLimiterThreshold(f32),
    SetAudioDriver(cuepool_core::AudioOutputDriver),
    SetAudioDevice(String),
    ApplyAudioSettings,
    ToggleVideoWindow,
    ToggleVideoFullscreen,
    ToggleProjectionWindow,
    OpenProjectionOutputs,
    Preload,
    UpdateCueQid { qid: Decimal, new_qid: Decimal },
    UpdateCueName { qid: Decimal, name: String },
    UpdateCueTrigger { qid: Decimal, trigger: cuepool_core::TriggerMode },
    LearnMidiTrigger { qid: Decimal },
    CaptureTimecodeTrigger { qid: Decimal },
    /// Snap the lighting engine's live state to these looks (inspector live mode).
    LightingLivePush {
        snapshot: std::collections::BTreeMap<u32, cuepool_core::FixtureLook>,
    },
    /// Start a recording pass on `file`, or stop-and-keep the running one.
    RecorderRecord { file: String },
    /// Stop and throw away the in-flight pass.
    RecorderDiscard,
    /// Swap `file` with its `.prev` (undo the last kept pass).
    RecorderRevert { file: String },
    RecorderSetMonitor(bool),
    /// Preview the take through the lighting output (no cue involved).
    RecorderPreview { file: String },
    RecorderStopPreview,
    /// Release every channel the OSC/MIDI live bridge holds.
    RecorderClearLive,
    /// Take-editor scrub: hold this frame on the lighting output (None = release).
    RecorderScrub { frame: Option<rustjay_lighting::MaskedFrame> },
    /// Step one video frame forward while paused (show clock follows).
    FrameStep,
    /// Step one video frame back while paused (show clock follows).
    FrameStepBack,
}

fn audio_driver_command(
    current: cuepool_core::AudioOutputDriver,
    selected: cuepool_core::AudioOutputDriver,
) -> Option<AppCommand> {
    (current != selected).then_some(AppCommand::SetAudioDriver(selected))
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
    status_copied_at: Option<std::time::Instant>,
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
        }
    }

    pub fn with_show_file(show: ShowFile, path: Option<PathBuf>) -> Self {
        Self {
            state: Arc::new(Mutex::new(SharedState {
                show_file: show,
                project_path: path,
                ..SharedState::default()
            })),
            take_editor: Default::default(),
            status_copied_at: None,
        }
    }

    pub fn state(&self) -> &SharedStateHandle {
        &self.state
    }
}

impl CuePoolApp {
    pub fn update(&mut self, ui: &mut egui::Ui) {
        // Panels lay out in the root `ui`; windows/areas/input still go through
        // the context.
        let ctx = &ui.ctx().clone();
        // Keyboard shortcuts. Skip the bare Delete/Backspace cue-delete while a
        // text field is focused so editing isn't hijacked.
        let editing_text = ctx.egui_wants_keyboard_input();
        ctx.input(|i| {
            let modifiers = i.modifiers;

            // Undo / Redo
            if modifiers.command && i.key_pressed(egui::Key::Z) {
                let cmd = if modifiers.shift { AppCommand::Redo } else { AppCommand::Undo };
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(cmd);
                }
            }

            // New / Open / Save
            if modifiers.command && i.key_pressed(egui::Key::N) {
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(AppCommand::NewProject);
                }
            }
            if modifiers.command && i.key_pressed(egui::Key::O) {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("CuePool project", &["qproj"])
                    .pick_file()
                {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::OpenProject { path });
                    }
                }
            }
            if modifiers.command && i.key_pressed(egui::Key::S) {
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(AppCommand::SaveProject);
                }
            }

            // Delete selected cue (Delete, or Backspace which is the Mac "delete").
            if !editing_text
                && (i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
            {
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(AppCommand::DeleteSelectedCue);
                }
            }

            // Duplicate selected cue
            if modifiers.command && i.key_pressed(egui::Key::D) {
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(AppCommand::DuplicateSelectedCue);
                }
            }

            // Add new sound cue
            if modifiers.command && i.key_pressed(egui::Key::T) {
                if let Ok(mut state) = self.state.lock() {
                    state.command_queue.push(AppCommand::AddCue { cue_type: CueType::Sound });
                }
            }

            // Move selected cue up/down
            if modifiers.command {
                if i.key_pressed(egui::Key::ArrowUp) {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::MoveSelectedCueUp);
                    }
                }
                if i.key_pressed(egui::Key::ArrowDown) {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::MoveSelectedCueDown);
                    }
                }
            }

            // Go / Stop / Pause (transport shortcuts)
            if !modifiers.command && !modifiers.alt {
                if i.key_pressed(egui::Key::Space) {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::Go);
                    }
                }
                if i.key_pressed(egui::Key::Escape) {
                    if let Ok(mut state) = self.state.lock() {
                        state.command_queue.push(AppCommand::Stop);
                    }
                }
            }
        });

        // Top menu bar
        egui::Panel::top("menu_bar").show_inside(ui, |ui| {
            self.menu_bar(ui);
        });

        // Transport controls
        egui::Panel::top("transport").show_inside(ui, |ui| {
            crate::transport::show(ui, &self.state);
        });

        // Status bar
        egui::Panel::bottom("status_bar").show_inside(ui, |ui| {
            self.status_bar(ui);
        });

        // Active cues panel (left side)
        egui::Panel::left("active_cues")
            .default_size(220.0)
            .show_inside(ui, |ui| {
                crate::active_cues::show(ui, &self.state);
            });

        // Cue inspector (right side)
        egui::Panel::right("inspector")
            .default_size(280.0)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    crate::inspector::show(ui, &self.state);
                });
            });

        // Main cue list (central: fills what the panels above left over).
        egui::CentralPanel::default().show_inside(ui, |ui| {
            crate::cue_list::show(ui, &self.state);
        });

        // Progress overlay
        let overlay = {
            let Ok(state) = self.state.lock() else { return; };
            state.progress_overlay.clone()
        };
        if let Some(overlay) = overlay {
            egui::Area::new(egui::Id::new("progress_overlay"))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    let screen_rect = ctx.content_rect();
                    ui.painter().rect_filled(screen_rect, 0.0, egui::Color32::from_rgba_premultiplied(0, 0, 0, 180));

                    let modal_size = egui::vec2(320.0, 120.0);
                    let modal_rect = egui::Rect::from_center_size(screen_rect.center(), modal_size);
                    ui.painter().rect_filled(modal_rect, 8.0, ui.visuals().panel_fill);
                    ui.painter().rect_stroke(modal_rect, 8.0, egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);

                    ui.scope_builder(egui::UiBuilder::new().max_rect(modal_rect.shrink(16.0)), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.heading("Please Wait");
                            ui.add_space(8.0);
                            ui.label(&overlay.message);
                            ui.add_space(8.0);
                            let progress = overlay.progress.clamp(0.0, 1.0);
                            ui.add(egui::ProgressBar::new(progress).show_percentage());
                        });
                    });
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
                                        for name in &devices {
                                            if ui.selectable_label(name == &current_device, name).clicked() {
                                                audio_device_cmd = Some(AppCommand::SetAudioDevice(name.clone()));
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
                        });
                        ui.separator();

                        egui::CollapsingHeader::new("OSC / Remote").default_open(false).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label("NIC:");
                                settings_changed |= ui.text_edit_singleline(&mut settings.osc_nic).changed();
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
                                let is_active = node.last_seen.map(|t| now.duration_since(t).as_secs_f64() < 5.0).unwrap_or(false);
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
                let path = state.selected_cue().and_then(|cue| match cue {
                    cuepool_core::Cue::Sound { path, .. } | cuepool_core::Cue::Video { path, .. } => Some(path.clone()),
                    _ => None,
                }).unwrap_or_default();
                let peaks = state.waveform_cache.get(&path).cloned();
                (path, peaks, state.waveform_window_zoom, state.waveform_window_scroll)
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
                    } else if let Some(peaks) = peaks {
                        ui.label(format!("{}", std::path::Path::new(&selected_path).file_name().and_then(|n| n.to_str()).unwrap_or(&selected_path)));
                        let (new_zoom, new_scroll) = crate::waveform::draw(ui, &peaks, zoom, scroll, 200.0);
                        if let Ok(mut state) = self.state.lock() {
                            state.waveform_window_zoom = new_zoom;
                            state.waveform_window_scroll = new_scroll;
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
        let pending_close = self.state.lock().map(|s| s.pending_close_confirm).unwrap_or(false);
        if pending_close {
            egui::Modal::new(egui::Id::new("quit_confirm")).show(ctx, |ui| {
                ui.set_width(320.0);
                ui.heading("Quit CuePool?");
                ui.add_space(4.0);
                ui.label("You have unsaved changes or running cues. Discard and quit?");
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard & Quit").clicked() {
                        if let Ok(mut s) = self.state.lock() {
                            s.pending_close_confirm = false;
                            s.quit = true;
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        if let Ok(mut s) = self.state.lock() {
                            s.pending_close_confirm = false;
                        }
                    }
                });
            });
        }

        // Status window (live diagnostics for bug reports)
        let mut show_status = if let Ok(state) = self.state.lock() {
            state.show_status_window
        } else {
            false
        };
        if show_status {
            egui::Window::new("Status")
                .collapsible(false)
                .resizable(true)
                .default_size([460.0, 600.0])
                .open(&mut show_status)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        crate::status_panel::show(ui, &self.state, &mut self.status_copied_at);
                    });
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_status_window = show_status;
        }

        // About window
        let mut show_about = if let Ok(state) = self.state.lock() {
            state.show_about_window
        } else {
            false
        };
        if show_about {
            egui::Window::new("About CuePool")
                .collapsible(false)
                .resizable(false)
                .default_size([320.0, 180.0])
                .open(&mut show_about)
                .show(ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.heading("CuePool");
                        ui.label("A professional audio/video playback application");
                        ui.separator();
                        ui.label("Version: 0.2.0");
                        ui.hyperlink_to("GitHub", "https://github.com/BlueJayLouche/CuePool");
                        ui.label("License: GPL-3.0");
                    });
                });
        }
        if let Ok(mut state) = self.state.lock() {
            state.show_about_window = show_about;
        }

        // Import-from-project modal (File → Import from Project…)
        let import = self.state.lock().ok().and_then(|s| s.import_request.clone());
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
                        if ui.add_enabled(any_checked, egui::Button::new("Import")).clicked() {
                            if let Ok(mut state) = self.state.lock() {
                                apply_project_import(&mut state, &request.show, request.sections);
                                state.import_request = None;
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            if let Ok(mut state) = self.state.lock() {
                                state.import_request = None;
                            }
                        }
                    });
                });
            // Persist checkbox edits unless Import/Cancel cleared the request.
            if let Ok(mut state) = self.state.lock() {
                if state.import_request.is_some() {
                    state.import_request = Some(request);
                }
            }
        }

        // Process any commands queued during the frame
        self.process_commands(ctx);
    }
}

impl CuePoolApp {
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
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
                    {
                        if let Ok(mut state) = self.state.lock() {
                            state.command_queue.push(AppCommand::OpenProject { path });
                        }
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
                    {
                        if let Ok(mut state) = self.state.lock() {
                            state.command_queue.push(AppCommand::SaveProjectAs { path });
                        }
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
        let (active_count, cue_count, show_mode, dirty) = {
            let Ok(state) = self.state.lock() else { return; };
            (
                state.active_cues.len(),
                state.show_file.cues.len(),
                state.show_mode,
                state.dirty,
            )
        };

        ui.horizontal(|ui| {
            // Status text
            let status = if active_count > 0 {
                format!("▶ Playing {} cue{}", active_count, if active_count == 1 { "" } else { "s" })
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
                ui.label(egui::RichText::new("● Unsaved changes").small().color(egui::Color32::YELLOW));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Audio active indicator
                let audio_color = if active_count > 0 {
                    egui::Color32::from_rgb(100, 220, 100)
                } else {
                    egui::Color32::from_rgb(120, 120, 120)
                };
                let audio_text = if active_count > 0 { "Audio: On" } else { "Audio: Off" };
                ui.label(egui::RichText::new(audio_text).small().color(audio_color));
            });
        });
    }

    fn confirm_discard(state: &SharedStateHandle) -> bool {
        let (dirty, has_running) = {
            let Ok(state) = state.lock() else { return false };
            (state.dirty, !state.active_cues.is_empty())
        };
        if has_running {
            let choice = rfd::MessageDialog::new()
                .set_title("Running Cues")
                .set_description("There are cues currently playing. Stop them and proceed?")
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show();
            if !matches!(choice, rfd::MessageDialogResult::Ok) {
                return false;
            }
        }
        if dirty {
            let choice = rfd::MessageDialog::new()
                .set_title("Unsaved Changes")
                .set_description("You have unsaved changes. Discard them?")
                .set_buttons(rfd::MessageButtons::OkCancel)
                .show();
            if !matches!(choice, rfd::MessageDialogResult::Ok) {
                return false;
            }
        }
        true
    }

    fn process_commands(&mut self, _ctx: &egui::Context) {
        let commands = {
            let Ok(mut state) = self.state.lock() else { return };
            let cmds: Vec<_> = state.command_queue.drain(..).collect();
            cmds
        };

        let mut unhandled = Vec::new();
        for cmd in commands {
            match cmd {
                AppCommand::NewProject => {
                    if !Self::confirm_discard(&self.state) {
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
                    if !Self::confirm_discard(&self.state) {
                        continue;
                    }
                    log::info!("Open project: {:?}", path);
                    match std::fs::read_to_string(&path) {
                        Ok(data) => {
                            if let Ok(mut state) = self.state.lock() {
                                let snapshot = Snapshot::from_state(&state);
                                state.undo_redo.push(snapshot);
                                if let Err(e) = state.load_show_file(&path, &data) {
                                    log::error!("Failed to parse show file: {}", e);
                                } else {
                                    state.push_recent_file(&path);
                                }
                            }
                        }
                        Err(e) => {
                            log::error!("Failed to read file: {}", e);
                        }
                    }
                }
                AppCommand::SaveProject => {
                    let path = {
                        let Ok(state) = self.state.lock() else { continue };
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
                        {
                            if let Err(e) = self.save_to_path(&path) {
                                log::error!("Failed to save project: {}", e);
                            }
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
                    if let Ok(mut state) = self.state.lock() {
                        // Capture snapshot before switching cues so inspector edits are undoable
                        let snapshot = Snapshot::from_state(&state);
                        state.undo_redo.push(snapshot);
                        state.selected_cue_id = Some(id);
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

                        let mut base = cuepool_core::CueBase {
                            qid: next_qid,
                            name: format!("New {:?} Cue", cue_type),
                            ..Default::default()
                        };
                        // Show clock running (or paused mid-step): pre-fill the
                        // timecode trigger with the current time, so "pause at
                        // the moment, add cue" lands armed at the right spot.
                        if let Some(t) = state.show_time {
                            base.triggers.timecode =
                                Some(cuepool_core::TimecodeTrigger { time: cuepool_core::Timespan::from_secs_f64(t) });
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
                        state.show_file.cues.push(cue);
                        state.dirty = true;
                    }
                }
                AppCommand::DeleteSelectedCue => {
                    if let Ok(mut state) = self.state.lock() {
                        if let Some(id) = state.selected_cue_id {
                            let snapshot = Snapshot::from_state(&state);
                            state.undo_redo.push(snapshot);
                            state.show_file.cues.retain(|c| c.base().qid != id);
                            state.selected_cue_id = None;
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::DuplicateSelectedCue => {
                    if let Ok(mut state) = self.state.lock() {
                        if let Some(cue) = state.selected_cue().cloned() {
                            let snapshot = Snapshot::from_state(&state);
                            state.undo_redo.push(snapshot);

                            let mut new_cue = cue;
                            let original_qid = new_cue.base().qid;
                            let next_qid = state.show_file.choose_qid(Some(original_qid));
                            new_cue.base_mut().qid = next_qid;
                            new_cue.base_mut().name.push_str(" (copy)");
                            state.show_file.cues.push(new_cue);
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::MoveSelectedCueUp => {
                    if let Ok(mut state) = self.state.lock() {
                        if let Some(id) = state.selected_cue_id {
                            let idx = state.show_file.cues.iter().position(|c| c.base().qid == id);
                            if let Some(i) = idx {
                                if i > 0 {
                                    let snapshot = Snapshot::from_state(&state)
                                        .with_merge_key("move_cue");
                                    state.undo_redo.push(snapshot);
                                    state.show_file.cues.swap(i, i - 1);
                                    state.dirty = true;
                                }
                            }
                        }
                    }
                }
                AppCommand::MoveSelectedCueDown => {
                    if let Ok(mut state) = self.state.lock() {
                        if let Some(id) = state.selected_cue_id {
                            let len = state.show_file.cues.len();
                            let idx = state.show_file.cues.iter().position(|c| c.base().qid == id);
                            if let Some(i) = idx {
                                if i + 1 < len {
                                    let snapshot = Snapshot::from_state(&state)
                                        .with_merge_key("move_cue");
                                    state.undo_redo.push(snapshot);
                                    state.show_file.cues.swap(i, i + 1);
                                    state.dirty = true;
                                }
                            }
                        }
                    }
                }
                AppCommand::MoveCue { from_idx, to_idx, parent } => {
                    if let Ok(mut state) = self.state.lock() {
                        let len = state.show_file.cues.len();
                        if from_idx < len && from_idx != to_idx {
                            let snapshot = Snapshot::from_state(&state)
                                .with_merge_key("move_cue");
                            state.undo_redo.push(snapshot);
                            let mut cue = state.show_file.cues.remove(from_idx);
                            // Drag sets group membership: parent = the group the cue
                            // was dropped into, or None to free it. Groups are always
                            // top-level (no nesting / self-membership).
                            let parent = if matches!(cue, cuepool_core::Cue::Group { .. })
                                || parent == Some(cue.base().qid)
                            {
                                None
                            } else {
                                parent
                            };
                            cue.base_mut().parent = parent;
                            // Removing shifts everything after from_idx left by one.
                            let insert_idx = if to_idx > from_idx { to_idx - 1 } else { to_idx };
                            let insert_idx = insert_idx.min(state.show_file.cues.len());
                            state.show_file.cues.insert(insert_idx, cue);
                            state.dirty = true;
                        }
                    }
                }
                AppCommand::UpdateCueQid { qid, new_qid } => {
                    if let Ok(mut state) = self.state.lock() {
                        // Qid is the cue's identity — refuse duplicates.
                        if state.show_file.cues.iter().any(|c| c.base().qid == new_qid) {
                            log::warn!("Cue {} already exists — keeping {}", new_qid, qid);
                            continue;
                        }
                        let idx = state.show_file.cues.iter().position(|c| c.base().qid == qid);
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
                                    cuepool_core::Cue::Stop { stop_qid, .. } if *stop_qid == qid => *stop_qid = new_qid,
                                    cuepool_core::Cue::Volume { sound_qid, .. } if *sound_qid == qid => *sound_qid = new_qid,
                                    cuepool_core::Cue::Goto { target_qid, .. } if *target_qid == qid => *target_qid = new_qid,
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
                        let idx = state.show_file.cues.iter().position(|c| c.base().qid == qid);
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
                        let idx = state.show_file.cues.iter().position(|c| c.base().qid == qid);
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
        if !unhandled.is_empty() {
            if let Ok(mut state) = self.state.lock() {
                state.command_queue.extend(unhandled);
            }
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

        let folder_name = folder.file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Packed");
        let proj_path = folder.join(format!("{}.qproj", folder_name));

        // Collect file paths and build path mapping
        let mut path_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();

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
                    cuepool_core::Cue::Text { font, .. } => {
                        if !font.is_empty() && !file_paths.contains(font) {
                            file_paths.push(font.clone());
                        }
                    }
                    _ => {}
                }
            }

            // Build collision map: filename -> list of (original_path, absolute_path)
            let mut by_filename: std::collections::HashMap<String, Vec<(String, std::path::PathBuf)>> =
                std::collections::HashMap::new();

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
                by_filename.entry(fname).or_default().push((original.clone(), abs));
            }

            // Copy files and build path mapping
            for (_fname, entries) in &by_filename {
                if entries.len() > 1 {
                    // Name collision: preserve subdir structure by finding common prefix
                    let abs_paths: Vec<_> = entries.iter().map(|(_, abs)| abs.clone()).collect();
                    let common = common_path_prefix(&abs_paths);
                    for (original, abs) in entries {
                        let rel = abs.strip_prefix(&common).unwrap_or(std::path::Path::new(abs.file_name().unwrap_or_default()));
                        let dst = media_dir.join(rel);
                        if let Some(parent) = dst.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if abs.exists() {
                            std::fs::copy(abs, &dst)?;
                        }
                        let new_rel = pathdiff::diff_paths(&dst, folder)
                            .unwrap_or_else(|| dst.clone());
                        path_map.insert(original.clone(), new_rel.to_string_lossy().to_string());
                    }
                } else {
                    // Unique name: copy directly to Media/
                    let (original, abs) = &entries[0];
                    let dst = media_dir.join(abs.file_name().unwrap_or_default());
                    if abs.exists() {
                        std::fs::copy(abs, &dst)?;
                    }
                    let new_rel = pathdiff::diff_paths(&dst, folder)
                        .unwrap_or_else(|| dst.clone());
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

    #[test]
    fn test_shared_state_default() {
        let state = SharedState::new();
        assert!(state.show_file.cues.is_empty());
        assert_eq!(state.selected_cue_id, None);
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
        assert!(state
            .command_queue
            .iter()
            .any(|command| matches!(command, AppCommand::ApplyAudioSettings)));
    }

    #[test]
    fn undo_queues_audio_apply_when_output_settings_change() {
        let mut state = SharedState::new();
        state.show_file.show_settings.audio_output_driver = cuepool_core::AudioOutputDriver::ASIO;
        state.show_file.show_settings.audio_output_device = "ASIO device".into();
        state.undo_redo.push(Snapshot::from_state(&state));
        state.show_file.show_settings.audio_output_driver =
            cuepool_core::AudioOutputDriver::WASAPI;
        state.show_file.show_settings.audio_output_device = "WASAPI device".into();

        let current = Snapshot::from_state(&state);
        state.undo_redo.undo(current).unwrap().apply(&mut state);

        assert_eq!(
            state.show_file.show_settings.audio_output_driver,
            cuepool_core::AudioOutputDriver::ASIO
        );
        assert!(state
            .command_queue
            .iter()
            .any(|command| matches!(command, AppCommand::ApplyAudioSettings)));
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
