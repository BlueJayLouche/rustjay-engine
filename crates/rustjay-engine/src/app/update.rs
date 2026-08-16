use super::App;
use rustjay_core::{EffectPlugin, EngineState};
#[allow(unused_imports)] // used only by the macOS/Windows input paths
use rustjay_core::InputType;
use std::sync::Arc;

/// Minimum interval between device-enumeration polls (audio/MIDI/input lists).
/// Devices change on a human timescale, so polling once per frame wastes CPU.
const DEVICE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);

pub(super) struct LfoFrame {
    modulation: Arc<std::sync::Mutex<rustjay_core::modulation::ModulationEngine>>,
    bpm: f32,
    stable_beat_phase: f32,
    volume: f32,
    sample_rate: f32,
}

pub(super) struct WebModulationUpdate {
    modulation: Arc<std::sync::Mutex<rustjay_core::modulation::ModulationEngine>>,
    audio_routes: Vec<rustjay_core::routing::AudioRoute>,
    audio_routing_enabled: bool,
    bpm: f32,
    tap_tempo_info: String,
}

fn should_refresh_preview(show_preview: bool, show_stage_preview: bool) -> bool {
    show_preview || show_stage_preview
}

impl<P: EffectPlugin> App<P> {
    pub(super) fn update_input(&mut self, state: &mut EngineState) {
        // Slot 1 always uploads. Slot 2 only uploads when the active effect
        // actually samples a second input — uploading a full-res frame costs a
        // CPU memmove into wgpu's staging buffer (matters on CPU-bound targets).
        // Device housekeeping (manager.update / frame drain) still runs for both.
        // Read the cached count: `self.plugin` is None after resumed() moves the
        // plugin into the engine, so it can't be queried directly here.
        let second_needed = self.plugin_input_count >= 2;
        self.update_input_slot(state, false, true);
        self.update_input_slot(state, true, second_needed);
    }

