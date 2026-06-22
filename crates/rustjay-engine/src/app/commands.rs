use super::App;
use rustjay_audio::list_audio_devices;
use rustjay_control::{MidiMapping, OscServer};
use rustjay_control::{WebCommand as WebServerCommand, WebConfig, WebServer};
use rustjay_core::EffectPlugin;
use rustjay_core::{
    AudioCommand, EngineState, InputCommand, LinkCommand, MidiCommand, ModulationCommand, OscCommand,
    OutputCommand, PresetCommand, ProDjCommand, WebCommand,
};
use std::sync::Arc;

fn lock(state: &std::sync::Mutex<EngineState>) -> std::sync::MutexGuard<'_, EngineState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

fn param_id_to_modulation_target(param_id: &str) -> rustjay_core::ModulationTarget {
    for t in rustjay_core::ModulationTarget::all() {
        if t.param_id() == Some(param_id) {
            return t.clone();
        }
    }
    rustjay_core::ModulationTarget::Custom(param_id.to_string())
}

/// All pending commands popped from shared state in a single lock acquisition.
struct PendingCommands {
    input: InputCommand,
    second_input: InputCommand,
    output: OutputCommand,
    audio: AudioCommand,
    midi: MidiCommand,
    modulation: ModulationCommand,
    osc: OscCommand,
    preset: PresetCommand,
    web: WebCommand,
    link: LinkCommand,
    prodj: ProDjCommand,
}

impl<P: EffectPlugin> App<P> {
    pub(super) fn dispatch_commands(&mut self) {
        // Pop all nine command slots in a single lock — saves 8 mutex acquires per frame
        // over the previous one-lock-per-process_* pattern.
        let cmds = {
            let mut s = lock(&self.shared_state);
            PendingCommands {
                input: std::mem::replace(&mut s.input_command, InputCommand::None),
                second_input: std::mem::replace(&mut s.second_input_command, InputCommand::None),
                output: std::mem::replace(&mut s.output_command, OutputCommand::None),
                audio: std::mem::replace(&mut s.audio_command, AudioCommand::None),
                midi: std::mem::replace(&mut s.midi_command, MidiCommand::None),
                modulation: std::mem::replace(&mut s.modulation_command, ModulationCommand::None),
                osc: std::mem::replace(&mut s.osc_command, OscCommand::None),
                preset: std::mem::replace(&mut s.preset_command, PresetCommand::None),
                web: std::mem::replace(&mut s.web_command, WebCommand::None),
                link: std::mem::replace(&mut s.link_command, LinkCommand::None),
                prodj: std::mem::replace(&mut s.prodj_command, ProDjCommand::None),
            }
        };
        self.process_input_commands(cmds.input);
        self.process_second_input_commands(cmds.second_input);
        self.process_output_commands(cmds.output);
        self.process_audio_commands(cmds.audio);
        self.process_midi_commands(cmds.midi);
        self.process_modulation_commands(cmds.modulation);
        self.process_osc_commands(cmds.osc);
        self.process_preset_commands(cmds.preset);
        self.process_web_commands(cmds.web);
        self.process_link_commands(cmds.link);
        self.process_prodj_commands(cmds.prodj);
    }

    fn process_input_commands(&mut self, command: InputCommand) {
        self.process_input_command_internal(command, false);
    }

    fn process_second_input_commands(&mut self, command: InputCommand) {
        self.process_input_command_internal(command, true);
    }

