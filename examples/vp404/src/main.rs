//! VP-404 — SP-404-style video sampler, ported onto rustjay-engine.
//!
//! **Phases 1d–2:** 16 pads with in/out points + Free/Synced tempo playback are
//! composited through `rustjay-mixer`. Live sampling (`capture` feature) records
//! from a `rustjay-io` input into a HAP5 clip and assigns it to a pad. A
//! polyphonic step sequencer (slaved to the engine beat clock) triggers pads
//! via `PadCmd`. See `404_PORT.md`.

mod bank;
mod grid_tab;
#[cfg(feature = "capture")]
mod live_sampler;
mod output_tab;
mod pad;
mod pad_channel;
mod sample;
mod sequencer;
mod sequencer_tab;
mod api_state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bank::{Bank, BankHandle, PadCmd, PadInfo, PAD_COUNT};
use grid_tab::PadGridTab;
use hap_wgpu::QtHapReader;
use output_tab::OutputTab;
use pad::PlaybackMode;
use pad::TriggerMode;
use pad_channel::PadChannel;
use rustjay_core::{EffectInstance, RenderCtx, RenderTarget};
use rustjay_engine::prelude::*;
use rustjay_mixer::{Channel, Mixer};
use sequencer_tab::SequencerTab;

/// Default test clip (override with argv[1] or VP404_CLIP).
const DEFAULT_CLIP: &str =
    "/Users/ac/developer/rust/rustjay-404/samples/Screen Recording 2026-05-07 at 20.42.17_converted.hap.mov";

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Vp404State {
    /// Polyphonic pad sequencer state (patterns + current pattern).
    #[serde(default)]
    pub sequencer: sequencer::SequencerEngine,
}

/// Cached engine parameter keys for one pad.
struct PadParamKeys {
    speed: String,
    mode: String,
    division: String,
}

struct Vp404 {
    clip_path: PathBuf,
    bank: Arc<Mutex<Bank>>,
    handle: BankHandle,
    mixer: Arc<Mutex<Mixer>>,
    /// Cached parameter keys for each pad.
    pad_param_keys: Vec<PadParamKeys>,
    /// Previous `pad<i>_trig` param values — used for edge detection in prepare().
    prev_trig: Vec<f32>,
    /// Previous `in_point`/`out_point` knob values — used to detect a knob move
    /// so the SP-404 trim only re-ranges the last pad when actually adjusted.
    prev_in: f32,
    prev_out: f32,
    /// MIDI step-write cursor: the step position where the next stopped-mode
    /// pad trigger will be recorded. Wraps at `pattern.length()`.
    edit_step: usize,
    /// When true, pad triggers while the sequencer is stopped write steps
    /// instead of (or in addition to) triggering playback. Off by default so
    /// normal pad triggering works without accidentally entering record mode.
    record_mode: bool,
    /// Total elapsed beats from the engine tempo clock, used for synced pads.
    accumulated_beats: f32,
    last_tick: Instant,
    /// Live sampler (capture → HAP5 → pad), only present when `capture` is enabled.
    #[cfg(feature = "capture")]
    live_sampler: Option<std::sync::Mutex<live_sampler::LiveSampler>>,
}

impl Vp404 {
    fn new(clip_path: PathBuf, handle: BankHandle) -> Self {
        let bank = Arc::new(Mutex::new(Bank::new(PAD_COUNT)));
        let mut mixer = Mixer::new();
        let mut pad_param_keys = Vec::with_capacity(PAD_COUNT);

        for i in 0..PAD_COUNT {
            let uuid = format!("pad{i}");
            let channel = Channel::new(
                uuid.clone(),
                format!("Pad {}", i + 1),
                Box::new(PadChannel::new(bank.clone(), i)),
            );
            pad_param_keys.push(PadParamKeys {
                speed: format!("ch_{uuid}_speed"),
                mode: format!("ch_{uuid}_mode"),
                division: format!("ch_{uuid}_division"),
            });
            // Channel opacity becomes the pad opacity; blend defaults to alpha-over.
            if let Err(e) = mixer.add_channel(channel) {
                log::warn!("VP-404: failed to add channel for pad {i}: {e}");
            }
        }

        Self {
            clip_path,
            bank,
            handle,
            mixer: Arc::new(Mutex::new(mixer)),
            pad_param_keys,
            prev_trig: vec![0.0; PAD_COUNT],
            prev_in: 0.0,
            prev_out: 1.0,
            edit_step: 0,
            record_mode: false,
            accumulated_beats: 0.0,
            last_tick: Instant::now(),
            #[cfg(feature = "capture")]
            live_sampler: None,
        }
    }
}

