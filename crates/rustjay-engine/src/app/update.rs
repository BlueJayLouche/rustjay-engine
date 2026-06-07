use super::App;
use rustjay_core::EffectPlugin;
use rustjay_core::InputType;
use std::sync::Arc;

/// Minimum interval between device-enumeration polls (audio/MIDI/input lists).
/// Devices change on a human timescale, so polling once per frame wastes CPU.
const DEVICE_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(750);

impl<P: EffectPlugin> App<P> {
    pub(super) fn update_input(&mut self) {
        // Slot 1 always uploads. Slot 2 only uploads when the active effect
        // actually samples a second input — uploading a full-res frame costs a
        // CPU memmove into wgpu's staging buffer (matters on CPU-bound targets).
        // Device housekeeping (manager.update / frame drain) still runs for both.
        // Read the cached count: `self.plugin` is None after resumed() moves the
        // plugin into the engine, so it can't be queried directly here.
        let second_needed = self.plugin_input_count >= 2;
        self.update_input_slot(false, true);
        self.update_input_slot(true, second_needed);
    }

    fn update_input_slot(&mut self, is_second: bool, upload_texture: bool) {
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
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
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
                    if upload_texture {
                        if let Some(texture) = manager.syphon_output_texture() {
                            if let Some(ref mut engine) = self.output_engine {
                                if is_second {
                                    engine.second_input_texture.set_external_texture(texture);
                                } else {
                                    engine.input_texture.set_external_texture(texture);
                                }
                            }
                        }
                    }
                    manager.clear_syphon_frame();
                    let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    let input = if is_second {
                        &mut state.second_input
                    } else {
                        &mut state.input
                    };
                    input.width = width;
                    input.height = height;
                }
            }
        } else {
            if let Some(frame_data) = manager.take_frame() {
                let (width, height) = manager.resolution();
                if upload_texture {
                    if let Some(ref mut engine) = self.output_engine {
                        if is_second {
                            engine
                                .second_input_texture
                                .update(&frame_data, width, height);
                        } else {
                            engine.input_texture.update(&frame_data, width, height);
                        }
                    }
                }
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let input = if is_second {
                    &mut state.second_input
                } else {
                    &mut state.input
                };
                input.width = width;
                input.height = height;
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
                        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                        let input = if is_second {
                            &mut state.second_input
                        } else {
                            &mut state.input
                        };
                        input.width = width;
                        input.height = height;
                    }
                    manager.clear_spout_frame();
                }
            } else if let Some(frame_data) = manager.take_frame() {
                let (width, height) = manager.resolution();
                if upload_texture {
                    if let Some(ref mut engine) = self.output_engine {
                        if is_second {
                            engine
                                .second_input_texture
                                .update(&frame_data, width, height);
                        } else {
                            engine.input_texture.update(&frame_data, width, height);
                        }
                    }
                }
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let input = if is_second {
                    &mut state.second_input
                } else {
                    &mut state.input
                };
                input.width = width;
                input.height = height;
            }
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            if let Some(frame_data) = manager.take_frame() {
                let (width, height) = manager.resolution();
                if upload_texture {
                    if let Some(ref mut engine) = self.output_engine {
                        if is_second {
                            engine
                                .second_input_texture
                                .update(&frame_data, width, height);
                        } else {
                            engine.input_texture.update(&frame_data, width, height);
                        }
                    }
                }
                let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                let input = if is_second {
                    &mut state.second_input
                } else {
                    &mut state.input
                };
                input.width = width;
                input.height = height;
            }
        }
    }

    pub(super) fn update_audio(&mut self) {
        if let Some(ref analyzer) = self.audio_analyzer {
            if analyzer.take_stream_error() {
                let device = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    state.audio.selected_device.clone()
                };
                log::warn!(
                    "[Audio] Stream error — attempting reconnect (device: {:?})",
                    device
                );
                if let Some(ref mut analyzer) = self.audio_analyzer {
                    match analyzer.start_with_device(device.as_deref()) {
                        Ok(actual_name) => {
                            let mut state =
                                self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                            state.audio.selected_device = Some(actual_name);
                        }
                        Err(e) => log::error!("[Audio] Reconnect failed: {}", e),
                    }
                }
            }
        }

        if let Some(ref analyzer) = self.audio_analyzer {
            // Push last-frame's cached params to the analyzer — avoids a lock acquisition
            // on the hot path. The cache is refreshed below at the end of the same call so
            // it is at most one frame stale (16 ms at 60 fps — imperceptible for audio params).
            analyzer.set_amplitude(self.cached_audio_amplitude);
            analyzer.set_smoothing(self.cached_audio_smoothing);
            analyzer.set_normalize(self.cached_audio_normalize);
            analyzer.set_pink_noise_shaping(self.cached_audio_pink_noise);

            let fft = analyzer.get_fft();
            let volume = analyzer.get_volume();
            let beat = analyzer.is_beat();
            let phase = analyzer.get_beat_phase();

            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            if state.audio.enabled {
                state.audio.fft = fft;
                state.audio.volume = volume;
                state.audio.beat = beat;
                state.audio.beat_phase = phase;

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

    pub(super) fn update_lfo(&mut self) {
        // --- F1 fix: read from state, drop lock, tick modulation, then re-acquire ---
        let (mod_arc, bpm, stable_beat_phase, volume) = {
            let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            let mod_arc = state.modulation.clone();
            let bpm = state.effective_bpm();
            let stable_beat_phase = state.stable_beat_phase();
            // S1: copy FFT into reusable scratch buffer (avoids per-frame allocation).
            self.cached_fft.clear();
            if state.audio.enabled {
                self.cached_fft.extend_from_slice(&state.audio.fft);
            }
            (mod_arc, bpm, stable_beat_phase, state.audio.volume)
        };

        // Build AudioValues after dropping state (borrows from self.cached_fft).
        let audio = {
            let mut values = rustjay_core::modulation::AudioValues::default();
            if !self.cached_fft.is_empty() {
                values.sources.insert(
                    0,
                    rustjay_core::modulation::AudioSourceValues {
                        fft: &self.cached_fft,
                        level: volume,
                        sample_rate: 48000.0,
                    },
                );
            }
            values
        };

        // Tick the unified modulation engine without holding shared_state.
        let offsets = {
            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
            mod_eng.update(self.elapsed_time, bpm, stable_beat_phase, &audio);

            let mut offsets = Vec::with_capacity(mod_eng.assignments.len());
            for param_id in mod_eng.assignments.keys() {
                let offset = mod_eng.get_modulation(param_id);
                offsets.push((param_id.clone(), offset));
            }
            offsets
        };

        // Re-acquire shared_state and write back.
        let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
        state.modulation_offsets = offsets;

        // NOTE: HSB params are no longer pre-computed here.
        // get_param("hue_shift"|"saturation"|"brightness") reads modulation_offsets
        // on demand, eliminating the double-modulation bug (F4).
    }

    #[cfg(feature = "link")]
    pub(super) fn update_link(&mut self) {
        if let Some(ref mut manager) = self.link_manager {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            manager.update(&mut state.link);
        }
    }

    #[cfg(feature = "prodj")]
    pub(super) fn update_prodj(&mut self) {
        if let Some(ref mut manager) = self.prodj_manager {
            let mut state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
            manager.update(&mut state.prodj);
        }
    }

    pub(super) fn update_midi(&mut self) {
        if let Some(ref mut manager) = self.midi_manager {
            if let Some(false) = manager.check_device_available_if_needed() {
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
                if let Ok(mut state) = self.shared_state.lock() {
                    state.midi_selected_device = None;
                    state.midi_enabled = false;
                }
            }
        }

        if let Some(ref manager) = self.midi_manager {
            // Collect dirty MIDI values and snapshot learn/mapping state in one lock.
            self.midi_dirty_scratch.clear();
            let (learn_active, learning_name, mapping_snapshot) = {
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
                (learn_active, learning_name, mapping_snapshot)
            };

            if let Ok(mut shared) = self.shared_state.lock() {
                // Sync learn state.
                shared.midi_learn_active = learn_active;
                if !learn_active {
                    shared.midi_learning_param_name = None;
                } else if learning_name.is_some() {
                    shared.midi_learning_param_name = learning_name;
                }
                shared.midi_mappings = mapping_snapshot;

                // Apply dirty parameter values.
                for (path, value) in &self.midi_dirty_scratch {
                    match path.as_str() {
                        "color/hue_shift" => {
                            shared.hsb_params.hue_shift = value.clamp(-180.0, 180.0)
                        }
                        "color/saturation" => shared.hsb_params.saturation = value.clamp(0.0, 2.0),
                        "color/brightness" => shared.hsb_params.brightness = value.clamp(0.0, 2.0),
                        "audio/amplitude" => shared.audio.amplitude = value.clamp(0.0, 5.0),
                        "audio/smoothing" => shared.audio.smoothing = value.clamp(0.0, 1.0),
                        _ => {
                            // Try app-specific param resolver first (hierarchical paths).
                            let resolved = shared
                                .param_resolver
                                .as_ref()
                                .and_then(|r| r.resolve(path))
                                .unwrap_or_else(|| path.clone());
                            let id = resolved.split('/').next_back().unwrap_or(&resolved);
                            if shared.param_descriptors.iter().any(|d| d.id == id) {
                                shared.set_param_base(id, *value);
                            }
                        }
                    }
                }
            }
        }

        // MTC: refresh port list, age out playing flag, copy state into EngineState.
        #[cfg(feature = "mtc")]
        if let Some(ref mut receiver) = self.mtc_receiver {
            receiver.refresh();
            receiver.tick();
            let mtc = receiver.clone_state();
            if let Ok(mut shared) = self.shared_state.lock() {
                shared.mtc = mtc;
            }
        }
    }

    pub(super) fn update_osc(&mut self) {
        if let Some(ref server) = self.osc_server {
            if let Ok(mut shared) = self.shared_state.lock() {
                if let Ok(mut osc_state) = server.state().lock() {
                    // Hardcoded HSB / audio params
                    if let Some(v) = osc_state.get_value_if_dirty("/color/hue_shift") {
                        shared.hsb_params.hue_shift = v.clamp(-180.0, 180.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/color/saturation") {
                        shared.hsb_params.saturation = v.clamp(0.0, 2.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/color/brightness") {
                        shared.hsb_params.brightness = v.clamp(0.0, 2.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/color/enabled") {
                        shared.color_enabled = v > 0.5;
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/audio/amplitude") {
                        shared.audio.amplitude = v.clamp(0.0, 5.0);
                    }
                    if let Some(v) = osc_state.get_value_if_dirty("/audio/smoothing") {
                        shared.audio.smoothing = v.clamp(0.0, 1.0);
                    }

                    // Effect-declared custom params
                    let descriptors = Arc::clone(&shared.param_descriptors);
                    if !descriptors.is_empty() {
                        log::trace!("OSC checking {} custom params", descriptors.len());
                    }
                    for (i, desc) in descriptors.iter().enumerate() {
                        if let Some(addr) = shared.param_osc_addresses.get(i) {
                            if let Some(v) = osc_state.get_value_if_dirty(addr) {
                                log::debug!("OSC apply: {} ({}) = {}", desc.id, addr, v);
                                shared.set_param_base(&desc.id, v.clamp(desc.min, desc.max));
                            } else if !osc_state.message_log.is_empty() {
                                // Debug: log if message exists but param wasn't dirty
                                let full_addr = format!("/rustjay{}", addr);
                                if osc_state.parameters.contains_key(&full_addr) {
                                    log::trace!("OSC param not dirty: {}", addr);
                                }
                            }
                        } else {
                            log::warn!("OSC param_osc_addresses missing index {}", i);
                        }
                    }

                    // Sync recent messages for GUI display
                    shared.osc_message_log = osc_state.message_log.clone();
                }
            }
        }
    }

    pub(super) fn update_web(&mut self) {
        if let Some(ref mut server) = self.web_server {
            if !server.is_running() {
                return;
            }
            if let Ok(state) = self.shared_state.lock() {
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
                // Broadcast custom param values
                let descriptors = Arc::clone(&state.param_descriptors);
                for (i, desc) in descriptors.iter().enumerate() {
                    if let Some(addr) = state.param_osc_addresses.get(i) {
                        // OSC addresses are "/category/id"; web uses "category/id" (no leading slash)
                        let id = addr.trim_start_matches('/');
                        let value = state.get_param_base(&desc.id).unwrap_or(desc.default);
                        server.update_parameter(id, value);
                    }
                }
            }

            // Drain structural dirty flags for panel broadcasts.
            if server.input_dirty {
                if let Ok(state) = self.shared_state.lock() {
                    server.send_input_state(&rustjay_control::InputStateJson {
                        devices: state.input.available_devices.clone(),
                        active_index: state.input.device_index,
                        active_name: state.input.source_name.clone(),
                        width: state.input.width,
                        height: state.input.height,
                        fps: state.input.fps,
                    });
                }
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
                ) = {
                    if let Ok(state) = self.shared_state.lock() {
                        (
                            state.osc_enabled,
                            state.osc_port,
                            state.midi_enabled,
                            state.midi_selected_device.clone(),
                            state.midi_available_devices.clone(),
                            state.midi_learn_active,
                            state.midi_learning_param_name.clone(),
                        )
                    } else {
                        (false, 9000, false, None, vec![], false, None)
                    }
                };
                let midi_mappings: Vec<rustjay_core::MidiMappingSnapshot> =
                    if let Some(ref m) = self.midi_manager {
                        if let Ok(midi_st) = m.state().lock() {
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
                        } else {
                            vec![]
                        }
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
                // F1 fix: clone Arc out of shared_state, drop guard, then lock modulation alone.
                let (mod_arc, audio_routes, audio_routing_enabled, bpm, tap_tempo_info) = {
                    let state = self.shared_state.lock().unwrap_or_else(|e| e.into_inner());
                    (
                        Arc::clone(&state.modulation),
                        state.audio_routing.matrix.routes().to_vec(),
                        state.audio_routing.enabled,
                        state.audio.bpm,
                        state.audio.tap_tempo_info.clone(),
                    )
                };
                let mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                server.send_modulation_state(&rustjay_control::ModulationStateJson {
                    lfos: mod_eng.to_lfo_vec(),
                    audio_routes,
                    audio_routing_enabled,
                    bpm,
                    tap_tempo_info,
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
    }

    pub(super) fn poll_device_discovery(&mut self) {
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
        if done {
            if let Some(manager) = self.input_manager.as_ref() {
                if self.use_egui {
                    #[cfg(feature = "egui")]
                    if let Some(ref mut gui) = self.egui_control_gui.as_mut() {
                        gui.update_device_lists(manager);
                    }
                } else if let Some(ref mut gui) = self.control_gui.as_mut() {
                    gui.update_device_lists(manager);
                }
            }
            self.shared_state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .input_discovering = false;
        }
    }

    pub(super) fn update_preview_textures(&mut self) {
        let show_preview = self
            .shared_state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .show_preview;
        if !show_preview {
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
                    if let Some(ref engine) = self.output_engine {
                        if let Some(preview_id) = gui.output_preview_texture_id {
                            if let Some(preview_tex) = renderer.get_preview_texture(preview_id) {
                                let preview_view = preview_tex
                                    .create_view(&wgpu::TextureViewDescriptor::default());
                                engine.blit_output_to(&mut encoder, &preview_view);
                                any_work = true;
                            }
                        }
                    }
                }

                if any_work {
                    renderer.queue().submit(std::iter::once(encoder.finish()));
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
                if let Some(ref engine) = self.output_engine {
                    if let Some(preview_id) = gui.output_preview_texture_id {
                        if let Some(preview_view) = renderer.get_preview_view(preview_id) {
                            engine.blit_output_to(&mut encoder, preview_view);
                            any_work = true;
                        }
                    }
                }
            }

            if any_work {
                renderer.queue().submit(std::iter::once(encoder.finish()));
            }
        }
    }
}
