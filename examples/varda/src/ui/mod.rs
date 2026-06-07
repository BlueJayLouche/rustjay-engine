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
pub struct MixerTab;

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
}

impl Default for EffectsTab {
    fn default() -> Self {
        Self {
            selected_channel_uuid: String::new(),
            stream_url: String::new(),
            stream_name: String::new(),
            stream_kind: "rtmp".to_string(),
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
        }
    }
}

/// Geometry tab — properties panel for the selected surface.
pub struct GeometryTab;

impl Default for GeometryTab {
    fn default() -> Self {
        Self::new()
    }
}

impl GeometryTab {
    pub fn new() -> Self {
        Self
    }
}

/// Outputs tab — window/display/NDI/stream/record assignment.
pub struct OutputsTab {
    recording_path: String,
    recording_codec: rustjay_core::RecorderCodec,
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
                .downcast_ref::<VardaAppState>()
                .expect("MixerTab expects VardaAppState");

            // Cmd+S manual save
            if ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
                state.save_workspace();
            }

            ui.heading("Mixer");
            ui.separator();
            param_slider(ui, engine, "crossfader", "Crossfader", 0.0, 1.0);
            ui.separator();

            let mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

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

            if !mixer.master.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Master FX").strong());
                for slot in &mixer.master {
                    let status = if slot.enabled { "●" } else { "○" };
                    ui.label(format!("{} {}", status, slot.effect.label()));
                }
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

            ui.heading("Decks");
            ui.separator();

            let mut removals: Vec<crate::PendingRemoval> = Vec::new();

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

                                    if !deck.chain.is_empty() {
                                        ui.label("FX:");
                                        let mut fx_i = 0;
                                        while fx_i < deck.chain.len() {
                                            let mut enabled = deck.chain[fx_i].enabled;
                                            let fx_label = deck.chain[fx_i].effect.label();
                                            if ui.checkbox(&mut enabled, fx_label).changed() {
                                                deck.set_effect_enabled(fx_i, enabled);
                                            }
                                            fx_i += 1;
                                        }
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
                    ui.horizontal(|ui| {
                        ui.label("Path:");
                        ui.text_edit_singleline(&mut self.file_path);
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

            let mut mixer = state.mixer.lock().unwrap_or_else(|e| e.into_inner());

            for ch in &mut mixer.channels {
                ui.push_id(ch.uuid.clone(), |ui| {
                    ui.collapsing(format!("Channel: {}", ch.name), |ui| {
                        // Channel FX
                        if !ch.chain.is_empty() {
                            ui.label("Channel FX:");
                            let mut fx_i = 0;
                            while fx_i < ch.chain.len() {
                                let mut enabled = ch.chain[fx_i].enabled;
                                let fx_label = ch.chain[fx_i].effect.label();
                                if ui
                                    .push_id(("ch_fx", fx_i), |ui| {
                                        ui.checkbox(&mut enabled, fx_label).changed()
                                    })
                                    .inner
                                {
                                    ch.chain[fx_i].enabled = enabled;
                                }
                                fx_i += 1;
                            }
                        }

                        // Deck FX
                        let Some(compositor) = ch.effect.as_any_mut() else {
                            return;
                        };
                        let Some(compositor) = compositor.downcast_mut::<DeckCompositor>() else {
                            return;
                        };
                        for deck in &mut compositor.decks {
                            if deck.chain.is_empty() {
                                continue;
                            }
                            ui.push_id(deck.uuid.clone(), |ui| {
                                ui.label(format!("Deck {} FX:", deck.name));
                                let mut fx_i = 0;
                                while fx_i < deck.chain.len() {
                                    let mut enabled = deck.chain[fx_i].enabled;
                                    let fx_label = deck.chain[fx_i].effect.label();
                                    if ui
                                        .push_id(fx_i, |ui| {
                                            ui.checkbox(&mut enabled, fx_label).changed()
                                        })
                                        .inner
                                    {
                                        deck.set_effect_enabled(fx_i, enabled);
                                    }
                                    fx_i += 1;
                                }
                            });
                        }
                    });
                });
            }

            if !mixer.master.is_empty() {
                ui.collapsing("Master FX", |ui| {
                    let mut fx_i = 0;
                    while fx_i < mixer.master.len() {
                        let mut enabled = mixer.master[fx_i].enabled;
                        let fx_label = mixer.master[fx_i].effect.label();
                        if ui
                            .push_id(("master_fx", fx_i), |ui| {
                                ui.checkbox(&mut enabled, fx_label).changed()
                            })
                            .inner
                        {
                            mixer.master[fx_i].enabled = enabled;
                        }
                        fx_i += 1;
                    }
                });
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
                let state = _app_state
                    .downcast_mut::<VardaAppState>()
                    .expect("StageTab expects VardaAppState");
                Self::draw_stage_tab(ui, state, &mut self.import_path);
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
            ui: &mut egui::Ui,
            state: &mut VardaAppState,
            import_path: &mut String,
        ) {
            use crate::stage::{ContentMapping, SurfaceSource, VardaSurface};
            use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};

            // Set when a surface warp is edited this frame, so we publish to the
            // projector only on change (avoids per-frame mesh rebuilds).
            let mut warp_dirty = false;

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
                                            content_mapping: ContentMapping::Fill,
                                            extra_contours: Vec::new(),
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
                                            content_mapping: ContentMapping::Fill,
                                            extra_contours,
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

                // Center: 2D canvas
                let canvas_rect = ui.available_rect_before_wrap();
                let canvas_size = canvas_rect.size().min_elem();
                let canvas_rect = Rect::from_min_size(canvas_rect.min, Vec2::splat(canvas_size));

                let response = ui.interact(
                    canvas_rect,
                    ui.id().with("stage_canvas"),
                    egui::Sense::click(),
                );
                let painter = ui.painter_at(canvas_rect);

                // Background
                painter.rect_filled(canvas_rect, CornerRadius::ZERO, Color32::from_gray(30));
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
                        painter.add(egui::Shape::convex_polygon(
                            points.clone(),
                            color.linear_multiply(0.3),
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

                // ── Warp drag handles ────────────────────────────────────────
                let mut handle_dragged = false;

                // Clone handle positions to avoid borrow issues during interaction
                let corner_handles: Option<[[f32; 2]; 4]> = state.stage.surfaces
                    .get(state.stage.selected_surface_index)
                    .and_then(|s| match &s.warp {
                        rustjay_projection::WarpMode::CornerPin { corners } => Some(*corners),
                        _ => None,
                    });

                let mesh_handle_positions: Option<Vec<[f32; 2]>> = state.stage.surfaces
                    .get(state.stage.selected_surface_index)
                    .and_then(|s| match &s.warp {
                        rustjay_projection::WarpMode::Mesh(mesh) => {
                            Some(mesh.points.iter().map(|p| p.position).collect())
                        }
                        _ => None,
                    });

                // Draw corner-pin handles
                if let Some(corners) = corner_handles {
                    for (i, corner) in corners.iter().enumerate() {
                        let pos = Pos2::new(
                            canvas_rect.min.x + corner[0] * canvas_rect.width(),
                            canvas_rect.min.y + corner[1] * canvas_rect.height(),
                        );
                        let handle_rect = Rect::from_center_size(pos, Vec2::splat(12.0));
                        let handle_id = ui.id().with(("warp_handle", i));
                        let handle_response = ui.interact(handle_rect, handle_id, egui::Sense::drag());

                        let handle_color = if handle_response.dragged() {
                            Color32::YELLOW
                        } else if handle_response.hovered() {
                            Color32::WHITE
                        } else {
                            Color32::from_rgb(200, 200, 200)
                        };
                        painter.circle_filled(pos, 5.0, handle_color);

                        if handle_response.dragged() {
                            handle_dragged = true;
                            let dx = handle_response.drag_delta().x / canvas_rect.width();
                            let dy = handle_response.drag_delta().y / canvas_rect.height();
                            if let Some(surf) = state.stage.surfaces.get_mut(state.stage.selected_surface_index) {
                                if let rustjay_projection::WarpMode::CornerPin { corners } = &mut surf.warp {
                                    corners[i][0] = (corners[i][0] + dx).clamp(0.0, 1.0);
                                    corners[i][1] = (corners[i][1] + dy).clamp(0.0, 1.0);
                                }
                            }
                            warp_dirty = true;
                        }
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

                // Surface selection via canvas click (skip if a handle is being dragged)
                if !handle_dragged && response.clicked_by(egui::PointerButton::Primary) {
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

                ui.allocate_rect(canvas_rect, egui::Sense::hover());

                ui.separator();

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

            });

            // Push warp edits to the projector's live warp stage (version-bumped,
            // so the projector re-applies only on an actual change).
            if warp_dirty {
                state.stage.publish_warp();
                state.save_workspace();
            }
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // GeometryTab
    // ─────────────────────────────────────────────────────────────────────────

    impl AnyEguiTab for GeometryTab {
        fn name(&self) -> &str {
            "Geometry"
        }
        fn draw(
            &mut self,
            ui: &mut egui::Ui,
            _app_state: &mut dyn std::any::Any,
            _engine: &mut EngineState,
        ) {
            ui.heading("Geometry");
            ui.separator();

            #[cfg(feature = "projection")]
            {
                let state = _app_state
                    .downcast_mut::<VardaAppState>()
                    .expect("GeometryTab expects VardaAppState");
                draw_surface_properties(ui, state);
            }

            #[cfg(not(feature = "projection"))]
            {
                ui.label("Projection feature is not enabled. Enable it to use surface geometry.");
            }
        }
    }

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
                            if ui
                                .add(egui::DragValue::new(&mut surf_idx).speed(1).range(-1..=surf_count.max(1) as i32 - 1))
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
                                });
                            if proj.output_type != prev_type {
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
                    state.stage.projectors.remove(i);
                    state.save_workspace();
                }
                if ui.button("+ Add projector").clicked() {
                    state
                        .stage
                        .projectors
                        .push(crate::stage::VardaProjector::default());
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
            }

            #[cfg(not(feature = "projection"))]
            {
                ui.label("Projection feature not enabled.");
                ui.label("Enable the 'projection' feature for multi-output support.");
            }

            ui.separator();
            ui.label(egui::RichText::new("Recording").strong());
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