    fn update_input_slot(
        &mut self,
        state: &mut EngineState,
        is_second: bool,
        upload_texture: bool,
    ) {
        let manager_opt = if is_second {
            self.second_input_manager.as_mut()
        } else {
            self.input_manager.as_mut()
        };
        let Some(manager) = manager_opt else { return };

        #[cfg(feature = "ndi")]
        if manager.input_type() == InputType::Ndi && manager.is_ndi_source_lost() {
            log::warn!(
                "[NDI] Source lost — clearing input {} state",
                if is_second { 2 } else { 1 }
            );
            let input = if is_second {
                &mut state.second_input
            } else {
                &mut state.input
            };
            input.is_active = false;
            input.source_name = "Signal lost".to_string();
        }

        manager.update();

        #[cfg(target_os = "macos")]
        if manager.input_type() == InputType::Syphon {
            if manager.has_frame() {
                let dims = manager
                    .syphon_output_texture()
                    .map(|t| (t.width(), t.height()));
                if let Some((width, height)) = dims {
                    if upload_texture
                        && let Some(texture) = manager.syphon_output_texture()
                            && let Some(ref mut engine) = self.output_engine {
                                if is_second {
                                    engine.second_input_texture.set_external_texture(texture);
                                } else {
                                    engine.input_texture.set_external_texture(texture);
                                }
                            }
                    manager.clear_syphon_frame();
                    let input = if is_second {
                        &mut state.second_input
                    } else {
                        &mut state.input
                    };
                    input.width = width;
                    input.height = height;
                    input.frame_seq += 1;
                }
            }
        } else {
            if let Some(frame_data) = manager.take_frame() {
                let (width, height) = manager.resolution();
                if upload_texture
                    && let Some(ref mut engine) = self.output_engine {
                        if is_second {
                            engine
                                .second_input_texture
                                .update(&frame_data, width, height);
                        } else {
                            engine.input_texture.update(&frame_data, width, height);
                        }
                    }
                let input = if is_second {
                    &mut state.second_input
                } else {
                    &mut state.input
                };
                input.width = width;
                input.height = height;
                input.frame_seq += 1;
            }
        }

        #[cfg(target_os = "windows")]
        {
            if manager.input_type() == InputType::Spout {
                if manager.has_frame() {
                    let (width, height) = manager.resolution();
                    if let Some(pixels) = manager.spout_pixels() {
                        if upload_texture {
                            if let Some(ref mut engine) = self.output_engine {
                                if is_second {
                                    engine.second_input_texture.update(pixels, width, height);
                                } else {
                                    engine.input_texture.update(pixels, width, height);
                                }
                            }
                        }
                        let input = if is_second {
                            &mut state.second_input
                        } else {
                            &mut state.input
                        };
                        input.width = width;
                        input.height = height;
                        input.frame_seq += 1;
                    }
                    manager.clear_spout_frame();
                }
            } else if let Some(frame_data) = manager.take_frame() {
                let (width, height) = manager.resolution();
                if upload_texture
                    && let Some(ref mut engine) = self.output_engine
                {
                    if is_second {
                        engine
                            .second_input_texture
                            .update(&frame_data, width, height);
                    } else {
                        engine.input_texture.update(&frame_data, width, height);
                    }
                }
                let input = if is_second {
                    &mut state.second_input
                } else {
                    &mut state.input
                };
                input.width = width;
                input.height = height;
                input.frame_seq += 1;
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            if let Some(frame_data) = manager.take_frame() {
                let (width, height) = manager.resolution();
                if upload_texture
                    && let Some(ref mut engine) = self.output_engine
                {
                    if is_second {
                        engine
                            .second_input_texture
                            .update(&frame_data, width, height);
                    } else {
                        engine.input_texture.update(&frame_data, width, height);
                    }
                }
                let input = if is_second {
                    &mut state.second_input
                } else {
                    &mut state.input
                };
                input.width = width;
                input.height = height;
                input.frame_seq += 1;
            }
        }
    }

    pub(super) fn update_audio(&mut self, state: &mut EngineState) {
        if let Some(ref analyzer) = self.audio_analyzer {
            // Push last-frame's cached params to the analyzer — avoids a lock acquisition
            // on the hot path. The cache is refreshed below at the end of the same call so
            // it is at most one frame stale (16 ms at 60 fps — imperceptible for audio params).
            analyzer.set_amplitude(self.cached_audio_amplitude);
            analyzer.set_smoothing(self.cached_audio_smoothing);
            analyzer.set_normalize(self.cached_audio_normalize);
            analyzer.set_pink_noise_shaping(self.cached_audio_pink_noise);

            let fft = analyzer.get_fft();
            analyzer.get_spectrum_into(&mut self.cached_spectrum);
            let volume = analyzer.get_volume();
            let beat = analyzer.is_beat();
            let phase = analyzer.get_beat_phase();

            if state.audio.enabled {
                state.audio.fft = fft;
                std::mem::swap(&mut state.audio.spectrum, &mut self.cached_spectrum);
                state.audio.volume = volume;
                state.audio.beat = beat;
                state.audio.beat_phase = phase;
                state.audio.sample_rate = analyzer.sample_rate();

                // Always reset modulated params to their base values before applying
                // this frame's modulations — prevents accumulation across frames.
                state.reset_custom_params_to_base();

                if state.audio_routing.enabled {
                    let delta_time = self.frame_delta_time;
                    let descriptors = Arc::clone(&state.param_descriptors);
                    state.audio_routing.matrix.process(&fft, delta_time);
                    // Temporarily take slices to avoid split-borrow on `state`.
                    let mut custom_params = std::mem::take(&mut state.custom_params);
                    let custom_param_bases = std::mem::take(&mut state.custom_param_bases);
                    state.audio_routing.matrix.apply_to_params(
                        &mut custom_params,
                        &custom_param_bases,
                        &descriptors,
                    );
                    state.custom_params = custom_params;
                    state.custom_param_bases = custom_param_bases;
                }
            }
            // Refresh cache from state so next frame's push uses current values.
            self.cached_audio_amplitude = state.audio.amplitude;
            self.cached_audio_smoothing = state.audio.smoothing;
            self.cached_audio_normalize = state.audio.normalize;
            self.cached_audio_pink_noise = state.audio.pink_noise_shaping;
        }
    }

    pub(super) fn prepare_lfo(&mut self, state: &EngineState) -> LfoFrame {
        // S1: copy full spectrum into reusable scratch buffer (avoids per-frame allocation).
        self.cached_fft.clear();
        if state.audio.enabled {
            self.cached_fft.extend_from_slice(&state.audio.spectrum);
        }
        LfoFrame {
            modulation: Arc::clone(&state.modulation),
            bpm: state.effective_bpm(),
            stable_beat_phase: state.stable_beat_phase(),
            volume: state.audio.volume,
            sample_rate: state.audio.sample_rate,
        }
    }

    pub(super) fn update_lfo(&mut self, frame: LfoFrame) -> Vec<(String, f32)> {
        // Build AudioValues after dropping state (borrows from self.cached_fft).
        let audio = {
            let mut values = rustjay_core::modulation::AudioValues::default();
            if !self.cached_fft.is_empty() {
                values.sources.insert(
                    0,
                    rustjay_core::modulation::AudioSourceValues {
                        fft: &self.cached_fft,
                        level: frame.volume,
                        sample_rate: frame.sample_rate,
                    },
                );
            }
            values
        };

        // Tick the unified modulation engine without holding shared_state.
        // Use wall-clock time so LFO dt is real seconds, not the clamped
        // frame_delta_time accumulator which runs fast under ControlFlow::Poll.
        let mod_time = self.modulation_start.elapsed().as_secs_f32();
        let offsets = {
            let mut mod_eng = frame
                .modulation
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            log::debug!(
                "[update_lfo] mod_time={:.3} bpm={:.1} beat_phase={:.2}",
                mod_time, frame.bpm, frame.stable_beat_phase
            );
            mod_eng.update(mod_time, frame.bpm, frame.stable_beat_phase, &audio);

            let mut offsets = Vec::with_capacity(mod_eng.assignments.len());
            for param_id in mod_eng.assignments.keys() {
                let offset = mod_eng.get_modulation(param_id);
                offsets.push((param_id.clone(), offset));
            }
            offsets
        };

        // NOTE: HSB params are no longer pre-computed here.
        // get_param("hue_shift"|"saturation"|"brightness") reads modulation_offsets
        // on demand, eliminating the double-modulation bug (F4).
        offsets
    }

    #[cfg(feature = "link")]
    pub(super) fn update_link(&mut self, state: &mut EngineState) {
        // Lazily construct/drop around the `enabled` toggle. LinkManager::new()
        // spawns Ableton Link's background threads (Link Main/Dispatcher), so
        // keeping it alive while disabled burned idle CPU for nothing.
        match (state.link.enabled, self.link_manager.is_some()) {
            (true, false) => self.link_manager = Some(rustjay_sync::LinkManager::new()),
            (false, true) => {
                // One last update lets the manager leave the session and clear
                // the UI state before we drop it (which stops the threads).
                if let Some(ref mut manager) = self.link_manager {
                    manager.update(&mut state.link);
                }
                self.link_manager = None;
            }
            _ => {}
        }
        if let Some(ref mut manager) = self.link_manager {
            manager.update(&mut state.link);
        }
    }

    #[cfg(feature = "prodj")]
    pub(super) fn update_prodj(&mut self, state: &mut EngineState) {
        // Lazily construct/drop around the `enabled` toggle. ProDjManager::new()
        // binds UDP 50000/50002 and JOINS the Pro DJ Link network on construction,
        // so every launch was joining a DJ network for a feature nobody enabled.
        match (state.prodj.enabled, self.prodj_manager.is_some()) {
            (true, false) => self.prodj_manager = Some(rustjay_sync::ProDjManager::new()),
            (false, true) => {
                if let Some(ref mut manager) = self.prodj_manager {
                    manager.update(&mut state.prodj);
                }
                self.prodj_manager = None;
            }
            _ => {}
        }
        if let Some(ref mut manager) = self.prodj_manager {
            manager.update(&mut state.prodj);
        }
    }

    pub(super) fn poll_midi_device(&mut self) -> bool {
        if let Some(ref mut manager) = self.midi_manager
            && let Some(false) = manager.check_device_available_if_needed() {
                let name = manager
                    .state()
                    .lock()
                    .map(|s| s.selected_device.clone().unwrap_or_default())
                    .unwrap_or_default();
                log::warn!(
                    "[MIDI] Device '{}' no longer available — disconnecting",
                    name
                );
                manager.disconnect();
                return true;
            }
        false
    }

    #[cfg(feature = "mtc")]
    pub(super) fn poll_mtc(&mut self) -> Option<rustjay_core::MtcState> {
        self.mtc_receiver.as_mut().map(|receiver| {
            // refresh() may enumerate and open hardware every five seconds, so
            // it must run without the frame's EngineState guard.
            receiver.refresh();
            receiver.tick();
            receiver.clone_state()
        })
    }

    pub(super) fn update_midi(&mut self, state: &mut EngineState, disconnected: bool) {
        if disconnected {
            state.midi_selected_device = None;
            state.midi_enabled = false;
        }

        if let Some(ref manager) = self.midi_manager {
            // Collect dirty MIDI values and snapshot learn/mapping state in one lock.
            self.midi_dirty_scratch.clear();
            let (learn_active, learning_name, mapping_snapshot, last_input) = {
                let midi_state_arc = manager.state();
                let mut midi_state = midi_state_arc.lock().unwrap_or_else(|e| e.into_inner());
                for mapping in &mut midi_state.mappings {
                    if mapping.is_dirty() {
                        self.midi_dirty_scratch
                            .push((mapping.param_path.clone(), mapping.get_scaled_value()));
                    }
                }
                let learn_active = midi_state.learn_state != rustjay_control::LearnState::Idle;
                let learning_name = midi_state.learning_param_name.clone();
                let last_input = midi_state
                    .last_input
                    .map(|e| (e.kind, e.channel, e.selector, e.value));
                let mapping_snapshot: Vec<rustjay_core::MidiMappingSnapshot> = midi_state
                    .mappings
                    .iter()
                    .map(|m| rustjay_core::MidiMappingSnapshot {
                        name: m.name.clone(),
                        param_path: m.param_path.clone(),
                        kind: m.kind,
                        selector: m.selector,
                        channel: m.channel,
                        min_value: m.min_value,
                        max_value: m.max_value,
                    })
                    .collect();
                (learn_active, learning_name, mapping_snapshot, last_input)
            };

            state.midi_last_input = last_input;
            state.midi_learn_active = learn_active;
            if !learn_active {
                state.midi_learning_param_name = None;
            } else if learning_name.is_some() {
                state.midi_learning_param_name = learning_name;
            }
            state.midi_mappings = mapping_snapshot;

            for (path, value) in &self.midi_dirty_scratch {
                match path.as_str() {
                    "color/hue_shift" => {
                        state.hsb_params.hue_shift = value.clamp(-180.0, 180.0)
                    }
                    "color/saturation" => state.hsb_params.saturation = value.clamp(0.0, 2.0),
                    "color/brightness" => state.hsb_params.brightness = value.clamp(0.0, 2.0),
                    "audio/amplitude" => state.audio.amplitude = value.clamp(0.0, 5.0),
                    "audio/smoothing" => state.audio.smoothing = value.clamp(0.0, 1.0),
                    _ => {
                        // Try app-specific param resolver first (hierarchical paths).
                        let resolved = state
                            .param_resolver
                            .as_ref()
                            .and_then(|r| r.resolve(path))
                            .unwrap_or_else(|| path.clone());
                        let id = resolved.split('/').next_back().unwrap_or(&resolved);
                        if state.param_descriptors.iter().any(|d| d.id == id) {
                            state.set_param_base(id, *value);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn update_osc(&mut self, shared: &mut EngineState) {
        if let Some(ref server) = self.osc_server
            && let Ok(mut osc_state) = server.state().lock() {
                    if let Some(v) = osc_state.get_value_if_dirty("/rustjay/color/hue_shift") {
                        shared.hsb_params.hue_shift = v.clamp(-180.0, 180.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/rustjay/color/saturation") {
                        shared.hsb_params.saturation = v.clamp(0.0, 2.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/rustjay/color/brightness") {
                        shared.hsb_params.brightness = v.clamp(0.0, 2.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/rustjay/color/enabled") {
                        shared.color_enabled = v > 0.5;
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/rustjay/audio/amplitude") {
                        shared.audio.amplitude = v.clamp(0.0, 5.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/rustjay/audio/smoothing") {
                        shared.audio.smoothing = v.clamp(0.0, 1.0);
                    }

                    let descriptors = Arc::clone(&shared.param_descriptors);
                    if !descriptors.is_empty() {
                        log::trace!("OSC checking {} custom params", descriptors.len());
                    }
                    for (i, desc) in descriptors.iter().enumerate() {
                        if let Some(addr) = shared.param_osc_addresses.get(i) {
                            if let Some(v) = osc_state.get_value_if_dirty(addr) {
                                log::debug!("OSC apply: {} ({}) = {}", desc.id, addr, v);
                                shared.set_param_base(&desc.id, v.clamp(desc.min, desc.max));
                            } else if !osc_state.message_log.is_empty()
                                && osc_state.parameters.contains_key(addr) {
                                    log::trace!("OSC param not dirty: {}", addr);
                                }
                        } else {
                            log::warn!("OSC param_osc_addresses missing index {}", i);
                        }
                    }

                    // Apply app-published param values (e.g. vp404 pad-loaded
                    // flags) so the mirrors below — web and controller — see them.
                    let queued = shared
                        .app_param_queue
                        .lock()
                        .map(|mut g| std::mem::take(&mut *g))
                        .unwrap_or_default();
                    for (id, v) in queued {
                        shared.set_param_base(&id, v);
                    }

                    // Mirror engine values into the OSC state so changes made
                    // elsewhere (web UI, presets) reach the feedback target;
                    // set_value's delta guard suppresses echo of OSC-origin
                    // changes.
                    #[cfg(feature = "osc-feedback")]
                    {
                        osc_state.set_value("/rustjay/color/hue_shift", shared.hsb_params.hue_shift);
                        osc_state.set_value("/rustjay/color/saturation", shared.hsb_params.saturation);
                        osc_state.set_value("/rustjay/color/brightness", shared.hsb_params.brightness);
                        osc_state.set_value(
                            "/rustjay/color/enabled",
                            if shared.color_enabled { 1.0 } else { 0.0 },
                        );
                        osc_state.set_value("/rustjay/audio/amplitude", shared.audio.amplitude);
                        osc_state.set_value("/rustjay/audio/smoothing", shared.audio.smoothing);
                        for (i, desc) in descriptors.iter().enumerate() {
                            if let Some(addr) = shared.param_osc_addresses.get(i)
                                && let Some(v) = shared.get_param_base(&desc.id) {
                                    osc_state.set_value(addr, v);
                                }
                        }
                    }

                    shared.osc_message_log = osc_state.message_log.clone();
                }
    }

    pub(super) fn update_web(&mut self, state: &EngineState) -> Option<WebModulationUpdate> {
        if let Some(ref mut server) = self.web_server {
            if !server.is_running() {
                return None;
            }
            server.update_parameter("color/hue_shift", state.hsb_params.hue_shift);
            server.update_parameter("color/saturation", state.hsb_params.saturation);
            server.update_parameter("color/brightness", state.hsb_params.brightness);
            server
                .update_parameter("color/enabled", if state.color_enabled { 1.0 } else { 0.0 });
            server.update_parameter("audio/amplitude", state.audio.amplitude);
            server.update_parameter("audio/smoothing", state.audio.smoothing);
            server
                .update_parameter("audio/enabled", if state.audio.enabled { 1.0 } else { 0.0 });
            server.update_parameter(
                "audio/normalize",
                if state.audio.normalize { 1.0 } else { 0.0 },
            );
            server.update_parameter(
                "audio/pink_noise",
                if state.audio.pink_noise_shaping {
                    1.0
                } else {
                    0.0
                },
            );
            server.update_parameter(
                "output/fullscreen",
                if state.output_fullscreen { 1.0 } else { 0.0 },
            );
            let descriptors = Arc::clone(&state.param_descriptors);
            for (i, desc) in descriptors.iter().enumerate() {
                if let Some(addr) = state.param_osc_addresses.get(i) {
                    // OSC full addresses are "/rustjay/category/id"; web uses "category/id"
                    let id = addr.strip_prefix("/rustjay/").unwrap_or(addr.trim_start_matches('/'));
                    let value = state.get_param_base(&desc.id).unwrap_or(desc.default);
                    server.update_parameter(id, value);
                }
            }

            if server.input_dirty {
                server.send_input_state(&rustjay_control::InputStateJson {
                    devices: state.input.available_devices.clone(),
                    active_index: state.input.device_index,
                    active_name: state.input.source_name.clone(),
                    width: state.input.width,
                    height: state.input.height,
                    fps: state.input.fps,
                });
                server.input_dirty = false;
            }
            if server.control_dirty {
                let (
                    osc_enabled,
                    osc_port,
                    midi_enabled,
                    midi_selected_device,
                    midi_devices,
                    midi_learn_active,
                    midi_learning_param_name,
                ) = (
                    state.osc_enabled,
                    state.osc_port,
                    state.midi_enabled,
                    state.midi_selected_device.clone(),
                    state.midi_available_devices.clone(),
                    state.midi_learn_active,
                    state.midi_learning_param_name.clone(),
                );
                let midi_mappings: Vec<rustjay_core::MidiMappingSnapshot> =
                    if let Some(ref m) = self.midi_manager {
                        match m.state().lock() { Ok(midi_st) => {
                            midi_st
                                .mappings
                                .iter()
                                .map(|m| rustjay_core::MidiMappingSnapshot {
                                    name: m.name.clone(),
                                    param_path: m.param_path.clone(),
                                    kind: m.kind,
                                    selector: m.selector,
                                    channel: m.channel,
                                    min_value: m.min_value,
                                    max_value: m.max_value,
                                })
                                .collect()
                        } _ => {
                            vec![]
                        }}
                    } else {
                        vec![]
                    };
                server.send_control_state(&rustjay_control::ControlStateJson {
                    osc_enabled,
                    osc_port,
                    midi_enabled,
                    midi_selected_device,
                    midi_devices,
                    midi_mappings,
                    midi_learn_active,
                    midi_learning_param_name,
                });
                server.control_dirty = false;
            }
            if server.modulation_dirty {
                return Some(WebModulationUpdate {
                    modulation: Arc::clone(&state.modulation),
                    audio_routes: state.audio_routing.matrix.routes().to_vec(),
                    audio_routing_enabled: state.audio_routing.enabled,
                    bpm: state.audio.bpm,
                    tap_tempo_info: state.audio.tap_tempo_info.clone(),
                });
            }
        }
        None
    }

    pub(super) fn finish_web_update(&mut self, modulation: Option<WebModulationUpdate>) {
        let Some(ref mut server) = self.web_server else { return };
        if !server.is_running() {
            return;
        }
        if let Some(update) = modulation {
            // The caller has dropped EngineState before entering this function.
            let mod_eng = update.modulation.lock().unwrap_or_else(|e| e.into_inner());
            server.send_modulation_state(&rustjay_control::ModulationStateJson {
                lfos: mod_eng.to_lfo_vec(),
                audio_routes: update.audio_routes,
                audio_routing_enabled: update.audio_routing_enabled,
                bpm: update.bpm,
                tap_tempo_info: update.tap_tempo_info,
            });
            server.modulation_dirty = false;
        }
        if server.preset_dirty {
            if let Some(ref bank) = self.preset_bank {
                server.send_preset_state(&rustjay_control::PresetStateJson {
                    presets: bank
                        .presets
                        .iter()
                        .enumerate()
                        .map(|(i, p)| rustjay_control::PresetInfo {
                            index: i,
                            name: p.name.clone(),
                        })
                        .collect(),
                });
            }
            server.preset_dirty = false;
        }
    }

    pub(super) fn poll_device_discovery(&mut self, state: &mut EngineState) {
        // Device discovery completes on a human timescale, so polling the
        // background scan every frame wastes CPU (perf: matters on the Pi
        // target). Throttle to ~750 ms — a slower device-list refresh is fine.
        let poll_now = std::time::Instant::now();
        if poll_now.duration_since(self.last_device_poll) < DEVICE_POLL_INTERVAL {
            return;
        }
        self.last_device_poll = poll_now;

        let done = self
            .input_manager
            .as_mut()
            .is_some_and(|m| m.poll_discovery());

        // Syphon's server directory populates via run-loop notifications, so
        // the one-shot startup discovery sees an empty list. Re-snapshot it on
        // this (already throttled) tick — an in-process read — so servers
        // appear/disappear live without a manual refresh.
        #[cfg(target_os = "macos")]
        let syphon_changed = self
            .input_manager
            .as_mut()
            .is_some_and(|m| m.refresh_syphon_servers());
        #[cfg(not(target_os = "macos"))]
        let syphon_changed = false;

        if (done || syphon_changed)
            && let Some(manager) = self.input_manager.as_ref()
        {
            if self.use_egui {
                #[cfg(feature = "egui")]
                if let Some(ref mut gui) = self.egui_control_gui.as_mut() {
                    gui.update_device_lists(manager, &state.audio.available_devices);
                }
            } else if let Some(ref mut gui) = self.control_gui.as_mut() {
                gui.update_device_lists(manager, &state.audio.available_devices);
            }
        }
        if done {
            state.input_discovering = false;
        }
    }

    pub(super) fn update_preview_textures(&mut self) {
        let should_refresh = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            should_refresh_preview(state.show_preview, state.show_stage_preview)
        };
        if !should_refresh {
            return;
        }

        if self.use_egui {
            #[cfg(feature = "egui")]
            if let (Some(ref mut renderer), Some(gui)) =
                (self.egui_renderer.as_mut(), self.egui_control_gui.as_ref())
            {
                let mut encoder =
                    renderer
                        .device()
                        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                            label: Some("Preview Encoder"),
                        });
                let mut any_work = false;

                {
                    let input_src = self
                        .output_engine
                        .as_ref()
                        .and_then(|e| e.input_texture.texture.as_ref().map(|t| &t.texture));
                    if let (Some(tex), Some(preview_id)) = (input_src, gui.input_preview_texture_id)
                    {
                        renderer.update_preview_texture(preview_id, tex, &mut encoder);
                        any_work = true;
                    }
                }

                {
                    let second_input_src = self
                        .output_engine
                        .as_ref()
                        .and_then(|e| e.second_input_texture.texture.as_ref().map(|t| &t.texture));
                    if let (Some(tex), Some(preview_id)) =
                        (second_input_src, gui.second_input_preview_texture_id)
                    {
                        renderer.update_preview_texture(preview_id, tex, &mut encoder);
                        any_work = true;
                    }
                }

                {
                    if let Some(ref engine) = self.output_engine
                        && let Some(preview_id) = gui.output_preview_texture_id
                            && let Some(preview_tex) = renderer.get_preview_texture(preview_id) {
                                let preview_view = preview_tex
                                    .create_view(&wgpu::TextureViewDescriptor::default());
                                engine.blit_output_to(&mut encoder, &preview_view);
                                any_work = true;
                            }
                }

                if any_work
                    && let Some(ref mut engine) = self.output_engine {
                        engine.enqueue_command(encoder.finish());
                    }
            }
        } else if let (Some(ref mut renderer), Some(gui)) =
            (self.imgui_renderer.as_mut(), self.control_gui.as_ref())
        {
            let mut encoder =
                renderer
                    .device()
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                        label: Some("Preview Encoder"),
                    });
            let mut any_work = false;

            {
                let input_src = self
                    .output_engine
                    .as_ref()
                    .and_then(|e| e.input_texture.texture.as_ref().map(|t| &t.texture));
                if let (Some(tex), Some(preview_id)) = (input_src, gui.input_preview_texture_id) {
                    renderer.update_preview_texture(preview_id, tex, &mut encoder);
                    any_work = true;
                }
            }

            {
                let second_input_src = self
                    .output_engine
                    .as_ref()
                    .and_then(|e| e.second_input_texture.texture.as_ref().map(|t| &t.texture));
                if let (Some(tex), Some(preview_id)) =
                    (second_input_src, gui.second_input_preview_texture_id)
                {
                    renderer.update_preview_texture(preview_id, tex, &mut encoder);
                    any_work = true;
                }
            }

            {
                if let Some(ref engine) = self.output_engine
                    && let Some(preview_id) = gui.output_preview_texture_id
                        && let Some(preview_view) = renderer.get_preview_view(preview_id) {
                            engine.blit_output_to(&mut encoder, preview_view);
                            any_work = true;
                        }
            }

            if any_work
                && let Some(ref mut engine) = self.output_engine {
                    engine.enqueue_command(encoder.finish());
                }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::should_refresh_preview;

    #[test]
    fn preview_refreshes_when_any_consumer_is_visible() {
        for (show_preview, show_stage_preview, expected) in [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, true),
        ] {
            assert_eq!(
                should_refresh_preview(show_preview, show_stage_preview),
                expected,
                "show_preview={show_preview}, show_stage_preview={show_stage_preview}"
            );
        }
    }
}
