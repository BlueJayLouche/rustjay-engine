use super::App;
use rustjay_core::EffectPlugin;
use rustjay_audio::list_audio_devices;
use rustjay_core::{AudioCommand, InputCommand, OutputCommand, MidiCommand, OscCommand, PresetCommand, EngineState, WebCommand, LinkCommand, ProDjCommand};
use rustjay_control::{MidiMapping, OscServer};
use rustjay_control::{WebServer, WebConfig, WebCommand as WebServerCommand};

fn lock(state: &std::sync::Mutex<EngineState>) -> std::sync::MutexGuard<'_, EngineState> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// All pending commands popped from shared state in a single lock acquisition.
struct PendingCommands {
    input:         InputCommand,
    second_input:  InputCommand,
    output:        OutputCommand,
    audio:         AudioCommand,
    midi:          MidiCommand,
    osc:           OscCommand,
    preset:        PresetCommand,
    web:           WebCommand,
    link:          LinkCommand,
    prodj:         ProDjCommand,
}

impl<P: EffectPlugin> App<P> {
    pub(super) fn dispatch_commands(&mut self) {
        // Pop all nine command slots in a single lock — saves 8 mutex acquires per frame
        // over the previous one-lock-per-process_* pattern.
        let cmds = {
            let mut s = lock(&self.shared_state);
            PendingCommands {
                input:         std::mem::replace(&mut s.input_command,         InputCommand::None),
                second_input:  std::mem::replace(&mut s.second_input_command,  InputCommand::None),
                output:        std::mem::replace(&mut s.output_command,        OutputCommand::None),
                audio:         std::mem::replace(&mut s.audio_command,         AudioCommand::None),
                midi:          std::mem::replace(&mut s.midi_command,          MidiCommand::None),
                osc:           std::mem::replace(&mut s.osc_command,           OscCommand::None),
                preset:        std::mem::replace(&mut s.preset_command,        PresetCommand::None),
                web:           std::mem::replace(&mut s.web_command,           WebCommand::None),
                link:          std::mem::replace(&mut s.link_command,          LinkCommand::None),
                prodj:         std::mem::replace(&mut s.prodj_command,         ProDjCommand::None),
            }
        };
        self.process_input_commands(cmds.input);
        self.process_second_input_commands(cmds.second_input);
        self.process_output_commands(cmds.output);
        self.process_audio_commands(cmds.audio);
        self.process_midi_commands(cmds.midi);
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
            InputCommand::StartWebcam { device_index, width, height, fps } => {
                log::info!("{} starting webcam: device={}", slot_prefix, device_index);
                if let Some(ref mut manager) = manager_opt {
                    match manager.start_webcam(device_index, width, height, fps) {
                        Ok(_) => {
                            let mut state = lock(&self.shared_state);
                            let input = if is_second { &mut state.second_input } else { &mut state.input };
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
                            let input = if is_second { &mut state.second_input } else { &mut state.input };
                            input.is_active = true;
                            input.input_type = rustjay_core::InputType::Ndi;
                            input.source_name = source_name;
                        }
                        Err(e) => log::error!("{} failed to start NDI: {:?}", slot_prefix, e),
                    }
                }
            }
            #[cfg(target_os = "macos")]
            InputCommand::StartSyphon { server_name, server_uuid } => {
                log::info!("{} starting Syphon: {} (uuid={})", slot_prefix, server_name, server_uuid);
                if let Some(ref mut manager) = manager_opt {
                    match manager.start_syphon(&server_name, &server_uuid) {
                        Ok(_) => {
                            let mut state = lock(&self.shared_state);
                            let input = if is_second { &mut state.second_input } else { &mut state.input };
                            input.is_active = true;
                            input.input_type = rustjay_core::InputType::Syphon;
                            input.source_name = server_name;
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
                            let input = if is_second { &mut state.second_input } else { &mut state.input };
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
                        log::info!("{} starting V4L2 input: {} (index {})", slot_prefix, device_path, idx);
                        match manager.start_webcam(idx as usize, 1920, 1080, 30) {
                            Ok(_) => {
                                let mut state = lock(&self.shared_state);
                                let input = if is_second { &mut state.second_input } else { &mut state.input };
                                input.is_active = true;
                                input.input_type = rustjay_core::InputType::V4l2;
                                input.source_name = device_path;
                            }
                            Err(e) => log::error!("{} failed to start V4L2 input: {:?}", slot_prefix, e),
                        }
                    }
                    (None, _) => log::error!("{} StartV4l2: could not parse device index from '{}'", slot_prefix, device_path),
                    _ => {}
                }
            }
            InputCommand::StopInput => {
                if let Some(ref mut manager) = manager_opt {
                    manager.stop();
                    let mut state = lock(&self.shared_state);
                    let input = if is_second { &mut state.second_input } else { &mut state.input };
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
                        (state.ndi_output.stream_name.clone(), state.ndi_output.include_alpha)
                    };
                    if let Err(e) = engine.start_ndi_output(&name, include_alpha) {
                        log::error!("Failed to start NDI output: {:?}", e);
                    } else {
                        lock(&self.shared_state).ndi_output.is_active = true;
                    }
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
                    if let Err(e) = engine.start_syphon_output(&name) {
                        log::error!("Failed to start Syphon output: {:?}", e);
                    } else {
                        lock(&self.shared_state).syphon_output.enabled = true;
                    }
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
            OutputCommand::ResizeOutput => {
                if let (Some(ref output_window), Some(ref mut engine)) =
                    (self.output_window.as_ref(), self.output_engine.as_mut())
                {
                    let (output_width, output_height, internal_width, internal_height) = {
                        let state = lock(&self.shared_state);
                        (state.output_width, state.output_height,
                         state.resolution.internal_width, state.resolution.internal_height)
                    };
                    engine.resize(output_width, output_height);
                    engine.resize_render_target(internal_width, internal_height);
                    let _ = output_window.request_inner_size(winit::dpi::LogicalSize::new(output_width, output_height));
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
                        Err(e) => log::error!("Failed to start audio with device '{}': {}", device_name, e),
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
                        Err(e) => log::error!("Failed to restart audio with FFT size {}: {}", size, e),
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
                            log::error!("Failed to connect to MIDI device '{}': {}", device_name, e);
                        }
                    }
                }
            }
            MidiCommand::StartLearn { param_path, param_name, min, max } => {
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
                                s.kind, s.selector, s.channel,
                                &s.name, &s.param_path,
                                s.min_value, s.max_value,
                            ));
                        }
                        log::info!("Restored {} MIDI mappings from preset", midi_state.mappings.len());
                    }
                }
            }
            _ => {}
        }
    }

    fn process_osc_commands(&mut self, command: OscCommand) {

        match command {
            OscCommand::Start => {
                if let Some(ref mut server) = self.osc_server {
                    if let Err(e) = server.start() {
                        log::error!("Failed to start OSC server: {}", e);
                    } else {
                        lock(&self.shared_state).osc_enabled = true;
                    }
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
                    let plugin_state = bank.get_slot(slot)
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
            let slot_names: [Option<String>; 8] = std::array::from_fn(|i| {
                bank.get_slot_name(i + 1).map(|s| s.to_string())
            });
            let mut state = lock(&self.shared_state);
            state.preset_names = names;
            state.preset_quick_slot_names = slot_names;
        }
    }

    fn process_web_commands(&mut self, command: WebCommand) {

        match command {
            WebCommand::Start => {
                if let Some(ref mut server) = self.web_server {
                    if let Err(e) = server.start() {
                        log::error!("Failed to start web server: {}", e);
                    } else {
                        let token = server.get_token();
                        let full_url = server.get_full_url();
                        let mut state = lock(&self.shared_state);
                        state.web_enabled = true;
                        state.web_token = token;
                        state.web_full_url = full_url.clone();
                        log::info!("Web server started — access at {}", full_url);
                    }
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
                    let config = WebConfig { host, port, app_name: "rustjay".to_string(), enabled: false, lan_trust };
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
                                    state.hsb_params.hue_shift = value.clamp(-180.0, 180.0);
                                    let (h, s, b) = (state.hsb_params.hue_shift, state.hsb_params.saturation, state.hsb_params.brightness);
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/saturation" => {
                                    state.hsb_params.saturation = value.clamp(0.0, 2.0);
                                    let (h, s, b) = (state.hsb_params.hue_shift, state.hsb_params.saturation, state.hsb_params.brightness);
                                    state.audio_routing.update_base_values(h, s, b);
                                }
                                "color/brightness" => {
                                    state.hsb_params.brightness = value.clamp(0.0, 2.0);
                                    let (h, s, b) = (state.hsb_params.hue_shift, state.hsb_params.saturation, state.hsb_params.brightness);
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
                                    // Fallback: check if this is an effect-declared custom param
                                    if let Some(desc) = state.param_descriptors.iter().find(|d| {
                                        format!("{}/{}", d.category.name().to_lowercase(), d.id) == id
                                    }) {
                                        let (desc_id, desc_min, desc_max) = (desc.id.clone(), desc.min, desc.max);
                                        state.set_param_base(&desc_id, value.clamp(desc_min, desc_max));
                                    }
                                }
                            }
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
