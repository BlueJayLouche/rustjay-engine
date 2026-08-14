use crate::clock::VirtualClock;
use crate::sink::NullSink;
use anyhow::{Context, Result};
use cuepool::{EngineAction, EngineCommand, EngineEvent, EngineSnapshot, EngineTrace, ShowEngine};
use cuepool_audio::AudioEngine;
use cuepool_core::{Cue, LockExt, ShowFile};
use cuepool_video::{VideoFrame, VideoSource};
use rust_decimal::Decimal;
use std::fs;
use std::path::Path;
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const DEFAULT_BLOCK_FRAMES: usize = 480;

#[derive(Debug, Clone, PartialEq)]
pub enum RunnerTrace {
    Engine(EngineTrace),
    VideoFrame {
        qid: Decimal,
        instance_id: u64,
        epoch: u64,
        pts: f64,
    },
    VideoEof {
        qid: Decimal,
        instance_id: u64,
        epoch: u64,
    },
    VideoSeek {
        qid: Decimal,
        instance_id: u64,
        target_secs: f64,
        paused: bool,
    },
    SideEffect {
        qid: Option<Decimal>,
        kind: &'static str,
    },
    RemoteGo {
        node: String,
        qid: Decimal,
    },
}

struct HeadlessVideo {
    qid: Decimal,
    instance_id: u64,
    epoch: u64,
    path: String,
    source: VideoSource,
    pending: Option<VideoFrame>,
    eof: bool,
    media_offset_secs: f64,
    clock_origin: Duration,
    paused_position: Option<f64>,
}

impl HeadlessVideo {
    fn open(
        qid: Decimal,
        instance_id: u64,
        epoch: u64,
        path: String,
        media_offset_secs: f64,
        now: Duration,
        paused: bool,
    ) -> Result<Self> {
        let mut source = VideoSource::open(&path)
            .with_context(|| format!("failed to open video '{}' for Q{qid}", path))?;
        if media_offset_secs > 0.0 {
            source
                .seek_before(media_offset_secs)
                .with_context(|| format!("failed to seek video '{}' for Q{qid}", path))?;
        }
        Ok(Self {
            qid,
            instance_id,
            epoch,
            path,
            source,
            pending: None,
            eof: false,
            media_offset_secs,
            clock_origin: now,
            paused_position: paused.then_some(0.0),
        })
    }

    fn position(&self, now: Duration) -> f64 {
        self.paused_position
            .unwrap_or_else(|| now.saturating_sub(self.clock_origin).as_secs_f64())
    }

    fn set_paused(&mut self, paused: bool, now: Duration) {
        match (paused, self.paused_position) {
            (true, None) => self.paused_position = Some(self.position(now)),
            (false, Some(position)) => {
                self.clock_origin = now.saturating_sub(Duration::from_secs_f64(position));
                self.paused_position = None;
            }
            _ => {}
        }
    }

    fn seek(&mut self, target_secs: f64, media_offset_secs: f64, now: Duration) -> Result<()> {
        self.source
            .seek_before(target_secs + media_offset_secs)
            .with_context(|| format!("failed to seek video '{}' for Q{}", self.path, self.qid))?;
        self.pending = None;
        self.eof = false;
        self.media_offset_secs = media_offset_secs;
        if self.paused_position.is_some() {
            self.paused_position = Some(target_secs);
        } else {
            self.clock_origin = now.saturating_sub(Duration::from_secs_f64(target_secs));
        }
        Ok(())
    }
}

/// Deterministic, device-free adapter around the same [`ShowEngine`] used by
/// CuePool's winit application.
pub struct HeadlessShowRunner {
    engine: ShowEngine,
    sink: NullSink,
    clock: VirtualClock,
    video: Option<HeadlessVideo>,
    trace: Vec<RunnerTrace>,
}