    fn process_input_command_internal(&mut self, command: InputCommand, is_second: bool) {
        let mut manager_opt = if is_second {
            self.second_input_manager.as_mut()
        } else {
            self.input_manager.as_mut()
        };
        let slot_prefix = if is_second { "Input 2" } else { "Input 1" };

        match command {
            InputCommand::StartWebcam {
                device_index,
                width,
                height,
                fps,
            } => {
                log::info!("{} starting webcam: device={}", slot_prefix, device_index);
                if let Some(ref mut manager) = manager_opt {
                    match manager.start_webcam(device_index, width, height, fps) {
                        Ok(_) => {
                            let mut state = lock(&self.shared_state);
                            let input = if is_second {
                                &mut state.second_input
                            } else {
                                &mut state.input
                            };
                            input.is_active = true;
                            input.input_type = rustjay_core::InputType::Webcam;
                            input.source_name = format!("Webcam {}", device_index);
                            input.device_index = Some(device_index);
                        }
                        Err(e) => log::error!("{} failed to start webcam: {:?}", slot_prefix, e),
                    }
                }
            }
            #[cfg(feature = "ndi")]
            InputCommand::StartNdi { source_name } => {
                log::info!("{} starting NDI: {}", slot_prefix, source_name);
                if let Some(ref mut manager) = manager_opt {
                    match manager.start_ndi(&source_name) {
                        Ok(_) => {
                            let mut state = lock(&self.shared_state);
                            let input = if is_second {
                                &mut state.second_input
                            } else {
                                &mut state.input
                            };
                            input.is_active = true;
                            input.input_type = rustjay_core::InputType::Ndi;
                            input.source_name = source_name;
                        }
                        Err(e) => log::error!("{} failed to start NDI: {:?}", slot_prefix, e),
                    }
                }
            }
            #[cfg(target_os = "macos")]
            InputCommand::StartSyphon {
                server_name,
                server_uuid,
            } => {
                log::info!(
                    "{} starting Syphon: {} (uuid={})",
                    slot_prefix,
                    server_name,
                    server_uuid
                );
                if let Some(ref mut manager) = manager_opt {
                    match manager.start_syphon(&server_name, &server_uuid) {
                        Ok(_) => {
                            let mut state = lock(&self.shared_state);
                            let input = if is_second {
                                &mut state.second_input
                            } else {
                                &mut state.input
                            };
                            input.is_active = true;
                            input.input_type = rustjay_core::InputType::Syphon;
                            input.source_name = server_name;
                            input.source_uuid = server_uuid;
                        }
                        Err(e) => log::error!("{} failed to start Syphon: {:?}", slot_prefix, e),
                    }
                }
            }
            #[cfg(target_os = "windows")]
            InputCommand::StartSpout { sender_name } => {
                log::info!("{} starting Spout: {}", slot_prefix, sender_name);
                if let Some(ref mut manager) = manager_opt {
                    match manager.start_spout(&sender_name) {
                        Ok(_) => {
                            let mut state = lock(&self.shared_state);
                            let input = if is_second {
                                &mut state.second_input
                            } else {
                                &mut state.input
                            };
                            input.is_active = true;
                            input.input_type = rustjay_core::InputType::Spout;
                            input.source_name = sender_name;
                        }
                        Err(e) => log::error!("{} failed to start Spout: {:?}", slot_prefix, e),
                    }
                }
            }
            #[cfg(target_os = "linux")]
            InputCommand::StartV4l2 { device_path } => {
                use std::path::Path;
                let index = Path::new(&device_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_prefix("video"))
                    .and_then(|s| s.parse::<u32>().ok());
                match (index, manager_opt) {
                    (Some(idx), Some(manager)) => {
                        log::info!(
                            "{} starting V4L2 input: {} (index {})",
                            slot_prefix,
                            device_path,
                            idx
                        );
                        match manager.start_webcam(idx as usize, 1920, 1080, 30) {
                            Ok(_) => {
                                let mut state = lock(&self.shared_state);
                                let input = if is_second {
                                    &mut state.second_input
                                } else {
                                    &mut state.input
                                };
                                input.is_active = true;
                                input.input_type = rustjay_core::InputType::V4l2;
                                input.source_name = device_path;
                            }
                            Err(e) => {
                                log::error!("{} failed to start V4L2 input: {:?}", slot_prefix, e)
                            }
                        }
                    }
                    (None, _) => log::error!(
                        "{} StartV4l2: could not parse device index from '{}'",
                        slot_prefix,
                        device_path
                    ),
                    _ => {}
                }
            }
            InputCommand::StopInput => {
                if let Some(ref mut manager) = manager_opt {
                    manager.stop();
                    let mut state = lock(&self.shared_state);
                    let input = if is_second {
                        &mut state.second_input
                    } else {
                        &mut state.input
                    };
                    input.is_active = false;
                    input.source_name.clear();
                }
            }
            InputCommand::RefreshDevices => {
                if let Some(ref mut manager) = manager_opt {
                    manager.begin_refresh_devices();
                    if !is_second {
                        lock(&self.shared_state).input_discovering = true;
                    }
                }
                if !is_second {
                    self.process_audio_commands(AudioCommand::RefreshDevices);
                }
            }
            _ => {}
        }
    }

    fn process_output_commands(&mut self, command: OutputCommand) {
        match command {
            #[cfg(feature = "ndi")]
            OutputCommand::StartNdi => {
                if let Some(ref mut engine) = self.output_engine {
                    let (name, include_alpha) = {
                        let state = lock(&self.shared_state);
                        (
                            state.ndi_output.stream_name.clone(),
                            state.ndi_output.include_alpha,
                        )
                    };
                    match engine.start_ndi_output(&name, include_alpha) { Err(e) => {
                        log::error!("Failed to start NDI output: {:?}", e);
                    } _ => {
                        lock(&self.shared_state).ndi_output.is_active = true;
                    }}
                }
            }
            #[cfg(feature = "ndi")]
            OutputCommand::StopNdi => {
                if let Some(ref mut engine) = self.output_engine {
                    engine.stop_ndi_output();
                }
                lock(&self.shared_state).ndi_output.is_active = false;
            }
            #[cfg(target_os = "macos")]
            OutputCommand::StartSyphon => {
                if let Some(ref mut engine) = self.output_engine {
                    let name = lock(&self.shared_state).syphon_output.server_name.clone();
                    match engine.start_syphon_output(&name) { Err(e) => {
                        log::error!("Failed to start Syphon output: {:?}", e);
                    } _ => {
                        lock(&self.shared_state).syphon_output.enabled = true;
                    }}
                }
            }
            #[cfg(target_os = "macos")]
            OutputCommand::StopSyphon => {
                if let Some(ref mut engine) = self.output_engine {
                    engine.stop_syphon_output();
                }
                lock(&self.shared_state).syphon_output.enabled = false;
            }
            #[cfg(target_os = "windows")]
            OutputCommand::StartSpout { sender_name } => {
                if let Some(ref mut engine) = self.output_engine {
                    if let Err(e) = engine.start_spout_output(&sender_name) {
                        log::error!("Failed to start Spout output: {:?}", e);
                    } else {
                        let mut state = lock(&self.shared_state);
                        state.spout_output.sender_name = sender_name.clone();
                        state.spout_output.enabled = true;
                    }
                }
            }
            #[cfg(target_os = "windows")]
            OutputCommand::StopSpout => {
                if let Some(ref mut engine) = self.output_engine {
                    engine.stop_spout_output();
                }
                lock(&self.shared_state).spout_output.enabled = false;
            }
            #[cfg(target_os = "linux")]
            OutputCommand::StartV4l2 { device_path } => {
                if let Some(ref mut engine) = self.output_engine {
                    if let Err(e) = engine.start_v4l2_output(&device_path) {
                        log::error!("Failed to start V4L2 output: {:?}", e);
                    } else {
                        let mut state = lock(&self.shared_state);
                        state.v4l2_output.device_path = device_path.clone();
                        state.v4l2_output.enabled = true;
                    }
                }
            }
            #[cfg(target_os = "linux")]
            OutputCommand::StopV4l2 => {
                if let Some(ref mut engine) = self.output_engine {
                    engine.stop_v4l2_output();
                }
                lock(&self.shared_state).v4l2_output.enabled = false;
            }
            OutputCommand::StartRecording { path, codec } => {
                if let Some(ref mut engine) = self.output_engine {
                    let fps = lock(&self.shared_state).target_fps as f32;
                    let p = std::path::PathBuf::from(path);
                    match engine.start_recording(&p, fps, codec) { Err(e) => {
                        log::error!("Failed to start recording: {}", e);
                    } _ => {
                        lock(&self.shared_state).recording_active = true;
                    }}
                }
            }
            OutputCommand::StopRecording => {
                if let Some(ref mut engine) = self.output_engine {
                    engine.stop_recording();
                }
                lock(&self.shared_state).recording_active = false;
            }
            OutputCommand::ResizeOutput => {
                if let (Some(output_window), Some(ref mut engine)) =
                    (self.output_window.as_ref(), self.output_engine.as_mut())
                {
                    let (output_width, output_height, internal_width, internal_height) = {
                        let state = lock(&self.shared_state);
                        (
                            state.output_width,
                            state.output_height,
                            state.resolution.internal_width,
                            state.resolution.internal_height,
                        )
                    };
                    engine.resize(output_width, output_height);
                    engine.resize_render_target(internal_width, internal_height);
                    let _ = output_window.request_inner_size(winit::dpi::LogicalSize::new(
                        output_width,
                        output_height,
                    ));
                }
            }
            _ => {}
        }
    }

