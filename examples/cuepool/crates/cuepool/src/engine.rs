use cuepool_audio::{AudioEngine, CueChainParams, FileDecoder, SampleProvider};
use cuepool_core::{
    Cue, CueBase, LockExt, LoopMode, ShowFile, ShowSettings, StopMode, Timespan, TriggerMode,
};
use cuepool_gui::SharedStateHandle;
use cuepool_gui::app::CueState;
use rust_decimal::Decimal;
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum EngineCommand {
    Go,
    Fire(Decimal),
    Stop,
    Pause,
    Resume,
    TogglePause,
    Select(Decimal),
    Preload,
    Seek {
        instance_id: u64,
        secs: f32,
    },
    /// Live level edit from the inspector: apply to a playing cue without
    /// re-firing it.
    SetLevel {
        qid: Decimal,
        volume: f32,
        pan: f32,
    },
}

#[derive(Debug, Clone)]
pub enum EngineEvent {
    VideoEof { instance_id: u64, epoch: u64 },
    VideoFailed { instance_id: u64, epoch: u64 },
    ExternalFinished { qid: Decimal },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EngineTrace {
    CueStarted {
        qid: Decimal,
        instance_id: Option<u64>,
    },
    CueFinished {
        qid: Decimal,
    },
}

#[derive(Debug, Clone)]
pub enum EngineAction {
    PlayVideo {
        qid: Decimal,
        instance_id: u64,
        epoch: u64,
        clock_origin: Duration,
        path: String,
        start_time: Timespan,
        duration: Timespan,
        follow_mtc: bool,
        mtc_start: Timespan,
    },
    SeekVideo {
        qid: Decimal,
        instance_id: u64,
        path: String,
        target_secs: f64,
        media_offset_secs: f64,
        paused: bool,
    },
    StopVideo {
        fade_out_secs: f32,
    },
    SetVideoPaused(bool),
    FireExternal(Box<Cue>),
    StopExternal {
        qid: Decimal,
        mode: StopMode,
        fade_out_secs: f32,
        fade_type: cuepool_core::FadeType,
    },
    StopAllExternal,
    RemoteGo {
        node: String,
        qid: Decimal,
    },
    Trace(EngineTrace),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveCueSnapshot {
    pub instance_id: u64,
    pub qid: Decimal,
    pub name: String,
    pub state: CueState,
    pub position_secs: f64,
    pub length_secs: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoSnapshot {
    pub qid: Decimal,
    pub instance_id: u64,
    pub epoch: u64,
    pub path: String,
    pub paused: bool,
    pub position_secs: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineSnapshot {
    pub standby_qid: Option<Decimal>,
    pub paused: bool,
    pub show_elapsed_secs: Option<f64>,
    pub active_cues: Vec<ActiveCueSnapshot>,
    pub video: Option<VideoSnapshot>,
}

#[derive(Clone)]
struct ActiveCue {
    instance_id: u64,
    qid: Decimal,
    name: String,
    input: Arc<cuepool_audio::MixerInput>,
    state: CueState,
    loop_counter: Option<Arc<AtomicU32>>,
    video_loop_count: u32,
    video_loop_limit: Option<u32>,
    loop_start_frame: u64,
    loop_end_frame: u64,
    fade_out: f32,
    fade_type: cuepool_core::FadeType,
    fade_out_started: bool,
    pending_stop: Option<PendingStop>,
    /// Cleared by a faded Stop All: the cues fade out rather than being
    /// cleared, but nothing may chain off them finishing — a stop-everything
    /// that then starts cues again is not a stop.
    follow_after_last: bool,
}

#[derive(Clone, Copy)]
struct PendingStop {
    mode: StopMode,
    fade_out_time: f32,
    fade_type: cuepool_core::FadeType,
}

struct DelayedCue {
    cue: Cue,
    start_at: Duration,
}

#[derive(Clone)]
struct CurrentVideo {
    qid: Decimal,
    instance_id: u64,
    epoch: u64,
    path: String,
    start_time: Timespan,
    duration: Timespan,
    loop_mode: LoopMode,
    follow_mtc: bool,
    mtc_start: Timespan,
    has_audio: bool,
    clock_origin: Duration,
    paused_position: Option<f64>,
}

impl CurrentVideo {
    /// True when the picture reaches an end of its own accord, so an AfterLast
    /// follow must wait for it as well as for the audio track.
    ///
    /// Looping and hold-last pictures never end on their own and an
    /// MTC-followed picture is driven externally — for those the audio stays
    /// the sole authority, or the follow would wait forever.
    fn picture_ends_on_its_own(&self) -> bool {
        !self.follow_mtc && matches!(self.loop_mode, LoopMode::OneShot)
    }
}

pub struct ShowEngine {
    state: SharedStateHandle,
    audio_engine: Option<AudioEngine>,
    active_cues: Vec<ActiveCue>,
    next_active_cue_instance_id: u64,
    delayed_cues: Vec<DelayedCue>,
    paused: bool,
    now: Duration,
    show_start: Option<Duration>,
    show_pause_started: Option<Duration>,
    show_paused_offset: Duration,
    show_adjustment_secs: f64,
    triggered_timecodes: HashSet<Decimal>,
    active_timecodes: Vec<(Decimal, Duration)>,
    current_video: Option<CurrentVideo>,
    /// A one-shot video cue with an audio track ends when BOTH streams do:
    /// picture and audio lengths routinely differ inside one container. The
    /// half that finishes first is recorded here as (qid, instance_id); the
    /// second one fires the AfterLast follow.
    pending_video_half: Option<(Decimal, u64)>,
    current_picture_qid: Option<Decimal>,
    actions: VecDeque<EngineAction>,
}

impl ShowEngine {
    pub fn from_show_file(
        show_file: ShowFile,
        project_path: Option<PathBuf>,
        audio_engine: Option<AudioEngine>,
    ) -> Self {
        let app = cuepool_gui::CuePoolApp::with_show_file(show_file, project_path);
        Self::new(app.state().clone(), audio_engine)
    }

    pub fn new(state: SharedStateHandle, audio_engine: Option<AudioEngine>) -> Self {
        Self {
            state,
            audio_engine,
            active_cues: Vec::new(),
            next_active_cue_instance_id: 1,
            delayed_cues: Vec::new(),
            paused: false,
            now: Duration::ZERO,
            show_start: None,
            show_pause_started: None,
            show_paused_offset: Duration::ZERO,
            show_adjustment_secs: 0.0,
            triggered_timecodes: HashSet::new(),
            active_timecodes: Vec::new(),
            current_video: None,
            pending_video_half: None,
            current_picture_qid: None,
            actions: VecDeque::new(),
        }
    }

    pub fn state(&self) -> &SharedStateHandle {
        &self.state
    }

    pub fn audio_engine(&self) -> Option<&AudioEngine> {
        self.audio_engine.as_ref()
    }

    pub fn replace_audio_engine(&mut self, audio_engine: Option<AudioEngine>) {
        self.audio_engine = audio_engine;
    }

    pub fn command(&mut self, command: EngineCommand, now: Duration) -> Vec<EngineAction> {
        self.now = now;
        match command {
            EngineCommand::Go => self.go(),
            EngineCommand::Fire(qid) => self.fire(qid),
            EngineCommand::Stop => self.stop_all(),
            EngineCommand::Pause if !self.paused => self.pause(),
            EngineCommand::Resume if self.paused => self.resume(),
            EngineCommand::Pause | EngineCommand::Resume => {}
            EngineCommand::TogglePause => {
                if self.paused {
                    self.resume()
                } else {
                    self.pause()
                }
            }
            EngineCommand::Select(qid) => self.state.lock_unpoisoned().selected_cue_id = Some(qid),
            EngineCommand::Preload => self.preload(),
            EngineCommand::Seek { instance_id, secs } => self.seek(instance_id, secs),
            EngineCommand::SetLevel { qid, volume, pan } => self.set_level(qid, volume, pan),
        }
        self.take_actions()
    }

    pub fn event(&mut self, event: EngineEvent, now: Duration) -> Vec<EngineAction> {
        self.now = now;
        match event {
            EngineEvent::VideoEof { instance_id, epoch } => self.video_eof(instance_id, epoch),
            EngineEvent::VideoFailed { instance_id, epoch } => {
                self.video_failed(instance_id, epoch)
            }
            EngineEvent::ExternalFinished { qid } => self.finish_external(qid),
        }
        self.take_actions()
    }

    pub fn tick(&mut self, now: Duration) -> Vec<EngineAction> {
        self.now = now;
        self.check_fade_outs();
        self.check_pending_stops();
        self.check_finished_cues();
        self.check_video_loops();
        self.check_delayed_cues();
        self.check_timecodes();
        if let Some(engine) = &self.audio_engine {
            engine.refresh();
        }
        self.take_actions()
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        let active_cues = self
            .active_cues
            .iter()
            .map(|cue| {
                let channels = cue.input.channels().max(1);
                let sample_rate = cue.input.sample_rate().max(1);
                let loop_frames = cue.loop_end_frame.saturating_sub(cue.loop_start_frame);
                let (position, length) = if cue.loop_counter.is_some() && loop_frames > 0 {
                    let frames = cue.input.position() / channels;
                    let relative = frames % usize::try_from(loop_frames).unwrap_or(usize::MAX);
                    (
                        relative.saturating_mul(channels),
                        usize::try_from(loop_frames)
                            .ok()
                            .and_then(|frames| frames.checked_mul(channels)),
                    )
                } else {
                    (cue.input.position(), active_cue_length_samples(cue))
                };
                ActiveCueSnapshot {
                    instance_id: cue.instance_id,
                    qid: cue.qid,
                    name: cue.name.clone(),
                    state: cue.state,
                    position_secs: position as f64 / channels as f64 / sample_rate as f64,
                    length_secs: length
                        .map(|samples| samples as f64 / channels as f64 / sample_rate as f64),
                }
            })
            .collect();
        let standby_qid = self.state.lock_unpoisoned().selected_cue_id;
        EngineSnapshot {
            standby_qid,
            paused: self.paused,
            show_elapsed_secs: self.show_elapsed().map(|time| time.as_secs_f64()),
            active_cues,
            video: self.current_video.as_ref().map(|video| VideoSnapshot {
                qid: video.qid,
                instance_id: video.instance_id,
                epoch: video.epoch,
                path: video.path.clone(),
                paused: self.paused,
                position_secs: self.video_position(video),
            }),
        }
    }

    /// Whether any cue is still running. `snapshot()` answers the same question
    /// but allocates and takes the shared-state lock on the way, and the quit
    /// paths ask this on every event-loop tick.
    pub fn has_active_cues(&self) -> bool {
        !self.active_cues.is_empty()
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn current_video_qid(&self) -> Option<Decimal> {
        self.current_video.as_ref().map(|video| video.qid)
    }

    pub fn adjust_show_time(&mut self, delta_secs: f64) {
        if delta_secs.is_finite() {
            self.show_adjustment_secs += delta_secs;
        }
    }

    pub fn reset_for_project_change(&mut self) -> Vec<EngineAction> {
        self.stop_all();
        self.take_actions()
    }

    fn take_actions(&mut self) -> Vec<EngineAction> {
        self.actions.drain(..).collect()
    }

    fn trace(&mut self, event: EngineTrace) {
        self.actions.push_back(EngineAction::Trace(event));
    }

    fn allocate_instance_id(&mut self) -> u64 {
        let id = self.next_active_cue_instance_id;
        self.next_active_cue_instance_id = self.next_active_cue_instance_id.wrapping_add(1).max(1);
        id
    }

    fn go(&mut self) {
        if self.show_start.is_none() {
            self.triggered_timecodes.clear();
            self.active_timecodes.clear();
        }

        let (start_idx, cues) = {
            let state = self.state.lock_unpoisoned();
            let Some(qid) = state.selected_cue_id else {
                log::info!("Go pressed but no cue selected");
                return;
            };
            let Some(index) = state
                .show_file
                .cues
                .iter()
                .position(|cue| cue.base().qid == qid)
            else {
                log::warn!("Selected cue Q{qid} not found in cue list");
                return;
            };
            let mut cues = Vec::new();
            for (offset, cue) in state.show_file.cues[index..].iter().enumerate() {
                if !cue.enabled() {
                    if offset == 0 {
                        break;
                    }
                    continue;
                }
                if offset == 0 || cue.base().trigger == TriggerMode::WithLast {
                    cues.push(cue.clone());
                } else {
                    break;
                }
            }
            (index, cues)
        };

        for cue in cues {
            self.play_cue(cue);
        }

        // Arm the show clock *after* the chain has played: shows open with a
        // stop-all RESET cue, and a stop-all resets the clock — arming first
        // would hand the show it just started a dead clock (no LTC output, no
        // timecode triggers). `self.now` is fixed for the whole command, so
        // the clock still reads zero at the instant the chain fired.
        if self.show_start.is_none() {
            self.show_start = Some(self.now);
            self.show_paused_offset = Duration::ZERO;
            self.show_adjustment_secs = 0.0;
            self.show_pause_started = self.paused.then_some(self.now);
        }

        let next = {
            let state = self.state.lock_unpoisoned();
            if matches!(state.show_file.cues.get(start_idx), Some(Cue::Goto { .. })) {
                None
            } else {
                next_standby_qid(&state.show_file.cues, start_idx)
            }
        };
        if let Some(qid) = next {
            self.state.lock_unpoisoned().selected_cue_id = Some(qid);
        }
    }

    fn fire(&mut self, qid: Decimal) {
        let cue = self
            .state
            .lock_unpoisoned()
            .show_file
            .cues
            .iter()
            .find(|cue| cue.base().qid == qid)
            .cloned();
        if let Some(cue) = cue {
            self.play_cue(cue);
        } else {
            log::warn!("Trigger referenced unknown cue Q{qid}");
        }
    }

    /// Surface a misconfiguration the operator can still act on: the alert panel
    /// during the show, the log window after it. A cue that goes to the wrong
    /// machine — or to none — is not something to find out about from a log file.
    fn warn_operator(&self, message: String) {
        log::warn!("{message}");
        self.state.lock_unpoisoned().report_operator_error(message);
    }

    fn play_cue(&mut self, cue: Cue) {
        if !cue.enabled() {
            return;
        }
        let qid = cue.base().qid;
        if !cue.base().retriggerable && self.cue_is_active(qid) {
            return;
        }

        // Routing is resolved before anything is prepared locally: a cue bound to
        // another machine must not touch this one's outputs.
        if !cue.base().remote_node.trim().is_empty() {
            let settings = self.state.lock_unpoisoned().show_file.show_settings.clone();
            match resolve_remote_routing(cue.base(), &settings) {
                RemoteRouting::Local => {}
                RemoteRouting::Remote(node) => {
                    // Sent even when the node is unknown to us: discovery is a
                    // broadcast, and a switch that filters broadcast makes a live
                    // node look absent. Refusing here would kill a working cue, so
                    // the mismatch is reported instead of enforced.
                    if !settings
                        .remote_nodes
                        .iter()
                        .any(|known| known.name.trim() == node)
                    {
                        self.warn_operator(format!(
                            "Q{qid} targets remote node '{node}', which has not been \
                             detected — sending anyway. Check the name and that the node \
                             is on the network."
                        ));
                    }
                    self.actions.push_back(EngineAction::RemoteGo { node, qid });
                    return;
                }
                RemoteRouting::LocalRemoteControlOff(node) => {
                    self.warn_operator(format!(
                        "Q{qid} targets remote node '{node}', but Remote Control is off \
                         — playing it on this machine instead."
                    ));
                }
            }
        }

        if cue.base().delay.as_secs_f64() > 0.0 {
            let mut delayed = cue;
            let delay = delayed.base().delay.as_secs_f64();
            delayed.base_mut().delay = Timespan::ZERO;
            self.delayed_cues.push(DelayedCue {
                cue: delayed,
                start_at: self.now + Duration::from_secs_f64(delay),
            });
            return;
        }

        if let Some(instance_id) = self
            .active_cues
            .iter_mut()
            .find(|active| active.qid == qid && active.state == CueState::Ready)
            .map(|active| {
                active.input.set_active(true);
                active.state = if matches!(
                    cue.base().loop_mode,
                    LoopMode::Looped | LoopMode::LoopedInfinite
                ) {
                    CueState::PlayingLooped
                } else {
                    CueState::Playing
                };
                active.instance_id
            })
        {
            self.trace(EngineTrace::CueStarted {
                qid,
                instance_id: Some(instance_id),
            });
            return;
        }

        match &cue {
            Cue::Sound {
                path,
                start_time,
                duration,
                volume,
                pan,
                fade_in,
                fade_out,
                fade_type,
                eq,
                routing,
                ..
            } => {
                self.play_audio(
                    path,
                    qid,
                    &cue.base().name,
                    cue.base().loop_mode,
                    cue.base().loop_count,
                    *start_time,
                    *duration,
                    *volume,
                    *fade_in,
                    *fade_out,
                    *fade_type,
                    *eq,
                    *pan,
                    routing.clone(),
                    false,
                );
            }
            Cue::Video {
                path,
                start_time,
                duration,
                volume,
                pan,
                fade_in,
                fade_out,
                fade_type,
                eq,
                routing,
                follow_mtc,
                mtc_start,
                ..
            } => {
                self.current_picture_qid = None;
                let fallback_instance = self.allocate_instance_id();
                let before = self.active_cues.len();
                if !follow_mtc {
                    self.play_audio(
                        path,
                        qid,
                        &cue.base().name,
                        cue.base().loop_mode,
                        cue.base().loop_count,
                        *start_time,
                        *duration,
                        *volume,
                        *fade_in,
                        *fade_out,
                        *fade_type,
                        *eq,
                        *pan,
                        routing.clone(),
                        false,
                    );
                }
                let audio_instance = self
                    .active_cues
                    .get(before)
                    .filter(|active| active.qid == qid)
                    .map(|active| active.instance_id);
                let instance_id = audio_instance.unwrap_or(fallback_instance);
                let resolved = self.resolve_path(path).unwrap_or_else(|| path.clone());
                self.current_video = Some(CurrentVideo {
                    qid,
                    instance_id,
                    epoch: 0,
                    path: resolved.clone(),
                    start_time: *start_time,
                    duration: *duration,
                    loop_mode: cue.base().loop_mode,
                    follow_mtc: *follow_mtc,
                    mtc_start: *mtc_start,
                    has_audio: audio_instance.is_some(),
                    clock_origin: self.now,
                    paused_position: self.paused.then_some(0.0),
                });
                let video = self
                    .current_video
                    .clone()
                    .expect("video was just installed");
                self.push_video_action(video);
                if audio_instance.is_none() {
                    self.trace(EngineTrace::CueStarted {
                        qid,
                        instance_id: Some(instance_id),
                    });
                }
            }
            Cue::Stop {
                stop_qid,
                stop_mode,
                fade_out_time,
                fade_type,
                stop_all,
                ..
            } => {
                self.trace(EngineTrace::CueStarted {
                    qid,
                    instance_id: None,
                });
                if *stop_all {
                    self.stop_all_faded(*fade_out_time, *fade_type);
                } else {
                    self.stop_target(*stop_qid, *stop_mode, *fade_out_time, *fade_type);
                }
                self.finish_instant(qid);
            }
            Cue::Volume {
                sound_qid,
                volume,
                fade_time,
                fade_type,
                ..
            } => {
                self.trace(EngineTrace::CueStarted {
                    qid,
                    instance_id: None,
                });
                self.set_volume(*sound_qid, *volume, *fade_time, *fade_type);
                self.finish_instant(qid);
            }
            Cue::Group { .. } => {
                let members = self
                    .state
                    .lock_unpoisoned()
                    .show_file
                    .cues
                    .iter()
                    .filter(|member| {
                        member.base().parent == Some(qid)
                            && member.base().qid != qid
                            && member.base().trigger != TriggerMode::AfterLast
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                for member in members {
                    self.play_cue(member);
                }
            }
            Cue::TimeCode {
                start_time,
                duration,
                ..
            } => {
                if duration.as_secs_f64() > 0.0 {
                    self.active_timecodes.push((
                        qid,
                        Duration::from_secs_f64(start_time.as_secs_f64() + duration.as_secs_f64()),
                    ));
                } else {
                    self.play_after_last(qid);
                }
            }
            Cue::Goto { target_qid, .. } => {
                let target = resolve_goto_target(
                    &self.state.lock_unpoisoned().show_file.cues,
                    qid,
                    *target_qid,
                );
                if let Some(target) = target {
                    self.state.lock_unpoisoned().selected_cue_id = Some(target);
                }
            }
            _ => {
                if matches!(cue, Cue::Image { .. }) {
                    self.current_video = None;
                    self.current_picture_qid = Some(qid);
                }
                self.trace(EngineTrace::CueStarted {
                    qid,
                    instance_id: None,
                });
                self.actions
                    .push_back(EngineAction::FireExternal(Box::new(cue.clone())));
                if !matches!(cue, Cue::DmxShow { .. }) {
                    self.trace(EngineTrace::CueFinished { qid });
                    self.play_after_last(qid);
                }
            }
        }
    }

    // ponytail: Preserve the persisted cue-to-DSP mapping; use a parameter object only if it grows.
    #[allow(clippy::too_many_arguments)]
    fn play_audio(
        &mut self,
        path: &str,
        qid: Decimal,
        name: &str,
        loop_mode: LoopMode,
        loop_count: i32,
        start_time: Timespan,
        duration: Timespan,
        volume: f32,
        fade_in: f32,
        fade_out: f32,
        fade_type: cuepool_core::FadeType,
        eq: Option<cuepool_core::EQSettings>,
        pan: f32,
        routing: cuepool_core::AudioRouting,
        preload_only: bool,
    ) -> Option<u64> {
        let (requested_driver, requested_device, configured_error) = {
            let state = self.state.lock_unpoisoned();
            (
                state.show_file.show_settings.audio_output_driver,
                state.show_file.show_settings.audio_output_device.clone(),
                state.audio_error.clone(),
            )
        };
        let Some(audio_engine) = self.audio_engine.as_ref().filter(|engine| {
            engine.device_name() == "headless"
                || (engine.driver() == requested_driver
                    && (requested_device.is_empty() || requested_device == engine.device_name()))
        }) else {
            let reason =
                configured_error.unwrap_or_else(|| "configured audio output is not active".into());
            log::error!("Cannot play audio cue Q{qid}: {reason}");
            return None;
        };
        let resolved = self.resolve_path(path).unwrap_or_else(|| path.to_string());
        let decoder = match FileDecoder::open(&resolved) {
            Ok(decoder) => decoder,
            Err(cuepool_audio::DecodeError::NoAudioTrack) => return None,
            Err(error) => {
                log::error!("Failed to open audio for {path}: {error}");
                return None;
            }
        };
        let sample_rate = decoder.sample_rate();
        let source_end_frame = decoder
            .length()
            .map(|samples| samples / decoder.channels().max(1) as usize)
            .and_then(|frames| u64::try_from(frames).ok());
        let out_scale = audio_engine.sample_rate() as f64 / sample_rate as f64;
        let start_frame = (start_time.as_secs_f64() * sample_rate as f64) as u64;
        let end_frame = if duration.as_secs_f64() > 0.0 {
            start_frame + (duration.as_secs_f64() * sample_rate as f64) as u64
        } else {
            0
        };
        let effective_end_frame = match (end_frame, source_end_frame) {
            (0, Some(source_end)) => source_end,
            (requested, Some(source_end)) => requested.min(source_end),
            (requested, None) => requested,
        };
        let playback = match audio_engine.play_cue(
            Box::new(decoder),
            CueChainParams {
                start_frame,
                end_frame,
                loop_mode,
                loop_count: loop_count as u32,
                eq,
                fade_in_secs: fade_in,
                fade_type,
            },
        ) {
            Ok(playback) => playback,
            Err(error) => {
                log::error!("Cannot play audio cue Q{qid}: {error}");
                return None;
            }
        };
        playback.input.set_volume(volume);
        playback.input.set_pan(pan);
        playback
            .input
            .set_routing(routing.out_pair, routing.send, routing.crosspoints);
        if preload_only {
            playback.input.set_active(false);
        }
        let instance_id = self.allocate_instance_id();
        let state = if preload_only {
            CueState::Ready
        } else if playback.loop_counter.is_some() {
            CueState::PlayingLooped
        } else {
            CueState::Playing
        };
        self.active_cues.push(ActiveCue {
            instance_id,
            qid,
            name: name.to_string(),
            input: playback.input,
            state,
            loop_counter: playback.loop_counter,
            video_loop_count: 0,
            video_loop_limit: match loop_mode {
                LoopMode::Looped => Some((loop_count as u32).saturating_sub(1)),
                LoopMode::LoopedInfinite => None,
                _ => None,
            },
            loop_start_frame: (start_frame as f64 * out_scale) as u64,
            loop_end_frame: (effective_end_frame as f64 * out_scale) as u64,
            fade_out,
            fade_type,
            fade_out_started: false,
            pending_stop: None,
            follow_after_last: true,
        });
        if !preload_only {
            self.trace(EngineTrace::CueStarted {
                qid,
                instance_id: Some(instance_id),
            });
        }
        Some(instance_id)
    }

    fn cue_is_active(&self, qid: Decimal) -> bool {
        self.current_video
            .as_ref()
            .is_some_and(|video| video.qid == qid)
            || self.current_picture_qid == Some(qid)
            || self.active_cues.iter().any(|cue| {
                cue.qid == qid
                    && matches!(
                        cue.state,
                        CueState::Playing | CueState::PlayingLooped | CueState::Paused
                    )
            })
    }

    fn preload(&mut self) {
        let cue = self.state.lock_unpoisoned().selected_cue().cloned();
        let Some(cue) = cue else { return };
        if self
            .active_cues
            .iter()
            .any(|active| active.qid == cue.base().qid)
        {
            return;
        }
        match &cue {
            Cue::Sound {
                path,
                start_time,
                duration,
                volume,
                pan,
                fade_in,
                fade_out,
                fade_type,
                eq,
                routing,
                ..
            }
            | Cue::Video {
                path,
                start_time,
                duration,
                volume,
                pan,
                fade_in,
                fade_out,
                fade_type,
                eq,
                routing,
                ..
            } => {
                self.play_audio(
                    path,
                    cue.base().qid,
                    &cue.base().name,
                    cue.base().loop_mode,
                    cue.base().loop_count,
                    *start_time,
                    *duration,
                    *volume,
                    *fade_in,
                    *fade_out,
                    *fade_type,
                    *eq,
                    *pan,
                    routing.clone(),
                    true,
                );
            }
            _ => {}
        }
    }

    fn stop_target(
        &mut self,
        qid: Decimal,
        mode: StopMode,
        fade_out_secs: f32,
        fade_type: cuepool_core::FadeType,
    ) {
        for target in self.resolve_stop_targets(qid) {
            self.stop_single(target, mode, fade_out_secs, fade_type);
        }
        // Deliberately no reset_show_clock(): a targeted stop is surgical
        // (kill the pre-show loop, cut one music bed) — the show clock, and
        // the LTC everything downstream chases, belongs to the show, not to
        // the cue. Only stop-all ends the show.
    }

    /// A Stop aimed at a Group means the whole group: the target plus every
    /// member, transitively — the mirror of how playing a Group plays its
    /// members. A non-group target resolves to itself alone.
    fn resolve_stop_targets(&self, qid: Decimal) -> Vec<Decimal> {
        let mut targets = vec![qid];
        let state = self.state.lock_unpoisoned();
        let cues = &state.show_file.cues;
        if !cues
            .iter()
            .any(|cue| matches!(cue, Cue::Group { .. }) && cue.base().qid == qid)
        {
            return targets;
        }
        let mut index = 0;
        while index < targets.len() {
            let parent = targets[index];
            let members = cues
                .iter()
                .map(|cue| cue.base())
                .filter(|base| base.parent == Some(parent) && !targets.contains(&base.qid))
                .map(|base| base.qid)
                .collect::<Vec<_>>();
            targets.extend(members);
            index += 1;
        }
        targets
    }

    fn stop_single(
        &mut self,
        qid: Decimal,
        mode: StopMode,
        fade_out_secs: f32,
        fade_type: cuepool_core::FadeType,
    ) {
        if let Some(index) = self.active_cues.iter().position(|active| active.qid == qid) {
            if mode == StopMode::LoopEnd {
                self.active_cues[index].pending_stop = Some(PendingStop {
                    mode,
                    fade_out_time: fade_out_secs,
                    fade_type,
                });
            } else if fade_out_secs > 0.0 {
                let frames = (fade_out_secs * self.audio_sample_rate() as f32) as u32;
                self.active_cues[index]
                    .input
                    .start_fade(0.0, frames.max(1), fade_type);
            } else {
                self.active_cues[index].input.set_active(false);
                self.active_cues[index].input.set_volume(0.0);
                self.active_cues[index].state = CueState::Done;
            }
        }
        if mode != StopMode::LoopEnd {
            // A stop also cancels what has not started yet: a cue waiting on
            // its Delay, or a timecode window armed for an AfterLast follow.
            self.delayed_cues
                .retain(|delayed| delayed.cue.base().qid != qid);
            self.active_timecodes.retain(|(tc_qid, _)| *tc_qid != qid);
        }
        if self
            .current_video
            .as_ref()
            .is_some_and(|video| video.qid == qid)
            && mode != StopMode::LoopEnd
        {
            self.actions
                .push_back(EngineAction::StopVideo { fade_out_secs });
            self.current_video = None;
            self.pending_video_half = None;
        }
        if self.current_picture_qid == Some(qid) && mode != StopMode::LoopEnd {
            self.current_picture_qid = None;
        }
        self.actions.push_back(EngineAction::StopExternal {
            qid,
            mode,
            fade_out_secs,
            fade_type,
        });
    }

    /// A Stop cue's Stop All with a fade: everything fades out together
    /// instead of cutting. The transport's Stop (and a project change) keep
    /// the instant [`stop_all`](Self::stop_all) — that one is the panic
    /// button.
    fn stop_all_faded(&mut self, fade_out_secs: f32, fade_type: cuepool_core::FadeType) {
        // While paused nothing is rendered, so a fade could never finish.
        if fade_out_secs <= 0.0 || self.paused {
            self.stop_all();
            return;
        }
        let frames = (fade_out_secs * self.audio_sample_rate() as f32) as u32;
        for cue in &mut self.active_cues {
            cue.pending_stop = None;
            cue.follow_after_last = false;
            if cue.input.is_active() {
                cue.input.start_fade(0.0, frames.max(1), fade_type);
            } else {
                // A preloaded input is never rendered; a fade would strand it.
                cue.input.set_volume(0.0);
                cue.state = CueState::Done;
            }
        }
        self.delayed_cues.clear();
        self.active_timecodes.clear();
        // Only a video can ride the canvas fade; a still picture has no fade
        // machinery, so anything else blanks instantly, as the instant stop
        // does.
        let video_fade = if self.current_video.is_some() {
            fade_out_secs
        } else {
            0.0
        };
        self.current_video = None;
        self.pending_video_half = None;
        self.current_picture_qid = None;
        self.actions.push_back(EngineAction::StopAllExternal);
        self.actions.push_back(EngineAction::StopVideo {
            fade_out_secs: video_fade,
        });
        self.reset_show_clock();
    }

    fn stop_all(&mut self) {
        if let Some(engine) = &self.audio_engine {
            engine.stop_all();
        }
        self.active_cues.clear();
        self.delayed_cues.clear();
        self.active_timecodes.clear();
        self.current_video = None;
        self.pending_video_half = None;
        self.current_picture_qid = None;
        self.paused = false;
        self.reset_show_clock();
        self.actions.push_back(EngineAction::StopAllExternal);
        self.actions
            .push_back(EngineAction::StopVideo { fade_out_secs: 0.0 });
        self.actions.push_back(EngineAction::SetVideoPaused(false));
    }

    fn pause(&mut self) {
        for cue in &mut self.active_cues {
            cue.input.set_active(false);
            if matches!(cue.state, CueState::Playing | CueState::PlayingLooped) {
                cue.state = CueState::Paused;
            }
        }
        let video_position = self
            .current_video
            .as_ref()
            .map(|video| self.video_position(video));
        if let (Some(video), Some(position)) = (&mut self.current_video, video_position) {
            video.paused_position = Some(position);
        }
        self.paused = true;
        self.show_pause_started.get_or_insert(self.now);
        self.actions.push_back(EngineAction::SetVideoPaused(true));
    }

    fn resume(&mut self) {
        for cue in &mut self.active_cues {
            cue.input.set_active(true);
            if cue.state == CueState::Paused {
                cue.state = if cue.loop_counter.is_some() {
                    CueState::PlayingLooped
                } else {
                    CueState::Playing
                };
            }
        }
        if let Some(video) = &mut self.current_video
            && let Some(position) = video.paused_position.take()
        {
            video.clock_origin = self.now.saturating_sub(Duration::from_secs_f64(position));
        }
        self.paused = false;
        if let Some(paused_at) = self.show_pause_started.take() {
            self.show_paused_offset += self.now.saturating_sub(paused_at);
        }
        self.actions.push_back(EngineAction::SetVideoPaused(false));
    }

    fn seek(&mut self, instance_id: u64, secs: f32) {
        let qid = self
            .active_cues
            .iter()
            .find(|cue| cue.instance_id == instance_id)
            .map(|cue| cue.qid)
            .or_else(|| {
                self.current_video
                    .as_ref()
                    .filter(|video| video.instance_id == instance_id)
                    .map(|video| video.qid)
            });
        let Some(qid) = qid else { return };
        let cue = self
            .state
            .lock_unpoisoned()
            .show_file
            .cues
            .iter()
            .find(|cue| cue.base().qid == qid)
            .cloned();
        let Some(cue) = cue else { return };
        let audio_target = self.seek_audio(instance_id, secs);
        if let Cue::Video {
            start_time,
            duration,
            ..
        } = cue
        {
            let Some(video) = self
                .current_video
                .as_ref()
                .filter(|video| video.instance_id == instance_id)
                .cloned()
            else {
                return;
            };
            let configured_length = (duration.as_secs_f64() > 0.0).then(|| duration.as_secs_f64());
            let target = clamp_seek(
                audio_target.unwrap_or_else(|| sanitize_seek(secs)),
                configured_length,
            );
            if let Some(current) = self.current_video.as_mut() {
                if self.paused {
                    current.paused_position = Some(target);
                } else {
                    current.clock_origin = self.now.saturating_sub(Duration::from_secs_f64(target));
                }
            }
            self.actions.push_back(EngineAction::SeekVideo {
                qid,
                instance_id,
                path: video.path,
                target_secs: target,
                media_offset_secs: start_time.as_secs_f64(),
                paused: self.paused,
            });
        }
    }

    fn seek_audio(&mut self, instance_id: u64, secs: f32) -> Option<f64> {
        let cue = self
            .active_cues
            .iter_mut()
            .find(|cue| cue.instance_id == instance_id)?;
        let channels = cue.input.channels().max(1);
        let rate = cue.input.sample_rate().max(1);
        let length = active_cue_length_samples(cue)?;
        if length == 0 {
            return None;
        }
        let requested = sanitize_seek(secs);
        let max_samples = length.saturating_sub(channels);
        let samples = ((requested * rate as f64) as usize)
            .saturating_mul(channels)
            .min(max_samples);
        cue.input.seek(samples);
        if cue.fade_out > 0.0 && cue.loop_counter.is_none() {
            let fade_frames = (cue.fade_out * rate as f32) as u32;
            let fade_samples = fade_frames as usize * channels;
            match tail_fade_seek_action(cue.fade_out_started, samples, length, fade_samples) {
                TailFadeSeekAction::Unchanged => {}
                TailFadeSeekAction::Rearm => {
                    cue.input.cancel_fade();
                    cue.fade_out_started = false;
                }
                TailFadeSeekAction::Restart => {
                    if cue.fade_out_started {
                        cue.input.cancel_fade();
                    }
                    cue.input.start_fade(0.0, fade_frames.max(1), cue.fade_type);
                    cue.fade_out_started = true;
                }
            }
        }
        Some(samples as f64 / channels as f64 / rate as f64)
    }

    /// Every playing instance of `qid`, not just the first — a cue fired twice
    /// has two, and a Volume cue naming that Q# means both of them.
    fn set_volume(
        &self,
        qid: Decimal,
        target: f32,
        fade_secs: f32,
        fade_type: cuepool_core::FadeType,
    ) {
        let frames = (fade_secs * self.audio_sample_rate() as f32) as u32;
        for cue in self.active_cues.iter().filter(|cue| cue.qid == qid) {
            if fade_secs > 0.0 {
                cue.input
                    .start_fade(target.max(0.0), frames.max(1), fade_type);
            } else {
                cue.input.set_volume(target.max(0.0));
            }
        }
    }

    /// Live level edit from the inspector. No fade — the user is dragging a
    /// slider and wants to hear it now. Video cues route their audio through
    /// the same `MixerInput`, so this covers both cue types.
    fn set_level(&self, qid: Decimal, volume: f32, pan: f32) {
        for cue in self.active_cues.iter().filter(|cue| cue.qid == qid) {
            cue.input.set_volume_live(volume.max(0.0));
            cue.input.set_pan(pan);
        }
    }

    fn check_fade_outs(&mut self) {
        let rate = self.audio_sample_rate();
        for cue in &mut self.active_cues {
            if cue.fade_out <= 0.0
                || cue.fade_out_started
                || cue.loop_counter.is_some()
                || cue.state != CueState::Playing
            {
                continue;
            }
            let Some(end) = active_cue_length_samples(cue) else {
                continue;
            };
            let frames = (cue.fade_out * rate as f32) as u32;
            let trigger = end.saturating_sub(frames as usize * cue.input.channels());
            if cue.input.position() >= trigger {
                cue.input.start_fade(0.0, frames.max(1), cue.fade_type);
                cue.fade_out_started = true;
            }
        }
    }

    fn check_pending_stops(&mut self) {
        let rate = self.audio_sample_rate();
        for cue in &mut self.active_cues {
            let Some(pending) = cue.pending_stop else {
                continue;
            };
            if pending.mode != StopMode::LoopEnd {
                continue;
            }
            let Some(end) = active_cue_length_samples(cue) else {
                continue;
            };
            let frames = (pending.fade_out_time * rate as f32) as u32;
            let trigger = end.saturating_sub(frames as usize * cue.input.channels());
            if cue.input.position() >= trigger {
                if pending.fade_out_time > 0.0 {
                    cue.input.start_fade(0.0, frames.max(1), pending.fade_type);
                } else {
                    cue.input.set_active(false);
                    cue.input.set_volume(0.0);
                    cue.state = CueState::Done;
                }
                cue.pending_stop = None;
            }
        }
    }

    fn check_finished_cues(&mut self) {
        let finished = self
            .active_cues
            .iter()
            .filter(|cue| cue.input.is_finished() || cue.state == CueState::Done)
            .map(|cue| (cue.instance_id, cue.qid, cue.follow_after_last))
            .collect::<Vec<_>>();
        for (instance_id, qid, follow_after_last) in finished {
            self.active_cues
                .retain(|cue| cue.instance_id != instance_id);
            // This instance may be the audio half of a video cue, whose picture
            // can outlast it. Hold the follow until the picture ends too.
            if self.audio_half_of_pending_video(qid, instance_id)
                && !self.record_video_half(qid, instance_id)
            {
                continue;
            }
            self.trace(EngineTrace::CueFinished { qid });
            if follow_after_last {
                self.play_after_last(qid);
            }
        }
    }

    /// True when `(qid, instance_id)` is the audio track of a one-shot video
    /// cue whose picture ends on its own — either still playing, or already
    /// ended and waiting on this half.
    fn audio_half_of_pending_video(&self, qid: Decimal, instance_id: u64) -> bool {
        self.current_video.as_ref().is_some_and(|video| {
            video.instance_id == instance_id && video.has_audio && video.picture_ends_on_its_own()
        }) || self.pending_video_half == Some((qid, instance_id))
    }

    /// Record one half (picture or audio) of a video cue finishing. Returns
    /// true once both halves are in, i.e. when the follow should fire.
    fn record_video_half(&mut self, qid: Decimal, instance_id: u64) -> bool {
        if self.pending_video_half == Some((qid, instance_id)) {
            self.pending_video_half = None;
            true
        } else {
            self.pending_video_half = Some((qid, instance_id));
            false
        }
    }

    fn check_video_loops(&mut self) {
        let Some(video) = self.current_video.clone() else {
            return;
        };
        if video.follow_mtc {
            return;
        }
        let looped = self
            .active_cues
            .iter_mut()
            .find(|cue| cue.qid == video.qid)
            .and_then(|cue| {
                cue.loop_counter.as_ref()?;
                let loop_frames = cue.loop_end_frame.saturating_sub(cue.loop_start_frame);
                if loop_frames == 0 {
                    return None;
                }
                let played_frames =
                    u64::try_from(cue.input.position() / cue.input.channels().max(1))
                        .unwrap_or(u64::MAX);
                let current = u32::try_from(played_frames / loop_frames).unwrap_or(u32::MAX);
                let current = cue
                    .video_loop_limit
                    .map_or(current, |limit| current.min(limit));
                let previous = cue.video_loop_count;
                cue.video_loop_count = current;
                if current > previous { Some(()) } else { None }
            })
            .is_some();
        if looped {
            self.push_video_action(video);
        }
    }

    fn check_delayed_cues(&mut self) {
        let mut ready = Vec::new();
        self.delayed_cues.retain(|delayed| {
            if delayed.start_at <= self.now {
                ready.push(delayed.cue.clone());
                false
            } else {
                true
            }
        });
        for cue in ready {
            self.play_cue(cue);
        }
    }

    fn check_timecodes(&mut self) {
        let Some(elapsed) = self.show_elapsed() else {
            return;
        };
        let cues = {
            let state = self.state.lock_unpoisoned();
            state
                .show_file
                .cues
                .iter()
                .filter(|cue| {
                    matches!(cue, Cue::TimeCode { start_time, .. } if start_time.as_secs_f64() > 0.0 && elapsed.as_secs_f64() >= start_time.as_secs_f64())
                        && cue.enabled()
                        && !self.triggered_timecodes.contains(&cue.base().qid)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        for cue in cues {
            self.triggered_timecodes.insert(cue.base().qid);
            self.play_cue(cue);
        }
        let expired = self
            .active_timecodes
            .iter()
            .filter(|(_, deadline)| *deadline <= elapsed)
            .map(|(qid, _)| *qid)
            .collect::<Vec<_>>();
        self.active_timecodes
            .retain(|(_, deadline)| *deadline > elapsed);
        for qid in expired {
            self.play_after_last(qid);
        }
    }

    fn video_eof(&mut self, instance_id: u64, epoch: u64) {
        let Some(video) = self
            .current_video
            .as_ref()
            .filter(|video| video.instance_id == instance_id && video.epoch == epoch)
            .cloned()
        else {
            return;
        };
        if video.follow_mtc {
            return;
        }
        match video.loop_mode {
            LoopMode::Looped | LoopMode::LoopedInfinite if video.has_audio => {}
            LoopMode::Looped | LoopMode::LoopedInfinite => {
                self.push_video_action(video);
            }
            LoopMode::HoldLast => {}
            _ => {
                self.current_video = None;
                self.actions
                    .push_back(EngineAction::StopVideo { fade_out_secs: 0.0 });
                // With an audio track the follow waits for whichever stream
                // ends last; without one the picture is the whole cue.
                let follow_now = if video.has_audio {
                    self.record_video_half(video.qid, video.instance_id)
                } else {
                    true
                };
                if follow_now {
                    self.trace(EngineTrace::CueFinished { qid: video.qid });
                    self.play_after_last(video.qid);
                }
            }
        }
    }

    fn video_failed(&mut self, instance_id: u64, epoch: u64) {
        let Some(video) = self
            .current_video
            .as_ref()
            .filter(|video| video.instance_id == instance_id && video.epoch == epoch)
            .cloned()
        else {
            return;
        };
        self.current_video = None;
        self.actions
            .push_back(EngineAction::StopVideo { fade_out_secs: 0.0 });
        if !video.has_audio {
            self.trace(EngineTrace::CueFinished { qid: video.qid });
        }
    }

    fn finish_external(&mut self, qid: Decimal) {
        self.trace(EngineTrace::CueFinished { qid });
        self.play_after_last(qid);
    }

    fn finish_instant(&mut self, qid: Decimal) {
        self.trace(EngineTrace::CueFinished { qid });
        self.play_after_last(qid);
    }

    fn push_video_action(&mut self, mut video: CurrentVideo) {
        video.epoch = video.epoch.wrapping_add(1).max(1);
        video.clock_origin = self.now;
        video.paused_position = self.paused.then_some(0.0);
        self.current_video = Some(video.clone());
        self.actions.push_back(EngineAction::PlayVideo {
            qid: video.qid,
            instance_id: video.instance_id,
            epoch: video.epoch,
            clock_origin: video.clock_origin,
            path: video.path.clone(),
            start_time: video.start_time,
            duration: video.duration,
            follow_mtc: video.follow_mtc,
            mtc_start: video.mtc_start,
        });
    }

    fn play_after_last(&mut self, qid: Decimal) {
        let cue = next_after_last(&self.state.lock_unpoisoned().show_file.cues, qid).cloned();
        if let Some(cue) = cue {
            self.play_cue(cue);
        }
    }

    fn reset_show_clock(&mut self) {
        self.show_start = None;
        self.show_pause_started = None;
        self.show_paused_offset = Duration::ZERO;
        self.show_adjustment_secs = 0.0;
        self.triggered_timecodes.clear();
    }

    fn show_elapsed(&self) -> Option<Duration> {
        let start = self.show_start?;
        let now = self.show_pause_started.unwrap_or(self.now);
        let elapsed = now
            .saturating_sub(start)
            .saturating_sub(self.show_paused_offset)
            .as_secs_f64()
            + self.show_adjustment_secs;
        Some(Duration::from_secs_f64(elapsed.max(0.0)))
    }

    fn audio_sample_rate(&self) -> u32 {
        self.audio_engine
            .as_ref()
            .map(AudioEngine::sample_rate)
            .unwrap_or(48_000)
    }

    fn video_position(&self, video: &CurrentVideo) -> f64 {
        let position = video
            .paused_position
            .unwrap_or_else(|| self.now.saturating_sub(video.clock_origin).as_secs_f64());
        let duration = video.duration.as_secs_f64();
        if duration > 0.0 {
            position.min(duration)
        } else {
            position
        }
    }

    fn resolve_path(&self, path: &str) -> Option<String> {
        let path = Path::new(path);
        if path.is_absolute() && path.exists() {
            return Some(path.to_string_lossy().into_owned());
        }
        let project_dir = self
            .state
            .lock_unpoisoned()
            .project_path
            .as_ref()?
            .parent()?
            .to_path_buf();
        let relative = project_dir.join(path);
        if relative.exists() {
            return Some(relative.to_string_lossy().into_owned());
        }
        find_in_dir(&project_dir, path.file_name()?).map(|path| path.to_string_lossy().into_owned())
    }
}

fn active_cue_length_samples(cue: &ActiveCue) -> Option<usize> {
    let frames = cue.loop_end_frame.saturating_sub(cue.loop_start_frame);
    if frames > 0 {
        usize::try_from(frames)
            .ok()?
            .checked_mul(cue.input.channels())
    } else {
        cue.input.length()
    }
}

fn sanitize_seek(secs: f32) -> f64 {
    if secs.is_nan() || secs <= 0.0 {
        0.0
    } else {
        f64::from(secs)
    }
}

fn clamp_seek(target: f64, length: Option<f64>) -> f64 {
    match length.filter(|length| length.is_finite() && *length > 0.0) {
        Some(length) => target.min(length.next_down()),
        None if target.is_finite() => target,
        None => 0.0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TailFadeSeekAction {
    Unchanged,
    Rearm,
    Restart,
}

fn tail_fade_seek_action(
    fade_started: bool,
    target_samples: usize,
    end_samples: usize,
    fade_samples: usize,
) -> TailFadeSeekAction {
    if target_samples >= end_samples.saturating_sub(fade_samples) {
        TailFadeSeekAction::Restart
    } else if fade_started {
        TailFadeSeekAction::Rearm
    } else {
        TailFadeSeekAction::Unchanged
    }
}

fn next_standby_qid(cues: &[Cue], start_idx: usize) -> Option<Decimal> {
    let mut index = start_idx + 1;
    if matches!(cues.get(start_idx), Some(Cue::Group { .. })) {
        let group = cues[start_idx].base().qid;
        while index < cues.len() && cues[index].base().parent == Some(group) {
            index += 1;
        }
    } else {
        while index < cues.len()
            && matches!(
                cues[index].base().trigger,
                TriggerMode::WithLast | TriggerMode::AfterLast
            )
        {
            index += 1;
        }
    }
    cues.get(index).map(|cue| cue.base().qid)
}

fn next_after_last(cues: &[Cue], qid: Decimal) -> Option<&Cue> {
    let index = cues.iter().position(|cue| cue.base().qid == qid)?;
    cues[index + 1..]
        .iter()
        .take_while(|cue| cue.base().trigger == TriggerMode::AfterLast)
        .find(|cue| cue.enabled())
}

/// Which machine a cue fires on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteRouting {
    /// Fire here: no remote node is set, or the one set names this machine.
    Local,
    /// Dispatch to a named peer over OSC.
    Remote(String),
    /// The cue names a peer, but remote control is off — so it falls back to
    /// this machine. Kept distinct from `Local` because it is a fallback the
    /// operator has to be told about: a cue authored for the video rig playing
    /// out of the sound desk is not a silent event.
    LocalRemoteControlOff(String),
}

/// Resolve where `base` fires.
///
/// Names are compared trimmed. The Remote Node field is free text, so a stray
/// trailing space would otherwise route the cue to a name no node can ever
/// match — the cue would leave this machine and arrive nowhere.
pub fn resolve_remote_routing(base: &CueBase, settings: &ShowSettings) -> RemoteRouting {
    let target = base.remote_node.trim();
    if target.is_empty() || target == settings.node_name.trim() {
        return RemoteRouting::Local;
    }
    if !settings.enable_remote_control {
        return RemoteRouting::LocalRemoteControlOff(target.to_string());
    }
    RemoteRouting::Remote(target.to_string())
}

fn resolve_goto_target(cues: &[Cue], goto_qid: Decimal, first_target: Decimal) -> Option<Decimal> {
    let mut current = first_target;
    let mut visited = HashSet::from([goto_qid]);
    loop {
        if !visited.insert(current) {
            return None;
        }
        match cues.iter().find(|cue| cue.base().qid == current) {
            Some(Cue::Goto { target_qid, .. }) => current = *target_qid,
            Some(_) => return Some(current),
            None => return None,
        }
    }
}

fn find_in_dir(dir: &Path, target: &std::ffi::OsStr) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()? {
        let path = entry.ok()?.path();
        if path.is_file() && path.file_name()? == target {
            return Some(path);
        }
        if path.is_dir()
            && let Some(found) = find_in_dir(&path, target)
        {
            return Some(found);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct PositionSource {
        position: Arc<AtomicUsize>,
    }

    impl SampleProvider for PositionSource {
        fn read(&self, _buffer: &mut [f32]) -> usize {
            0
        }

        fn seek(&self, sample: usize) {
            self.position.store(sample, Ordering::Relaxed);
        }

        fn position(&self) -> usize {
            self.position.load(Ordering::Relaxed)
        }

        fn length(&self) -> Option<usize> {
            None
        }

        fn sample_rate(&self) -> u32 {
            48_000
        }

        fn channels(&self) -> u16 {
            2
        }
    }

    fn looped_video_engine(loop_mode: LoopMode) -> (ShowEngine, Arc<AtomicUsize>, Arc<AtomicU32>) {
        let app = cuepool_gui::CuePoolApp::new();
        let mut engine = ShowEngine::new(app.state().clone(), None);
        let position = Arc::new(AtomicUsize::new(0));
        let input = Arc::new(cuepool_audio::MixerInput::new(
            Box::new(PositionSource {
                position: Arc::clone(&position),
            }),
            2,
        ));
        let loop_counter = Arc::new(AtomicU32::new(0));
        engine.active_cues.push(ActiveCue {
            instance_id: 1,
            qid: Decimal::ONE,
            name: "looped video".into(),
            input,
            state: CueState::PlayingLooped,
            loop_counter: Some(Arc::clone(&loop_counter)),
            video_loop_count: 0,
            video_loop_limit: matches!(loop_mode, LoopMode::Looped).then_some(1),
            loop_start_frame: 0,
            loop_end_frame: 100,
            fade_out: 0.0,
            fade_type: Default::default(),
            fade_out_started: false,
            pending_stop: None,
            follow_after_last: true,
        });
        engine.current_video = Some(CurrentVideo {
            qid: Decimal::ONE,
            instance_id: 1,
            epoch: 1,
            path: "video.mov".into(),
            start_time: Timespan::ZERO,
            duration: Timespan::ZERO,
            loop_mode,
            follow_mtc: false,
            mtc_start: Timespan::ZERO,
            has_audio: true,
            clock_origin: Duration::ZERO,
            paused_position: None,
        });
        (engine, position, loop_counter)
    }

    fn play_video_count(actions: &[EngineAction]) -> usize {
        actions
            .iter()
            .filter(|action| matches!(action, EngineAction::PlayVideo { .. }))
            .count()
    }

    fn dummy(qid: i64, trigger: TriggerMode) -> Cue {
        Cue::Dummy {
            base: cuepool_core::CueBase {
                qid: Decimal::from(qid),
                trigger,
                ..Default::default()
            },
        }
    }

    fn goto(qid: i64, target: i64) -> Cue {
        Cue::Goto {
            base: cuepool_core::CueBase {
                qid: Decimal::from(qid),
                ..Default::default()
            },
            target_qid: Decimal::from(target),
        }
    }

    #[test]
    fn deterministic_clock_freezes_while_paused() {
        let state = cuepool_gui::CuePoolApp::new().state().clone();
        let mut engine = ShowEngine::new(state, None);
        engine.show_start = Some(Duration::from_secs(1));
        engine.now = Duration::from_secs(3);
        engine.pause();
        engine.now = Duration::from_secs(8);
        assert_eq!(engine.show_elapsed(), Some(Duration::from_secs(2)));
        engine.resume();
        engine.now = Duration::from_secs(10);
        assert_eq!(engine.show_elapsed(), Some(Duration::from_secs(4)));
    }

    #[test]
    fn video_action_carries_engine_clock_origin() {
        let (mut engine, _, _) = looped_video_engine(LoopMode::Looped);
        let clock_origin = Duration::from_secs(12);
        engine.now = clock_origin;
        engine.push_video_action(engine.current_video.clone().unwrap());

        assert!(matches!(
            engine.take_actions().as_slice(),
            [EngineAction::PlayVideo {
                clock_origin: action_origin,
                ..
            }] if *action_origin == clock_origin
        ));
    }

    fn remote_settings(node_name: &str, enabled: bool) -> ShowSettings {
        ShowSettings {
            node_name: node_name.into(),
            enable_remote_control: enabled,
            ..Default::default()
        }
    }

    fn addressed_to(node: &str) -> CueBase {
        CueBase {
            remote_node: node.into(),
            ..Default::default()
        }
    }

    #[test]
    fn unaddressed_cue_fires_locally() {
        assert_eq!(
            resolve_remote_routing(&addressed_to(""), &remote_settings("foh", true)),
            RemoteRouting::Local
        );
        // Whitespace is not a node name.
        assert_eq!(
            resolve_remote_routing(&addressed_to("   "), &remote_settings("foh", true)),
            RemoteRouting::Local
        );
    }

    #[test]
    fn cue_addressed_to_this_machine_fires_locally() {
        assert_eq!(
            resolve_remote_routing(&addressed_to("foh"), &remote_settings("foh", true)),
            RemoteRouting::Local
        );
    }

    #[test]
    fn node_names_are_matched_trimmed() {
        // A stray space in the inspector's free-text field used to address the
        // cue to a name nothing could match, so it played nowhere at all.
        assert_eq!(
            resolve_remote_routing(&addressed_to(" foh "), &remote_settings("foh", true)),
            RemoteRouting::Local
        );
        assert_eq!(
            resolve_remote_routing(&addressed_to(" video "), &remote_settings("foh", true)),
            RemoteRouting::Remote("video".into())
        );
    }

    #[test]
    fn addressed_cue_dispatches_when_remote_control_is_on() {
        assert_eq!(
            resolve_remote_routing(&addressed_to("video"), &remote_settings("foh", true)),
            RemoteRouting::Remote("video".into())
        );
    }

    #[test]
    fn addressed_cue_falls_back_locally_when_remote_control_is_off() {
        assert_eq!(
            resolve_remote_routing(&addressed_to("video"), &remote_settings("foh", false)),
            RemoteRouting::LocalRemoteControlOff("video".into())
        );
    }

    #[test]
    fn goto_cycles_are_rejected() {
        assert_eq!(
            resolve_goto_target(&[goto(1, 2), goto(2, 1)], Decimal::ONE, Decimal::TWO),
            None
        );
        assert_eq!(
            resolve_goto_target(&[goto(1, 99)], Decimal::ONE, Decimal::from(99)),
            None
        );
    }

    #[test]
    fn sequencing_helpers_preserve_standby_and_after_last_rules() {
        use TriggerMode::{AfterLast, Go, WithLast};
        let mut cues = vec![
            dummy(1, Go),
            dummy(2, WithLast),
            dummy(3, AfterLast),
            dummy(4, Go),
        ];
        assert_eq!(next_standby_qid(&cues, 0), Some(Decimal::from(4)));
        assert_eq!(
            next_after_last(&cues, Decimal::from(2)).map(|cue| cue.base().qid),
            Some(Decimal::from(3))
        );
        cues[2].base_mut().enabled = false;
        assert_eq!(next_after_last(&cues, Decimal::from(2)), None);

        let grouped = vec![
            Cue::Group {
                base: cuepool_core::CueBase {
                    qid: Decimal::from(10),
                    ..Default::default()
                },
            },
            Cue::Dummy {
                base: cuepool_core::CueBase {
                    qid: Decimal::from(11),
                    parent: Some(Decimal::from(10)),
                    ..Default::default()
                },
            },
            dummy(20, Go),
        ];
        assert_eq!(next_standby_qid(&grouped, 0), Some(Decimal::from(20)));
    }

    #[test]
    fn goto_chains_resolve_to_the_real_cue() {
        let cues = vec![goto(1, 2), goto(2, 3), dummy(3, TriggerMode::Go)];
        assert_eq!(
            resolve_goto_target(&cues, Decimal::ONE, Decimal::TWO),
            Some(Decimal::from(3))
        );
    }

    #[test]
    fn seeks_rearm_or_restart_a_tail_fade() {
        assert_eq!(
            tail_fade_seek_action(true, 30, 100, 20),
            TailFadeSeekAction::Rearm
        );
        assert_eq!(
            tail_fade_seek_action(true, 80, 100, 20),
            TailFadeSeekAction::Restart
        );
        assert_eq!(
            tail_fade_seek_action(false, 30, 100, 20),
            TailFadeSeekAction::Unchanged
        );
    }

    #[test]
    fn instant_stop_continues_its_after_last_chain() {
        let app = cuepool_gui::CuePoolApp::new();
        {
            let mut state = app.state().lock_unpoisoned();
            state.selected_cue_id = Some(Decimal::ONE);
            state.show_file.cues = vec![
                Cue::Stop {
                    base: cuepool_core::CueBase {
                        qid: Decimal::ONE,
                        ..Default::default()
                    },
                    stop_qid: Decimal::from(99),
                    stop_mode: StopMode::Immediate,
                    fade_out_time: 0.0,
                    fade_type: Default::default(),
                    stop_all: false,
                },
                Cue::Dummy {
                    base: cuepool_core::CueBase {
                        qid: Decimal::TWO,
                        trigger: TriggerMode::AfterLast,
                        ..Default::default()
                    },
                },
            ];
        }
        let mut engine = ShowEngine::new(app.state().clone(), None);
        let started = engine
            .command(EngineCommand::Go, Duration::ZERO)
            .into_iter()
            .filter_map(|action| match action {
                EngineAction::Trace(EngineTrace::CueStarted { qid, .. }) => Some(qid),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(started, vec![Decimal::ONE, Decimal::TWO]);
    }

    /// A gallery show's opening cue: Q1 RESET (stop-all) with the feature
    /// chained `WithLast` behind it. GO must hand that show a *running*
    /// clock — it drives LTC output and timecode triggers — not the dead
    /// one the RESET left behind.
    fn reset_then_feature_app(fade_out_time: f32) -> cuepool_gui::CuePoolApp {
        let app = cuepool_gui::CuePoolApp::new();
        {
            let mut state = app.state().lock_unpoisoned();
            state.selected_cue_id = Some(Decimal::ONE);
            state.show_file.cues = vec![
                Cue::Stop {
                    base: cuepool_core::CueBase {
                        qid: Decimal::ONE,
                        ..Default::default()
                    },
                    stop_qid: Decimal::ZERO,
                    stop_mode: StopMode::Immediate,
                    fade_out_time,
                    fade_type: Default::default(),
                    stop_all: true,
                },
                dummy(2, TriggerMode::WithLast),
            ];
        }
        app
    }

    #[test]
    fn go_through_a_stop_all_reset_still_starts_the_show_clock() {
        let app = reset_then_feature_app(0.0);
        let mut engine = ShowEngine::new(app.state().clone(), None);
        engine.command(EngineCommand::Go, Duration::from_secs(5));
        assert_eq!(
            engine.show_elapsed(),
            Some(Duration::ZERO),
            "the RESET cue killed the clock GO just started"
        );
    }

    /// Same guarantee when the RESET fades instead of cutting: the faded
    /// stop-all resets the clock at fire time, and GO re-arms it after the
    /// chain — the fade landing later must not touch the new clock.
    #[test]
    fn go_through_a_faded_stop_all_reset_still_starts_the_show_clock() {
        let app = reset_then_feature_app(2.0);
        let mut engine = ShowEngine::new(app.state().clone(), None);
        engine.command(EngineCommand::Go, Duration::from_secs(5));
        assert_eq!(engine.show_elapsed(), Some(Duration::ZERO));
    }

    /// Re-running the show without a transport STOP in between (the gallery
    /// loop): the RESET's stop-all wipes the old clock, and GO arms a fresh
    /// one, so the second show's timecode starts from zero too.
    #[test]
    fn go_on_a_stop_all_mid_show_restarts_the_clock_from_zero() {
        let app = reset_then_feature_app(0.0);
        let mut engine = ShowEngine::new(app.state().clone(), None);
        engine.command(EngineCommand::Go, Duration::from_secs(5));
        app.state().lock_unpoisoned().selected_cue_id = Some(Decimal::ONE);
        engine.command(EngineCommand::Go, Duration::from_secs(1200));
        assert_eq!(
            engine.show_elapsed(),
            Some(Duration::ZERO),
            "the second show must not inherit the first show's clock"
        );
    }

    /// Stopping one cue mid-show is surgical (cut a music bed, kill the
    /// pre-show loop) — the show clock, and the LTC everything downstream
    /// chases, must keep running.
    #[test]
    fn a_targeted_stop_does_not_reset_the_show_clock() {
        let state = cuepool_gui::CuePoolApp::new().state().clone();
        let mut engine = ShowEngine::new(state, None);
        engine.show_start = Some(Duration::from_secs(1));
        engine.now = Duration::from_secs(9);
        engine.stop_target(
            Decimal::from(7),
            StopMode::Immediate,
            0.0,
            Default::default(),
        );
        assert_eq!(engine.show_elapsed(), Some(Duration::from_secs(8)));
    }

    /// Stop-all is the "show over" verb — it alone resets the clock.
    #[test]
    fn stop_all_still_resets_the_show_clock() {
        let state = cuepool_gui::CuePoolApp::new().state().clone();
        let mut engine = ShowEngine::new(state, None);
        engine.show_start = Some(Duration::from_secs(1));
        engine.stop_all();
        assert_eq!(engine.show_elapsed(), None);
    }

    #[test]
    fn image_replaces_video_and_remains_active_for_retrigger_guards() {
        let app = cuepool_gui::CuePoolApp::new();
        {
            let mut state = app.state().lock_unpoisoned();
            state.show_file.cues = vec![
                Cue::Video {
                    base: cuepool_core::CueBase {
                        qid: Decimal::ONE,
                        ..Default::default()
                    },
                    path: "missing-video".into(),
                    start_time: Timespan::ZERO,
                    duration: Timespan::ZERO,
                    volume: 1.0,
                    pan: 0.0,
                    fade_in: 0.0,
                    fade_out: 0.0,
                    fade_type: Default::default(),
                    eq: None,
                    routing: Default::default(),
                    follow_mtc: false,
                    mtc_start: Timespan::ZERO,
                },
                Cue::Image {
                    base: cuepool_core::CueBase {
                        qid: Decimal::TWO,
                        retriggerable: false,
                        ..Default::default()
                    },
                    path: "still.png".into(),
                    fit: Default::default(),
                },
            ];
        }
        let mut engine = ShowEngine::new(app.state().clone(), None);
        engine.command(EngineCommand::Fire(Decimal::ONE), Duration::ZERO);
        assert!(engine.snapshot().video.is_some());
        engine.command(EngineCommand::Fire(Decimal::TWO), Duration::ZERO);
        assert!(engine.snapshot().video.is_none());
        assert!(
            engine
                .command(EngineCommand::Fire(Decimal::TWO), Duration::ZERO)
                .is_empty()
        );
    }

    #[test]
    fn audio_backed_video_restarts_only_after_played_loop_boundary() {
        let (mut engine, position, decoder_loops) = looped_video_engine(LoopMode::Looped);

        decoder_loops.store(1, Ordering::Relaxed);
        engine.check_video_loops();
        assert_eq!(play_video_count(&engine.take_actions()), 0);

        position.store(200, Ordering::Relaxed);
        engine.check_video_loops();
        assert_eq!(play_video_count(&engine.take_actions()), 1);
        engine.check_video_loops();
        assert_eq!(play_video_count(&engine.take_actions()), 0);

        position.store(0, Ordering::Relaxed);
        engine.check_video_loops();
        assert_eq!(play_video_count(&engine.take_actions()), 0);
        position.store(200, Ordering::Relaxed);
        engine.check_video_loops();
        assert_eq!(play_video_count(&engine.take_actions()), 1);

        decoder_loops.store(2, Ordering::Relaxed);
        position.store(400, Ordering::Relaxed);
        engine.check_video_loops();
        assert_eq!(play_video_count(&engine.take_actions()), 0);
    }

    #[test]
    fn audio_backed_loop_holds_video_at_eof_until_audio_boundary() {
        let (mut engine, _, _) = looped_video_engine(LoopMode::Looped);
        engine.video_eof(1, 1);
        assert!(engine.current_video.is_some());
        assert!(engine.take_actions().is_empty());

        let (mut video_only, _, _) = looped_video_engine(LoopMode::Looped);
        video_only.current_video.as_mut().unwrap().has_audio = false;
        video_only.video_eof(1, 1);
        assert_eq!(play_video_count(&video_only.take_actions()), 1);

        let (mut one_shot, _, _) = looped_video_engine(LoopMode::OneShot);
        one_shot.video_eof(1, 1);
        assert!(one_shot.current_video.is_none());
        assert!(matches!(
            one_shot.take_actions().as_slice(),
            [EngineAction::StopVideo { .. }]
        ));
    }

    /// Picture and audio lengths differ inside one container, so a one-shot
    /// video cue's AfterLast follow must wait for whichever ends last.
    #[test]
    fn one_shot_video_follow_waits_for_the_later_of_picture_and_audio() {
        fn arm(loop_mode: LoopMode) -> ShowEngine {
            let (mut engine, _, _) = looped_video_engine(loop_mode);
            engine.state.lock_unpoisoned().show_file.cues =
                vec![dummy(1, TriggerMode::Go), dummy(2, TriggerMode::AfterLast)];
            engine.take_actions();
            engine
        }
        fn followed(engine: &mut ShowEngine) -> bool {
            engine.take_actions().iter().any(|action| {
                matches!(action, EngineAction::FireExternal(cue)
                    if cue.base().qid == Decimal::TWO)
            })
        }
        fn finish_audio(engine: &mut ShowEngine) {
            engine.active_cues[0].state = CueState::Done;
            engine.check_finished_cues();
        }

        // Audio ends first (short audio track): hold until picture EOF.
        let mut engine = arm(LoopMode::OneShot);
        finish_audio(&mut engine);
        assert!(
            !followed(&mut engine),
            "followed while the picture was live"
        );
        engine.video_eof(1, 1);
        assert!(followed(&mut engine), "no follow once both streams ended");

        // Picture ends first (short picture): hold until the audio finishes.
        let mut engine = arm(LoopMode::OneShot);
        engine.video_eof(1, 1);
        assert!(!followed(&mut engine), "followed while the audio was live");
        finish_audio(&mut engine);
        assert!(followed(&mut engine), "no follow once both streams ended");

        // A looping picture never ends on its own, so the audio stays the sole
        // authority — deferring there would strand the follow forever.
        let mut engine = arm(LoopMode::Looped);
        finish_audio(&mut engine);
        assert!(
            followed(&mut engine),
            "looped follow waited for a picture end"
        );
        assert_eq!(engine.pending_video_half, None);
    }

    #[test]
    fn decoder_failure_stops_without_restarting_a_loop() {
        let (mut engine, _, _) = looped_video_engine(LoopMode::Looped);
        engine.current_video.as_mut().unwrap().has_audio = false;
        engine.take_actions();

        engine.video_failed(1, 1);

        assert!(engine.current_video.is_none());
        let actions = engine.take_actions();
        assert!(matches!(
            actions.first(),
            Some(EngineAction::StopVideo { .. })
        ));
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, EngineAction::PlayVideo { .. }))
        );
    }

    fn stop_cue(qid: i64, stop_qid: Decimal, stop_all: bool, fade: f32) -> Cue {
        Cue::Stop {
            base: cuepool_core::CueBase {
                qid: Decimal::from(qid),
                ..Default::default()
            },
            stop_qid,
            stop_mode: StopMode::Immediate,
            fade_out_time: fade,
            fade_type: Default::default(),
            stop_all,
        }
    }

    fn active_audio(instance_id: u64, qid: Decimal) -> (ActiveCue, Arc<cuepool_audio::MixerInput>) {
        let input = Arc::new(cuepool_audio::MixerInput::new(
            Box::new(PositionSource {
                position: Arc::new(AtomicUsize::new(0)),
            }),
            2,
        ));
        let cue = ActiveCue {
            instance_id,
            qid,
            name: format!("audio {qid}"),
            input: Arc::clone(&input),
            state: CueState::Playing,
            loop_counter: None,
            video_loop_count: 0,
            video_loop_limit: None,
            loop_start_frame: 0,
            loop_end_frame: 0,
            fade_out: 0.0,
            fade_type: Default::default(),
            fade_out_started: false,
            pending_stop: None,
            follow_after_last: true,
        };
        (cue, input)
    }

    #[test]
    fn a_stop_all_cue_with_a_fade_fades_instead_of_cutting() {
        let (mut engine, _, _) = looped_video_engine(LoopMode::OneShot);
        engine.state.lock_unpoisoned().show_file.cues = vec![];
        engine.delayed_cues.push(DelayedCue {
            cue: dummy(7, TriggerMode::Go),
            start_at: Duration::from_secs(5),
        });
        let input = Arc::clone(&engine.active_cues[0].input);
        engine.take_actions();

        engine.play_cue(stop_cue(99, Decimal::ZERO, true, 2.0));

        assert!(input.is_fading(), "the running cue should fade, not cut");
        assert!(input.is_active(), "a fading input must keep rendering");
        assert_eq!(
            engine.active_cues.len(),
            1,
            "the cue is reaped by its fade completing, not cleared"
        );
        assert!(!engine.active_cues[0].follow_after_last);
        assert!(engine.current_video.is_none());
        assert!(
            engine.delayed_cues.is_empty(),
            "a delayed cue must not start after a stop all"
        );
        assert!(
            engine.take_actions().iter().any(|action| matches!(
                action,
                EngineAction::StopVideo { fade_out_secs } if *fade_out_secs == 2.0
            )),
            "the video fades over the cue's fade time"
        );
    }

    #[test]
    fn a_faded_stop_all_does_not_chain_after_last() {
        let (mut engine, _, _) = looped_video_engine(LoopMode::OneShot);
        engine.state.lock_unpoisoned().show_file.cues =
            vec![dummy(1, TriggerMode::Go), dummy(2, TriggerMode::AfterLast)];
        engine.stop_all_faded(2.0, Default::default());
        engine.take_actions();

        // The fade lands: the input finishes and the reaper collects it.
        engine.active_cues[0].state = CueState::Done;
        engine.check_finished_cues();

        let actions = engine.take_actions();
        assert!(actions.iter().any(|action| matches!(
            action,
            EngineAction::Trace(EngineTrace::CueFinished { qid }) if *qid == Decimal::ONE
        )));
        assert!(
            !actions.iter().any(|action| matches!(
                action,
                EngineAction::FireExternal(cue) if cue.base().qid == Decimal::TWO
            )),
            "a stop-everything must not start the next cue"
        );
    }

    #[test]
    fn the_transport_stop_remains_an_instant_cut() {
        let (mut engine, _, _) = looped_video_engine(LoopMode::OneShot);
        engine.take_actions();
        let actions = engine.command(EngineCommand::Stop, Duration::ZERO);
        assert!(
            engine.active_cues.is_empty(),
            "the panic stop clears, never fades"
        );
        assert!(actions.iter().any(|action| matches!(
            action,
            EngineAction::StopVideo { fade_out_secs } if *fade_out_secs == 0.0
        )));
    }

    #[test]
    fn stopping_a_group_stops_its_members_with_the_fade() {
        let group = |qid: Decimal, parent: Option<Decimal>| Cue::Group {
            base: cuepool_core::CueBase {
                qid,
                parent,
                ..Default::default()
            },
        };
        let member = |qid: Decimal, parent: Decimal| Cue::Dummy {
            base: cuepool_core::CueBase {
                qid,
                parent: Some(parent),
                ..Default::default()
            },
        };
        let q = |units: i64, scale: u32| Decimal::new(units, scale);

        let app = cuepool_gui::CuePoolApp::new();
        let mut engine = ShowEngine::new(app.state().clone(), None);
        {
            let mut state = engine.state.lock_unpoisoned();
            state.show_file.cues = vec![
                group(Decimal::ONE, None),
                member(q(11, 1), Decimal::ONE), // 1.1 — playing, with the video
                group(q(12, 1), Some(Decimal::ONE)), // 1.2 — nested group
                member(q(121, 2), q(12, 1)),    // 1.21 — nested, playing
                member(q(13, 1), Decimal::ONE), // 1.3 — waiting on its delay
                member(Decimal::from(9), Decimal::TWO), // another group's member
            ];
        }
        let (direct, direct_input) = active_audio(1, q(11, 1));
        let (nested, nested_input) = active_audio(2, q(121, 2));
        engine.active_cues.push(direct);
        engine.active_cues.push(nested);
        engine.current_video = Some(CurrentVideo {
            qid: q(11, 1),
            instance_id: 1,
            epoch: 1,
            path: "member.mov".into(),
            start_time: Timespan::ZERO,
            duration: Timespan::ZERO,
            loop_mode: LoopMode::OneShot,
            follow_mtc: false,
            mtc_start: Timespan::ZERO,
            has_audio: true,
            clock_origin: Duration::ZERO,
            paused_position: None,
        });
        engine.delayed_cues.push(DelayedCue {
            cue: member(q(13, 1), Decimal::ONE),
            start_at: Duration::from_secs(5),
        });
        engine.delayed_cues.push(DelayedCue {
            cue: dummy(9, TriggerMode::Go),
            start_at: Duration::from_secs(5),
        });

        engine.play_cue(stop_cue(99, Decimal::ONE, false, 2.0));

        assert!(direct_input.is_fading(), "a direct member fades");
        assert!(nested_input.is_fading(), "a nested member fades");
        assert!(engine.current_video.is_none());
        let delayed = engine
            .delayed_cues
            .iter()
            .map(|delayed| delayed.cue.base().qid)
            .collect::<Vec<_>>();
        assert_eq!(
            delayed,
            vec![Decimal::from(9)],
            "only the group's own delayed member is cancelled"
        );
        let actions = engine.take_actions();
        assert!(actions.iter().any(|action| matches!(
            action,
            EngineAction::StopVideo { fade_out_secs } if *fade_out_secs == 2.0
        )));
        let stopped = actions
            .iter()
            .filter_map(|action| match action {
                EngineAction::StopExternal { qid, .. } => Some(*qid),
                _ => None,
            })
            .collect::<Vec<_>>();
        for expected in [Decimal::ONE, q(11, 1), q(12, 1), q(121, 2), q(13, 1)] {
            assert!(
                stopped.contains(&expected),
                "expected a stop for Q{expected}"
            );
        }
        assert!(
            !stopped.contains(&Decimal::from(9)),
            "another group's member is untouched"
        );
    }
}
