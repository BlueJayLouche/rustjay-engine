//! GUI — egui tabs for Varda's panels.
//!
//! Each tab is a non-replacing `AnyEguiTab` (it gets its own sidebar button via
//! the engine host) so the built-in tabs (incl. the working LFO/MIDI panels)
//! stay available alongside them. Params are driven through
//! `engine.get_param*` / `set_param_base` (canonical ids) so the GUI stays
//! co-equal with MIDI/OSC/HTTP/LFO. Per-item `ui.push_id(uuid)` scopes avoid
//! egui id collisions across channels/decks/FX.
//!
//! See VARDA_PORT.md §5 and `examples/delta-egui`.

/// Mixer tab — crossfader, per-channel opacity, master FX.
pub struct MixerTab {
    /// Async result from the native file picker for master FX.
    pending_effect: std::sync::Arc<std::sync::Mutex<Option<crate::PendingEffect>>>,
}

impl Default for MixerTab {
    fn default() -> Self {
        Self {
            pending_effect: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// Active tab in the Add Source section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AddSourceTab {
    File,
    Camera,
    Ndi,
    Syphon,
}

/// Deck tab — source picker, opacity/blend/scaling, deck FX.
pub struct DeckTab {
    /// Target channel UUID for "Add to channel" actions.
    selected_channel_uuid: String,
    add_tab: AddSourceTab,
    file_path: String,
    selected_camera_index: usize,
    selected_ndi_index: usize,
    selected_syphon_index: usize,
    /// Async result from the native file picker (source files).
    pending_file: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    /// Async result from the native file picker (effect shaders).
    pending_effect: std::sync::Arc<std::sync::Mutex<Option<crate::PendingEffect>>>,
}

impl Default for DeckTab {
    fn default() -> Self {
        Self {
            selected_channel_uuid: String::new(),
            add_tab: AddSourceTab::File,
            file_path: String::new(),
            selected_camera_index: 0,
            selected_ndi_index: 0,
            selected_syphon_index: 0,
            pending_file: std::sync::Arc::new(std::sync::Mutex::new(None)),
            pending_effect: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// Effects / Library tab — registry list + add/enable/reorder.
pub struct EffectsTab {
    /// Target channel UUID for "Add to channel" actions.
    selected_channel_uuid: String,
    /// Manual stream URL input.
    stream_url: String,
    /// Manual stream name input.
    stream_name: String,
    /// Manual stream kind (srt / hls / dash / rtmp).
    stream_kind: String,
    /// Async result from the native file picker (effect shaders).
    pending_effect: std::sync::Arc<std::sync::Mutex<Option<crate::PendingEffect>>>,
}

impl Default for EffectsTab {
    fn default() -> Self {
        Self {
            selected_channel_uuid: String::new(),
            stream_url: String::new(),
            stream_name: String::new(),
            stream_kind: "rtmp".to_string(),
            pending_effect: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

/// Sequencer tab — transition sequences.
pub struct SequencerTab;

/// MIDI tab — device select, learn/unlearn, mapping table.
pub struct MidiTab;

/// Stage tab — 2D surface editor, warp handles, import.
pub struct StageTab {
    #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
    import_path: String,
    /// Canvas zoom factor relative to fit-to-panel. `1.0` = the canvas exactly
    /// fits the available area; larger values enlarge the canvas (pixel-perfect
    /// editing) and pan via a scroll area. Only meaningful in edit mode.
    #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
    canvas_zoom: f32,
    /// When true, the canvas can be enlarged beyond fit (zoom slider active) for
    /// precise surface mapping. When false, the canvas fits the panel for a
    /// clean live preview of the master output.
    #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
    edit_mode: bool,
    /// When true, the Stage preview overlays lighting segment regions and lets
    /// the user drag them to set sampling rectangles.
    #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
    lighting_regions_active: bool,
    /// Currently selected lighting segment for editing on the Stage preview.
    /// `(lighting_output_index, segment_index)`.
    #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
    selected_light_segment: Option<(usize, usize)>,
}

impl Default for StageTab {
    fn default() -> Self {
        Self::new()
    }
}

impl StageTab {
    pub fn new() -> Self {
        Self {
            #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
            import_path: String::new(),
            #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
            canvas_zoom: 1.0,
            #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
            edit_mode: false,
            #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
            lighting_regions_active: false,
            #[cfg(all(feature = "mixer", feature = "egui", feature = "projection"))]
            selected_light_segment: None,
        }
    }
}

/// Outputs tab — window/display/NDI/stream/record assignment.
pub struct OutputsTab {
    recording_path: String,
    recording_codec: rustjay_core::RecorderCodec,
    /// Async result from the native save dialog.
    pending_save_path: std::sync::Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
}

impl Default for OutputsTab {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputsTab {
    pub fn new() -> Self {
        Self {
            recording_path: String::from("recording.mp4"),
            recording_codec: rustjay_core::RecorderCodec::H264,
            pending_save_path: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    #[cfg(feature = "projection")]
    /// Generate an auto-incrementing recording path.
    fn auto_record_path(&self, name: &str) -> std::path::PathBuf {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let ext = match self.recording_codec {
            rustjay_core::RecorderCodec::ProRes422 => "mov",
            _ => "mp4",
        };
        let dir = std::path::PathBuf::from("recordings");
        std::fs::create_dir_all(&dir).ok();
        dir.join(format!("{}_{}.{}", name, ts, ext))
    }

    #[cfg(feature = "projection")]
    fn io_codec(&self) -> rustjay_io::RecorderCodec {
        match self.recording_codec {
            rustjay_core::RecorderCodec::H264 => rustjay_io::RecorderCodec::H264,
            rustjay_core::RecorderCodec::H265 => rustjay_io::RecorderCodec::H265,
            rustjay_core::RecorderCodec::AV1 => rustjay_io::RecorderCodec::AV1,
            rustjay_core::RecorderCodec::ProRes422 => rustjay_io::RecorderCodec::ProRes422,
        }
    }
}

/// Inspector tab — context panel for selected node.
pub struct InspectorTab;

#[cfg(all(feature = "mixer", feature = "egui"))]
mod egui_impl {
    use super::*;
    use crate::graph::DeckCompositor;
    use crate::VardaAppState;
    use rustjay_core::EngineState;
    use rustjay_engine::prelude::*;
    use rustjay_mixer::BlendMode;

    /// Helper: draw a blend-mode combo bound to a canonical engine param key.
    fn blend_combo(ui: &mut egui::Ui, engine: &mut EngineState, key: &str, label: &str) {
        let mut idx = engine.get_param_base(key).unwrap_or(0.0).round() as usize;
        let prev = idx;
        let names: Vec<&str> = BlendMode::all().iter().map(|m| m.short_name()).collect();
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(key)
                .width(ui.available_width())
                .selected_text(*names.get(idx).unwrap_or(&"???"))
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(idx == i, *name).clicked() {
                            idx = i;
                        }
                    }
                });
        });
        if idx != prev {
            engine.set_param_base(key, idx as f32);
        }
    }

    /// Helper: draw a loop-mode combo bound to a canonical engine param key.
    /// 0 = None, 1 = Loop, 2 = PingPong (matches FfmpegSource/HapSource).
    fn loop_combo(ui: &mut egui::Ui, engine: &mut EngineState, key: &str, label: &str) {
        let mut idx = engine.get_param_base(key).unwrap_or(1.0).round() as usize;
        let prev = idx;
        let names = ["None", "Loop", "PingPong"];
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(key)
                .width(ui.available_width())
                .selected_text(*names.get(idx).unwrap_or(&"???"))
                .show_ui(ui, |ui| {
                    for (i, name) in names.iter().enumerate() {
                        if ui.selectable_label(idx == i, *name).clicked() {
                            idx = i;
                        }
                    }
                });
        });
        if idx != prev {
            engine.set_param_base(key, idx as f32);
        }
    }

    /// Helper: spawn a native ISF-shader file picker on a background thread; the
    /// chosen path is delivered into `pending` (polled next frame) and the UI is
    /// repainted. Shared by every "Add FX" button (deck / channel / master).
    fn spawn_effect_picker(
        pending: &std::sync::Arc<std::sync::Mutex<Option<crate::PendingEffect>>>,
        ctx: &egui::Context,
        target: crate::EffectTarget,
    ) {
        let pending = pending.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("ISF Shader", &["fs"])
                .set_title("Pick an ISF shader (.fs)")
                .pick_file()
            {
                if let Ok(mut guard) = pending.lock() {
                    *guard = Some(crate::PendingEffect { path, target });
                }
                ctx.request_repaint();
            }
        });
    }

    /// Helper: render an FX chain as an enable-checkbox + remove-button list,
    /// applying removals in place. Returns `true` if a slot was removed — a
    /// structural edit the caller should surface via `params_dirty_request` so
    /// the plugin re-registers parameters and drops the orphaned descriptors.
    fn fx_chain_ui(ui: &mut egui::Ui, chain: &mut Vec<rustjay_mixer::EffectSlot>) -> bool {
        let mut removals: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < chain.len() {
            let mut enabled = chain[i].enabled;
            let label = chain[i].effect.label().to_string();
            ui.push_id(i, |ui| {
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut enabled, &label).changed() {
                        chain[i].enabled = enabled;
                    }
                    if ui.small_button("✖").clicked() {
                        removals.push(i);
                    }
                });
            });
            i += 1;
        }
        let removed = !removals.is_empty();
        for idx in removals.into_iter().rev() {
            chain.remove(idx);
        }
        removed
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MixerTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for MixerTab {
        fn name(&self) -> &str {
            "Mixer"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_mut::<VardaAppState>()
                .expect("MixerTab expects VardaAppState");

            // Poll async file picker result.
            if let Ok(mut guard) = self.pending_effect.lock() {
                if let Some(req) = guard.take() {
                    state.pending_effects.push(req);
                }
            }

            // Cmd+S manual save
            if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
                state.save_workspace();
            }

            ui.heading("Mixer");
            ui.separator();
            param_slider(ui, engine, "crossfader", "Crossfader", 0.0, 1.0);
            ui.separator();

            let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

            for ch in &mixer.channels {
                ui.push_id(&ch.uuid, |ui| {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new(&ch.name).strong());
                        let opacity_key = format!("ch_{}_opacity", ch.uuid);
                        param_slider(ui, engine, &opacity_key, "Opacity", 0.0, 1.0);
                        blend_combo(ui, engine, &format!("ch_{}_blend", ch.uuid), "Blend:");
                    });
                });
            }

            // Master FX list + add button
            ui.separator();
            ui.label(egui::RichText::new("Master FX").strong());
            if fx_chain_ui(ui, &mut mixer.master) {
                state.params_dirty_request = true;
            }
            if ui.button("➕ Add Master FX…").clicked() {
                spawn_effect_picker(&self.pending_effect, ui.ctx(), crate::EffectTarget::Master);
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // DeckTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for DeckTab {
        fn name(&self) -> &str {
            "Deck"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_mut::<VardaAppState>()
                .expect("DeckTab expects VardaAppState");

            // Poll async file picker result.
            if let Ok(mut guard) = self.pending_effect.lock() {
                if let Some(req) = guard.take() {
                    state.pending_effects.push(req);
                }
            }

            ui.heading("Decks");
            ui.separator();

            let mut removals: Vec<crate::PendingRemoval> = Vec::new();
            // Set when an FX is removed in place, so prepare() re-registers params.
            let mut fx_removed = false;

            {
                let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

                for ch in &mut mixer.channels {
                    ui.collapsing(&ch.name, |ui| {
                        let Some(compositor) = ch.effect.as_any_mut() else {
                            return;
                        };
                        let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() else {
                            return;
                        };

                        for deck in &mut compositor.decks {
                            ui.push_id(deck.uuid.clone(), |ui| {
                                ui.group(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(egui::RichText::new(&deck.name).strong());
                                        ui.label(
                                            egui::RichText::new(format!("[{:?}]", deck.source_kind))
                                                .monospace()
                                                .color(ui.visuals().weak_text_color()),
                                        );
                                        if ui.small_button("✖").clicked() {
                                            removals.push(crate::PendingRemoval {
                                                channel_uuid: ch.uuid.clone(),
                                                deck_uuid: deck.uuid.clone(),
                                            });
                                        }
                                    });
                                    param_slider(ui, engine, &deck.opacity_key, "Opacity", 0.0, 1.0);
                                    blend_combo(ui, engine, &deck.blend_key, "Blend:");

                                    // Deck FX list + add button
                                    if !deck.chain.is_empty() {
                                        ui.label("FX:");
                                    }
                                    if fx_chain_ui(ui, &mut deck.chain) {
                                        fx_removed = true;
                                    }
                                    if ui.small_button("➕ Add FX…").clicked() {
                                        spawn_effect_picker(
                                            &self.pending_effect,
                                            ui.ctx(),
                                            crate::EffectTarget::Deck {
                                                channel_uuid: ch.uuid.clone(),
                                                deck_uuid: deck.uuid.clone(),
                                            },
                                        );
                                    }

                                    // Playback controls for video sources.
                                    if deck.source_kind == crate::sources::SourceKind::Video {
                                        ui.separator();
                                        let prefix = deck.full_prefix();

                                        let playing_key = format!("{}playing", prefix);
                                        let mut playing =
                                            engine.get_param_base(&playing_key).unwrap_or(1.0) > 0.5;
                                        if ui.checkbox(&mut playing, "Playing").changed() {
                                            engine.set_param_base(
                                                &playing_key,
                                                if playing { 1.0 } else { 0.0 },
                                            );
                                        }

                                        param_slider(
                                            ui,
                                            engine,
                                            &format!("{}speed", prefix),
                                            "Speed",
                                            -5.0,
                                            5.0,
                                        );
                                        loop_combo(
                                            ui,
                                            engine,
                                            &format!("{}loop", prefix),
                                            "Loop:",
                                        );
                                        param_slider(
                                            ui,
                                            engine,
                                            &format!("{}position", prefix),
                                            "Position",
                                            0.0,
                                            1.0,
                                        );
                                    }
                                });
                            });
                        }
                    });
                }
            }

            for req in removals {
                state.pending_removals.push(req);
            }
            if fx_removed {
                state.params_dirty_request = true;
            }

            ui.separator();
            ui.heading("Add Source");

            // Channel selector.
            let channel_names: Vec<(String, String)> = {
                let mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());
                mixer.channels.iter().map(|c| (c.uuid.clone(), c.name.clone())).collect()
            };
            if self.selected_channel_uuid.is_empty() {
                if let Some((uuid, _)) = channel_names.first() {
                    self.selected_channel_uuid = uuid.clone();
                }
            }
            ui.horizontal(|ui| {
                ui.label("Channel:");
                egui::ComboBox::from_id_salt("deck_add_channel")
                    .selected_text(
                        channel_names
                            .iter()
                            .find(|(u, _)| u == &self.selected_channel_uuid)
                            .map(|(_, n)| n.as_str())
                            .unwrap_or("--"),
                    )
                    .show_ui(ui, |ui| {
                        for (uuid, name) in &channel_names {
                            ui.selectable_value(&mut self.selected_channel_uuid, uuid.clone(), name);
                        }
                    });
            });

            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.add_tab, AddSourceTab::File, "📁 File");
                ui.selectable_value(&mut self.add_tab, AddSourceTab::Camera, "📷 Camera");
                #[cfg(feature = "ndi")]
                ui.selectable_value(&mut self.add_tab, AddSourceTab::Ndi, "📡 NDI");
                #[cfg(target_os = "macos")]
                ui.selectable_value(&mut self.add_tab, AddSourceTab::Syphon, "🖥 Syphon");
            });
            ui.separator();

            let target_uuid = self.selected_channel_uuid.clone();
            if target_uuid.is_empty() {
                ui.label("No channel available.");
                return;
            }

            match self.add_tab {
                AddSourceTab::File => {
                    if let Ok(mut guard) = self.pending_file.lock() {
                        if let Some(path) = guard.take() {
                            self.file_path = path.to_string_lossy().to_string();
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label("Path:");
                        ui.text_edit_singleline(&mut self.file_path);
                        if ui.button("Browse…").clicked() {
                            let pending = self.pending_file.clone();
                            let ctx = ui.ctx().clone();
                            std::thread::spawn(move || {
                                if let Some(path) = rfd::FileDialog::new()
                                    .add_filter("Video", &["mp4", "mov", "avi", "mkv", "webm", "m4v"])
                                    .add_filter("Image", &["png", "jpg", "jpeg"])
                                    .add_filter("ISF Shader", &["fs"])
                                    .pick_file()
                                {
                                    if let Ok(mut guard) = pending.lock() {
                                        *guard = Some(path);
                                    }
                                    ctx.request_repaint();
                                }
                            });
                        }
                    });
                    let path = std::path::PathBuf::from(&self.file_path);
                    let exists = path.exists();
                    if !exists && !self.file_path.is_empty() {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "File not found",
                        );
                    }
                    if ui.button("Add File").clicked() && exists {
                        let ext = path
                            .extension()
                            .and_then(|e| e.to_str())
                            .unwrap_or("")
                            .to_lowercase();
                        let name = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("File")
                            .to_string();
                        let kind = match ext.as_str() {
                            "mp4" | "mov" | "avi" | "mkv" | "webm" => crate::sources::SourceKind::Video,
                            "png" | "jpg" | "jpeg" => crate::sources::SourceKind::Image,
                            "fs" => crate::sources::SourceKind::Isf,
                            _ => crate::sources::SourceKind::Video,
                        };
                        let id = name.to_lowercase().replace(' ', "_");
                        state.pending_decks.push(crate::PendingDeck {
                            channel_uuid: target_uuid.clone(),
                            source: crate::sources::SourceEntry {
                                id,
                                name,
                                kind,
                                path: Some(path),
                                device_index: 0,
                            },
                        });
                        self.file_path.clear();
                    }
                }
                AddSourceTab::Camera => {
                    let cameras: Vec<&crate::sources::SourceEntry> = state
                        .registry
                        .builtins
                        .iter()
                        .filter(|e| e.kind == crate::sources::SourceKind::Camera)
                        .collect();
                    if cameras.is_empty() {
                        ui.label("No cameras found.");
                    } else {
                        let names: Vec<String> = cameras.iter().map(|c| c.name.clone()).collect();
                        egui::ComboBox::from_id_salt("deck_add_camera")
                            .selected_text(
                                names.get(self.selected_camera_index).map(|s| s.as_str()).unwrap_or("--"),
                            )
                            .show_ui(ui, |ui| {
                                for (i, name) in names.iter().enumerate() {
                                    ui.selectable_value(&mut self.selected_camera_index, i, name);
                                }
                            });
                        if ui.button("Add Camera").clicked() {
                            if let Some(entry) = cameras.get(self.selected_camera_index) {
                                state.pending_decks.push(crate::PendingDeck {
                                    channel_uuid: target_uuid.clone(),
                                    source: (*entry).clone(),
                                });
                            }
                        }
                    }
                }
                #[cfg(feature = "ndi")]
                AddSourceTab::Ndi => {
                    let sources: Vec<&crate::sources::SourceEntry> = state
                        .registry
                        .builtins
                        .iter()
                        .filter(|e| e.kind == crate::sources::SourceKind::Ndi)
                        .collect();
                    if sources.is_empty() {
                        ui.label("No NDI sources found.");
                    } else {
                        let names: Vec<String> = sources.iter().map(|s| s.name.clone()).collect();
                        egui::ComboBox::from_id_salt("deck_add_ndi")
                            .selected_text(
                                names.get(self.selected_ndi_index).map(|s| s.as_str()).unwrap_or("--"),
                            )
                            .show_ui(ui, |ui| {
                                for (i, name) in names.iter().enumerate() {
                                    ui.selectable_value(&mut self.selected_ndi_index, i, name);
                                }
                            });
                        if ui.button("Add NDI").clicked() {
                            if let Some(entry) = sources.get(self.selected_ndi_index) {
                                state.pending_decks.push(crate::PendingDeck {
                                    channel_uuid: target_uuid.clone(),
                                    source: (*entry).clone(),
                                });
                            }
                        }
                    }
                }
                #[cfg(not(feature = "ndi"))]
                AddSourceTab::Ndi => {
                    ui.label("NDI support not enabled.");
                }
                #[cfg(target_os = "macos")]
                AddSourceTab::Syphon => {
                    let servers: Vec<&crate::sources::SourceEntry> = state
                        .registry
                        .builtins
                        .iter()
                        .filter(|e| e.kind == crate::sources::SourceKind::Syphon)
                        .collect();
                    if servers.is_empty() {
                        ui.label("No Syphon servers found.");
                    } else {
                        let names: Vec<String> = servers.iter().map(|s| s.name.clone()).collect();
                        egui::ComboBox::from_id_salt("deck_add_syphon")
                            .selected_text(
                                names.get(self.selected_syphon_index).map(|s| s.as_str()).unwrap_or("--"),
                            )
                            .show_ui(ui, |ui| {
                                for (i, name) in names.iter().enumerate() {
                                    ui.selectable_value(&mut self.selected_syphon_index, i, name);
                                }
                            });
                        if ui.button("Add Syphon").clicked() {
                            if let Some(entry) = servers.get(self.selected_syphon_index) {
                                state.pending_decks.push(crate::PendingDeck {
                                    channel_uuid: target_uuid.clone(),
                                    source: (*entry).clone(),
                                });
                            }
                        }
                    }
                }
                #[cfg(not(target_os = "macos"))]
                AddSourceTab::Syphon => {
                    ui.label("Syphon is only available on macOS.");
                }
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // EffectsTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for EffectsTab {
        fn name(&self) -> &str {
            "Effects"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_mut::<VardaAppState>()
                .expect("EffectsTab expects VardaAppState");

            // Poll async file picker result.
            if let Ok(mut guard) = self.pending_effect.lock() {
                if let Some(req) = guard.take() {
                    state.pending_effects.push(req);
                }
            }

            ui.heading("Effects / Library");
            ui.separator();

            // Channel selector for runtime deck creation.
            let channel_names: Vec<(String, String)> = {
                let mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());
                mixer
                    .channels
                    .iter()
                    .map(|c| (c.uuid.clone(), c.name.clone()))
                    .collect()
            };
            if self.selected_channel_uuid.is_empty() {
                if let Some((uuid, _)) = channel_names.first() {
                    self.selected_channel_uuid = uuid.clone();
                }
            }

            ui.horizontal(|ui| {
                ui.label("Add to:");
                egui::ComboBox::from_id_salt("effects_target_channel")
                    .selected_text(
                        channel_names
                            .iter()
                            .find(|(u, _)| u == &self.selected_channel_uuid)
                            .map(|(_, n)| n.as_str())
                            .unwrap_or("--"),
                    )
                    .show_ui(ui, |ui| {
                        for (uuid, name) in &channel_names {
                            ui.selectable_value(
                                &mut self.selected_channel_uuid,
                                uuid.clone(),
                                name,
                            );
                        }
                    });
            });
            ui.add_space(4.0);

            // Library listing — clicking "➕" queues a PendingDeck for materialisation in prepare().
            ui.label(egui::RichText::new("Library").strong());
            let target_uuid = self.selected_channel_uuid.clone();
            egui::ScrollArea::vertical()
                .max_height(140.0)
                .show(ui, |ui| {
                    let mut queue_deck: Option<crate::PendingDeck> = None;
                    for entry in state
                        .registry
                        .shaders
                        .iter()
                        .chain(&state.registry.images)
                        .chain(&state.registry.videos)
                        .chain(&state.registry.streams)
                    {
                        ui.horizontal(|ui| {
                            let icon = match entry.kind {
                                crate::sources::SourceKind::Isf => "🎨",
                                crate::sources::SourceKind::Image => "🖼",
                                crate::sources::SourceKind::Video => "🎬",
                                crate::sources::SourceKind::SolidColor => "🎨",
                                crate::sources::SourceKind::Camera => "📷",
                                crate::sources::SourceKind::Srt => "📡",
                                crate::sources::SourceKind::Hls => "📡",
                                crate::sources::SourceKind::Dash => "📡",
                                crate::sources::SourceKind::Rtmp => "📡",
                                _ => "📁",
                            };
                            ui.label(format!("{} {}", icon, entry.name));
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("➕").clicked() && !target_uuid.is_empty() {
                                        queue_deck = Some(crate::PendingDeck {
                                            channel_uuid: target_uuid.clone(),
                                            source: entry.clone(),
                                        });
                                    }
                                },
                            );
                        });
                    }
                    if let Some(req) = queue_deck {
                        let name = req.source.name.clone();
                        state.pending_decks.push(req);
                        engine.notify(
                            format!("Queued '{}' for creation", name),
                            rustjay_core::NotificationLevel::Info,
                            std::time::Duration::from_secs(3),
                        );
                    }
                });

            // Manual stream input
            ui.collapsing("Add Stream URL", |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.stream_name);
                });
                ui.horizontal(|ui| {
                    ui.label("URL:");
                    ui.text_edit_singleline(&mut self.stream_url);
                    egui::ComboBox::from_id_salt("stream_kind")
                        .selected_text(&self.stream_kind)
                        .show_ui(ui, |ui| {
                            for kind in &["rtmp", "srt", "hls", "dash"] {
                                ui.selectable_value(&mut self.stream_kind, kind.to_string(), *kind);
                            }
                        });
                });
                if ui.button("Add Stream").clicked() && !self.stream_url.is_empty() && !self.stream_name.is_empty() && !target_uuid.is_empty() {
                    let kind = match self.stream_kind.as_str() {
                        "srt" => crate::sources::SourceKind::Srt,
                        "hls" => crate::sources::SourceKind::Hls,
                        "dash" => crate::sources::SourceKind::Dash,
                        _ => crate::sources::SourceKind::Rtmp,
                    };
                    let entry = crate::sources::SourceEntry {
                        id: self.stream_name.to_lowercase().replace(' ', "_"),
                        name: self.stream_name.clone(),
                        kind,
                        path: Some(std::path::PathBuf::from(&self.stream_url)),
                        device_index: 0,
                    };
                    state.pending_decks.push(crate::PendingDeck {
                        channel_uuid: target_uuid.clone(),
                        source: entry,
                    });
                    engine.notify(
                        format!("Queued stream '{}' for creation", self.stream_name),
                        rustjay_core::NotificationLevel::Info,
                        std::time::Duration::from_secs(3),
                    );
                    self.stream_url.clear();
                    self.stream_name.clear();
                }
            });
            ui.separator();

            // Live FX chains
            ui.label(egui::RichText::new("Live FX Chains").strong());

            // Set when an FX is removed in place, so prepare() re-registers params.
            let mut fx_removed = false;
            let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

            for ch in &mut mixer.channels {
                ui.push_id(ch.uuid.clone(), |ui| {
                    ui.collapsing(format!("Channel: {}", ch.name), |ui| {
                        // Channel FX list + add button
                        if !ch.chain.is_empty() {
                            ui.label("Channel FX:");
                        }
                        if fx_chain_ui(ui, &mut ch.chain) {
                            fx_removed = true;
                        }
                        if ui.small_button("➕ Add Channel FX…").clicked() {
                            spawn_effect_picker(
                                &self.pending_effect,
                                ui.ctx(),
                                crate::EffectTarget::Channel {
                                    channel_uuid: ch.uuid.clone(),
                                },
                            );
                        }

                        // Deck FX
                        let Some(compositor) = ch.effect.as_any_mut() else {
                            return;
                        };
                        let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() else {
                            return;
                        };
                        for deck in &mut compositor.decks {
                            ui.push_id(deck.uuid.clone(), |ui| {
                                if !deck.chain.is_empty() {
                                    ui.label(format!("Deck {} FX:", deck.name));
                                }
                                if fx_chain_ui(ui, &mut deck.chain) {
                                    fx_removed = true;
                                }
                                if ui.small_button("➕ Add Deck FX…").clicked() {
                                    spawn_effect_picker(
                                        &self.pending_effect,
                                        ui.ctx(),
                                        crate::EffectTarget::Deck {
                                            channel_uuid: ch.uuid.clone(),
                                            deck_uuid: deck.uuid.clone(),
                                        },
                                    );
                                }
                            });
                        }
                    });
                });
            }

            // Master FX list + add button
            ui.collapsing("Master FX", |ui| {
                if fx_chain_ui(ui, &mut mixer.master) {
                    fx_removed = true;
                }
                if ui.small_button("➕ Add Master FX…").clicked() {
                    spawn_effect_picker(&self.pending_effect, ui.ctx(), crate::EffectTarget::Master);
                }
            });

            drop(mixer);
            if fx_removed {
                state.params_dirty_request = true;
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // MidiTab
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for MidiTab {
        fn name(&self) -> &str {
            "MIDI"
        }

        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("MIDI");
            ui.separator();
            ui.label("MIDI device selection, learn/unlearn, and mapping are managed by the engine's built-in MIDI system.");
            ui.label("Connect a controller and move a knob to auto-learn its mapping to the currently selected parameter.");
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // StageTab — 2D surface editor
    // ─────────────────────────────────────────────────────────────────────────
    impl AnyEguiTab for StageTab {
        fn name(&self) -> &str {
            "Stage"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Stage");
            ui.separator();

            #[cfg(feature = "projection")]
            {
                // The stage canvas is mapped against the live master-output
                // resolution so surfaces can be sized pixel-perfectly. The live
                // preview texture (if the GUI is up) backs the canvas.
                let master_res = [
                    _engine.resolution.internal_width.max(1),
                    _engine.resolution.internal_height.max(1),
                ];
                let preview_tex = _engine
                    .stage_preview_texture_id
                    .map(egui::TextureId::User);

                let state = _app_state
                    .downcast_mut::<VardaAppState>()
                    .expect("StageTab expects VardaAppState");

                // Keep the design resolution in sync with the master output so
                // pixel-based surface sizing is accurate.
                state.stage.canvas_size = master_res;

                // Surface list + 2D editor canvas (live master-output preview).
                self.draw_stage_tab(ui, state, master_res, preview_tex);

                // Geometry of the selected surface lives below the canvas so the
                // canvas can use the full panel width (merged in from the former
                // standalone Geometry tab).
                ui.separator();
                ui.collapsing(
                    egui::RichText::new("Geometry").heading(),
                    |ui| {
                        draw_surface_properties(ui, state);
                    },
                )
                .header_response
                .on_hover_text("Properties of the selected surface");
            }

            #[cfg(not(feature = "projection"))]
            {
                ui.label("Projection feature is not enabled. Enable it to use surfaces.");
            }
        }
    }

    #[cfg(feature = "projection")]
    impl StageTab {
        fn draw_stage_tab(
            &mut self,
            ui: &mut egui::Ui,
            state: &mut VardaAppState,
            master_res: [u32; 2],
            preview_tex: Option<egui::TextureId>,
        ) {
            use crate::stage::{ContentMapping, SurfaceSource, VardaSurface};
            use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};

            // Disjoint field borrows so the closures below can capture each
            // independently of `self`.
            let import_path = &mut self.import_path;
            let canvas_zoom = &mut self.canvas_zoom;
            let edit_mode = &mut self.edit_mode;
            let lighting_regions_active = &mut self.lighting_regions_active;
            let selected_light_segment = &mut self.selected_light_segment;

            // Set when a surface warp is edited this frame, so we publish to the
            // projector only on change (avoids per-frame mesh rebuilds).
            let mut warp_dirty = false;
            // Set when a lighting segment region is edited this frame.
            let mut regions_dirty = false;

            // Left panel: surface list
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(180.0);
                    ui.label(egui::RichText::new("Surfaces").strong());
                    ui.separator();

                    let mut to_remove: Option<usize> = None;
                    for (i, surf) in state.stage.surfaces.iter().enumerate() {
                        ui.horizontal(|ui| {
                            let label = if i == state.stage.selected_surface_index {
                                egui::RichText::new(&surf.name).strong()
                            } else {
                                egui::RichText::new(&surf.name)
                            };
                            if ui.selectable_label(i == state.stage.selected_surface_index, label).clicked() {
                                state.stage.selected_surface_index = i;
                            }
                            if ui.small_button("✖").clicked() {
                                to_remove = Some(i);
                            }
                        });
                    }
                    if let Some(i) = to_remove {
                        state.stage.surfaces.remove(i);
                        if state.stage.selected_surface_index >= state.stage.surfaces.len() && !state.stage.surfaces.is_empty() {
                            state.stage.selected_surface_index = state.stage.surfaces.len() - 1;
                        }
                    }

                    if ui.button("+ Add Rectangle").clicked() {
                        let idx = state.stage.surfaces.len() + 1;
                        state.stage.surfaces.push(VardaSurface::full_frame(
                            format!("Surface {}", idx),
                            format!("surf{}", idx),
                        ));
                    }
                    if ui.button("+ Add Circle").clicked() {
                        let idx = state.stage.surfaces.len() + 1;
                        state.stage.surfaces.push(VardaSurface::circle(
                            format!("Circle {}", idx),
                            format!("circle{}", idx),
                            [0.5, 0.5],
                            0.25,
                        ));
                    }
                    ui.separator();
                    ui.label("Import:");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(import_path).hint_text("Path to SVG/DXF..."),
                        );
                        if ui.button("Import").clicked() && !import_path.is_empty() {
                            let path = std::path::Path::new(&*import_path);
                            let params =
                                rustjay_projection::surface_import::DetectionParams::default();
                            match rustjay_projection::surface_import::detect_from_file(
                                path, &params,
                            ) {
                                Ok(result) => {
                                    if result.contours.is_empty() {
                                        log::warn!("No contours found in {}", import_path);
                                    } else if result.contours.len() == 1 {
                                        let contour = &result.contours[0];
                                        let s = contour.to_surface(0);
                                        state.stage.surfaces.push(VardaSurface {
                                            name: s.name,
                                            uuid: "import0".to_string(),
                                            vertices: s.vertices,
                                            is_circular: s.is_circular,
                                            radius: contour
                                                .circle_fit
                                                .map(|(_, r)| r)
                                                .unwrap_or(0.1),
                                            source: SurfaceSource::Master,
                                            content_mapping: ContentMapping::Mapped,
                                            extra_contours: Vec::new(),
                                            uv_crop_rect: [0.0, 0.0, 1.0, 1.0],
                                            warp: rustjay_projection::WarpMode::identity(),
                                        });
                                    } else {
                                        // Multi-contour import: largest = primary, rest = extra_contours
                                        let primary = &result.contours[0];
                                        let s = primary.to_surface(0);
                                        let extra_contours: Vec<Vec<[f32; 2]>> = result.contours[1..]
                                            .iter()
                                            .map(|c| c.vertices.clone())
                                            .collect();
                                        state.stage.surfaces.push(VardaSurface {
                                            name: s.name,
                                            uuid: "import0".to_string(),
                                            vertices: s.vertices,
                                            is_circular: s.is_circular,
                                            radius: primary
                                                .circle_fit
                                                .map(|(_, r)| r)
                                                .unwrap_or(0.1),
                                            source: SurfaceSource::Master,
                                            content_mapping: ContentMapping::Mapped,
                                            extra_contours,
                                            uv_crop_rect: [0.0, 0.0, 1.0, 1.0],
                                            warp: rustjay_projection::WarpMode::identity(),
                                        });
                                    }
                                    log::info!(
                                        "Imported {} contour(s) from {}",
                                        result.contours.len(),
                                        import_path
                                    );
                                }
                                Err(e) => {
                                    log::warn!("Import failed for {}: {}", import_path, e);
                                }
                            }
                        }
                    });
                });

                ui.separator();

                // Center: control row + 2D canvas (live master-output preview).
                ui.vertical(|ui| {
                    // ── Canvas controls ────────────────────────────────────
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Canvas: {}×{} px",
                                master_res[0], master_res[1]
                            ))
                            .strong(),
                        );
                        ui.separator();
                        ui.checkbox(edit_mode, "Edit mode");
                        ui.checkbox(lighting_regions_active, "Lighting regions");
                        if *edit_mode {
                            ui.label("Zoom:");
                            ui.add(
                                egui::Slider::new(canvas_zoom, 0.25..=4.0)
                                    .fixed_decimals(2)
                                    .suffix("×"),
                            );
                            if ui.small_button("Fit").clicked() {
                                *canvas_zoom = 1.0;
                            }
                        } else {
                            // Preview mode always fits the panel.
                            *canvas_zoom = 1.0;
                        }
                        if preview_tex.is_none() {
                            ui.label(
                                egui::RichText::new(
                                    "(no live preview — enable Preview in Settings)",
                                )
                                .weak(),
                            );
                        }
                    });
                    ui.separator();

                    // ── Canvas size: fit master aspect, scaled by zoom ──────
                    // Use the bounded visible rect (not `available_size`, which is
                    // unbounded inside the outer vertical scroll area and would let
                    // the canvas spill over the Geometry side panel).
                    let aspect = master_res[0] as f32 / master_res[1].max(1) as f32;
                    let avail = ui.available_rect_before_wrap();
                    let aw = avail.width().max(64.0);
                    // Reserve vertical room for the Geometry section below so it
                    // stays reachable without the canvas filling the whole view.
                    let ah = (avail.height() - 220.0).max(180.0);
                    let (base_w, base_h) = if aw / ah > aspect {
                        (ah * aspect, ah)
                    } else {
                        (aw, aw / aspect)
                    };
                    let canvas_dims = Vec2::new(base_w * *canvas_zoom, base_h * *canvas_zoom);

                    // Bound the scroll viewport to the visible area so a zoomed
                    // (larger) canvas pans inside it instead of overflowing.
                    egui::ScrollArea::both()
                        .max_width(aw)
                        .max_height(ah)
                        .show(ui, |ui| {
                    let (canvas_rect, response) =
                        ui.allocate_exact_size(canvas_dims, egui::Sense::click());
                    let painter = ui.painter_at(canvas_rect);

                    // Background: the live master output, dimmed so the whole
                    // frame stays visible as context. Each surface then redraws
                    // the region it samples at full brightness below, so its
                    // position/size visibly select (crop) part of the master.
                    if let Some(tex) = preview_tex {
                        painter.image(
                            tex,
                            canvas_rect,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::from_gray(80), // tint < 255 dims the image
                        );
                    } else {
                        painter.rect_filled(canvas_rect, CornerRadius::ZERO, Color32::from_gray(18));
                    }
                    painter.rect_stroke(
                        canvas_rect,
                        CornerRadius::ZERO,
                        Stroke::new(1.0, Color32::from_gray(80)),
                        egui::StrokeKind::Inside,
                    );

                    // Grid
                for i in 0..=10 {
                    let t = i as f32 / 10.0;
                    let x = canvas_rect.min.x + t * canvas_rect.width();
                    let y = canvas_rect.min.y + t * canvas_rect.height();
                    painter.line_segment(
                        [
                            Pos2::new(x, canvas_rect.min.y),
                            Pos2::new(x, canvas_rect.max.y),
                        ],
                        Stroke::new(0.5, Color32::from_gray(50)),
                    );
                    painter.line_segment(
                        [
                            Pos2::new(canvas_rect.min.x, y),
                            Pos2::new(canvas_rect.max.x, y),
                        ],
                        Stroke::new(0.5, Color32::from_gray(50)),
                    );
                }

                // ── Surface content: each Master surface's crop region
                // (`uv_crop_rect`) — which is the same rectangle as its
                // position/size box — is redrawn from the master at full
                // brightness in place. So moving/resizing a surface selects
                // (crops) a region of the live master, visibly, and matches what
                // the projector outputs.
                if let Some(tex) = preview_tex {
                    for surf in state.stage.surfaces.iter() {
                        if surf.source != SurfaceSource::Master || surf.is_circular {
                            continue;
                        }
                        let [u0, v0, u1, v1] = surf.uv_crop_rect;
                        let dst = Rect::from_min_max(
                            Pos2::new(
                                canvas_rect.min.x + u0 * canvas_rect.width(),
                                canvas_rect.min.y + v0 * canvas_rect.height(),
                            ),
                            Pos2::new(
                                canvas_rect.min.x + u1 * canvas_rect.width(),
                                canvas_rect.min.y + v1 * canvas_rect.height(),
                            ),
                        );
                        painter.image(
                            tex,
                            dst,
                            Rect::from_min_max(Pos2::new(u0, v0), Pos2::new(u1, v1)),
                            Color32::WHITE,
                        );
                    }
                }

                // Draw surfaces
                for (i, surf) in state.stage.surfaces.iter().enumerate() {
                    let is_selected = i == state.stage.selected_surface_index;
                    let color = if is_selected {
                        Color32::from_rgb(100, 150, 255)
                    } else {
                        Color32::from_rgb(200, 100, 100)
                    };

                    if surf.is_circular && !surf.vertices.is_empty() {
                        let center = surf.vertices[0];
                        let cx = canvas_rect.min.x + center[0] * canvas_rect.width();
                        let cy = canvas_rect.min.y + center[1] * canvas_rect.height();
                        let r = surf.radius * canvas_rect.width();
                        painter.circle_stroke(Pos2::new(cx, cy), r, Stroke::new(2.0, color));
                        painter.text(
                            Pos2::new(cx, cy),
                            egui::Align2::CENTER_CENTER,
                            &surf.name,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                    } else if surf.vertices.len() >= 3 {
                        let points: Vec<Pos2> = surf
                            .vertices
                            .iter()
                            .map(|v| {
                                Pos2::new(
                                    canvas_rect.min.x + v[0] * canvas_rect.width(),
                                    canvas_rect.min.y + v[1] * canvas_rect.height(),
                                )
                            })
                            .collect();
                        // Outline only — the surface content is drawn above, so a
                        // filled overlay would tint it. Selected surfaces get a
                        // faint tint for feedback.
                        let fill = if is_selected {
                            color.linear_multiply(0.12)
                        } else {
                            Color32::TRANSPARENT
                        };
                        painter.add(egui::Shape::convex_polygon(
                            points.clone(),
                            fill,
                            Stroke::new(if is_selected { 3.0 } else { 2.0 }, color),
                        ));
                        // Label at centroid
                        let centroid_x: f32 = surf.vertices.iter().map(|v| v[0]).sum::<f32>()
                            / surf.vertices.len() as f32;
                        let centroid_y: f32 = surf.vertices.iter().map(|v| v[1]).sum::<f32>()
                            / surf.vertices.len() as f32;
                        painter.text(
                            Pos2::new(
                                canvas_rect.min.x + centroid_x * canvas_rect.width(),
                                canvas_rect.min.y + centroid_y * canvas_rect.height(),
                            ),
                            egui::Align2::CENTER_CENTER,
                            &surf.name,
                            egui::FontId::proportional(12.0),
                            Color32::WHITE,
                        );
                    }

                    // Extra contours: dashed outlines
                    if is_selected {
                        for contour in &surf.extra_contours {
                            if contour.len() >= 2 {
                                let pts: Vec<Pos2> = contour
                                    .iter()
                                    .map(|v| {
                                        Pos2::new(
                                            canvas_rect.min.x + v[0] * canvas_rect.width(),
                                            canvas_rect.min.y + v[1] * canvas_rect.height(),
                                        )
                                    })
                                    .collect();
                                // Draw dashed polyline
                                let dash_len = 6.0;
                                let gap_len = 3.0;
                                let dash_color = Color32::from_rgb(180, 180, 180);
                                for seg in pts.windows(2) {
                                    let a = seg[0];
                                    let b = seg[1];
                                    let dx = b.x - a.x;
                                    let dy = b.y - a.y;
                                    let len = (dx * dx + dy * dy).sqrt();
                                    if len < 0.1 {
                                        continue;
                                    }
                                    let steps = (len / (dash_len + gap_len)).ceil() as usize;
                                    for s in 0..steps {
                                        let t0 = (s as f32 * (dash_len + gap_len)).min(len) / len;
                                        let t1 = ((s as f32 * (dash_len + gap_len) + dash_len)).min(len) / len;
                                        painter.line_segment(
                                            [Pos2::new(a.x + dx * t0, a.y + dy * t0), Pos2::new(a.x + dx * t1, a.y + dy * t1)],
                                            Stroke::new(1.5, dash_color),
                                        );
                                    }
                                }
                                // Close the contour with a dashed line
                                if let (Some(first), Some(last)) = (pts.first(), pts.last()) {
                                    let a = *last;
                                    let b = *first;
                                    let dx = b.x - a.x;
                                    let dy = b.y - a.y;
                                    let len = (dx * dx + dy * dy).sqrt();
                                    if len >= 0.1 {
                                        let steps = (len / (dash_len + gap_len)).ceil() as usize;
                                        for s in 0..steps {
                                            let t0 = (s as f32 * (dash_len + gap_len)).min(len) / len;
                                            let t1 = ((s as f32 * (dash_len + gap_len) + dash_len)).min(len) / len;
                                            painter.line_segment(
                                                [Pos2::new(a.x + dx * t0, a.y + dy * t0), Pos2::new(a.x + dx * t1, a.y + dy * t1)],
                                                Stroke::new(1.5, dash_color),
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── UV crop drag handles ───────────────────────────────────
                // Corner handles on the stage canvas now control the UV sampling
                // region (which part of the source texture is shown), not the
                // output warp. Output warp remains editable on the Geometry tab.
                let mut handle_dragged = false;

                let uv_handles: Option<[[f32; 2]; 4]> = state.stage.surfaces
                    .get(state.stage.selected_surface_index)
                    .map(|s| {
                        let [min_u, min_v, max_u, max_v] = s.uv_crop_rect;
                        [
                            [min_u, min_v], // TL
                            [max_u, min_v], // TR
                            [max_u, max_v], // BR
                            [min_u, max_v], // BL
                        ]
                    });

                let mesh_handle_positions: Option<Vec<[f32; 2]>> = state.stage.surfaces
                    .get(state.stage.selected_surface_index)
                    .and_then(|s| match &s.warp {
                        rustjay_projection::WarpMode::Mesh(mesh) => {
                            Some(mesh.points.iter().map(|p| p.position).collect())
                        }
                        _ => None,
                    });

                // Draw UV crop handles and connecting rectangle
                if let Some(corners) = uv_handles {
                    let handle_labels = ["TL", "TR", "BR", "BL"];
                    let mut positions = [Pos2::ZERO; 4];
                    for (i, corner) in corners.iter().enumerate() {
                        let pos = Pos2::new(
                            canvas_rect.min.x + corner[0] * canvas_rect.width(),
                            canvas_rect.min.y + corner[1] * canvas_rect.height(),
                        );
                        positions[i] = pos;
                        let handle_rect = Rect::from_center_size(pos, Vec2::splat(12.0));
                        let handle_id = ui.id().with(("uv_crop_handle", i));
                        let handle_response = ui.interact(handle_rect, handle_id, egui::Sense::drag());

                        let handle_color = if handle_response.dragged() {
                            Color32::from_rgb(255, 200, 0) // amber for UV crop
                        } else if handle_response.hovered() {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(180, 180, 180)
                        };
                        painter.circle_filled(pos, 5.0, handle_color);

                        // Label
                        painter.text(
                            pos - Vec2::new(0.0, 8.0),
                            egui::Align2::CENTER_BOTTOM,
                            handle_labels[i],
                            egui::FontId::proportional(9.0),
                            Color32::from_rgb(200, 200, 200),
                        );

                        if handle_response.dragged() {
                            handle_dragged = true;
                            let dx = handle_response.drag_delta().x / canvas_rect.width();
                            let dy = handle_response.drag_delta().y / canvas_rect.height();
                            if let Some(surf) = state.stage.surfaces.get_mut(state.stage.selected_surface_index) {
                                let [min_u, min_v, max_u, max_v] = &mut surf.uv_crop_rect;
                                match i {
                                    0 => { // TL
                                        *min_u = (*min_u + dx).clamp(0.0, 1.0);
                                        *min_v = (*min_v + dy).clamp(0.0, 1.0);
                                    }
                                    1 => { // TR
                                        *max_u = (*max_u + dx).clamp(0.0, 1.0);
                                        *min_v = (*min_v + dy).clamp(0.0, 1.0);
                                    }
                                    2 => { // BR
                                        *max_u = (*max_u + dx).clamp(0.0, 1.0);
                                        *max_v = (*max_v + dy).clamp(0.0, 1.0);
                                    }
                                    3 => { // BL
                                        *min_u = (*min_u + dx).clamp(0.0, 1.0);
                                        *max_v = (*max_v + dy).clamp(0.0, 1.0);
                                    }
                                    _ => {}
                                }
                                // Ensure min <= max
                                if *min_u > *max_u { std::mem::swap(min_u, max_u); }
                                if *min_v > *max_v { std::mem::swap(min_v, max_v); }
                                // Keep a simple rectangle surface's box in sync
                                // with the crop, so the outline tracks the crop.
                                let crop = surf.uv_crop_rect;
                                if !surf.is_circular && surf.vertices.len() == 4 {
                                    surf.vertices = vec![
                                        [crop[0], crop[1]],
                                        [crop[2], crop[1]],
                                        [crop[2], crop[3]],
                                        [crop[0], crop[3]],
                                    ];
                                }
                            }
                            warp_dirty = true;
                        }
                    }
                    // Draw connecting lines to show crop rect
                    let crop_line_color = Color32::from_rgba_premultiplied(255, 200, 0, 80);
                    for i in 0..4 {
                        let j = (i + 1) % 4;
                        painter.line_segment([positions[i], positions[j]], egui::Stroke::new(1.0, crop_line_color));
                    }
                }

                // Draw mesh handles
                if let Some(positions) = mesh_handle_positions {
                    for (i, pos_norm) in positions.iter().enumerate() {
                        let pos = Pos2::new(
                            canvas_rect.min.x + pos_norm[0] * canvas_rect.width(),
                            canvas_rect.min.y + pos_norm[1] * canvas_rect.height(),
                        );
                        let handle_rect = Rect::from_center_size(pos, Vec2::splat(8.0));
                        let handle_id = ui.id().with(("mesh_handle", i));
                        let handle_response = ui.interact(handle_rect, handle_id, egui::Sense::drag());

                        let handle_color = if handle_response.dragged() {
                            Color32::YELLOW
                        } else if handle_response.hovered() {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(200, 200, 200)
                        };
                        painter.circle_filled(pos, 3.0, handle_color);

                        if handle_response.dragged() {
                            handle_dragged = true;
                            let dx = handle_response.drag_delta().x / canvas_rect.width();
                            let dy = handle_response.drag_delta().y / canvas_rect.height();
                            if let Some(surf) = state.stage.surfaces.get_mut(state.stage.selected_surface_index) {
                                if let rustjay_projection::WarpMode::Mesh(mesh) = &mut surf.warp {
                                    mesh.points[i].position[0] = (mesh.points[i].position[0] + dx).clamp(0.0, 1.0);
                                    mesh.points[i].position[1] = (mesh.points[i].position[1] + dy).clamp(0.0, 1.0);
                                }
                            }
                            warp_dirty = true;
                        }
                    }
                }

                // Surface selection via canvas click (skip if a handle is being dragged
                // or if lighting region editing is active).
                if !*lighting_regions_active && !handle_dragged && response.clicked_by(egui::PointerButton::Primary) {
                    if let Some(pos) = response.interact_pointer_pos() {
                        let norm_x = (pos.x - canvas_rect.min.x) / canvas_rect.width();
                        let norm_y = (pos.y - canvas_rect.min.y) / canvas_rect.height();
                        // Find top-most surface containing the click point
                        for (i, surf) in state.stage.surfaces.iter().enumerate().rev() {
                            let [min_x, min_y, max_x, max_y] = surf.bounding_box();
                            if norm_x >= min_x && norm_x <= max_x && norm_y >= min_y && norm_y <= max_y {
                                state.stage.selected_surface_index = i;
                                break;
                            }
                        }
                    }
                }

                // ── Lighting region overlay ──────────────────────────────────
                if *lighting_regions_active {
                    let mut light_region_dragged = false;
                    // Draw all segment regions first, then handles on top of the selected one.
                    for (oi, light) in state.stage.lighting_outputs.iter().enumerate() {
                        if !light.enabled {
                            continue;
                        }
                        let color = egui::Color32::from_rgb(
                            ((oi * 73) % 200 + 55) as u8,
                            ((oi * 137) % 200 + 55) as u8,
                            ((oi * 211) % 200 + 55) as u8,
                        );
                        for (si, seg) in light.segments.iter().enumerate() {
                            if !seg.enabled {
                                continue;
                            }
                            let [u0, v0, u1, v1] = seg.region;
                            let min = egui::Pos2::new(
                                canvas_rect.min.x + u0 * canvas_rect.width(),
                                canvas_rect.min.y + v0 * canvas_rect.height(),
                            );
                            let max = egui::Pos2::new(
                                canvas_rect.min.x + u1 * canvas_rect.width(),
                                canvas_rect.min.y + v1 * canvas_rect.height(),
                            );
                            let rect = egui::Rect::from_min_max(min, max);
                            let is_selected = selected_light_segment == &Some((oi, si));
                            let stroke_width = if is_selected { 3.0 } else { 1.5 };
                            painter.rect_stroke(
                                rect,
                                egui::CornerRadius::ZERO,
                                egui::Stroke::new(stroke_width, color),
                                egui::StrokeKind::Inside,
                            );
                            if is_selected {
                                painter.rect_filled(rect, egui::CornerRadius::ZERO, color.linear_multiply(0.12));
                            }
                            let label = format!("{} / {}", light.name, seg.name);
                            painter.text(
                                min + egui::Vec2::new(4.0, 2.0),
                                egui::Align2::LEFT_TOP,
                                label,
                                egui::FontId::proportional(11.0),
                                egui::Color32::WHITE,
                            );
                        }
                    }

                    // Drag handles for the selected segment.
                    if let Some((sel_oi, sel_si)) = *selected_light_segment {
                        let handle_labels = ["TL", "TR", "BR", "BL"];
                        let mut handle_positions = [egui::Pos2::ZERO; 4];
                        let mut corners_opt: Option<[[f32; 2]; 4]> = None;
                        if let Some(light) = state.stage.lighting_outputs.get(sel_oi) {
                            if let Some(seg) = light.segments.get(sel_si) {
                                let [u0, v0, u1, v1] = seg.region;
                                corners_opt = Some([
                                    [u0, v0],
                                    [u1, v0],
                                    [u1, v1],
                                    [u0, v1],
                                ]);
                            }
                        }
                        if let Some(corners) = corners_opt {
                            for (hi, corner) in corners.iter().enumerate() {
                                let pos = egui::Pos2::new(
                                    canvas_rect.min.x + corner[0] * canvas_rect.width(),
                                    canvas_rect.min.y + corner[1] * canvas_rect.height(),
                                );
                                handle_positions[hi] = pos;
                                let handle_rect = egui::Rect::from_center_size(pos, egui::Vec2::splat(12.0));
                                let handle_id = ui.id().with(("light_region_handle", sel_oi, sel_si, hi));
                                let handle_response = ui.interact(handle_rect, handle_id, egui::Sense::drag());
                                let handle_color = if handle_response.dragged() {
                                    egui::Color32::YELLOW
                                } else if handle_response.hovered() {
                                    egui::Color32::WHITE
                                } else {
                                    egui::Color32::from_gray(180)
                                };
                                painter.circle_filled(pos, 5.0, handle_color);
                                painter.text(
                                    pos - egui::Vec2::new(0.0, 8.0),
                                    egui::Align2::CENTER_BOTTOM,
                                    handle_labels[hi],
                                    egui::FontId::proportional(9.0),
                                    egui::Color32::from_gray(200),
                                );
                                if handle_response.dragged() {
                                    light_region_dragged = true;
                                    let dx = handle_response.drag_delta().x / canvas_rect.width();
                                    let dy = handle_response.drag_delta().y / canvas_rect.height();
                                    if let Some(light) = state.stage.lighting_outputs.get_mut(sel_oi) {
                                        if let Some(seg) = light.segments.get_mut(sel_si) {
                                            let [min_u, min_v, max_u, max_v] = &mut seg.region;
                                            match hi {
                                                0 => { *min_u = (*min_u + dx).clamp(0.0, 1.0); *min_v = (*min_v + dy).clamp(0.0, 1.0); }
                                                1 => { *max_u = (*max_u + dx).clamp(0.0, 1.0); *min_v = (*min_v + dy).clamp(0.0, 1.0); }
                                                2 => { *max_u = (*max_u + dx).clamp(0.0, 1.0); *max_v = (*max_v + dy).clamp(0.0, 1.0); }
                                                3 => { *min_u = (*min_u + dx).clamp(0.0, 1.0); *max_v = (*max_v + dy).clamp(0.0, 1.0); }
                                                _ => {}
                                            }
                                            if *min_u > *max_u { std::mem::swap(min_u, max_u); }
                                            if *min_v > *max_v { std::mem::swap(min_v, max_v); }
                                            regions_dirty = true;
                                        }
                                    }
                                }
                            }
                            for hi in 0..4 {
                                let j = (hi + 1) % 4;
                                painter.line_segment([handle_positions[hi], handle_positions[j]], egui::Stroke::new(1.0, egui::Color32::YELLOW));
                            }
                        }
                    }

                    // Select a segment by clicking inside its region.
                    if !light_region_dragged && response.clicked_by(egui::PointerButton::Primary) {
                        if let Some(pos) = response.interact_pointer_pos() {
                            let norm_x = (pos.x - canvas_rect.min.x) / canvas_rect.width();
                            let norm_y = (pos.y - canvas_rect.min.y) / canvas_rect.height();
                            let mut new_selection: Option<(usize, usize)> = None;
                            for (oi, light) in state.stage.lighting_outputs.iter().enumerate().rev() {
                                if !light.enabled {
                                    continue;
                                }
                                for (si, seg) in light.segments.iter().enumerate().rev() {
                                    if !seg.enabled {
                                        continue;
                                    }
                                    let [u0, v0, u1, v1] = seg.region;
                                    if norm_x >= u0.min(u1) && norm_x <= u0.max(u1)
                                        && norm_y >= v0.min(v1) && norm_y <= v0.max(v1)
                                    {
                                        new_selection = Some((oi, si));
                                        break;
                                    }
                                }
                                if new_selection.is_some() {
                                    break;
                                }
                            }
                            *selected_light_segment = new_selection;
                        }
                    }
                }

                // Bounding box overlay for Mapped mode
                if let Some(surf) = state.stage.surfaces.get(state.stage.selected_surface_index) {
                    if surf.content_mapping == ContentMapping::Mapped {
                        let [min_x, min_y, max_x, max_y] = surf.bounding_box();
                        let min = Pos2::new(
                            canvas_rect.min.x + min_x * canvas_rect.width(),
                            canvas_rect.min.y + min_y * canvas_rect.height(),
                        );
                        let max = Pos2::new(
                            canvas_rect.min.x + max_x * canvas_rect.width(),
                            canvas_rect.min.y + max_y * canvas_rect.height(),
                        );
                        let _bbox_rect = Rect::from_min_max(min, max);
                        // Dashed stroke effect using multiple short segments
                        let dash_len = 8.0;
                        let gap_len = 4.0;
                        let dash_color = Color32::from_rgb(255, 255, 0);
                        let sides = [
                            (min, Pos2::new(max.x, min.y)), // top
                            (Pos2::new(max.x, min.y), max), // right
                            (max, Pos2::new(min.x, max.y)), // bottom
                            (Pos2::new(min.x, max.y), min), // left
                        ];
                        for (a, b) in sides {
                            let dx = b.x - a.x;
                            let dy = b.y - a.y;
                            let len = (dx * dx + dy * dy).sqrt();
                            let steps = (len / (dash_len + gap_len)).ceil() as usize;
                            for s in 0..steps {
                                let t0 = (s as f32 * (dash_len + gap_len)).min(len) / len;
                                let t1 = ((s as f32 * (dash_len + gap_len) + dash_len)).min(len) / len;
                                painter.line_segment(
                                    [Pos2::new(a.x + dx * t0, a.y + dy * t0), Pos2::new(a.x + dx * t1, a.y + dy * t1)],
                                    Stroke::new(1.5, dash_color),
                                );
                            }
                        }
                    }
                }

                        }); // ScrollArea::both
                }); // ui.vertical (canvas column)
            });

            if regions_dirty {
                state.save_workspace();
            }

            // Push warp edits to the projector's live warp stage (version-bumped,
            // so the projector re-applies only on an actual change).
            if warp_dirty {
                state.stage.publish_warp();
                state.save_workspace();
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Surface geometry — rendered inside the Stage tab's right-hand panel.
    // ─────────────────────────────────────────────────────────────────────────

    #[cfg(feature = "projection")]
    fn draw_surface_properties(ui: &mut egui::Ui, state: &mut VardaAppState) {
        use crate::stage::{ContentMapping, SurfaceSource};

        // Collect channel/deck names for source selector, falling back to the
        // cached list when the mixer is contended.
        let source_options: Vec<(String, SurfaceSource)> = {
            let mut opts = vec![("Master".to_string(), SurfaceSource::Master)];
            if let Ok(mixer) = state.mixer.try_lock() {
                for ch in &mixer.channels {
                    opts.push((
                        format!("{} ({})", ch.name, &ch.uuid[..ch.uuid.len().min(4)]),
                        SurfaceSource::Channel(ch.uuid.clone()),
                    ));
                    if let Some(compositor) = ch.effect.as_any() {
                        if let Some(compositor) =
                            compositor.downcast_ref::<crate::graph::DeckCompositor>()
                        {
                            for deck in &compositor.decks {
                                opts.push((
                                    format!(
                                        "  {} ({})",
                                        deck.name,
                                        &deck.uuid[..deck.uuid.len().min(4)]
                                    ),
                                    SurfaceSource::Deck {
                                        channel_uuid: ch.uuid.clone(),
                                        deck_uuid: deck.uuid.clone(),
                                    },
                                ));
                            }
                        }
                    }
                }
                opts.push(("Domemaster".to_string(), SurfaceSource::Domemaster));
                state.stage.cached_source_options = opts.clone();
                opts
            } else {
                opts.push(("Domemaster".to_string(), SurfaceSource::Domemaster));
                if state.stage.cached_source_options.is_empty() {
                    opts
                } else {
                    state.stage.cached_source_options.clone()
                }
            }
        };

        let mut warp_dirty = false;
        let mut geo_dirty = false;

        // Stage design resolution in pixels, used for pixel-based sizing.
        let canvas_size = state.stage.canvas_size;

        ui.vertical_centered(|ui| {
            ui.set_max_width(400.0);
            ui.label(egui::RichText::new("Properties").strong());
            ui.separator();

            if let Some(surf) = state.stage.surfaces.get_mut(state.stage.selected_surface_index) {
                ui.label("Name:");
                if ui.text_edit_singleline(&mut surf.name).changed() {
                    geo_dirty = true;
                }
                ui.separator();

                ui.label("Source:");
                let current_label = surf.source.label();
                let prev_source = surf.source.clone();
                egui::ComboBox::from_id_salt("geo_source_sel")
                    .selected_text(&current_label)
                    .show_ui(ui, |ui| {
                        for (label, src) in &source_options {
                            if ui.selectable_label(surf.source == *src, label).clicked() {
                                surf.source = src.clone();
                            }
                        }
                    });
                if surf.source != prev_source {
                    geo_dirty = true;
                }
                ui.separator();

                ui.label("Content Mapping:");
                let mapping_label = surf.content_mapping.label();
                egui::ComboBox::from_id_salt("geo_content_mapping")
                    .selected_text(mapping_label)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(surf.content_mapping == ContentMapping::Fill, "Fill").clicked() {
                            surf.content_mapping = ContentMapping::Fill;
                            warp_dirty = true;
                        }
                        if ui.selectable_label(surf.content_mapping == ContentMapping::Mapped, "Mapped").clicked() {
                            surf.content_mapping = ContentMapping::Mapped;
                            warp_dirty = true;
                        }
                    });
                ui.separator();

                // ── Position & size in pixels ───────────────────────────
                // Edits the surface's axis-aligned bounding box in pixels of the
                // stage design resolution (= master output). Vertices are
                // scaled/translated to match so any polygon stays proportional.
                ui.label(egui::RichText::new("Position & Size (px)").strong());
                {
                    let cw = canvas_size[0].max(1) as f32;
                    let ch = canvas_size[1].max(1) as f32;
                    if surf.is_circular && !surf.vertices.is_empty() {
                        let center = surf.vertices[0];
                        let mut x_px = (center[0] - surf.radius) * cw;
                        let mut y_px = (center[1] - surf.radius) * ch;
                        let mut diam_px = surf.radius * 2.0 * cw;
                        let mut changed = false;
                        ui.horizontal(|ui| {
                            ui.label("X:");
                            if ui.add(egui::DragValue::new(&mut x_px).speed(1.0).suffix(" px")).changed() { changed = true; }
                            ui.label("Y:");
                            if ui.add(egui::DragValue::new(&mut y_px).speed(1.0).suffix(" px")).changed() { changed = true; }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Ø:");
                            if ui.add(egui::DragValue::new(&mut diam_px).speed(1.0).range(1.0..=f32::MAX).suffix(" px")).changed() { changed = true; }
                        });
                        if changed {
                            let new_r = (diam_px / 2.0 / cw).max(0.0001);
                            surf.radius = new_r;
                            surf.vertices[0] = [x_px / cw + new_r, y_px / ch + new_r];
                            // Crop region = the circle's bounding box on the master.
                            surf.uv_crop_rect = [
                                x_px / cw,
                                y_px / ch,
                                (x_px + diam_px) / cw,
                                (y_px + diam_px) / ch,
                            ];
                            geo_dirty = true;
                        }
                    } else if !surf.vertices.is_empty() {
                        let [min_x, min_y, max_x, max_y] = surf.bounding_box();
                        let mut x_px = min_x * cw;
                        let mut y_px = min_y * ch;
                        let mut w_px = (max_x - min_x) * cw;
                        let mut h_px = (max_y - min_y) * ch;
                        let mut changed = false;
                        ui.horizontal(|ui| {
                            ui.label("X:");
                            if ui.add(egui::DragValue::new(&mut x_px).speed(1.0).suffix(" px")).changed() { changed = true; }
                            ui.label("Y:");
                            if ui.add(egui::DragValue::new(&mut y_px).speed(1.0).suffix(" px")).changed() { changed = true; }
                        });
                        ui.horizontal(|ui| {
                            ui.label("W:");
                            if ui.add(egui::DragValue::new(&mut w_px).speed(1.0).range(1.0..=f32::MAX).suffix(" px")).changed() { changed = true; }
                            ui.label("H:");
                            if ui.add(egui::DragValue::new(&mut h_px).speed(1.0).range(1.0..=f32::MAX).suffix(" px")).changed() { changed = true; }
                        });
                        if changed {
                            let new_min_x = x_px / cw;
                            let new_min_y = y_px / ch;
                            let new_w = (w_px / cw).max(0.0001);
                            let new_h = (h_px / ch).max(0.0001);
                            let old_w = (max_x - min_x).max(0.0001);
                            let old_h = (max_y - min_y).max(0.0001);
                            for v in surf.vertices.iter_mut() {
                                v[0] = new_min_x + (v[0] - min_x) / old_w * new_w;
                                v[1] = new_min_y + (v[1] - min_y) / old_h * new_h;
                            }
                            for contour in surf.extra_contours.iter_mut() {
                                for v in contour.iter_mut() {
                                    v[0] = new_min_x + (v[0] - min_x) / old_w * new_w;
                                    v[1] = new_min_y + (v[1] - min_y) / old_h * new_h;
                                }
                            }
                            // The surface box IS the crop region over the master.
                            surf.uv_crop_rect =
                                [new_min_x, new_min_y, new_min_x + new_w, new_min_y + new_h];
                            geo_dirty = true;
                        }
                    }
                }
                ui.separator();

                // Extra contours
                if !surf.extra_contours.is_empty() {
                    ui.label(egui::RichText::new("Extra Contours").strong());
                    ui.horizontal(|ui| {
                        ui.label(format!("{} extra contour(s)", surf.extra_contours.len()));
                        if ui.small_button("Remove All").clicked() {
                            surf.extra_contours.clear();
                            geo_dirty = true;
                        }
                    });
                    ui.separator();
                }

                ui.label("Warp Mode:");
                let warp_label = match &surf.warp {
                    rustjay_projection::WarpMode::CornerPin { .. } => "Corner Pin",
                    rustjay_projection::WarpMode::Mesh(_) => "Mesh",
                };
                egui::ComboBox::from_id_salt("geo_warp_mode")
                    .selected_text(warp_label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                matches!(
                                    surf.warp,
                                    rustjay_projection::WarpMode::CornerPin { .. }
                                ),
                                "Corner Pin",
                            )
                            .clicked()
                        {
                            surf.warp = rustjay_projection::WarpMode::corner_pin([
                                [0.0, 0.0],
                                [1.0, 0.0],
                                [1.0, 1.0],
                                [0.0, 1.0],
                            ]);
                            warp_dirty = true;
                        }
                        if ui
                            .selectable_label(
                                matches!(surf.warp, rustjay_projection::WarpMode::Mesh(_)),
                                "Mesh",
                            )
                            .clicked()
                        {
                            surf.warp = rustjay_projection::WarpMode::Mesh(
                                rustjay_projection::WarpMesh::identity(4, 4),
                            );
                            warp_dirty = true;
                        }
                    });

                if let rustjay_projection::WarpMode::CornerPin { corners } = &mut surf.warp {
                    ui.label("Corners (normalized):");
                    for (i, corner) in corners.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(["TL", "TR", "BR", "BL"][i]);
                            if ui
                                .add(
                                    egui::DragValue::new(&mut corner[0])
                                        .speed(0.01)
                                        .range(0.0..=1.0),
                                )
                                .changed()
                            {
                                warp_dirty = true;
                            }
                            if ui
                                .add(
                                    egui::DragValue::new(&mut corner[1])
                                        .speed(0.01)
                                        .range(0.0..=1.0),
                                )
                                .changed()
                            {
                                warp_dirty = true;
                            }
                        });
                    }
                }

                ui.separator();
                ui.label(egui::RichText::new("UV Crop").strong());
                ui.label("Sampling region (stage canvas handles edit this visually):");
                {
                    let [min_u, min_v, max_u, max_v] = &mut surf.uv_crop_rect;
                    ui.horizontal(|ui| {
                        ui.label("Min U:");
                        if ui.add(egui::DragValue::new(min_u).speed(0.01).range(0.0..=1.0)).changed() {
                            warp_dirty = true;
                        }
                        ui.label("Min V:");
                        if ui.add(egui::DragValue::new(min_v).speed(0.01).range(0.0..=1.0)).changed() {
                            warp_dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Max U:");
                        if ui.add(egui::DragValue::new(max_u).speed(0.01).range(0.0..=1.0)).changed() {
                            warp_dirty = true;
                        }
                        ui.label("Max V:");
                        if ui.add(egui::DragValue::new(max_v).speed(0.01).range(0.0..=1.0)).changed() {
                            warp_dirty = true;
                        }
                    });
                    if ui.small_button("Reset UV Crop").clicked() {
                        *min_u = 0.0;
                        *min_v = 0.0;
                        *max_u = 1.0;
                        *max_v = 1.0;
                        warp_dirty = true;
                    }
                }

                // Dome config when this surface is routed to Domemaster
                if surf.source == crate::stage::SurfaceSource::Domemaster {
                    ui.separator();
                    ui.label(egui::RichText::new("Dome").strong());
                    let mut dome_enabled = false;
                    let mut dome_config = rustjay_projection::DomemasterConfig::default();
                    let mut dome_rotation = [0.0f32; 3];
                    if let Some(sync) = &state.stage.dome_sync {
                        if let Ok(g) = sync.lock() {
                            dome_enabled = g.enabled;
                            dome_config = g.config.clone();
                            dome_rotation = g.content_rotation;
                        }
                    }
                    let mut dirty = false;
                    if ui.checkbox(&mut dome_enabled, "Enabled").changed() {
                        dirty = true;
                    }
                    ui.horizontal(|ui| {
                        ui.label("Resolution:");
                        let mut res_idx = match dome_config.resolution {
                            rustjay_projection::DomemasterResolution::R1K => 0,
                            rustjay_projection::DomemasterResolution::R2K => 1,
                            rustjay_projection::DomemasterResolution::R4K => 2,
                        };
                        let prev = res_idx;
                        egui::ComboBox::from_id_salt("geo_dome_res")
                            .selected_text(["1K", "2K", "4K"][res_idx])
                            .show_ui(ui, |ui| {
                                for (i, name) in ["1K", "2K", "4K"].iter().enumerate() {
                                    if ui.selectable_label(res_idx == i, *name).clicked() {
                                        res_idx = i;
                                    }
                                }
                            });
                        if res_idx != prev {
                            dome_config.resolution = match res_idx {
                                0 => rustjay_projection::DomemasterResolution::R1K,
                                1 => rustjay_projection::DomemasterResolution::R2K,
                                _ => rustjay_projection::DomemasterResolution::R4K,
                            };
                            dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("FOV°:");
                        if ui
                            .add(
                                egui::DragValue::new(&mut dome_config.fov_degrees)
                                    .speed(1.0)
                                    .range(90.0..=220.0),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Tilt°:");
                        if ui
                            .add(
                                egui::DragValue::new(&mut dome_config.tilt_degrees)
                                    .speed(1.0)
                                    .range(-90.0..=90.0),
                            )
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Az:");
                        if ui
                            .add(egui::DragValue::new(&mut dome_rotation[0]).speed(0.01))
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.label("El:");
                        if ui
                            .add(egui::DragValue::new(&mut dome_rotation[1]).speed(0.01))
                            .changed()
                        {
                            dirty = true;
                        }
                        ui.label("Roll:");
                        if ui
                            .add(egui::DragValue::new(&mut dome_rotation[2]).speed(0.01))
                            .changed()
                        {
                            dirty = true;
                        }
                    });
                    if dirty {
                        state
                            .stage
                            .publish_dome(dome_enabled, dome_config, dome_rotation);
                    }
                }
            } else {
                ui.label("No surfaces. Add one from the Stage tab.");
            }
        });

        if warp_dirty {
            state.stage.publish_warp();
            state.save_workspace();
        }
        if geo_dirty {
            state.save_workspace();
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Stub tabs (Phase 8+)
    // ─────────────────────────────────────────────────────────────────────────

    impl AnyEguiTab for OutputsTab {
        fn name(&self) -> &str {
            "Outputs"
        }

        fn replaces(&self) -> Option<rustjay_engine::prelude::BuiltinTab> {
            Some(rustjay_engine::prelude::BuiltinTab::Output)
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            engine: &mut EngineState,
        ) {
            #[cfg_attr(not(feature = "projection"), allow(unused_variables))]
            let state = app_state
                .downcast_mut::<VardaAppState>()
                .expect("OutputsTab expects VardaAppState");

            ui.heading("Outputs");
            ui.separator();

            #[cfg(feature = "projection")]
            {
                // ── Projectors ──────────────────────────────────────────────
                ui.label(egui::RichText::new("Projectors").strong());
                let mut remove_proj: Option<usize> = None;
                let mut proj_dirty = false;
                for (i, proj) in state.stage.projectors.iter_mut().enumerate() {
                    ui.push_id(i, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut proj.enabled, "");
                            ui.text_edit_singleline(&mut proj.name);
                            ui.label("size:");
                            ui.add(
                                egui::DragValue::new(&mut proj.width)
                                    .speed(10)
                                    .range(1..=8192),
                            );
                            ui.label("×");
                            ui.add(
                                egui::DragValue::new(&mut proj.height)
                                    .speed(10)
                                    .range(1..=8192),
                            );
                            ui.label("monitor:");
                            let mut monitor =
                                proj.fullscreen_monitor.map(|m| m as i32).unwrap_or(-1);
                            if ui
                                .add(egui::DragValue::new(&mut monitor).speed(1).range(-1..=16))
                                .changed()
                            {
                                proj.fullscreen_monitor = if monitor < 0 {
                                    None
                                } else {
                                    Some(monitor as usize)
                                };
                                proj_dirty = true;
                            }
                            ui.label("surface:");
                            let surf_count = state.stage.surfaces.len();
                            let mut surf_idx = proj.surface_index.map(|s| s as i32).unwrap_or(-1);
                            let max_surf = surf_count.saturating_sub(1) as i32;
                            if ui
                                .add(egui::DragValue::new(&mut surf_idx).speed(1).range(-1..=max_surf))
                                .changed()
                            {
                                proj.surface_index = if surf_idx < 0 {
                                    None
                                } else {
                                    Some(surf_idx as usize)
                                };
                                proj_dirty = true;
                            }
                            ui.label("type:");
                            let prev_type = proj.output_type.clone();
                            egui::ComboBox::from_id_salt(format!("proj_type_{}", i))
                                .selected_text(proj.output_type.label())
                                .show_ui(ui, |ui| {
                                    use crate::stage::OutputType;
                                    ui.selectable_value(&mut proj.output_type, OutputType::Display, "Display");
                                    ui.selectable_value(&mut proj.output_type, OutputType::Ndi, "NDI");
                                    ui.selectable_value(&mut proj.output_type, OutputType::Recording, "Recording");
                                    #[cfg(target_os = "macos")]
                                    ui.selectable_value(&mut proj.output_type, OutputType::Syphon, "Syphon");
                                    #[cfg(target_os = "windows")]
                                    ui.selectable_value(&mut proj.output_type, OutputType::Spout, "Spout");
                                    #[cfg(target_os = "linux")]
                                    ui.selectable_value(&mut proj.output_type, OutputType::V4l2, "V4L2");
                                });
                            if proj.output_type != prev_type {
                                proj_dirty = true;
                            }
                            ui.label("rotate:");
                            let prev_rot = proj.rotation;
                            egui::ComboBox::from_id_salt(format!("proj_rot_{}", i))
                                .selected_text(proj.rotation.label())
                                .show_ui(ui, |ui| {
                                    use crate::stage::OutputRotation;
                                    ui.selectable_value(&mut proj.rotation, OutputRotation::Deg0, OutputRotation::Deg0.label());
                                    ui.selectable_value(&mut proj.rotation, OutputRotation::Deg90, OutputRotation::Deg90.label());
                                    ui.selectable_value(&mut proj.rotation, OutputRotation::Deg180, OutputRotation::Deg180.label());
                                    ui.selectable_value(&mut proj.rotation, OutputRotation::Deg270, OutputRotation::Deg270.label());
                                });
                            if proj.rotation != prev_rot {
                                proj_dirty = true;
                            }
                            if ui.button("🗑").clicked() {
                                remove_proj = Some(i);
                            }
                        });
                    });
                }
                if proj_dirty {
                    state.save_workspace();
                }
                if let Some(i) = remove_proj {
                    // Close the projector window if it exists.
                    #[cfg(feature = "projection")]
                    if let Some(handle) = state.projection_handle.as_ref() {
                        let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(sub) = any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() {
                            if let Some(window_id) = state.stage.projectors.get(i).and_then(|p| p.window_id) {
                                sub.remove_output(window_id);
                            } else {
                                // Window hasn't been created yet — clear pending queue.
                                sub.clear_pending();
                            }
                        }
                    }
                    state.stage.projectors.remove(i);
                    state.save_workspace();
                }
                // Sync runtime window IDs from the projection subsystem.
                #[cfg(feature = "projection")]
                if let Some(handle) = state.projection_handle.as_ref() {
                    let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(sub) = any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() {
                        let mut enabled_idx = 0;
                        for proj in state.stage.projectors.iter_mut() {
                            if proj.enabled {
                                if let Some(output) = sub.projectors.get(enabled_idx) {
                                    proj.window_id = Some(output.window_id);
                                }
                                enabled_idx += 1;
                            } else {
                                proj.window_id = None;
                            }
                        }
                    }
                }

                if ui.button("+ Add projector").clicked() {
                    let new_idx = state.stage.projectors.len();
                    state
                        .stage
                        .projectors
                        .push(crate::stage::VardaProjector::default());
                    // Ensure source_syncs, warp_syncs, and rotation_syncs exist for the new projector.
                    while state.stage.source_syncs.len() <= new_idx {
                        state.stage.source_syncs.push(std::sync::Arc::new(
                            std::sync::Mutex::new(crate::stage::SourceSync::default()),
                        ));
                    }
                    while state.stage.warp_syncs.len() <= new_idx {
                        state.stage.warp_syncs.push(std::sync::Arc::new(
                            std::sync::Mutex::new(crate::stage::WarpSync::default()),
                        ));
                    }
                    while state.stage.rotation_syncs.len() <= new_idx {
                        state.stage.rotation_syncs.push(std::sync::Arc::new(
                            std::sync::Mutex::new(rustjay_projection::RotationSync::default()),
                        ));
                    }
                    // Queue a window for the new projector.
                    #[cfg(feature = "projection")]
                    if let Some(handle) = state.projection_handle.as_ref() {
                        let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(sub) = any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() {
                            let proj = &state.stage.projectors[new_idx];
                            let attrs = winit::window::WindowAttributes::default()
                                .with_title(format!("Varda Projector {} - {}", new_idx + 1, proj.name))
                                .with_inner_size(winit::dpi::LogicalSize::new(proj.width, proj.height));
                            let w = state.stage.warp_syncs.get(new_idx).cloned().unwrap_or_else(|| {
                                std::sync::Arc::new(std::sync::Mutex::new(crate::stage::WarpSync::default()))
                            });
                            let d = state.stage.dome_sync.clone().unwrap();
                            let e = state.stage.edge_blend_sync.clone().unwrap();
                            let s = state.stage.source_syncs.get(new_idx).cloned().unwrap_or_else(|| {
                                std::sync::Arc::new(std::sync::Mutex::new(crate::stage::SourceSync::default()))
                            });
                            let r = state.stage.rotation_syncs.get(new_idx).cloned().unwrap_or_else(|| {
                                std::sync::Arc::new(std::sync::Mutex::new(rustjay_projection::RotationSync::default()))
                            });
                            sub.add_projector(attrs, proj.fullscreen_monitor, move |device, format| {
                                vec![
                                    Box::new(crate::stage::VardaSourceStage::new(device, format, s.clone())),
                                    Box::new(crate::stage::VardaDomeStage::new(device, format, d.clone())),
                                    Box::new(crate::stage::VardaEdgeBlendStage::new(device, format, e.clone())),
                                    Box::new(crate::stage::VardaWarpStage::new(device, format, w.clone())),
                                    Box::new(rustjay_projection::RotationStage::new(device, format, r.clone())),
                                ]
                            });
                            log::info!("[Outputs] Queued projector {} window creation", new_idx + 1);
                        }
                    }
                    state.save_workspace();
                }
                ui.separator();

                // ── Headless outputs ────────────────────────────────────────
                ui.label(egui::RichText::new("Headless Outputs").strong());
                let mut remove_hl: Option<usize> = None;
                let mut hl_dirty = false;
                for (i, hl) in state.stage.headless_outputs.iter_mut().enumerate() {
                    ui.push_id(format!("hl_{}", i), |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut hl.enabled, "");
                            ui.text_edit_singleline(&mut hl.name);
                            ui.label("size:");
                            ui.add(
                                egui::DragValue::new(&mut hl.width)
                                    .speed(10)
                                    .range(1..=8192),
                            );
                            ui.label("×");
                            ui.add(
                                egui::DragValue::new(&mut hl.height)
                                    .speed(10)
                                    .range(1..=8192),
                            );
                            ui.label("surface:");
                            let surf_count = state.stage.surfaces.len();
                            let mut surf_idx = hl.surface_index.map(|s| s as i32).unwrap_or(-1);
                            if ui
                                .add(egui::DragValue::new(&mut surf_idx).speed(1).range(-1..=surf_count.max(1) as i32 - 1))
                                .changed()
                            {
                                hl.surface_index = if surf_idx < 0 {
                                    None
                                } else {
                                    Some(surf_idx as usize)
                                };
                                hl_dirty = true;
                            }
                            ui.label("type:");
                            let prev_type = hl.output_type.clone();
                            egui::ComboBox::from_id_salt(format!("hl_type_{}", i))
                                .selected_text(hl.output_type.label())
                                .show_ui(ui, |ui| {
                                    use crate::stage::OutputType;
                                    ui.selectable_value(&mut hl.output_type, OutputType::Display, "Display");
                                    ui.selectable_value(&mut hl.output_type, OutputType::Ndi, "NDI");
                                    ui.selectable_value(&mut hl.output_type, OutputType::Recording, "Recording");
                                    #[cfg(target_os = "macos")]
                                    ui.selectable_value(&mut hl.output_type, OutputType::Syphon, "Syphon");
                                    #[cfg(target_os = "windows")]
                                    ui.selectable_value(&mut hl.output_type, OutputType::Spout, "Spout");
                                    #[cfg(target_os = "linux")]
                                    ui.selectable_value(&mut hl.output_type, OutputType::V4l2, "V4L2");
                                });
                            if hl.output_type != prev_type {
                                hl_dirty = true;
                            }
                            if ui.button("🗑").clicked() {
                                remove_hl = Some(i);
                            }
                        });
                    });
                }
                if hl_dirty {
                    state.save_workspace();
                }
                if let Some(i) = remove_hl {
                    #[cfg(feature = "projection")]
                    if let Some(handle) = state.projection_handle.as_ref() {
                        let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                        if let Some(sub) = any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() {
                            let mut enabled_idx = 0;
                            let mut found = false;
                            for (j, hl) in state.stage.headless_outputs.iter().enumerate() {
                                if j == i {
                                    found = true;
                                    break;
                                }
                                if hl.enabled && hl.pushed {
                                    enabled_idx += 1;
                                }
                            }
                            if found {
                                sub.remove_headless_output(enabled_idx);
                            }
                        }
                    }
                    state.stage.headless_outputs.remove(i);
                    state.save_workspace();
                }
                if ui.button("+ Add headless").clicked() {
                    state
                        .stage
                        .headless_outputs
                        .push(crate::stage::VardaHeadlessConfig::default());
                    state.save_workspace();
                }
                ui.separator();

                // ── Lighting outputs ────────────────────────────────────────
                ui.label(egui::RichText::new("Lighting Outputs").strong());
                state.stage.ensure_builtin_fixture_profiles();
                let mut remove_light: Option<usize> = None;
                let mut light_dirty = false;
                let profile_names: Vec<(String, String)> = state
                    .stage
                    .fixture_profiles
                    .iter()
                    .map(|p| (p.id.clone(), p.name.clone()))
                    .collect();
                for (i, light) in state.stage.lighting_outputs.iter_mut().enumerate() {
                    ui.push_id(format!("light_{}", i), |ui| {
                        // Output header
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut light.enabled, "").changed() {
                                light_dirty = true;
                            }
                            if ui.text_edit_singleline(&mut light.name).changed() {
                                light_dirty = true;
                            }
                            ui.label("type:");
                            let prev_type = light.output_type.clone();
                            egui::ComboBox::from_id_salt(format!("light_type_{}", i))
                                .selected_text(light.output_type.label())
                                .show_ui(ui, |ui| {
                                    use crate::stage::OutputType;
                                    ui.selectable_value(&mut light.output_type, OutputType::Sacn, "sACN");
                                    ui.selectable_value(&mut light.output_type, OutputType::ArtNet, "Art-Net");
                                });
                            if light.output_type != prev_type {
                                light_dirty = true;
                            }
                            if ui.button("🗑").clicked() {
                                remove_light = Some(i);
                            }
                        });
                        // Output-level transport
                        ui.horizontal(|ui| {
                            ui.label("gamma:");
                            if ui
                                .add(egui::DragValue::new(&mut light.gamma).speed(0.1).range(0.5..=4.0))
                                .changed()
                            {
                                light_dirty = true;
                            }
                            ui.label("priority:");
                            if ui
                                .add(egui::DragValue::new(&mut light.transport.priority).speed(1).range(0..=200))
                                .changed()
                            {
                                light_dirty = true;
                            }
                            ui.label("fps:");
                            if ui
                                .add(egui::DragValue::new(&mut light.transport.fps).speed(1).range(1.0..=100.0))
                                .changed()
                            {
                                light_dirty = true;
                            }
                            ui.label("dest IP:");
                            if ui.text_edit_singleline(&mut light.transport.dest_ip).changed() {
                                light_dirty = true;
                            }
                        });

                        // Segments list
                        let mut remove_segment: Option<usize> = None;
                        let mut add_segment = false;
                        ui.indent(format!("light_{}_segments", i), |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Segments").weak());
                                if ui.small_button("+ Add segment").clicked() {
                                    add_segment = true;
                                }
                            });
                            for (si, seg) in light.segments.iter_mut().enumerate() {
                                ui.push_id(format!("seg_{}_{}", i, si), |ui| {
                                    ui.group(|ui| {
                                        ui.horizontal(|ui| {
                                            if ui.checkbox(&mut seg.enabled, "").changed() {
                                                light_dirty = true;
                                            }
                                            if ui.text_edit_singleline(&mut seg.name).changed() {
                                                light_dirty = true;
                                            }
                                            if ui.small_button("✖").clicked() {
                                                remove_segment = Some(si);
                                            }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("source:");
                                            let sel_text = match &seg.source_surface {
                                                None => "Manual region".to_string(),
                                                Some(uuid) => state
                                                    .stage
                                                    .surfaces
                                                    .iter()
                                                    .find(|s| &s.uuid == uuid)
                                                    .map(|s| s.name.clone())
                                                    .unwrap_or_else(|| "<missing surface>".to_string()),
                                            };
                                            let prev_src = seg.source_surface.clone();
                                            egui::ComboBox::from_id_salt(format!("seg_src_{}_{}", i, si))
                                                .selected_text(sel_text)
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut seg.source_surface, None, "Manual region");
                                                    for surf in &state.stage.surfaces {
                                                        ui.selectable_value(
                                                            &mut seg.source_surface,
                                                            Some(surf.uuid.clone()),
                                                            &surf.name,
                                                        );
                                                    }
                                                });
                                            if seg.source_surface != prev_src { light_dirty = true; }
                                        });
                                        ui.add_enabled_ui(seg.source_surface.is_none(), |ui| {
                                            ui.horizontal(|ui| {
                                                ui.label("region u0:");
                                                if ui.add(egui::DragValue::new(&mut seg.region[0]).speed(0.01).range(0.0..=1.0)).changed() { light_dirty = true; }
                                                ui.label("v0:");
                                                if ui.add(egui::DragValue::new(&mut seg.region[1]).speed(0.01).range(0.0..=1.0)).changed() { light_dirty = true; }
                                                ui.label("u1:");
                                                if ui.add(egui::DragValue::new(&mut seg.region[2]).speed(0.01).range(0.0..=1.0)).changed() { light_dirty = true; }
                                                ui.label("v1:");
                                                if ui.add(egui::DragValue::new(&mut seg.region[3]).speed(0.01).range(0.0..=1.0)).changed() { light_dirty = true; }
                                            });
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("grid:");
                                            if ui.add(egui::DragValue::new(&mut seg.grid[0]).speed(1).range(1..=4096)).changed() { light_dirty = true; }
                                            ui.label("×");
                                            if ui.add(egui::DragValue::new(&mut seg.grid[1]).speed(1).range(1..=4096)).changed() { light_dirty = true; }
                                            ui.label("corner:");
                                            let prev_corner = seg.scan.start_corner;
                                            egui::ComboBox::from_id_salt(format!("seg_corner_{}_{}", i, si))
                                                .selected_text(seg.scan.start_corner.label())
                                                .width(50.0)
                                                .show_ui(ui, |ui| {
                                                    use crate::stage::Corner;
                                                    ui.selectable_value(&mut seg.scan.start_corner, Corner::TopLeft, "TL");
                                                    ui.selectable_value(&mut seg.scan.start_corner, Corner::TopRight, "TR");
                                                    ui.selectable_value(&mut seg.scan.start_corner, Corner::BottomLeft, "BL");
                                                    ui.selectable_value(&mut seg.scan.start_corner, Corner::BottomRight, "BR");
                                                });
                                            if seg.scan.start_corner != prev_corner { light_dirty = true; }
                                            if ui.checkbox(&mut seg.scan.serpentine, "serp").changed() { light_dirty = true; }
                                            ui.label("axis:");
                                            let prev_axis = seg.scan.primary;
                                            egui::ComboBox::from_id_salt(format!("seg_axis_{}_{}", i, si))
                                                .selected_text(seg.scan.primary.label())
                                                .width(60.0)
                                                .show_ui(ui, |ui| {
                                                    use crate::stage::Axis;
                                                    ui.selectable_value(&mut seg.scan.primary, Axis::Horizontal, "Horiz");
                                                    ui.selectable_value(&mut seg.scan.primary, Axis::Vertical, "Vert");
                                                });
                                            if seg.scan.primary != prev_axis { light_dirty = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("profile:");
                                            let selected_name = state
                                                .stage
                                                .fixture_profiles
                                                .iter()
                                                .find(|p| p.id == seg.profile)
                                                .map(|p| p.name.clone())
                                                .unwrap_or_else(|| "RGB".to_string());
                                            let prev_profile = seg.profile.clone();
                                            egui::ComboBox::from_id_salt(format!("seg_profile_{}_{}", i, si))
                                                .selected_text(selected_name)
                                                .show_ui(ui, |ui| {
                                                    for (id, name) in &profile_names {
                                                        ui.selectable_value(&mut seg.profile, id.clone(), name);
                                                    }
                                                });
                                            if seg.profile != prev_profile { light_dirty = true; }
                                            if ui.add(egui::DragValue::new(&mut seg.start_universe).speed(1).range(1..=63999)).changed() { light_dirty = true; }
                                            ui.label("ch:");
                                            if ui.add(egui::DragValue::new(&mut seg.start_channel).speed(1).range(1..=512)).changed() { light_dirty = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            let footprint = state
                                                .stage
                                                .fixture_profiles
                                                .iter()
                                                .find(|p| p.id == seg.profile)
                                                .map(|p| p.channels.len())
                                                .unwrap_or(3);
                                            let count = (seg.grid[0] as usize) * (seg.grid[1] as usize);
                                            let spans = rustjay_lighting::segment_spans(
                                                &light.name,
                                                &seg.name,
                                                seg.start_universe,
                                                seg.start_channel,
                                                footprint,
                                                count,
                                            );
                                            let span_text = if spans.is_empty() {
                                                "—".to_string()
                                            } else if spans.len() == 1 {
                                                format!("U{} ch{}–{}", spans[0].universe, spans[0].start, spans[0].end)
                                            } else {
                                                let first = spans.first().unwrap();
                                                let last = spans.last().unwrap();
                                                format!(
                                                    "U{} ch{}–{} → U{} ch{}–{}",
                                                    first.universe, first.start, first.end,
                                                    last.universe, last.start, last.end
                                                )
                                            };
                                            ui.label(egui::RichText::new(format!("patch: {}", span_text)).weak().monospace());
                                            ui.label(format!("sample: {}", seg.sample_mode.label())).on_hover_text("Only Point sampling is available in M3");
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("bright:");
                                            if ui.add(egui::DragValue::new(&mut seg.color.brightness).speed(0.01).range(0.0..=2.0)).changed() { light_dirty = true; }
                                            ui.label("gain R:");
                                            if ui.add(egui::DragValue::new(&mut seg.color.gain[0]).speed(0.01).range(0.0..=2.0)).changed() { light_dirty = true; }
                                            ui.label("G:");
                                            if ui.add(egui::DragValue::new(&mut seg.color.gain[1]).speed(0.01).range(0.0..=2.0)).changed() { light_dirty = true; }
                                            ui.label("B:");
                                            if ui.add(egui::DragValue::new(&mut seg.color.gain[2]).speed(0.01).range(0.0..=2.0)).changed() { light_dirty = true; }
                                            ui.label("dim:");
                                            if ui.add(egui::DragValue::new(&mut seg.color.master_dimmer).speed(0.01).range(0.0..=1.0)).changed() { light_dirty = true; }
                                        });
                                        ui.horizontal(|ui| {
                                            ui.label("white:");
                                            let mut white_mode = match seg.color.white {
                                                crate::stage::WhiteMode::Off => 0usize,
                                                crate::stage::WhiteMode::Min { .. } => 1,
                                                crate::stage::WhiteMode::MinSubtract { .. } => 2,
                                            };
                                            let prev_white = white_mode;
                                            egui::ComboBox::from_id_salt(format!("seg_white_{}_{}", i, si))
                                                .selected_text(match white_mode {
                                                    0 => "Off",
                                                    1 => "Min",
                                                    _ => "MinSubtract",
                                                })
                                                .show_ui(ui, |ui| {
                                                    ui.selectable_value(&mut white_mode, 0, "Off");
                                                    ui.selectable_value(&mut white_mode, 1, "Min");
                                                    ui.selectable_value(&mut white_mode, 2, "MinSubtract");
                                                });
                                            if white_mode != prev_white {
                                                let amount = match seg.color.white {
                                                    crate::stage::WhiteMode::Off => 1.0,
                                                    crate::stage::WhiteMode::Min { amount }
                                                    | crate::stage::WhiteMode::MinSubtract { amount } => amount,
                                                };
                                                seg.color.white = match white_mode {
                                                    0 => crate::stage::WhiteMode::Off,
                                                    1 => crate::stage::WhiteMode::Min { amount },
                                                    _ => crate::stage::WhiteMode::MinSubtract { amount },
                                                };
                                                light_dirty = true;
                                            }
                                            let mut amount = match seg.color.white {
                                                crate::stage::WhiteMode::Off => 1.0,
                                                crate::stage::WhiteMode::Min { amount }
                                                | crate::stage::WhiteMode::MinSubtract { amount } => amount,
                                            };
                                            if ui.add(egui::DragValue::new(&mut amount).speed(0.01).range(0.0..=2.0)).changed() {
                                                seg.color.white = match white_mode {
                                                    0 => crate::stage::WhiteMode::Off,
                                                    1 => crate::stage::WhiteMode::Min { amount },
                                                    _ => crate::stage::WhiteMode::MinSubtract { amount },
                                                };
                                                light_dirty = true;
                                            }
                                        });
                                    });
                                });
                            }
                        });
                        if add_segment {
                            light.segments.push(crate::stage::LightingSegment::default());
                            light_dirty = true;
                        }
                        if let Some(si) = remove_segment {
                            if light.segments.len() > 1 {
                                light.segments.remove(si);
                                light_dirty = true;
                            }
                        }

                        // Activity meters
                        if let Some(sampler_id) = light.sampler_id {
                            if let Some(frame) = state.lighting_last_frames.get(&sampler_id) {
                                ui.collapsing("Activity", |ui| {
                                    if frame.is_empty() {
                                        ui.label("No activity");
                                    } else {
                                        for (universe, data) in frame.iter() {
                                            let max = data.iter().copied().max().unwrap_or(0) as f32 / 255.0;
                                            ui.horizontal(|ui| {
                                                ui.label(format!("U{}", universe));
                                                ui.add(
                                                    egui::ProgressBar::new(max)
                                                        .text(format!("{:.0}", max * 255.0))
                                                        .desired_width(ui.available_width()),
                                                );
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    });
                }
                if light_dirty {
                    state.save_workspace();
                }
                if let Some(i) = remove_light {
                    if let Some(id) = state.stage.lighting_outputs[i].sampler_id {
                        if let Some(sender) = state.lighting_senders.remove(&id) {
                            sender.shutdown();
                        }
                    }
                    state.stage.lighting_outputs.remove(i);
                    state.save_workspace();
                }
                if ui.button("+ Add lighting").clicked() {
                    state
                        .stage
                        .lighting_outputs
                        .push(crate::stage::LightingOutput::default());
                    state.save_workspace();
                }

                // Overlap warnings
                if !state.lighting_overlap_warnings.is_empty() {
                    ui.group(|ui| {
                        ui.label(egui::RichText::new("⚠ Patch overlaps").color(ui.visuals().error_fg_color).strong());
                        for o in &state.lighting_overlap_warnings {
                            ui.label(format!(
                                "U{} ch{}–{}: {} / {}",
                                o.universe, o.start, o.end, o.a.owner, o.b.owner
                            ));
                        }
                    });
                }

                // ── Fixture profile library ─────────────────────────────────
                ui.label(egui::RichText::new("Fixture Profiles").strong());
                let builtin_ids: std::collections::HashSet<String> =
                    crate::stage::builtin_fixture_profiles()
                        .into_iter()
                        .map(|p| p.id)
                        .collect();
                let mut remove_profile: Option<usize> = None;
                let mut profile_dirty = false;
                for (i, profile) in state.stage.fixture_profiles.iter_mut().enumerate() {
                    ui.push_id(format!("profile_{}", i), |ui| {
                        let is_builtin = builtin_ids.contains(&profile.id);
                        if is_builtin {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&profile.name).strong());
                                ui.label(format!("{}ch", profile.channels.len()));
                                ui.label(profile.channels.iter().map(|r| r.label()).collect::<Vec<_>>().join(","));
                            });
                        } else {
                            ui.group(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label("name:");
                                    if ui.text_edit_singleline(&mut profile.name).changed() {
                                        profile_dirty = true;
                                    }
                                    ui.label(format!("{}ch", profile.channels.len()));
                                    if ui.small_button("✖").clicked() {
                                        remove_profile = Some(i);
                                    }
                                });
                                ui.horizontal(|ui| {
                                    ui.label("add:");
                                    use crate::stage::ChannelRole;
                                    let mut push = |role| {
                                        profile.channels.push(role);
                                        profile_dirty = true;
                                    };
                                    if ui.small_button("R").clicked() { push(ChannelRole::Red); }
                                    if ui.small_button("G").clicked() { push(ChannelRole::Green); }
                                    if ui.small_button("B").clicked() { push(ChannelRole::Blue); }
                                    if ui.small_button("W").clicked() { push(ChannelRole::White); }
                                    if ui.small_button("A").clicked() { push(ChannelRole::Amber); }
                                    if ui.small_button("UV").clicked() { push(ChannelRole::Uv); }
                                    if ui.small_button("D").clicked() { push(ChannelRole::Dimmer); }
                                    if ui.small_button("S").clicked() { push(ChannelRole::Static(255)); }
                                    if ui.small_button("✖ last").clicked() {
                                        profile.channels.pop();
                                        profile_dirty = true;
                                    }
                                    if ui.small_button("Clear").clicked() {
                                        profile.channels.clear();
                                        profile_dirty = true;
                                    }
                                });
                                // Editable value for the last channel if it is Static.
                                if let Some(crate::stage::ChannelRole::Static(v)) = profile.channels.last_mut() {
                                    ui.horizontal(|ui| {
                                        ui.label("static value:");
                                        if ui.add(egui::DragValue::new(v).speed(1).range(0..=255)).changed() {
                                            profile_dirty = true;
                                        }
                                    });
                                }
                                ui.horizontal(|ui| {
                                    ui.label("order:");
                                    ui.label(profile.channels.iter().map(|r| r.label()).collect::<Vec<_>>().join(","));
                                });
                            });
                        }
                    });
                }
                if let Some(i) = remove_profile {
                    // If any segment uses the deleted profile, fall back to "rgb".
                    let removed_id = state.stage.fixture_profiles[i].id.clone();
                    state.stage.fixture_profiles.remove(i);
                    for light in &mut state.stage.lighting_outputs {
                        for seg in &mut light.segments {
                            if seg.profile == removed_id {
                                seg.profile = "rgb".to_string();
                            }
                        }
                    }
                    profile_dirty = true;
                }
                ui.horizontal(|ui| {
                    ui.label("New from template:");
                    let mut new_template = 0usize;
                    let templates = crate::stage::builtin_fixture_profiles();
                    egui::ComboBox::from_id_salt("profile_template")
                        .selected_text("RGB")
                        .show_ui(ui, |ui| {
                            for (idx, p) in templates.iter().enumerate() {
                                ui.selectable_value(&mut new_template, idx, &p.name);
                            }
                        });
                    if ui.button("+ Add").clicked() {
                        let template = &templates[new_template];
                        let new_id = format!("{}_{}", template.id, state.stage.fixture_profiles.len());
                        let mut new_profile = template.clone();
                        new_profile.id = new_id;
                        new_profile.name = format!("{} Copy", new_profile.name);
                        state.stage.fixture_profiles.push(new_profile);
                        profile_dirty = true;
                    }
                });
                if profile_dirty {
                    state.save_workspace();
                }
                ui.separator();

                // Edge-blend controls
                ui.label(egui::RichText::new("Edge Blend").strong());
                let mut config = rustjay_projection::EdgeBlendConfig::default();
                if let Some(sync) = &state.stage.edge_blend_sync {
                    if let Ok(g) = sync.lock() {
                        config = g.config;
                    }
                }
                let mut dirty = false;
                let mut edge_ui = |ui: &mut egui::Ui,
                                   edge: &mut rustjay_projection::EdgeBlendEdge,
                                   label: &str| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut edge.enabled, label).changed() {
                            dirty = true;
                        }
                        if edge.enabled {
                            ui.label("width:");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut edge.width)
                                        .speed(0.001)
                                        .range(0.0..=0.5),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                            ui.label("γ:");
                            if ui
                                .add(
                                    egui::DragValue::new(&mut edge.gamma)
                                        .speed(0.1)
                                        .range(0.5..=4.0),
                                )
                                .changed()
                            {
                                dirty = true;
                            }
                        }
                    });
                };
                edge_ui(ui, &mut config.left, "Left");
                edge_ui(ui, &mut config.right, "Right");
                edge_ui(ui, &mut config.top, "Top");
                edge_ui(ui, &mut config.bottom, "Bottom");
                if dirty {
                    state.stage.publish_edge_blend(config);
                }

                // ── Per-output recording sync ───────────────────────────────
                #[cfg(feature = "projection")]
                if let Some(handle) = state.projection_handle.as_ref() {
                    let mut any_guard = handle.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(sub) = any_guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() {
                        let fps = engine.target_fps as f32;
                        let codec = self.io_codec();

                        // Sync projector recordings
                        let mut enabled_idx = 0;
                        for (i, proj) in state.stage.projectors.iter().enumerate() {
                            if proj.enabled {
                                match proj.output_type {
                                    crate::stage::OutputType::Recording => {
                                        if !sub.is_projector_recording(enabled_idx) {
                                            let path = self.auto_record_path(&format!("projector_{}_{}", i, proj.name));
                                            if let Err(e) = sub.start_projector_recording(enabled_idx, &path, fps, codec) {
                                                log::error!("[Outputs] Failed to start projector {i} recording: {e}");
                                            }
                                        }
                                    }
                                    _ => {
                                        if sub.is_projector_recording(enabled_idx) {
                                            sub.stop_projector_recording(enabled_idx);
                                        }
                                    }
                                }
                                enabled_idx += 1;
                            }
                        }

                        // Sync headless recordings
                        let mut enabled_idx = 0;
                        for (i, hl) in state.stage.headless_outputs.iter().enumerate() {
                            if hl.enabled && hl.pushed {
                                match hl.output_type {
                                    crate::stage::OutputType::Recording => {
                                        if !sub.is_headless_recording(enabled_idx) {
                                            let path = self.auto_record_path(&format!("headless_{}_{}", i, hl.name));
                                            if let Err(e) = sub.start_headless_recording(enabled_idx, &path, fps, codec) {
                                                log::error!("[Outputs] Failed to start headless {i} recording: {e}");
                                            }
                                        }
                                    }
                                    _ => {
                                        if sub.is_headless_recording(enabled_idx) {
                                            sub.stop_headless_recording(enabled_idx);
                                        }
                                    }
                                }
                                enabled_idx += 1;
                            }
                        }
                    }
                }
            }

            #[cfg(not(feature = "projection"))]
            {
                ui.label("Projection feature not enabled.");
                ui.label("Enable the 'projection' feature for multi-output support.");
            }

            ui.separator();
            ui.label(egui::RichText::new("Recording").strong());

            if let Ok(mut guard) = self.pending_save_path.lock() {
                if let Some(path) = guard.take() {
                    self.recording_path = path.to_string_lossy().to_string();
                }
            }

            ui.horizontal(|ui| {
                ui.label("Codec:");
                egui::ComboBox::from_id_salt("recorder_codec")
                    .selected_text(format!("{:?}", self.recording_codec))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.recording_codec, rustjay_core::RecorderCodec::H264, "H.264");
                        ui.selectable_value(&mut self.recording_codec, rustjay_core::RecorderCodec::H265, "H.265");
                        ui.selectable_value(&mut self.recording_codec, rustjay_core::RecorderCodec::AV1, "AV1");
                        ui.selectable_value(&mut self.recording_codec, rustjay_core::RecorderCodec::ProRes422, "ProRes 422");
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Path:");
                ui.text_edit_singleline(&mut self.recording_path)
                    .on_hover_text("Output file path (relative or absolute)");
                if ui.button("Browse…").clicked() {
                    let pending = self.pending_save_path.clone();
                    let ctx = ui.ctx().clone();
                    let ext = match self.recording_codec {
                        rustjay_core::RecorderCodec::H264 | rustjay_core::RecorderCodec::H265 | rustjay_core::RecorderCodec::AV1 => "mp4",
                        rustjay_core::RecorderCodec::ProRes422 => "mov",
                    };
                    std::thread::spawn(move || {
                        if let Some(path) = rfd::FileDialog::new()
                            .add_filter("Video", &[ext])
                            .set_file_name(format!("recording.{}", ext))
                            .save_file()
                        {
                            if let Ok(mut guard) = pending.lock() {
                                *guard = Some(path);
                            }
                            ctx.request_repaint();
                        }
                    });
                }
            });
            ui.horizontal(|ui| {
                let is_recording = engine.recording_active;
                if ui.add_enabled(!is_recording, egui::Button::new("⏺ Start")).clicked() {
                    engine.output_command = rustjay_core::OutputCommand::StartRecording {
                        path: self.recording_path.clone(),
                        codec: self.recording_codec,
                    };
                }
                if ui.add_enabled(is_recording, egui::Button::new("⏹ Stop")).clicked() {
                    engine.output_command = rustjay_core::OutputCommand::StopRecording;
                }
                if is_recording {
                    ui.label(egui::RichText::new("● REC").color(egui::Color32::RED));
                }
            });
        }
    }

    impl AnyEguiTab for SequencerTab {
        fn name(&self) -> &str {
            "Sequencer"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            let state = app_state
                .downcast_mut::<VardaAppState>()
                .expect("SequencerTab expects VardaAppState");

            ui.heading("Sequencer");
            ui.separator();

            let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());
            let seq = &mut mixer.sequencer;

            // Playback controls
            ui.horizontal(|ui| {
                if ui.button("▶ Play").clicked() {
                    seq.play();
                }
                if ui.button("⏸ Pause").clicked() {
                    seq.pause();
                }
                if ui.button("⏹ Stop").clicked() {
                    seq.stop();
                }
                ui.checkbox(&mut seq.looping, "Loop");
            });

            ui.separator();

            // Step list
            ui.label(egui::RichText::new("Steps").strong());
            let mut remove_idx: Option<usize> = None;
            for (i, step) in seq.steps.iter().enumerate() {
                ui.push_id(i, |ui| {
                    ui.horizontal(|ui| {
                        let label = match &step.kind {
                            rustjay_mixer::StepKind::Crossfade { target, beats } => {
                                format!(
                                    "Crossfade → {:.0}% over {:.1} beats",
                                    target * 100.0,
                                    beats
                                )
                            }
                            rustjay_mixer::StepKind::Hold { beats } => {
                                format!("Hold {:.1} beats", beats)
                            }
                            rustjay_mixer::StepKind::TimedCrossfade { target, seconds } => {
                                format!(
                                    "Timed Crossfade → {:.0}% over {:.1}s",
                                    target * 100.0,
                                    seconds
                                )
                            }
                            rustjay_mixer::StepKind::TimedHold { seconds } => {
                                format!("Timed Hold {:.1}s", seconds)
                            }
                            rustjay_mixer::StepKind::Effect(_) => "Effect".to_string(),
                        };
                        let marker = if seq.playing && seq.index == i {
                            "▶ "
                        } else {
                            "  "
                        };
                        ui.label(format!("{}{}", marker, label));
                        if ui.small_button("✖").clicked() {
                            remove_idx = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove_idx {
                seq.steps.remove(i);
                if seq.index >= seq.steps.len() && !seq.steps.is_empty() {
                    seq.index = seq.steps.len() - 1;
                }
            }

            ui.separator();
            ui.label(egui::RichText::new("Add Step").strong());

            // Add step controls
            ui.horizontal(|ui| {
                if ui.button("+ Crossfade (beats)").clicked() {
                    seq.steps
                        .push(rustjay_mixer::TransitionStep::crossfade(1.0, 4.0));
                }
                if ui.button("+ Hold (beats)").clicked() {
                    seq.steps.push(rustjay_mixer::TransitionStep::hold(4.0));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("+ Crossfade (timed)").clicked() {
                    seq.steps
                        .push(rustjay_mixer::TransitionStep::timed_crossfade(1.0, 2.0));
                }
                if ui.button("+ Hold (timed)").clicked() {
                    seq.steps
                        .push(rustjay_mixer::TransitionStep::timed_hold(3.0));
                }
            });

            ui.separator();
            ui.label(egui::RichText::new("Quick Transitions").strong());
            ui.horizontal(|ui| {
                if ui.button("Auto → A").clicked() {
                    mixer.auto = Some(rustjay_mixer::AutoCrossfade::new(
                        mixer.crossfader,
                        0.0,
                        1.0,
                        rustjay_mixer::Easing::EaseInOut,
                    ));
                }
                if ui.button("Auto → B").clicked() {
                    mixer.auto = Some(rustjay_mixer::AutoCrossfade::new(
                        mixer.crossfader,
                        1.0,
                        1.0,
                        rustjay_mixer::Easing::EaseInOut,
                    ));
                }
                if ui.button("Beat-Sync → A").clicked() {
                    mixer.beat_sync = Some(rustjay_mixer::BeatSyncCrossfade::new(0.0, 4.0));
                }
                if ui.button("Beat-Sync → B").clicked() {
                    mixer.beat_sync = Some(rustjay_mixer::BeatSyncCrossfade::new(1.0, 4.0));
                }
            });
        }
    }

    impl AnyEguiTab for InspectorTab {
        fn name(&self) -> &str {
            "Inspector"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Inspector");
            ui.separator();
            ui.label("Context panel for the selected node.");
            ui.label("Coming in a future phase.");
        }
    }
}