impl EffectPlugin for Vp404 {
    type State = Vp404State;
    type Uniforms = [f32; 4];

    fn app_name(&self) -> &str {
        "VP-404"
    }

    fn input_count(&self) -> u32 {
        // 1 so the engine's active input (webcam/Syphon/NDI/…) is available
        // as ctx.input in render() for live sampling. Vp404 still renders its
        // own pads (render() returns true), so the input is not displayed.
        1
    }

    fn shader_source(&self) -> &'static str {
        include_str!("passthrough.wgsl") // compiled but unused — render() overrides
    }

    fn build_uniforms(&self, _state: &Self::State, _engine: &EngineState) -> Self::Uniforms {
        [0.0; 4]
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let mut params: Vec<ParameterDescriptor> = (0..PAD_COUNT)
            .map(|i| {
                ParameterDescriptor::float(
                    &format!("pad{i}_trig"),
                    &format!("Pad {} Trigger", i + 1),
                    ParamCategory::Custom("Pads".into()),
                    0.0,
                    1.0,
                    0.0,
                    0.01,
                )
            })
            .collect();
        // Global SP-404 trim knobs — adjust the last-pressed pad's play range.
        params.push(ParameterDescriptor::float(
            "in_point",
            "Start (last pad)",
            ParamCategory::Custom("Pad".into()),
            0.0,
            1.0,
            0.0,
            0.001,
        ));
        params.push(ParameterDescriptor::float(
            "out_point",
            "End (last pad)",
            ParamCategory::Custom("Pad".into()),
            0.0,
            1.0,
            1.0,
            0.001,
        ));
        let mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        params.extend(mixer.parameters());
        params
    }

    #[allow(unused_variables)]
    fn on_engine_ready(&mut self, engine: &mut EngineState) {
        #[cfg(feature = "api")]
        {
            engine.app_ui_html =
                Some(std::sync::Arc::new(include_str!("pad_grid.html").to_string()));
            log::info!("VP-404: pad-grid UI registered at /api/app/ui");
        }
    }

    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        // Build the immutable convert-pass resources once and share them across
        // all 16 PadChannels (each pad keeps its own params uniform buffer).
        let shared = Arc::new(pad_channel::ConvertGpuShared::new(device));

        #[cfg(feature = "capture")]
        {
            self.live_sampler = Some(std::sync::Mutex::new(live_sampler::LiveSampler::new(
                Arc::new(device.clone()),
                Arc::new(_queue.clone()),
            )));
        }
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        for ch in &mut mixer.channels {
            if let Some(pad_ch) = ch
                .effect
                .as_any_mut()
                .and_then(|a| a.downcast_mut::<PadChannel>())
            {
                pad_ch.set_shared_gpu(shared.clone());
            }
        }
        drop(mixer);

        // Seed pad 0 with the launch clip so something plays immediately.
        match sample::Sample::open(&self.clip_path) {
            Ok(s) => {
                log::info!(
                    "VP-404 pad 0 ← '{}' {}x{}, {} frames @ {} fps, {:?}",
                    s.name,
                    s.dims.0,
                    s.dims.1,
                    s.frame_count,
                    s.fps,
                    s.format
                );
                let mut bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
                let pad = &mut bank.pads[0];
                pad.assign_sample(s);
                pad.trigger_mode = TriggerMode::Gate;
                pad.loop_enabled = true;
                pad.trigger();
                bank.last_triggered = Some(0);
            }
            Err(e) => log::error!("VP-404: cannot open {}: {e}", self.clip_path.display()),
        }
        self.last_tick = Instant::now();
    }

    fn prepare(
        &mut self,
        state: &mut Self::State,
        engine: &EngineState,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) {
        // Shift+Space: reset master beat clock, sequencer, and all synced pads.
        if engine.shift_space_pressed {
            self.accumulated_beats = 0.0;
            state.sequencer.reset_with_clock(0.0);
            let mut bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
            for pad in &mut bank.pads {
                if pad.playback_mode == PlaybackMode::Synced {
                    pad.current_frame = pad.sample
                        .as_ref()
                        .map(|s| s.in_point as f32)
                        .unwrap_or(0.0);
                }
            }
            log::info!("VP-404: phase reset");
        }

        // Space: play/pause the sequencer. Consumed HERE (not in the tab's draw)
        // because the engine clears `space_pressed` right after `prepare()`; the
        // control-window tab draws in a separate pass and would race that clear,
        // making Space intermittently miss. The on-screen Play button is unaffected.
        if engine.space_pressed {
            state.sequencer.toggle_playback();
        }

        // 1. Drain UI commands (Load does not need GPU resources; decode is deferred
        //    to PadChannel::render_to).
        let cmds = self
            .handle
            .cmds
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        {
            let mut bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
            for cmd in cmds {
                match cmd {
                    PadCmd::Load(i, path) => {
                        if let Some(pad) = bank.pads.get_mut(i) {
                            match sample::Sample::open(&path) {
                                Ok(s) => {
                                    log::info!("VP-404 pad {i} ← '{}'", s.name);
                                    pad.assign_sample(s);
                                }
                                Err(e) => log::error!("VP-404: load pad {i}: {e}"),
                            }
                        }
                    }
                    PadCmd::Trigger(i) => {
                        if let Some(p) = bank.pads.get_mut(i) {
                            p.trigger();
                            bank.last_triggered = Some(i);
                        }
                    }
                    PadCmd::Release(i) => {
                        if let Some(p) = bank.pads.get_mut(i) {
                            p.release();
                        }
                    }
                    PadCmd::Clear(i) => {
                        if let Some(p) = bank.pads.get_mut(i) {
                            p.clear();
                        }
                    }
                    PadCmd::SetMode(i, m) => {
                        if let Some(p) = bank.pads.get_mut(i) {
                            p.trigger_mode = m;
                        }
                    }
                    PadCmd::SetRange(i, in_pt, out_pt) => {
                        if let Some(pad) = bank.pads.get_mut(i) {
                            if let Some(sample) = pad.sample.as_mut() {
                                sample.set_range(in_pt, out_pt);
                            }
                        }
                    }
                    #[cfg(feature = "capture")]
                    PadCmd::StartSampling(i, frame_count) => {
                        if let Some(sampler) = self.live_sampler.as_mut() {
                            let sampler = sampler.get_mut().unwrap_or_else(|e| e.into_inner());
                            if let Err(e) = sampler.start_recording(
                                i,
                                frame_count,
                                engine.input.width,
                                engine.input.height,
                                engine.input.fps,
                            ) {
                                log::error!("VP-404 start sampling: {e}");
                            }
                        }
                    }
                    #[cfg(feature = "capture")]
                    PadCmd::StopSampling => {
                        if let Some(sampler) = self.live_sampler.as_mut() {
                            let sampler = sampler.get_mut().unwrap_or_else(|e| e.into_inner());
                            sampler.cancel();
                        }
                    }
                }
            }

            #[cfg(feature = "capture")]
            {
                if let Some(sampler) = self.live_sampler.as_mut() {
                    let sampler = sampler.get_mut().unwrap_or_else(|e| e.into_inner());
                    // Collect any completed GPU→CPU readback submitted last frame.
                    sampler.poll_readback();
                    let status = match sampler.state() {
                        live_sampler::SamplerState::Idle => bank::SamplerStatus::Idle,
                        live_sampler::SamplerState::Recording => bank::SamplerStatus::Recording,
                        live_sampler::SamplerState::Encoding => bank::SamplerStatus::Encoding,
                        live_sampler::SamplerState::Error => bank::SamplerStatus::Error,
                    };
                    self.handle.set_sampler_status(status);

                    if let Some((pad_index, path)) = sampler.update() {
                        match sample::Sample::open(&path) {
                            Ok(s) => {
                                log::info!(
                                    "VP-404 pad {pad_index} ← live sample '{}' ({} frames)",
                                    s.name,
                                    s.frame_count
                                );
                                if let Some(pad) = bank.pads.get_mut(pad_index) {
                                    pad.assign_sample(s);
                                    pad.loop_enabled = true;
                                    pad.trigger();
                                    bank.last_triggered = Some(pad_index);
                                }
                            }
                            Err(e) => {
                                log::error!("VP-404: load live sample {}: {e}", path.display())
                            }
                        }
                    }
                }
            }

            // 2a. Drain sequencer commands from `POST /api/app/command` (api feature).
            #[cfg(feature = "api")]
            {
                let cmds: Vec<serde_json::Value> = engine
                    .app_command_queue
                    .lock()
                    .map(|mut g| std::mem::take(&mut *g))
                    .unwrap_or_default();
                for v in cmds {
                    if let Ok(cmd) = serde_json::from_value::<api_state::SeqCmd>(v) {
                        // SetRecord touches plugin state, not the sequencer.
                        if let api_state::SeqCmd::SetRecord { enabled } = cmd {
                            self.record_mode = enabled;
                        } else {
                            cmd.apply(&mut state.sequencer, &mut self.edit_step);
                        }
                    }
                }
            }

            // 2b. Edge-detect pad trig params — MIDI Note-On/Off, OSC, and web all
            // set `pad<i>_trig`; rising edge fires trigger (or step-write when
            // sequencer is stopped), falling fires release.
            for i in 0..bank.pads.len() {
                let val = engine
                    .get_param_base(&format!("pad{i}_trig"))
                    .unwrap_or(0.0);
                let prev = self.prev_trig.get(i).copied().unwrap_or(0.0);
                match trig_edge(val, prev) {
                    Some(true) => {
                        if self.record_mode && !state.sequencer.is_playing {
                            // Step-write: record track i at cursor, then audition.
                            self.edit_step =
                                api_state::step_write(&mut state.sequencer, i, self.edit_step);
                        }
                        bank.pads[i].trigger();
                        bank.last_triggered = Some(i);
                    }
                    Some(false) => bank.pads[i].release(),
                    None => {}
                }
                if let Some(p) = self.prev_trig.get_mut(i) {
                    *p = val;
                }
            }

            // 2c. SP-404-style start/end trim: the global `in_point`/`out_point`
            // knobs adjust the *last-pressed* pad. Applied only when a knob
            // actually moves (MIDI/OSC/LFO/UI) so idle pads keep their own trim.
            {
                let in_v = engine.get_param("in_point").unwrap_or(0.0).clamp(0.0, 1.0);
                let out_v = engine.get_param("out_point").unwrap_or(1.0).clamp(0.0, 1.0);
                let moved =
                    (in_v - self.prev_in).abs() > 1e-4 || (out_v - self.prev_out).abs() > 1e-4;
                self.prev_in = in_v;
                self.prev_out = out_v;
                if moved {
                    if let Some(pad) = bank.last_triggered.and_then(|i| bank.pads.get_mut(i)) {
                        if let Some(sample) = pad.sample.as_mut() {
                            let last = sample.frame_count.saturating_sub(1) as f32;
                            sample.set_range(
                                (in_v * last).round() as u32,
                                (out_v * last).round() as u32,
                            );
                        }
                    }
                }
            }

            // 3. Apply per-pad engine params (MIDI/OSC/LFO reach these) and sync
            // the mixer channel's `active` flag to the pad's playing state so
            // idle channels are elided (no render pass, no composite step).
            {
                let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
                for (i, ch) in mixer.channels.iter_mut().enumerate() {
                    ch.active = bank.pads.get(i).is_some_and(|p| p.is_playing);
                }
            }
            for (i, keys) in self.pad_param_keys.iter().enumerate() {
                if let Some(pad) = bank.pads.get_mut(i) {
                    pad.speed = engine
                        .get_param(&keys.speed)
                        .unwrap_or(pad.speed)
                        .clamp(-5.0, 5.0);
                    // Discrete playback settings use the base value so LFO/audio
                    // modulation doesn't accidentally snap mode/division.
                    pad.playback_mode = engine
                        .get_param_base(&keys.mode)
                        .map(|v| PlaybackMode::from_index(v as usize))
                        .unwrap_or(pad.playback_mode);
                    pad.beat_division = engine
                        .get_param_base(&keys.division)
                        .map(|v| v as usize)
                        .unwrap_or(pad.beat_division)
                        .clamp(0, 7);
                }
            }

            // 4. Advance the global beat clock and all pads.
            let now = Instant::now();
            let dt = now - self.last_tick;
            self.last_tick = now;
            let bpm = engine.effective_bpm();
            let bpm = if bpm > 0.0 { bpm } else { 120.0 };
            self.accumulated_beats += bpm / 60.0 * dt.as_secs_f32();
            for p in &mut bank.pads {
                p.update(dt, self.accumulated_beats);
            }

            // 5. Tick the pad sequencer from the same master clock.
            state.sequencer.tick(self.accumulated_beats, &self.handle);
        }
    }

    #[allow(unused_variables)]
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, state: &mut Self::State) -> bool {
        // 1. Publish pad state for the grid tab.
        if let Ok(mut roster) = self.handle.roster.lock() {
            let bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
            roster.clear();
            roster.extend(bank.pads.iter().map(|p| PadInfo {
                name: p.name.clone(),
                color: p.color,
                loaded: p.has_sample(),
                playing: p.is_playing,
                progress: p.progress(),
                trigger_mode: p.trigger_mode,
                beat_division: p.beat_division,
                in_point: p.sample.as_ref().map(|s| s.in_point).unwrap_or(0),
                out_point: p.sample.as_ref().map(|s| s.out_point).unwrap_or(0),
                frame_count: p.sample.as_ref().map(|s| s.frame_count).unwrap_or(0),
            }));
        }

        // 2. Publish snapshot for GET /api/app/state each frame (api feature only).
        #[cfg(feature = "api")]
        {
            let bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
            let snapshot =
                api_state::build_snapshot(&bank, &state.sequencer, self.edit_step, self.record_mode);
            drop(bank);
            if let Ok(json) = serde_json::to_value(&snapshot) {
                if let Ok(mut guard) = ctx.engine_state.app_state.lock() {
                    *guard = Some(json);
                }
            }
        }

        // 3. Submit GPU→CPU readback of the engine's input for live sampling.
        #[cfg(feature = "capture")]
        if let Some(sampler) = self.live_sampler.as_mut() {
            let sampler = sampler.get_mut().unwrap_or_else(|e| e.into_inner());
            if sampler.state() == live_sampler::SamplerState::Recording {
                if let Some(texture) = ctx.input.as_ref().and_then(|i| i.texture) {
                    sampler.submit_readback(texture);
                }
            }
        }

        // 3. Composite all playing pads through the mixer.
        let size = [
            ctx.engine_state.resolution.internal_width,
            ctx.engine_state.resolution.internal_height,
        ];
        let mut render_ctx = RenderCtx {
            device: ctx.device,
            queue: ctx.queue,
            encoder: ctx.encoder,
            vertex_buffer: ctx.vertex_buffer,
        };
        let target = RenderTarget {
            view: ctx.target_view,
            size,
        };
        let mut mixer = self.mixer.lock().unwrap_or_else(|e| e.into_inner());
        mixer.render_to(&mut render_ctx, &[], target, ctx.engine_state);

        true
    }
}