impl HeadlessShowRunner {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_block_frames(path, DEFAULT_BLOCK_FRAMES)
    }

    pub fn open_with_block_frames(path: impl AsRef<Path>, block_frames: usize) -> Result<Self> {
        let path = path.as_ref();
        let show = load_project(path)?;
        let audio = AudioEngine::new_headless(CHANNELS, SAMPLE_RATE);
        let sink = NullSink::new(std::sync::Arc::clone(audio.mixer()), block_frames);
        Ok(Self {
            engine: ShowEngine::from_show_file(show, Some(path.to_path_buf()), Some(audio)),
            sink,
            clock: VirtualClock::new(SAMPLE_RATE, block_frames),
            video: None,
            trace: Vec::new(),
        })
    }

    pub fn replace_project(&mut self, path: impl AsRef<Path>) -> Result<()> {
        let path = path.as_ref();
        let show = load_project(path)?;
        let actions = self.engine.reset_for_project_change();
        self.apply_actions(actions)?;
        {
            let mut state = self.engine.state().lock_unpoisoned();
            state.show_file = show;
            state.project_path = Some(path.to_path_buf());
            state.selected_cue_id = None;
            state.project_generation = state.project_generation.wrapping_add(1);
        }
        self.video = None;
        Ok(())
    }

    pub fn select(&mut self, qid: Decimal) -> Result<()> {
        self.command(EngineCommand::Select(qid))
    }

    pub fn go(&mut self) -> Result<()> {
        self.command(EngineCommand::Go)
    }

    pub fn pause(&mut self) -> Result<()> {
        self.command(EngineCommand::Pause)
    }

    pub fn resume(&mut self) -> Result<()> {
        self.command(EngineCommand::Resume)
    }

    pub fn stop(&mut self) -> Result<()> {
        self.command(EngineCommand::Stop)
    }

    pub fn seek(&mut self, instance_id: u64, secs: f32) -> Result<()> {
        self.command(EngineCommand::Seek { instance_id, secs })
    }

    pub fn advance_blocks(&mut self, blocks: usize) -> Result<()> {
        for _ in 0..blocks {
            self.sink.render_block();
            self.clock.advance(1);
            self.consume_due_video()?;
            let actions = self.engine.tick(self.clock.elapsed());
            self.apply_actions(actions)?;
            std::thread::yield_now();
        }
        Ok(())
    }

    pub fn snapshot(&self) -> EngineSnapshot {
        self.engine.snapshot()
    }

    pub fn take_trace(&mut self) -> Vec<RunnerTrace> {
        std::mem::take(&mut self.trace)
    }

    pub fn elapsed(&self) -> Duration {
        self.clock.elapsed()
    }

    fn command(&mut self, command: EngineCommand) -> Result<()> {
        let actions = self.engine.command(command, self.clock.elapsed());
        self.apply_actions(actions)?;
        let actions = self.engine.tick(self.clock.elapsed());
        self.apply_actions(actions)
    }

    fn apply_actions(&mut self, actions: Vec<EngineAction>) -> Result<()> {
        for action in actions {
            match action {
                EngineAction::PlayVideo {
                    qid,
                    instance_id,
                    epoch,
                    path,
                    start_time,
                    ..
                } => {
                    self.video = Some(HeadlessVideo::open(
                        qid,
                        instance_id,
                        epoch,
                        path,
                        start_time.as_secs_f64(),
                        self.clock.elapsed(),
                        self.engine.is_paused(),
                    )?);
                }
                EngineAction::SeekVideo {
                    qid,
                    instance_id,
                    target_secs,
                    media_offset_secs,
                    paused,
                    ..
                } => {
                    self.trace.push(RunnerTrace::VideoSeek {
                        qid,
                        instance_id,
                        target_secs,
                        paused,
                    });
                    if let Some(video) = self
                        .video
                        .as_mut()
                        .filter(|video| video.instance_id == instance_id)
                    {
                        video.set_paused(paused, self.clock.elapsed());
                        video.seek(target_secs, media_offset_secs, self.clock.elapsed())?;
                    }
                }
                EngineAction::StopVideo { .. } => self.video = None,
                EngineAction::SetVideoPaused(paused) => {
                    if let Some(video) = &mut self.video {
                        video.set_paused(paused, self.clock.elapsed());
                    }
                }
                EngineAction::FireExternal(cue) => {
                    self.trace.push(RunnerTrace::SideEffect {
                        qid: Some(cue.base().qid),
                        kind: cue_kind(&cue),
                    });
                }
                EngineAction::StopExternal { qid, .. } => {
                    self.trace.push(RunnerTrace::SideEffect {
                        qid: Some(qid),
                        kind: "stop",
                    });
                }
                EngineAction::StopAllExternal => {
                    self.trace.push(RunnerTrace::SideEffect {
                        qid: None,
                        kind: "stop-all",
                    });
                }
                EngineAction::RemoteGo { node, qid } => {
                    self.trace.push(RunnerTrace::RemoteGo { node, qid });
                }
                EngineAction::Trace(event) => self.trace.push(RunnerTrace::Engine(event)),
            }
        }
        Ok(())
    }

    fn consume_due_video(&mut self) -> Result<()> {
        let now = self.clock.elapsed();
        let Some(video) = &mut self.video else {
            return Ok(());
        };
        if video.paused_position.is_some() {
            return Ok(());
        }
        if video.eof {
            return Ok(());
        }
        let due_media = video.position(now) + video.media_offset_secs;
        let mut newest = None;
        loop {
            if video.pending.is_none() {
                video.pending = video.source.read_frame();
                if video.pending.is_none() {
                    let (qid, instance_id, epoch) = (video.qid, video.instance_id, video.epoch);
                    video.eof = true;
                    self.trace.push(RunnerTrace::VideoEof {
                        qid,
                        instance_id,
                        epoch,
                    });
                    let actions = self
                        .engine
                        .event(EngineEvent::VideoEof { instance_id, epoch }, now);
                    self.apply_actions(actions)?;
                    break;
                }
            }
            if video
                .pending
                .as_ref()
                .is_some_and(|frame| frame.pts <= due_media + f64::EPSILON)
            {
                newest = video.pending.take();
                continue;
            }
            break;
        }
        if let (Some(video), Some(frame)) = (&self.video, newest) {
            self.trace.push(RunnerTrace::VideoFrame {
                qid: video.qid,
                instance_id: video.instance_id,
                epoch: video.epoch,
                pts: frame.pts,
            });
        }
        Ok(())
    }
}

fn load_project(path: &Path) -> Result<ShowFile> {
    let data = fs::read_to_string(path)
        .with_context(|| format!("failed to read CuePool project '{}'", path.display()))?;
    serde_json::from_str(&data)
        .with_context(|| format!("failed to parse CuePool project '{}'", path.display()))
}

fn cue_kind(cue: &Cue) -> &'static str {
    match cue {
        Cue::Group { .. } => "group",
        Cue::Dummy { .. } => "dummy",
        Cue::Sound { .. } => "sound",
        Cue::TimeCode { .. } => "timecode",
        Cue::Stop { .. } => "stop",
        Cue::Volume { .. } => "volume",
        Cue::Video { .. } => "video",
        Cue::Osc { .. } => "network",
        Cue::Text { .. } => "text",
        Cue::Image { .. } => "image",
        Cue::Goto { .. } => "goto",
        Cue::PixelMap { .. } => "pixel-map",
        Cue::Lighting { .. } => "lighting",
        Cue::DmxShow { .. } => "dmx",
    }
}
