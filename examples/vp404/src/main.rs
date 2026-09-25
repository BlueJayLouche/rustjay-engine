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

use bank::{Bank, BankHandle, PadCmd, PadInfo, PadSnap, PAD_COUNT};
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
    clip_path: Option<PathBuf>,
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
    /// Whether each trim knob has moved since launch (first move only seeds).
    trim_live: (bool, bool),
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
    /// Controller transport: previous param values for edge detection
    /// (record, seq_play, step_record, pattern_next, pattern_prev).
    prev_transport: [f32; 6],
    /// Record armed — the next pad press live-samples into that pad.
    rec_armed: bool,
    /// Last quarter-beat index a note-repeat retrigger fired on.
    retrig_step: i32,
    /// Previously published pad-loaded flags (change-detected into the queue).
    prev_loaded: Vec<bool>,
    /// Previously published sampler state (rec_state param).
    #[cfg(feature = "capture")]
    prev_rec_state: f32,
    /// Live sampler (capture → HAP5 → pad), only present when `capture` is enabled.
    #[cfg(feature = "capture")]
    live_sampler: Option<std::sync::Mutex<live_sampler::LiveSampler>>,
}

impl Vp404 {
    fn new(clip_path: Option<PathBuf>, handle: BankHandle) -> Self {
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
            trim_live: (false, false),
            edit_step: 0,
            record_mode: false,
            accumulated_beats: 0.0,
            last_tick: Instant::now(),
            prev_transport: [0.0; 6],
            rec_armed: false,
            retrig_step: -1,
            prev_loaded: vec![false; PAD_COUNT],
            #[cfg(feature = "capture")]
            prev_rec_state: 0.0,
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
                    format!("pad{i}_trig"),
                    format!("Pad {} Trigger", i + 1),
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
        // Controller transport (docs/design.md Mk1 mapping: rec / erase /
        // play / step / note-repeat / step_left+right).
        for (id, name) in [
            ("record", "Record (arm/stop)"),
            ("erase", "Erase (hold)"),
            ("seq_play", "Sequencer Play"),
            ("step_record", "Step Record"),
            ("retrigger", "Note Repeat (hold)"),
            ("pattern_next", "Pattern Next"),
            ("pattern_prev", "Pattern Prev"),
            ("loop_toggle", "Loop Toggle (last pad)"),
            ("rec_state", "Sampler State (out)"),
        ] {
            params.push(ParameterDescriptor::float(
                id,
                name,
                ParamCategory::Custom("Transport".into()),
                0.0,
                1.0,
                0.0,
                0.01,
            ));
        }
        // Read-only pad-loaded flags, published via `app_param_queue` —
        // controller LED sources (loaded pads light up).
        for i in 0..PAD_COUNT {
            params.push(ParameterDescriptor::float(
                format!("pad{i}_loaded"),
                format!("Pad {} Loaded (out)", i + 1),
                ParamCategory::Custom("Pads".into()),
                0.0,
                1.0,
                0.0,
                1.0,
            ));
        }
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
        // No clip given → start with empty pads.
        if let Some(clip_path) = self.clip_path.clone() {
        match sample::Sample::open(&clip_path) {
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
                pad.trigger();
                bank.last_triggered = Some(0);
            }
            Err(e) => log::error!("VP-404: cannot open {}: {e}", clip_path.display()),
        }
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
                    PadCmd::SetLoop(i, on) => {
                        if let Some(p) = bank.pads.get_mut(i) {
                            p.loop_enabled = on;
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
                            if let Err(e) = sampler.start_recording(i, frame_count) {
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
                                    // Loop stays off on load/record — enable
                                    // deliberately (checkbox or Grid button).
                                    // Don't auto-play — wait for a trigger.
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

            // 2b-. Controller transport (docs/design.md Mk1 mapping), all
            // edge-detected like pad trigs. `erase` is a hold-modifier read
            // by the pad loop below; `record` runs the SP-404 arm workflow:
            // arm → next pad press starts a free-length capture into that
            // pad → record again stops and encodes.
            let erase_held = engine.get_param_base("erase").unwrap_or(0.0) > 0.5;
            {
                let vals = [
                    engine.get_param_base("record").unwrap_or(0.0),
                    engine.get_param_base("seq_play").unwrap_or(0.0),
                    engine.get_param_base("step_record").unwrap_or(0.0),
                    engine.get_param_base("pattern_next").unwrap_or(0.0),
                    engine.get_param_base("pattern_prev").unwrap_or(0.0),
                    engine.get_param_base("loop_toggle").unwrap_or(0.0),
                ];
                let rising: Vec<bool> = vals
                    .iter()
                    .zip(self.prev_transport)
                    .map(|(&v, p)| trig_edge(v, p) == Some(true))
                    .collect();
                self.prev_transport = vals;

                if rising[0] {
                    #[cfg(feature = "capture")]
                    {
                        let sampler_recording = self
                            .live_sampler
                            .as_mut()
                            .map(|s| s.get_mut().unwrap_or_else(|e| e.into_inner()).state())
                            == Some(live_sampler::SamplerState::Recording);
                        if sampler_recording {
                            if let Some(s) = self.live_sampler.as_mut() {
                                s.get_mut().unwrap_or_else(|e| e.into_inner()).finish();
                            }
                        } else {
                            self.rec_armed = !self.rec_armed;
                        }
                    }
                    #[cfg(not(feature = "capture"))]
                    log::warn!("VP-404: record pressed but built without `capture`");
                }
                if rising[1] {
                    state.sequencer.toggle_playback();
                }
                if rising[2] {
                    self.record_mode = !self.record_mode;
                }
                let n = state.sequencer.patterns.len();
                if rising[3] && n > 0 {
                    state
                        .sequencer
                        .queue_pattern((state.sequencer.current_pattern + 1) % n);
                }
                if rising[4] && n > 0 {
                    state
                        .sequencer
                        .queue_pattern((state.sequencer.current_pattern + n - 1) % n);
                }
                // Loop toggle targets the last-pressed pad, same convention
                // as the in/out trim knobs and note-repeat.
                if rising[5] {
                    if let Some(p) = bank.last_triggered.and_then(|i| bank.pads.get_mut(i)) {
                        p.loop_enabled = !p.loop_enabled;
                        log::info!("VP-404: pad {} loop {}", p.index + 1, p.loop_enabled);
                    }
                }
            }

            // 2b. Edge-detect pad trig params — MIDI Note-On/Off, OSC, and web all
            // set `pad<i>_trig`; rising edge fires trigger (or step-write when
            // sequencer is stopped), falling fires release. Erase-hold clears
            // instead; an armed record captures into the pad instead.
            for i in 0..bank.pads.len() {
                let val = engine
                    .get_param_base(&format!("pad{i}_trig"))
                    .unwrap_or(0.0);
                let prev = self.prev_trig.get(i).copied().unwrap_or(0.0);
                match trig_edge(val, prev) {
                    Some(true) if erase_held => {
                        bank.pads[i].clear();
                    }
                    Some(true) if self.rec_armed => {
                        self.rec_armed = false;
                        #[cfg(feature = "capture")]
                        if let Some(s) = self.live_sampler.as_mut() {
                            let s = s.get_mut().unwrap_or_else(|e| e.into_inner());
                            if let Err(e) = s.start_recording(i, FREE_REC_MAX_FRAMES) {
                                log::error!("VP-404 start sampling: {e}");
                            }
                        }
                    }
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
            // knobs adjust the *last-pressed* pad, applied as RELATIVE deltas.
            // The Mk1 knobs are endless encoders reporting absolute 0..1
            // positions, so applying the value directly snaps the trim to
            // wherever the knob happens to sit (the "6 frames on a light
            // touch" artifact). Deltas are wrap-corrected for the 999->0
            // rollover; the first sample after launch only seeds the baseline.
            {
                let in_v = engine.get_param("in_point").unwrap_or(0.0).clamp(0.0, 1.0);
                let out_v = engine.get_param("out_point").unwrap_or(1.0).clamp(0.0, 1.0);
                let mut din = wrap_delta(in_v - self.prev_in);
                let mut dout = wrap_delta(out_v - self.prev_out);
                self.prev_in = in_v;
                self.prev_out = out_v;
                // The first movement of each knob after launch only seeds the
                // baseline — where the encoder happens to sit is meaningless.
                if din.abs() > 1e-4 && !self.trim_live.0 {
                    self.trim_live.0 = true;
                    din = 0.0;
                }
                if dout.abs() > 1e-4 && !self.trim_live.1 {
                    self.trim_live.1 = true;
                    dout = 0.0;
                }
                if din.abs() > 1e-4 || dout.abs() > 1e-4 {
                    if let Some(pad) = bank.last_triggered.and_then(|i| bank.pads.get_mut(i)) {
                        if let Some(sample) = pad.sample.as_mut() {
                            let last = sample.frame_count.saturating_sub(1) as f32;
                            if last > 0.0 {
                                let in_f =
                                    (sample.in_point as f32 / last + din).clamp(0.0, 1.0);
                                let out_f =
                                    (sample.out_point as f32 / last + dout).clamp(0.0, 1.0);
                                sample.set_range(
                                    (in_f * last).round() as u32,
                                    (out_f * last).round() as u32,
                                );
                            }
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

            // Note-repeat: while held, retrigger the last-pressed pad every
            // 1/4 beat. ponytail: fixed 1/16-note division — knob/division
            // select is later polish.
            if engine.get_param_base("retrigger").unwrap_or(0.0) > 0.5 {
                let step = (self.accumulated_beats / 0.25) as i32;
                if step != self.retrig_step {
                    self.retrig_step = step;
                    if let Some(p) = bank.last_triggered.and_then(|i| bank.pads.get_mut(i)) {
                        p.trigger();
                    }
                }
            }

            // Publish pad-loaded flags + sampler state for controller LEDs
            // (change-detected; drained by the engine into params, which the
            // osc-feedback mirror then pushes to the controller).
            {
                let mut q: Vec<(String, f32)> = Vec::new();
                for (i, p) in bank.pads.iter().enumerate() {
                    let loaded = p.has_sample();
                    if self.prev_loaded[i] != loaded {
                        self.prev_loaded[i] = loaded;
                        q.push((format!("pad{i}_loaded"), loaded as u8 as f32));
                    }
                }
                #[cfg(feature = "capture")]
                {
                    let rec_state = match self
                        .live_sampler
                        .as_mut()
                        .map(|s| s.get_mut().unwrap_or_else(|e| e.into_inner()).state())
                    {
                        Some(live_sampler::SamplerState::Recording) => 1.0,
                        Some(live_sampler::SamplerState::Encoding) => 0.7,
                        _ if self.rec_armed => 0.4,
                        _ => 0.0,
                    };
                    if (rec_state - self.prev_rec_state).abs() > 1e-3 {
                        self.prev_rec_state = rec_state;
                        q.push(("rec_state".into(), rec_state));
                    }
                }
                if !q.is_empty() {
                    if let Ok(mut g) = engine.app_param_queue.lock() {
                        g.extend(q);
                    }
                }
            }

            // 5. Tick the pad sequencer from the same master clock.
            state.sequencer.tick(self.accumulated_beats, &self.handle);
        }
    }

    /// Presets persist the pad/clip layout (paths + modes) and the sequencer
    /// patterns via the engine's plugin_state blob; clips restore by path.
    fn serialize_preset_state(&self, state: &Vp404State) -> Option<String> {
        let bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::to_string(&PresetBlob {
            pads: pad_snaps(&bank),
            sequencer: Some(state.sequencer.clone()),
        })
        .ok()
    }

    fn deserialize_preset_state(&self, data: &str, state: &mut Vp404State) {
        let blob: PresetBlob = match serde_json::from_str(data) {
            Ok(b) => b,
            Err(e) => {
                log::error!("VP-404: preset app state unreadable: {e}");
                return;
            }
        };
        restore_pads(
            &mut self.bank.lock().unwrap_or_else(|e| e.into_inner()),
            blob.pads,
        );
        if let Some(seq) = blob.sequencer {
            // serde(skip) runtime fields (is_playing, clocks) come back as
            // defaults — a loaded preset starts with the transport stopped.
            state.sequencer = seq;
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
                loop_enabled: p.loop_enabled,
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
                    sampler.submit_readback(texture, ctx.engine_state.input.frame_seq);
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
/// Free-length live-sampling cap. ponytail: frames are raw RGBA in RAM
/// (~8 MB/frame at 1080p), so ~10 s at 30 fps — streaming encode is the
/// upgrade path if longer grabs are ever needed.
#[cfg(feature = "capture")]
const FREE_REC_MAX_FRAMES: u32 = 300;

/// Delta between two endless-encoder positions (0..1), wrap-corrected.
/// App state carried in the engine preset `plugin_state` blob.
#[derive(serde::Serialize, serde::Deserialize)]
struct PresetBlob {
    pads: Vec<Option<PadSnap>>,
    #[serde(default)]
    sequencer: Option<sequencer::SequencerEngine>,
}

fn pad_snaps(bank: &Bank) -> Vec<Option<PadSnap>> {
    bank.pads
        .iter()
        .map(|p| {
            p.sample.as_ref().map(|s| PadSnap {
                path: s.path.clone(),
                trigger_mode: p.trigger_mode,
                loop_enabled: p.loop_enabled,
                playback_mode: p.playback_mode,
                beat_division: p.beat_division,
                in_point: s.in_point,
                out_point: s.out_point,
            })
        })
        .collect()
}

/// Rebuild the bank from a preset's pad snapshots. A missing clip file
/// clears its pad and keeps going — one moved file must not kill the set.
fn restore_pads(bank: &mut Bank, snaps: Vec<Option<PadSnap>>) {
    for (i, snap) in snaps.into_iter().enumerate() {
        let Some(pad) = bank.pads.get_mut(i) else { break };
        let Some(s) = snap else {
            pad.clear();
            continue;
        };
        match sample::Sample::open(&s.path) {
            Ok(mut smp) => {
                smp.set_range(s.in_point, s.out_point);
                pad.assign_sample(smp);
            }
            Err(e) => {
                log::warn!(
                    "VP-404: preset clip for pad {} missing ({e}); clearing",
                    i + 1
                );
                pad.clear();
            }
        }
        pad.trigger_mode = s.trigger_mode;
        pad.loop_enabled = s.loop_enabled;
        pad.playback_mode = s.playback_mode;
        pad.beat_division = s.beat_division;
    }
    log::info!("VP-404: pad bank restored from preset");
}

fn wrap_delta(d: f32) -> f32 {
    if d > 0.5 {
        d - 1.0
    } else if d < -0.5 {
        d + 1.0
    } else {
        d
    }
}

/// Press threshold ≈ cabl's 200/4095 for these pads; release sits lower so a
/// held pad whose pressure wobbles never re-crosses a single threshold.
const TRIG_PRESS: f32 = 0.05;
const TRIG_RELEASE: f32 = 0.03;

/// Edge-detect a trigger param. GUI/web/MIDI send clean 1.0/0.0; the Mk1
/// streams continuous pad pressure, so this is a Schmitt trigger, not a
/// single threshold — one threshold made every mid-hold pressure dip fire
/// release+retrigger (Gate stuttered, OneShot machine-gunned "forever").
/// ponytail: stateless (compares prev against both thresholds), ambiguous
/// only for holds parked inside the 0.03–0.05 band — feather-touch territory.
fn trig_edge(val: f32, prev: f32) -> Option<bool> {
    if val > TRIG_PRESS && prev <= TRIG_PRESS {
        Some(true)
    } else if val <= TRIG_RELEASE && prev > TRIG_RELEASE {
        Some(false)
    } else {
        None
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .init();

    // Optional launch clip — seeds pad 0 so something plays immediately.
    let clip_path = std::env::args()
        .nth(1)
        .or_else(|| std::env::var("VP404_CLIP").ok())
        .map(PathBuf::from);

    // VP404_PROBE=1: print clip metadata (format/frames/fps) and exit — no GUI.
    if std::env::var("VP404_PROBE").is_ok() {
        let clip_path = clip_path
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("VP404_PROBE needs a clip path"))?;
        let mut r = QtHapReader::open(clip_path)?;
        let (w, h) = r.resolution();
        let fmt = r.texture_format();
        let f0 = r.read_frame(0).map(|f| f.format);
        println!(
            "{}: {w}x{h}, {} frames @ {} fps, track-format {fmt:?}, frame0-format {f0:?}",
            clip_path.display(),
            r.frame_count(),
            r.fps()
        );
        return Ok(());
    }

    // Title-bar/taskbar icon on Windows + X11; the macOS bundle uses AppIcon.icns.
    rustjay_engine::set_window_icon(include_bytes!("../packaging/icon-256.png"));

    let handle = BankHandle::new();
    let grid_tab = PadGridTab::new(handle.clone());
    let seq_tab = SequencerTab::new(handle.clone());
    let output_tab = OutputTab::new("VP-404");
    rustjay_engine::run_with_egui_tabs(
        Vp404::new(clip_path, handle),
        vec![Box::new(grid_tab), Box::new(seq_tab), Box::new(output_tab)],
    )
}

#[cfg(test)]
mod tests {
    use super::trig_edge;

    #[test]
    fn edge_rising_through_press() {
        assert_eq!(trig_edge(1.0, 0.0), Some(true));
        assert_eq!(trig_edge(0.06, 0.04), Some(true));
    }

    #[test]
    fn edge_falling_through_release() {
        assert_eq!(trig_edge(0.0, 1.0), Some(false));
        assert_eq!(trig_edge(0.02, 0.04), Some(false));
    }

    #[test]
    fn no_edge_when_stable() {
        assert_eq!(trig_edge(1.0, 1.0), None);
        assert_eq!(trig_edge(0.0, 0.0), None);
    }

    #[test]
    fn held_pad_pressure_wobble_is_silent() {
        // A held Mk1 pad streams varying pressure; dips that stay above the
        // release threshold must not fire release or retrigger.
        assert_eq!(trig_edge(0.4, 0.9), None);
        assert_eq!(trig_edge(0.9, 0.4), None);
        assert_eq!(trig_edge(0.06, 0.9), None); // deep dip, still held
    }
}