    fn process_audio_commands(&mut self, command: AudioCommand) {
        match command {
            AudioCommand::RefreshDevices => {
                let devices = list_audio_devices();
                log::info!("[Audio] Refreshed devices: {} found", devices.len());
                lock(&self.shared_state).audio.available_devices = devices;
            }
            AudioCommand::SelectDevice(device_name) => {
                log::info!("[Audio] Selecting device: {}", device_name);
                if let Some(ref mut analyzer) = self.audio_analyzer {
                    analyzer.stop();
                    match analyzer.start_with_device(Some(&device_name)) {
                        Ok(actual_name) => {
                            lock(&self.shared_state).audio.selected_device = Some(actual_name);
                        }
                        Err(e) => log::error!(
                            "Failed to start audio with device '{}': {}",
                            device_name,
                            e
                        ),
                    }
                }
            }
            AudioCommand::Start => {
                if let Some(ref mut analyzer) = self.audio_analyzer {
                    let device = lock(&self.shared_state).audio.selected_device.clone();
                    match analyzer.start_with_device(device.as_deref()) {
                        Ok(actual_name) => {
                            lock(&self.shared_state).audio.selected_device = Some(actual_name);
                        }
                        Err(e) => log::error!("Failed to start audio: {}", e),
                    }
                }
            }
            AudioCommand::Stop => {
                if let Some(ref mut analyzer) = self.audio_analyzer {
                    analyzer.stop();
                }
            }
            AudioCommand::SetFftSize(size) => {
                if let Some(ref mut analyzer) = self.audio_analyzer {
                    analyzer.set_fft_size(size);
                    let device = lock(&self.shared_state).audio.selected_device.clone();
                    match analyzer.start_with_device(device.as_deref()) {
                        Ok(actual_name) => {
                            lock(&self.shared_state).audio.selected_device = Some(actual_name);
                        }
                        Err(e) => {
                            log::error!("Failed to restart audio with FFT size {}: {}", size, e)
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn process_midi_commands(&mut self, command: MidiCommand) {
        match command {
            MidiCommand::RefreshDevices => {
                if let Some(ref mut manager) = self.midi_manager {
                    let devices = manager.refresh_devices();
                    log::info!("MIDI devices refreshed: {} found", devices.len());
                    let mut state = lock(&self.shared_state);
                    state.midi_available_devices = devices;
                }
            }
            MidiCommand::SelectDevice(device_name) => {
                if let Some(ref mut manager) = self.midi_manager {
                    match manager.connect(&device_name) {
                        Ok(()) => {
                            let mut state = lock(&self.shared_state);
                            state.midi_selected_device = Some(device_name);
                            state.midi_enabled = true;
                        }
                        Err(e) => {
                            log::error!(
                                "Failed to connect to MIDI device '{}': {}",
                                device_name,
                                e
                            );
                        }
                    }
                }
            }
            MidiCommand::StartLearn {
                param_path,
                param_name,
                min,
                max,
            } => {
                if let Some(ref mut manager) = self.midi_manager {
                    manager.start_learn(&param_path, &param_name, min, max);
                    let mut state = lock(&self.shared_state);
                    state.midi_learn_active = true;
                    state.midi_learning_param_name = Some(param_name);
                }
            }
            MidiCommand::CancelLearn => {
                if let Some(ref mut manager) = self.midi_manager {
                    manager.cancel_learn();
                    let mut state = lock(&self.shared_state);
                    state.midi_learn_active = false;
                    state.midi_learning_param_name = None;
                }
            }
            MidiCommand::ClearMappings => {
                if let Some(ref mut manager) = self.midi_manager {
                    if let Ok(mut state) = manager.state().lock() {
                        state.mappings.clear();
                    }
                }
            }
            MidiCommand::Disconnect => {
                if let Some(ref mut manager) = self.midi_manager {
                    manager.disconnect();
                    let mut state = lock(&self.shared_state);
                    state.midi_selected_device = None;
                    state.midi_enabled = false;
                }
            }
            MidiCommand::RestoreMappings(snapshots) => {
                if let Some(ref mut manager) = self.midi_manager {
                    if let Ok(mut midi_state) = manager.state().lock() {
                        midi_state.mappings.clear();
                        for s in snapshots {
                            midi_state.mappings.push(MidiMapping::new(
                                s.kind,
                                s.selector,
                                s.channel,
                                &s.name,
                                &s.param_path,
                                s.min_value,
                                s.max_value,
                            ));
                        }
                        log::info!(
                            "Restored {} MIDI mappings from preset",
                            midi_state.mappings.len()
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn process_modulation_commands(&mut self, command: ModulationCommand) {
        if matches!(command, ModulationCommand::None) {
            return;
        }
        // Clone the Arc while holding shared_state, then drop the guard before locking
        // modulation — prevents the nested-lock deadlock (lock hierarchy: never hold
        // shared_state while acquiring modulation from a different call path).
        let mod_arc = {
            let state = lock(&self.shared_state);
            Arc::clone(&state.modulation)
        };
        let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
        match command {
            ModulationCommand::None => {}
            ModulationCommand::AddSource(source) => {
                mod_eng.add_source(source);
            }
            ModulationCommand::AddSourceWithUuid { uuid, source } => {
                mod_eng.add_source_with_uuid(uuid, source);
            }
            ModulationCommand::RemoveSource(uuid) => {
                mod_eng.remove_source(&uuid);
            }
            ModulationCommand::Assign {
                param,
                source_id,
                amount,
                component,
            } => {
                mod_eng.assign(&param, &source_id, amount, component);
            }
            ModulationCommand::AssignModOnMod {
                target_uuid,
                param,
                modulator_uuid,
                amount,
            } => {
                mod_eng.assign_mod_on_mod(&target_uuid, &param, &modulator_uuid, amount);
            }
            ModulationCommand::ClearAssignments(param) => {
                mod_eng.clear_assignments(&param);
            }
            ModulationCommand::TriggerAdsr(uuid) => {
                mod_eng.trigger_adsr(&uuid);
            }
            ModulationCommand::ReleaseAdsr(uuid) => {
                mod_eng.release_adsr(&uuid);
            }
            ModulationCommand::RestoreEngine(engine) => {
                *mod_eng = engine;
            }
        }
    }

    fn process_osc_commands(&mut self, command: OscCommand) {
        match command {
            OscCommand::Start => {
                if let Some(ref mut server) = self.osc_server {
                    match server.start() { Err(e) => {
                        log::error!("Failed to start OSC server: {}", e);
                    } _ => {
                        lock(&self.shared_state).osc_enabled = true;
                    }}
                }
            }
            OscCommand::Stop => {
                if let Some(ref mut server) = self.osc_server {
                    server.stop();
                    lock(&self.shared_state).osc_enabled = false;
                }
            }
            OscCommand::SetPort(port) => {
                if let Some(ref mut server) = self.osc_server {
                    server.stop();
                    let host = lock(&self.shared_state).osc_host.clone();
                    let new_server = OscServer::new(&host, port, "/rustjay");
                    if let Ok(mut state) = new_server.state().lock() {
                        state.register_default_parameters();
                    }
                    *server = new_server;
                    let mut state = lock(&self.shared_state);
                    state.osc_port = port;
                    state.osc_enabled = false;
                }
            }
            OscCommand::RefreshAddresses => {
                if let Some(ref mut server) = self.osc_server {
                    if let Ok(mut state) = server.state().lock() {
                        state.register_default_parameters();
                    }
                }
            }
            _ => {}
        }
    }

    fn process_preset_commands(&mut self, command: PresetCommand) {
        match command {
            PresetCommand::Save { name } => {
                if let Some(ref mut bank) = self.preset_bank {
                    let mut preset = {
                        let state = lock(&self.shared_state);
                        rustjay_presets::Preset::from_state(&name, &state)
                    };
                    if let Some(ref plugin) = self.plugin {
                        preset.plugin_state = plugin.serialize_preset_state(&self.app_state);
                    }
                    match bank.add_preset(preset) {
                        Ok(index) => log::info!("Saved preset '{}' at index {}", name, index),
                        Err(e) => log::error!("Failed to save preset: {}", e),
                    }
                }
                self.sync_preset_names_to_state();
            }
            PresetCommand::Load(index) => {
                if let Some(ref mut bank) = self.preset_bank {
                    let plugin_state = bank.presets.get(index).and_then(|p| p.plugin_state.clone());
                    {
                        let mut state = lock(&self.shared_state);
                        if let Err(e) = bank.apply_preset(index, &mut state) {
                            log::error!("Failed to load preset: {}", e);
                        }
                        if let Some(ref plugin) = self.plugin {
                            if let Some(ref data) = plugin_state {
                                plugin.deserialize_preset_state(data, &mut self.app_state);
                            }
                            plugin.on_preset_applied(&mut self.app_state, &mut state);
                        }
                    }
                }
            }
            PresetCommand::Delete(index) => {
                if let Some(ref mut bank) = self.preset_bank {
                    if let Err(e) = bank.delete_preset(index) {
                        log::error!("Failed to delete preset: {}", e);
                    }
                }
                self.sync_preset_names_to_state();
            }
            PresetCommand::ApplySlot(slot) => {
                if let Some(ref mut bank) = self.preset_bank {
                    let plugin_state = bank
                        .get_slot(slot)
                        .and_then(|idx| bank.presets.get(idx).and_then(|p| p.plugin_state.clone()));
                    {
                        let mut state = lock(&self.shared_state);
                        if let Err(e) = bank.apply_slot(slot, &mut state) {
                            log::warn!("Failed to apply preset slot {}: {}", slot, e);
                        }
                        if let Some(ref plugin) = self.plugin {
                            if let Some(ref data) = plugin_state {
                                plugin.deserialize_preset_state(data, &mut self.app_state);
                            }
                            plugin.on_preset_applied(&mut self.app_state, &mut state);
                        }
                    }
                }
            }
            PresetCommand::AssignSlot { preset_index, slot } => {
                if let Some(ref mut bank) = self.preset_bank {
                    if let Err(e) = bank.assign_to_slot(preset_index, slot) {
                        log::error!("Failed to assign slot: {}", e);
                    }
                }
                self.sync_preset_names_to_state();
            }
            PresetCommand::Refresh => {
                if let Some(ref mut bank) = self.preset_bank {
                    if let Err(e) = bank.refresh() {
                        log::error!("Failed to refresh presets: {}", e);
                    }
                }
                self.sync_preset_names_to_state();
            }
            _ => {}
        }
    }

    fn sync_preset_names_to_state(&mut self) {
        if let Some(ref bank) = self.preset_bank {
            let names: Vec<String> = bank.presets.iter().map(|p| p.name.clone()).collect();
            let slot_names: [Option<String>; 8] =
                std::array::from_fn(|i| bank.get_slot_name(i + 1).map(|s| s.to_string()));
            let mut state = lock(&self.shared_state);
            state.preset_names = names;
            state.preset_quick_slot_names = slot_names;
        }
    }

    fn process_web_commands(&mut self, command: WebCommand) {
        match command {
            WebCommand::Start => {
                if let Some(ref mut server) = self.web_server {
                    match server.start() { Err(e) => {
                        log::error!("Failed to start web server: {}", e);
                    } _ => {
                        let token = server.get_token();
                        let full_url = server.get_full_url();
                        let mut state = lock(&self.shared_state);
                        state.web_enabled = true;
                        state.web_token = token;
                        state.web_full_url = full_url.clone();
                        log::info!("Web server started — access at {}", full_url);
                    }}
                }
            }
            WebCommand::Stop => {
                if let Some(ref mut server) = self.web_server {
                    server.stop();
                    let mut state = lock(&self.shared_state);
                    state.web_enabled = false;
                    state.web_token = String::new();
                    state.web_full_url = String::new();
                }
            }
            WebCommand::SetPort(port) => {
                if let Some(ref mut server) = self.web_server {
                    server.stop();
                    let (host, lan_trust) = {
                        let s = lock(&self.shared_state);
                        (s.web_host.clone(), s.web_lan_trust)
                    };
                    let config = WebConfig {
                        host,
                        port,
                        app_name: "rustjay".to_string(),
                        enabled: false,
                        lan_trust,
                        token: None,
                    };
                    let (new_server, cmd_tx) = WebServer::new(config);
                    *server = new_server;
                    self.web_command_tx = Some(cmd_tx);
                    let mut state = lock(&self.shared_state);
                    state.web_port = port;
                    state.web_enabled = false;
                    state.web_token = String::new();
                    state.web_full_url = String::new();
                }
            }
            WebCommand::SetLanTrust(enabled) => {
                if let Some(ref server) = self.web_server {
                    server.set_lan_trust(enabled);
                }
                lock(&self.shared_state).web_lan_trust = enabled;
            }
            _ => {}
        }

        if let Some(ref mut server) = self.web_server {
            while let Ok(cmd) = server.command_rx.try_recv() {
                match cmd {
                    WebServerCommand::Set { id, value } => {
                        if let Ok(mut state) = self.shared_state.lock() {
                            match id.as_str() {
                                "color/hue_shift" => {
                                    let v = value.clamp(-180.0, 180.0);
                                    state.hsb_params.hue_shift = v;
                                    state.hsb_param_bases.hue_shift = v;
                                    let (h, s, b) = (
                                        v,
                                        state.hsb_param_bases.saturation,
                                        state.hsb_param_bases.brightness,
                                    );
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/saturation" => {
                                    let v = value.clamp(0.0, 2.0);
                                    state.hsb_params.saturation = v;
                                    state.hsb_param_bases.saturation = v;
                                    let (h, s, b) = (
                                        state.hsb_param_bases.hue_shift,
                                        v,
                                        state.hsb_param_bases.brightness,
                                    );
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/brightness" => {
                                    let v = value.clamp(0.0, 2.0);
                                    state.hsb_params.brightness = v;
                                    state.hsb_param_bases.brightness = v;
                                    let (h, s, b) = (
                                        state.hsb_param_bases.hue_shift,
                                        state.hsb_param_bases.saturation,
                                        v,
                                    );
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/enabled" => state.color_enabled = value > 0.5,
                                "audio/amplitude" => state.audio.amplitude = value.clamp(0.0, 5.0),
                                "audio/smoothing" => state.audio.smoothing = value.clamp(0.0, 1.0),
                                "audio/enabled" => state.audio.enabled = value > 0.5,
                                "audio/normalize" => state.audio.normalize = value > 0.5,
                                "audio/pink_noise" => state.audio.pink_noise_shaping = value > 0.5,
                                "output/fullscreen" => state.output_fullscreen = value > 0.5,
                                _ => {
                                    // App-specific param resolver (e.g. Varda's hierarchical paths).
                                    let resolved = state
                                        .param_resolver
                                        .as_ref()
                                        .and_then(|r| r.resolve(&id))
                                        .unwrap_or(id);
                                    // Fallback: check if this is an effect-declared custom param.
                                    // Accept either category/id (web UI) or raw canonical id (API/OSC/MIDI).
                                    if let Some(desc) = state.param_descriptors.iter().find(|d| {
                                        format!("{}/{}", d.category.name().to_lowercase(), d.id)
                                            == resolved
                                            || d.id == resolved
                                    }) {
                                        let (desc_id, desc_min, desc_max) =
                                            (desc.id.clone(), desc.min, desc.max);
                                        state.set_param_base(
                                            &desc_id,
                                            value.clamp(desc_min, desc_max),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    WebServerCommand::Input(input_cmd) => match input_cmd {
                        rustjay_control::InputWebCommand::SelectDevice {
                            index,
                            width,
                            height,
                            fps,
                        } => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.input_command = InputCommand::StartWebcam {
                                    device_index: index,
                                    width,
                                    height,
                                    fps,
                                };
                            }
                        }
                        rustjay_control::InputWebCommand::StopInput => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.input_command = InputCommand::StopInput;
                            }
                        }
                        rustjay_control::InputWebCommand::RefreshDevices => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.input_command = InputCommand::RefreshDevices;
                            }
                        }
                    },
                    WebServerCommand::Control(ctrl_cmd) => match ctrl_cmd {
                        rustjay_control::ControlWebCommand::Osc { enabled } => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.osc_command = if enabled {
                                    OscCommand::Start
                                } else {
                                    OscCommand::Stop
                                };
                            }
                        }
                        rustjay_control::ControlWebCommand::OscSetPort { port } => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.osc_command = OscCommand::SetPort(port);
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiLearn { param_id } => {
                            let (name, min, max) = {
                                let state = lock(&self.shared_state);
                                state
                                    .param_descriptors
                                    .iter()
                                    .find(|d| {
                                        let full = format!(
                                            "{}/{}",
                                            d.category.name().to_lowercase(),
                                            d.id
                                        );
                                        full == param_id || d.id == param_id
                                    })
                                    .map(|d| (d.name.clone(), d.min, d.max))
                                    .unwrap_or_default()
                            };
                            if !name.is_empty() {
                                if let Ok(mut state) = self.shared_state.lock() {
                                    state.midi_command = MidiCommand::StartLearn {
                                        param_path: param_id,
                                        param_name: name,
                                        min,
                                        max,
                                    };
                                }
                            } else {
                                log::warn!("Web MidiLearn: unknown param_id '{}'", param_id);
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiLearnCancel => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.midi_command = MidiCommand::CancelLearn;
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiUnlearn { cc, channel } => {
                            if let Some(ref m) = self.midi_manager {
                                if let Ok(mut midi_st) = m.state().lock() {
                                    midi_st.mappings.retain(|mapping| {
                                        !(mapping.selector == cc && mapping.channel == channel)
                                    });
                                }
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiRefreshDevices => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.midi_command = MidiCommand::RefreshDevices;
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiSelectDevice { device } => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.midi_command = MidiCommand::SelectDevice(device);
                            }
                        }
                        rustjay_control::ControlWebCommand::MidiDisconnect => {
                            if let Ok(mut state) = self.shared_state.lock() {
                                state.midi_command = MidiCommand::Disconnect;
                            }
                        }
                    },
                    WebServerCommand::Modulation(mod_cmd) => match mod_cmd {
                        rustjay_control::ModulationWebCommand::LfoSet { slot, config } => {
                            // Clone Arc before dropping shared_state to avoid nested-lock deadlock.
                            let mod_arc = {
                                let state = lock(&self.shared_state);
                                Arc::clone(&state.modulation)
                            };
                            let uuid = format!("lfo_{slot}");
                            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                            // Preserve runtime phase state so the LFO doesn't jump on config change (S1).
                            let (existing_phase, existing_last_beat) = mod_eng.sources.iter()
                                .find(|s| s.uuid == uuid)
                                .and_then(|s| if let rustjay_core::modulation::ModulationSource::LFO { phase, last_beat_phase, .. } = s.source { Some((phase, last_beat_phase)) } else { None })
                                .unwrap_or((0.0, 0.0));
                            let waveform = match config.waveform {
                                rustjay_core::lfo::Waveform::Sine => rustjay_core::modulation::LFOWaveform::Sine,
                                rustjay_core::lfo::Waveform::Triangle => rustjay_core::modulation::LFOWaveform::Triangle,
                                rustjay_core::lfo::Waveform::Square => rustjay_core::modulation::LFOWaveform::Square,
                                rustjay_core::lfo::Waveform::Ramp | rustjay_core::lfo::Waveform::Saw => rustjay_core::modulation::LFOWaveform::Sawtooth,
                            };
                            let new_source = rustjay_core::modulation::ModulationSource::LFO {
                                waveform,
                                frequency: config.rate,
                                phase: existing_phase,
                                amplitude: config.amplitude,
                                bipolar: true,
                                tempo_sync: config.tempo_sync,
                                division: config.division,
                                phase_offset_degrees: config.phase_offset,
                                enabled: config.enabled,
                                last_beat_phase: existing_last_beat,
                            };
                            if let Some(idx) = mod_eng.sources.iter().position(|s| s.uuid == uuid) {
                                mod_eng.sources[idx].source = new_source;
                            } else {
                                mod_eng.add_source_with_uuid(uuid, new_source);
                            }
                            mod_eng.ensure_index();
                        }
                        rustjay_control::ModulationWebCommand::LfoEnable { slot, enabled } => {
                            let mod_arc = {
                                let state = lock(&self.shared_state);
                                Arc::clone(&state.modulation)
                            };
                            let uuid = format!("lfo_{slot}");
                            let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                            if let Some(entry) = mod_eng.sources.iter_mut().find(|s| s.uuid == uuid) {
                                if let rustjay_core::modulation::ModulationSource::LFO { enabled: ref mut e, .. } = entry.source {
                                    *e = enabled;
                                }
                            }
                        }
                        rustjay_control::ModulationWebCommand::AudioRoute {
                            param_id,
                            band,
                            depth,
                        } => {
                            let target = param_id_to_modulation_target(&param_id);
                            if let Ok(mut state) = self.shared_state.lock() {
                                let ids_to_remove: Vec<usize> = state
                                    .audio_routing
                                    .matrix
                                    .routes()
                                    .iter()
                                    .filter(|r| r.band == band && r.target == target)
                                    .map(|r| r.id)
                                    .collect();
                                for id in ids_to_remove {
                                    state.audio_routing.matrix.remove_route(id);
                                }
                                if let Some(id) = state.audio_routing.matrix.add_route(band, target)
                                {
                                    if let Some(route) =
                                        state.audio_routing.matrix.get_route_mut(id)
                                    {
                                        route.amount = depth;
                                    }
                                }
                            }
                        }
                        rustjay_control::ModulationWebCommand::AudioUnroute { param_id } => {
                            let target = param_id_to_modulation_target(&param_id);
                            if let Ok(mut state) = self.shared_state.lock() {
                                let ids_to_remove: Vec<usize> = state
                                    .audio_routing
                                    .matrix
                                    .routes()
                                    .iter()
                                    .filter(|r| r.target == target)
                                    .map(|r| r.id)
                                    .collect();
                                for id in ids_to_remove {
                                    state.audio_routing.matrix.remove_route(id);
                                }
                            }
                        }
                        rustjay_control::ModulationWebCommand::TapTempo => {
                            use std::time::{SystemTime, UNIX_EPOCH};
                            let now = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_secs_f64();
                            // Phase 1: do all audio mutations under shared_state; capture
                            // mod_arc only if this is a first-tap phase reset, then drop guard.
                            let phase_reset_arc = {
                                let mut state = lock(&self.shared_state);
                                let is_first_tap = now - state.audio.last_tap_time > 2.0;
                                let mod_arc = if is_first_tap {
                                    state.audio.tap_times.clear();
                                    Some(Arc::clone(&state.modulation))
                                } else {
                                    None
                                };
                                state.audio.tap_times.push(now);
                                state.audio.last_tap_time = now;
                                if state.audio.tap_times.len() > 8 {
                                    state.audio.tap_times.remove(0);
                                }
                                state.audio.beat_phase = 0.0;
                                if state.audio.tap_times.len() >= 2 {
                                    let n = state.audio.tap_times.len();
                                    let avg_interval: f64 = state.audio.tap_times.windows(2)
                                        .map(|w| w[1] - w[0])
                                        .sum::<f64>() / (n - 1) as f64;
                                    if avg_interval > 0.1 && avg_interval < 3.0 {
                                        state.audio.bpm = (60.0 / avg_interval) as f32;
                                        state.audio.tap_tempo_info =
                                            format!("{:.1} BPM ({} taps)", state.audio.bpm, n);
                                    }
                                } else {
                                    state.audio.tap_tempo_info = "Tap again…".to_string();
                                }
                                mod_arc
                                // shared_state guard drops here
                            };
                            // Phase 2: reset LFO phases without holding shared_state.
                            if let Some(mod_arc) = phase_reset_arc {
                                let mut mod_eng = mod_arc.lock().unwrap_or_else(|e| e.into_inner());
                                for entry in &mut mod_eng.sources {
                                    if let rustjay_core::modulation::ModulationSource::LFO { ref mut phase, .. } = entry.source {
                                        *phase = 0.0;
                                    }
                                }
                            }
                            server.modulation_dirty = true;
                        }
                    },
                    WebServerCommand::Preset(preset_cmd) => {
                        match preset_cmd {
                            rustjay_control::PresetWebCommand::List => {
                                // Caller reads preset list from state; broadcast handled elsewhere.
                            }
                            rustjay_control::PresetWebCommand::Save { name } => {
                                let valid = !name.is_empty()
                                    && name.len() <= 64
                                    && !name.contains('/')
                                    && !name.contains('\\')
                                    && !name.contains("..");
                                if valid {
                                    if let Ok(mut state) = self.shared_state.lock() {
                                        state.preset_command = PresetCommand::Save { name };
                                    }
                                } else {
                                    log::warn!("Web preset save: invalid name");
                                }
                            }
                            rustjay_control::PresetWebCommand::Load { index } => {
                                if let Ok(mut state) = self.shared_state.lock() {
                                    state.preset_command = PresetCommand::Load(index);
                                }
                            }
                            rustjay_control::PresetWebCommand::Delete { index } => {
                                if let Ok(mut state) = self.shared_state.lock() {
                                    state.preset_command = PresetCommand::Delete(index);
                                }
                            }
                        }
                    }
                    WebServerCommand::Output(output_cmd) => {
                        let core_cmd = match output_cmd {
                            #[cfg(feature = "ndi")]
                            rustjay_control::OutputWebCommand::StartNdi => OutputCommand::StartNdi,
                            #[cfg(not(feature = "ndi"))]
                            rustjay_control::OutputWebCommand::StartNdi => {
                                log::warn!("NDI output not compiled in");
                                OutputCommand::None
                            }
                            #[cfg(feature = "ndi")]
                            rustjay_control::OutputWebCommand::StopNdi => OutputCommand::StopNdi,
                            #[cfg(not(feature = "ndi"))]
                            rustjay_control::OutputWebCommand::StopNdi => {
                                log::warn!("NDI output not compiled in");
                                OutputCommand::None
                            }
                            #[cfg(target_os = "macos")]
                            rustjay_control::OutputWebCommand::StartSyphon => {
                                OutputCommand::StartSyphon
                            }
                            #[cfg(not(target_os = "macos"))]
                            rustjay_control::OutputWebCommand::StartSyphon => {
                                log::warn!("Syphon output is only available on macOS");
                                OutputCommand::None
                            }
                            #[cfg(target_os = "macos")]
                            rustjay_control::OutputWebCommand::StopSyphon => {
                                OutputCommand::StopSyphon
                            }
                            #[cfg(not(target_os = "macos"))]
                            rustjay_control::OutputWebCommand::StopSyphon => {
                                log::warn!("Syphon output is only available on macOS");
                                OutputCommand::None
                            }
                            #[cfg(target_os = "windows")]
                            rustjay_control::OutputWebCommand::StartSpout { sender_name } => {
                                OutputCommand::StartSpout { sender_name }
                            }
                            #[cfg(not(target_os = "windows"))]
                            rustjay_control::OutputWebCommand::StartSpout { .. } => {
                                log::warn!("Spout output is only available on Windows");
                                OutputCommand::None
                            }
                            #[cfg(target_os = "windows")]
                            rustjay_control::OutputWebCommand::StopSpout => {
                                OutputCommand::StopSpout
                            }
                            #[cfg(not(target_os = "windows"))]
                            rustjay_control::OutputWebCommand::StopSpout => {
                                log::warn!("Spout output is only available on Windows");
                                OutputCommand::None
                            }
                            #[cfg(target_os = "linux")]
                            rustjay_control::OutputWebCommand::StartV4l2 { device_path } => {
                                OutputCommand::StartV4l2 { device_path }
                            }
                            #[cfg(not(target_os = "linux"))]
                            rustjay_control::OutputWebCommand::StartV4l2 { .. } => {
                                log::warn!("V4L2 output is only available on Linux");
                                OutputCommand::None
                            }
                            #[cfg(target_os = "linux")]
                            rustjay_control::OutputWebCommand::StopV4l2 => OutputCommand::StopV4l2,
                            #[cfg(not(target_os = "linux"))]
                            rustjay_control::OutputWebCommand::StopV4l2 => {
                                log::warn!("V4L2 output is only available on Linux");
                                OutputCommand::None
                            }
                            rustjay_control::OutputWebCommand::ResizeOutput => {
                                OutputCommand::ResizeOutput
                            }
                        };
                        if let Ok(mut state) = self.shared_state.lock() {
                            state.output_command = core_cmd;
                        }
                    }
                    WebServerCommand::Audio(audio_cmd) => {
                        let core_cmd = match audio_cmd {
                            rustjay_control::AudioWebCommand::Start => AudioCommand::Start,
                            rustjay_control::AudioWebCommand::Stop => AudioCommand::Stop,
                            rustjay_control::AudioWebCommand::RefreshDevices => {
                                AudioCommand::RefreshDevices
                            }
                            rustjay_control::AudioWebCommand::SelectDevice { device } => {
                                AudioCommand::SelectDevice(device)
                            }
                            rustjay_control::AudioWebCommand::SetFftSize { size } => {
                                AudioCommand::SetFftSize(size)
                            }
                        };
                        if let Ok(mut state) = self.shared_state.lock() {
                            state.audio_command = core_cmd;
                        }
                    }
                    WebServerCommand::Link(link_cmd) => {
                        let core_cmd = match link_cmd {
                            rustjay_control::LinkWebCommand::Enable => LinkCommand::Enable,
                            rustjay_control::LinkWebCommand::Disable => LinkCommand::Disable,
                            rustjay_control::LinkWebCommand::SetQuantum { quantum } => {
                                LinkCommand::SetQuantum(quantum)
                            }
                        };
                        if let Ok(mut state) = self.shared_state.lock() {
                            state.link_command = core_cmd;
                        }
                    }
                    WebServerCommand::ProDj(prodj_cmd) => {
                        let core_cmd = match prodj_cmd {
                            rustjay_control::ProDjWebCommand::Start => ProDjCommand::Start,
                            rustjay_control::ProDjWebCommand::Stop => ProDjCommand::Stop,
                        };
                        if let Ok(mut state) = self.shared_state.lock() {
                            state.prodj_command = core_cmd;
                        }
                    }
                }

                // MIDI mapping change detection (WR-3.3 / WR-6)
                if let Some(ref m) = self.midi_manager {
                    if let Ok(midi_st) = m.state().lock() {
                        let current: Vec<rustjay_core::MidiMappingSnapshot> = midi_st
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
                        if current != self.last_broadcast_mappings {
                            self.last_broadcast_mappings = current;
                            server.control_dirty = true;
                        }
                    }
                }
            }
        }
    }

    fn process_link_commands(&mut self, command: LinkCommand) {
        match command {
            LinkCommand::None => {}
            LinkCommand::Enable => {
                let mut state = lock(&self.shared_state);
                state.link.enabled = true;
                log::info!("[Link] Enable requested");
            }
            LinkCommand::Disable => {
                let mut state = lock(&self.shared_state);
                state.link.enabled = false;
                log::info!("[Link] Disable requested");
            }
            LinkCommand::SetQuantum(q) => {
                let mut state = lock(&self.shared_state);
                state.link.quantum = q.max(1.0);
                log::info!("[Link] Quantum set to {}", q);
            }
        }
    }

    fn process_prodj_commands(&mut self, command: ProDjCommand) {
        match command {
            ProDjCommand::None => {}
            ProDjCommand::Start => {
                let mut state = lock(&self.shared_state);
                state.prodj.enabled = true;
                log::info!("[ProDJ] Start requested");
            }
            ProDjCommand::Stop => {
                let mut state = lock(&self.shared_state);
                state.prodj.enabled = false;
                state.prodj.devices.clear();
                state.prodj.master_bpm = 0.0;
                state.prodj.master_beat_phase = 0.0;
                state.prodj.current_track_artist.clear();
                state.prodj.current_track_title.clear();
                log::info!("[ProDJ] Stop requested");
            }
        }
    }
}