/// Rising → `Some(true)`, falling → `Some(false)`, no edge → `None`.
fn trig_edge(val: f32, prev: f32) -> Option<bool> {
    if val > 0.5 && prev <= 0.5 {
        Some(true)
    } else if val <= 0.5 && prev > 0.5 {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::trig_edge;

    #[test]
    fn edge_rising_above_threshold() {
        assert_eq!(trig_edge(1.0, 0.0), Some(true));
        assert_eq!(trig_edge(0.51, 0.49), Some(true));
    }

    #[test]
    fn edge_falling_below_threshold() {
        assert_eq!(trig_edge(0.0, 1.0), Some(false));
        assert_eq!(trig_edge(0.49, 0.51), Some(false));
    }

    #[test]
    fn no_edge_when_stable() {
        assert_eq!(trig_edge(1.0, 1.0), None);
        assert_eq!(trig_edge(0.0, 0.0), None);
        assert_eq!(trig_edge(0.5, 0.5), None); // exactly at threshold = no edge
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .init();

    let clip_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("VP404_CLIP").ok())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CLIP));

    // VP404_PROBE=1: print clip metadata (format/frames/fps) and exit — no GUI.
    if std::env::var("VP404_PROBE").is_ok() {
        let mut r = QtHapReader::open(&clip_path)?;
        let (w, h) = r.resolution();
        let fmt = r.texture_format();
        let f0 = r.read_frame(0).map(|f| f.texture_format);
        println!(
            "{}: {w}x{h}, {} frames @ {} fps, track-format {fmt:?}, frame0-format {f0:?}",
            clip_path.display(),
            r.frame_count(),
            r.fps()
        );
        return Ok(());
    }

    let handle = BankHandle::new();
    let grid_tab = PadGridTab::new(handle.clone());
    let seq_tab = SequencerTab::new(handle.clone());
    let output_tab = OutputTab::new("VP-404");
    rustjay_engine::run_with_egui_tabs(
        Vp404::new(clip_path, handle),
        vec![Box::new(grid_tab), Box::new(seq_tab), Box::new(output_tab)],
    )
}
