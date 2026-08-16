//! Cue inspector — right-side panel showing details for the selected cue.

use crate::app::SharedStateHandle;
use crate::{colour_to_egui, cue_type_label};
use egui::RichText;
use rust_decimal::Decimal;

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    ui.heading("Inspector");
    ui.separator();

    // Spawn waveform generation for the selected cue at most once per path.
    // The path is marked in-flight BEFORE spawning (via HashSet::insert), and a
    // failed result is cached as an empty sentinel — so a dataless/unreadable file
    // (e.g. an un-downloaded iCloud file) is decoded once, not re-spawned every
    // frame. Without this the inspector leaked a decode thread per frame until the
    // OS thread limit was hit and the app panicked on thread::spawn.
    let waveform_path = {
        let Ok(mut state) = state.lock() else { return };
        let path =
            match state.selected_cue() {
                Some(
                    cuepool_core::Cue::Sound { path, .. } | cuepool_core::Cue::Video { path, .. },
                ) if !path.is_empty() => path.clone(),
                _ => String::new(),
            };
        if !path.is_empty()
            && !state.waveform_cache.contains_key(&path)
            && state.pending_waveforms.insert(path.clone())
        {
            Some(path)
        } else {
            None
        }
    };
    if let Some(path) = waveform_path {
        let state_clone = std::sync::Arc::clone(state);
        std::thread::spawn(move || {
            // unwrap_or_default(): cache an empty Vec on failure so we don't retry.
            let peaks = crate::waveform::generate_peaks(&path, 200).unwrap_or_default();
            if let Ok(mut state) = state_clone.lock() {
                state.pending_waveforms.remove(&path);
                state.waveform_cache.insert(path, peaks);
            }
        });
    }

    let Ok(mut state) = state.lock() else { return };

    // Pre-fetch waveform and active timeline data before taking a mutable cue reference.
    let waveform_data = state.selected_cue().and_then(|cue| {
        let (qid, path, region_start_secs, kind) = match cue {
            cuepool_core::Cue::Sound {
                base,
                path,
                start_time,
                ..
            } => (
                base.qid,
                path.clone(),
                start_time.as_secs_f64() as f32,
                crate::scrub::SeekKind::Sound,
            ),
            cuepool_core::Cue::Video {
                base,
                path,
                start_time,
                ..
            } => (
                base.qid,
                path.clone(),
                start_time.as_secs_f64() as f32,
                crate::scrub::SeekKind::Video,
            ),
            _ => return None,
        };
        let waveform = state.waveform_cache.get(&path).cloned();
        let pending = state.pending_waveforms.contains(&path);
        let active = state
            .active_cues
            .iter()
            .rev()
            .find(|active| active.qid == qid);
        let playhead = active.map(|active| {
            let seek_length_secs = active.length_secs.unwrap_or_default();
            crate::waveform::Playhead {
                position_secs: active.position_secs,
                region_start_secs,
                seek_length_secs,
            }
        });
        let interaction = if state.show_mode == crate::app::ShowMode::Show {
            crate::waveform::Interaction::Disabled
        } else if active
            .and_then(|active| active.length_secs)
            .is_some_and(|length| length > 0.0)
        {
            crate::waveform::Interaction::Scrub(kind)
        } else {
            crate::waveform::Interaction::Pan
        };
        Some((
            active.map_or(0, |active| active.instance_id),
            qid,
            waveform,
            pending,
            interaction,
            playhead,
        ))
    });
    let (mut waveform_zoom, mut waveform_scroll) = (state.waveform_zoom, state.waveform_scroll);

    // Pre-fetch the fixture patch for lighting cues (before the mutable cue borrow).
    let patched_fixtures: Vec<(u32, String)> = state
        .show_file
        .lighting
        .fixtures
        .iter()
        .map(|f| {
            (
                f.id,
                format!("{} (U{} Ch{})", f.name, f.universe, f.address),
            )
        })
        .collect();
    let mut lighting_live = state.lighting_live;
    let tc_fps = state.show_file.show_settings.timecode_fps;

    // ponytail: one whole-state clone per inspector frame so every edit is undoable.
    // For very large show files this could be replaced with per-field snapshots.
    let pre_edit_snapshot = crate::app::Snapshot::from_state(&state).with_merge_key("inspector");

    let Some(cue) = state.selected_cue_mut() else {
        ui.label("Select a cue to edit its properties.");
        return;
    };

    let cue_type = cue_type_label(cue);
    let base = cue.base_mut();
    let mut changed = false;
    let mut pending_commands: Vec<crate::app::AppCommand> = Vec::new();

    ui.horizontal(|ui| {
        let (swatch_rect, swatch_response) =
            ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(swatch_rect, 3.0, colour_to_egui(base.colour));
        swatch_response.on_hover_text("Cue colour tag");
        egui::Frame::new()
            .fill(ui.visuals().widgets.inactive.weak_bg_fill)
            .corner_radius(2.0)
            .inner_margin(egui::Margin::symmetric(4, 2))
            .show(ui, |ui| {
                ui.label(RichText::new(cue_type).monospace().strong().size(10.0));
            });
        ui.add(
            egui::Label::new(
                RichText::new(format!("Q{} · {}", base.qid, base.name))
                    .strong()
                    .size(18.0),
            )
            .truncate(),
        )
        .on_hover_text(format!("Q{} · {}", base.qid, base.name));
    });
    ui.add_space(8.0);

    // Common fields
    ui.horizontal(|ui| {
        ui.label("Name:");
        let response = ui.text_edit_singleline(&mut base.name);
        changed |= response.changed();
    });
    ui.horizontal(|ui| {
        ui.label("QID:");
        changed |= qid_edit(ui, "inspector_qid", &mut base.qid);
    });
    ui.horizontal(|ui| {
        let mut enabled = base.enabled;
        let response = ui.checkbox(&mut enabled, "Enabled");
        if response.changed() {
            base.enabled = enabled;
            changed = true;
        }
        let mut rt = base.retriggerable;
        let rt_resp = ui
            .checkbox(&mut rt, "Re-triggerable")
            .on_hover_text("If off, firing this cue again while it is still playing is ignored");
        if rt_resp.changed() {
            base.retriggerable = rt;
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Colour:");
        let mut col = egui::Color32::from_rgba_premultiplied(
            (base.colour.r * 255.0) as u8,
            (base.colour.g * 255.0) as u8,
            (base.colour.b * 255.0) as u8,
            (base.colour.a * 255.0) as u8,
        );
        if ui.color_edit_button_srgba(&mut col).changed() {
            base.colour.r = col.r() as f32 / 255.0;
            base.colour.g = col.g() as f32 / 255.0;
            base.colour.b = col.b() as f32 / 255.0;
            base.colour.a = col.a() as f32 / 255.0;
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Trigger:");
        egui::ComboBox::from_id_salt("trigger_mode")
            .selected_text(format!("{:?}", base.trigger))
            .show_ui(ui, |ui| {
                for variant in [
                    cuepool_core::TriggerMode::Go,
                    cuepool_core::TriggerMode::WithLast,
                    cuepool_core::TriggerMode::AfterLast,
                ] {
                    if ui
                        .selectable_value(&mut base.trigger, variant, format!("{:?}", variant))
                        .clicked()
                    {
                        changed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Delay (s):");
        let mut delay_secs = base.delay.as_secs_f64();
        let response = ui.add(
            egui::DragValue::new(&mut delay_secs)
                .speed(0.1)
                .range(0.0..=60.0),
        );
        if response.changed() {
            base.delay = cuepool_core::Timespan::from_secs_f64(delay_secs);
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Remote Node:");
        let response = ui.text_edit_singleline(&mut base.remote_node);
        changed |= response.changed();
    });
    ui.horizontal(|ui| {
        ui.label("Description:");
        let response = ui.text_edit_multiline(&mut base.description);
        changed |= response.changed();
    });
    ui.horizontal(|ui| {
        ui.label("Loop:");
        egui::ComboBox::from_id_salt("loop_mode")
            .selected_text(format!("{:?}", base.loop_mode))
            .show_ui(ui, |ui| {
                for variant in [
                    cuepool_core::LoopMode::OneShot,
                    cuepool_core::LoopMode::Looped,
                    cuepool_core::LoopMode::LoopedInfinite,
                    cuepool_core::LoopMode::HoldLast,
                ] {
                    if ui
                        .selectable_value(&mut base.loop_mode, variant, format!("{:?}", variant))
                        .clicked()
                    {
                        changed = true;
                    }
                }
            });
    });
    if base.loop_mode == cuepool_core::LoopMode::Looped {
        ui.horizontal(|ui| {
            ui.label("Loop Count:");
            let response = ui.add(
                egui::DragValue::new(&mut base.loop_count)
                    .speed(1)
                    .range(1..=999),
            );
            if response.changed() {
                changed = true;
            }
        });
    }

    ui.separator();

    match cue {
        cuepool_core::Cue::Sound {
            base,
            path,
            start_time: _,
            duration: _,
            volume,
            pan,
            fade_in,
            fade_out,
            fade_type,
            eq,
            routing,
        } => {
            ui.label(RichText::new("Sound Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked()
                    && let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("Audio", &["wav", "mp3", "flac", "ogg", "aiff", "wma"])
                        .pick_file()
                {
                    *path = new_path.to_string_lossy().to_string();
                    changed = true;
                }
            });
            if let Some((instance_id, _qid, Some(ref waveform), _, interaction, playhead)) =
                waveform_data
            {
                let response = crate::waveform::draw(
                    ui,
                    waveform,
                    waveform_zoom,
                    waveform_scroll,
                    48.0,
                    interaction,
                    playhead,
                );
                waveform_zoom = response.zoom;
                waveform_scroll = response.scroll_offset;
                if let Some(secs) = response.seek_target {
                    pending_commands.push(crate::app::AppCommand::SeekCue { instance_id, secs });
                }
            } else if let Some((_, _, None, true, _, _)) = waveform_data {
                ui.label(
                    egui::RichText::new("Generating waveform…")
                        .italics()
                        .color(egui::Color32::GRAY),
                );
            }
            ui.horizontal(|ui| {
                ui.label("Volume (dB):");
                let mut db = 20.0 * volume.log10();
                let response = ui.add(egui::Slider::new(&mut db, -60.0..=12.0));
                if response.changed() {
                    *volume = 10.0f32.powf(db / 20.0);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Pan:");
                let response = ui.add(egui::Slider::new(pan, -1.0..=1.0));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade In (s):");
                let response = ui.add(egui::DragValue::new(fade_in).speed(0.1));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Out (s):");
                let response = ui.add(egui::DragValue::new(fade_out).speed(0.1));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Type:");
                egui::ComboBox::from_id_salt("fade_type")
                    .selected_text(format!("{:?}", fade_type))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::FadeType::Linear,
                            cuepool_core::FadeType::SCurve,
                            cuepool_core::FadeType::Square,
                            cuepool_core::FadeType::InverseSquare,
                        ] {
                            if ui
                                .selectable_value(fade_type, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            eq_editor(ui, eq, &mut changed);
            routing_editor(ui, routing, &mut changed);
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Video {
            base,
            path,
            start_time: _,
            duration: _,
            volume,
            pan,
            fade_in,
            fade_out,
            fade_type,
            eq,
            routing,
            follow_mtc,
            mtc_start,
        } => {
            ui.label(RichText::new("Video Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked()
                    && let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("Video", &["mp4", "mov", "mkv", "avi"])
                        .pick_file()
                {
                    *path = new_path.to_string_lossy().to_string();
                    changed = true;
                }
            });
            if let Some((instance_id, _qid, Some(ref waveform), _, interaction, playhead)) =
                waveform_data
            {
                let response = crate::waveform::draw(
                    ui,
                    waveform,
                    waveform_zoom,
                    waveform_scroll,
                    48.0,
                    interaction,
                    playhead,
                );
                waveform_zoom = response.zoom;
                waveform_scroll = response.scroll_offset;
                if let Some(secs) = response.seek_target {
                    pending_commands.push(crate::app::AppCommand::SeekCue { instance_id, secs });
                }
            } else if let Some((_, _, None, true, _, _)) = waveform_data {
                ui.label(
                    egui::RichText::new("Generating waveform…")
                        .italics()
                        .color(egui::Color32::GRAY),
                );
            }
            ui.horizontal(|ui| {
                ui.label("Volume (dB):");
                let mut db = 20.0 * volume.log10();
                let response = ui.add(egui::Slider::new(&mut db, -60.0..=12.0));
                if response.changed() {
                    *volume = 10.0f32.powf(db / 20.0);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Pan:");
                let response = ui.add(egui::Slider::new(pan, -1.0..=1.0));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade In (s):");
                let response = ui.add(egui::DragValue::new(fade_in).speed(0.1));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Out (s):");
                let response = ui.add(egui::DragValue::new(fade_out).speed(0.1));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Type:");
                egui::ComboBox::from_id_salt("fade_type_vid")
                    .selected_text(format!("{:?}", fade_type))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::FadeType::Linear,
                            cuepool_core::FadeType::SCurve,
                            cuepool_core::FadeType::Square,
                            cuepool_core::FadeType::InverseSquare,
                        ] {
                            if ui
                                .selectable_value(fade_type, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                changed |= ui
                    .checkbox(follow_mtc, "Follow MTC")
                    .on_hover_text(
                        "Play this video under MIDI Timecode (e.g. Pro Tools over RTP-MIDI): \
                         silent playback, holds on frame 0 until MTC plays, seeks on locate",
                    )
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(*follow_mtc, |ui| {
                    ui.label("MTC start:");
                    let mut secs = mtc_start.as_secs_f64();
                    if timecode_edit(ui, "mtc_start", &mut secs, tc_fps) {
                        *mtc_start = cuepool_core::Timespan::from_secs_f64(secs);
                        changed = true;
                    }
                });
            });
            eq_editor(ui, eq, &mut changed);
            routing_editor(ui, routing, &mut changed);
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Group { base } => {
            ui.label(RichText::new("Group Cue").monospace().size(12.0));
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Stop {
            base,
            stop_qid,
            stop_mode,
            fade_out_time,
            fade_type,
            stop_all,
        } => {
            ui.label(RichText::new("Stop Cue").monospace().size(12.0));
            if ui
                .checkbox(stop_all, "Stop All (like transport Stop)")
                .changed()
            {
                changed = true;
            }
            ui.horizontal(|ui| {
                ui.label("Stops Q#:");
                changed |= qid_edit(ui, "stop_qid", stop_qid);
                if *stop_all {
                    // Target is ignored in Stop All mode.
                    ui.label(RichText::new("(ignored)").weak());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Stop Mode:");
                egui::ComboBox::from_id_salt("stop_mode")
                    .selected_text(format!("{:?}", stop_mode))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::StopMode::Immediate,
                            cuepool_core::StopMode::LoopEnd,
                        ] {
                            if ui
                                .selectable_value(stop_mode, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Fade Out (s):");
                let response = ui.add(egui::DragValue::new(fade_out_time).speed(0.1));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Type:");
                egui::ComboBox::from_id_salt("stop_fade_type")
                    .selected_text(format!("{:?}", fade_type))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::FadeType::Linear,
                            cuepool_core::FadeType::SCurve,
                            cuepool_core::FadeType::Square,
                            cuepool_core::FadeType::InverseSquare,
                        ] {
                            if ui
                                .selectable_value(fade_type, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Volume {
            base,
            sound_qid,
            volume,
            fade_time,
            fade_type,
        } => {
            ui.label(RichText::new("Volume Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Target Q#:");
                changed |= qid_edit(ui, "volume_qid", sound_qid);
            });
            ui.horizontal(|ui| {
                ui.label("Target dB:");
                let mut db = 20.0 * volume.log10();
                let response = ui.add(egui::Slider::new(&mut db, -60.0..=12.0));
                if response.changed() {
                    *volume = 10.0f32.powf(db / 20.0);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Fade Time (s):");
                let response = ui.add(egui::DragValue::new(fade_time).speed(0.1));
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Type:");
                egui::ComboBox::from_id_salt("volume_fade_type")
                    .selected_text(format!("{:?}", fade_type))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::FadeType::Linear,
                            cuepool_core::FadeType::SCurve,
                            cuepool_core::FadeType::Square,
                            cuepool_core::FadeType::InverseSquare,
                        ] {
                            if ui
                                .selectable_value(fade_type, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Dummy { base } => {
            ui.label(RichText::new("Dummy Cue").monospace().size(12.0));
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::TimeCode {
            base,
            start_time,
            duration,
        } => {
            ui.label(RichText::new("TimeCode Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Start (s):");
                let mut secs = start_time.as_secs_f64();
                let response = ui.add(egui::DragValue::new(&mut secs).speed(0.1));
                if response.changed() {
                    *start_time = cuepool_core::Timespan::from_secs_f64(secs);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Duration (s):");
                let mut secs = duration.as_secs_f64();
                let response = ui.add(egui::DragValue::new(&mut secs).speed(0.1));
                if response.changed() {
                    *duration = cuepool_core::Timespan::from_secs_f64(secs);
                    changed = true;
                }
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Osc { base, command } => {
            ui.label(RichText::new("OSC Cue").monospace().size(12.0));
            ui.label("Command format: /address,arg1,arg2,…");
            ui.label("Raw UDP: udp:payload > default target · udp:name:payload or udp:IP:payload > named target (Project Settings)");
            ui.horizontal(|ui| {
                ui.label("Command:");
                let response = ui.text_edit_singleline(command);
                changed |= response.changed();
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Text {
            base,
            text,
            font_size,
            font_colour,
            fit,
            font,
        } => {
            ui.label(RichText::new("Text Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Text:");
                let response = ui.text_edit_multiline(text);
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Font Size:");
                let response = ui.add(
                    egui::DragValue::new(font_size)
                        .speed(1.0)
                        .range(1.0..=512.0),
                );
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Font:");
                let shown = if font.is_empty() {
                    "(built-in)".to_string()
                } else {
                    std::path::Path::new(font.as_str())
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_else(|| font.clone())
                };
                ui.label(shown).on_hover_text(font.as_str());
                if ui.button("Browse…").clicked()
                    && let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("Font", &["ttf", "otf", "ttc"])
                        .pick_file()
                {
                    *font = new_path.to_string_lossy().to_string();
                    // Register now so the family is live before the cue fires
                    // (egui applies added fonts at the next frame).
                    if let Ok(bytes) = std::fs::read(&new_path) {
                        ui.ctx().add_font(egui::epaint::text::FontInsert::new(
                            font,
                            egui::FontData::from_owned(bytes),
                            vec![egui::epaint::text::InsertFontFamily {
                                family: egui::FontFamily::Name(font.clone().into()),
                                priority: egui::epaint::text::FontPriority::Highest,
                            }],
                        ));
                    }
                    changed = true;
                }
                if !font.is_empty()
                    && ui
                        .small_button("✕")
                        .on_hover_text("Use built-in font")
                        .clicked()
                {
                    font.clear();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Colour:");
                let mut col = egui::Color32::from_rgba_premultiplied(
                    (font_colour.r * 255.0) as u8,
                    (font_colour.g * 255.0) as u8,
                    (font_colour.b * 255.0) as u8,
                    (font_colour.a * 255.0) as u8,
                );
                if ui.color_edit_button_srgba(&mut col).changed() {
                    font_colour.r = col.r() as f32 / 255.0;
                    font_colour.g = col.g() as f32 / 255.0;
                    font_colour.b = col.b() as f32 / 255.0;
                    font_colour.a = col.a() as f32 / 255.0;
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Fit:");
                egui::ComboBox::from_id_salt("text_fit")
                    .selected_text(format!("{:?}", fit))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::CanvasFit::Stretch,
                            cuepool_core::CanvasFit::Fit,
                            cuepool_core::CanvasFit::Fill,
                        ] {
                            if ui
                                .selectable_value(fit, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Image { base, path, fit } => {
            ui.label(RichText::new("Image Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked()
                    && let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("Image", &["png", "jpg", "jpeg", "bmp", "tiff", "gif"])
                        .pick_file()
                {
                    *path = new_path.to_string_lossy().to_string();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Fit:");
                egui::ComboBox::from_id_salt("image_fit")
                    .selected_text(format!("{:?}", fit))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::CanvasFit::Stretch,
                            cuepool_core::CanvasFit::Fit,
                            cuepool_core::CanvasFit::Fill,
                        ] {
                            if ui
                                .selectable_value(fit, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Goto { base, target_qid } => {
            ui.label(RichText::new("Goto Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Target Q#:");
                changed |= qid_edit(ui, "goto_target", target_qid);
            });
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::PixelMap { base, path } => {
            ui.label(RichText::new("Pixel Map Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked()
                    && let Some(new_path) = rfd::FileDialog::new()
                        .add_filter(
                            "Media",
                            &["mp4", "mov", "mkv", "avi", "webm", "png", "jpg", "jpeg"],
                        )
                        .pick_file()
                {
                    *path = new_path.to_string_lossy().to_string();
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "Plays into the pixel-map texture (Window > Lighting > Pixel Map, source: PixelMap).",
                )
                .small()
                .weak(),
            );
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::DmxShow {
            base,
            path,
            fade_in,
            fade_out,
            fade_type,
            priority,
        } => {
            ui.label(RichText::new("DMX Show Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked()
                    && let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("DMX recording", &["dmxrec"])
                        .pick_file()
                {
                    *path = new_path.to_string_lossy().to_string();
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Fade In (s):");
                changed |= ui
                    .add(egui::DragValue::new(fade_in).speed(0.1).range(0.0..=600.0))
                    .changed();
                ui.label("Fade Out (s):");
                changed |= ui
                    .add(egui::DragValue::new(fade_out).speed(0.1).range(0.0..=600.0))
                    .changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Type:");
                egui::ComboBox::from_id_salt("dmxshow_fade_type")
                    .selected_text(format!("{:?}", fade_type))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::FadeType::Linear,
                            cuepool_core::FadeType::SCurve,
                            cuepool_core::FadeType::Square,
                            cuepool_core::FadeType::InverseSquare,
                        ] {
                            if ui
                                .selectable_value(fade_type, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
                ui.label("Priority:").on_hover_text(
                    "sACN-style merge priority vs the look engine and other shows (default 100)",
                );
                changed |= ui
                    .add(egui::DragValue::new(priority).speed(1).range(0..=255))
                    .changed();
            });
            ui.label(
                egui::RichText::new(
                    "Plays a recorded DMX show (.dmxrec) to the lighting output. Loop follows the cue's loop mode; HoldLast holds the final frame until stopped.",
                )
                .small()
                .weak(),
            );
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
        }
        cuepool_core::Cue::Lighting {
            base,
            snapshot,
            fade_time,
            fade_type,
        } => {
            let was_live = lighting_live;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Lighting Cue").monospace().size(12.0));
                ui.checkbox(&mut lighting_live, "🔴 Live").on_hover_text(
                    "Stream look edits straight to the fixtures (DMX) while programming",
                );
            });
            ui.horizontal(|ui| {
                ui.label("Fade (s):");
                let response = ui.add(
                    egui::DragValue::new(fade_time)
                        .speed(0.1)
                        .range(0.0..=600.0),
                );
                changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Fade Type:");
                egui::ComboBox::from_id_salt("lx_fade_type")
                    .selected_text(format!("{:?}", fade_type))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::FadeType::Linear,
                            cuepool_core::FadeType::SCurve,
                            cuepool_core::FadeType::Square,
                            cuepool_core::FadeType::InverseSquare,
                        ] {
                            if ui
                                .selectable_value(fade_type, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                changed = true;
                            }
                        }
                    });
            });
            ui.add_space(4.0);
            if patched_fixtures.is_empty() {
                ui.label(
                    egui::RichText::new("No fixtures patched — see Window > Lighting.")
                        .italics()
                        .weak(),
                );
            }
            // One row per patched fixture: include-in-cue checkbox + look editor.
            for (id, label) in &patched_fixtures {
                let mut included = snapshot.contains_key(id);
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut included, label.as_str()).changed() {
                        if included {
                            snapshot.insert(*id, Default::default());
                        } else {
                            snapshot.remove(id);
                        }
                        changed = true;
                    }
                });
                if let Some(look) = snapshot.get_mut(id) {
                    ui.indent(("lx_look", id), |ui| {
                        changed |= look_editor(ui, *id, look);
                    });
                }
            }
            // Snapshot entries for fixtures no longer in the patch.
            let orphans: Vec<u32> = snapshot
                .keys()
                .filter(|id| !patched_fixtures.iter().any(|(pid, _)| pid == *id))
                .copied()
                .collect();
            for id in orphans {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 160, 60),
                        format!("⚠ Fixture {id} not in patch"),
                    );
                    if ui.small_button("Remove").clicked() {
                        snapshot.remove(&id);
                        changed = true;
                    }
                });
            }
            triggers_editor(
                ui,
                &mut base.triggers,
                base.qid,
                tc_fps,
                &mut changed,
                &mut pending_commands,
            );
            // Live mode: push the cue's looks on any edit, and once on toggle-on
            // so the stage snaps to the cue being programmed.
            if lighting_live && (changed || !was_live) {
                pending_commands.push(crate::app::AppCommand::LightingLivePush {
                    snapshot: snapshot.clone(),
                });
            }
        }
    }

    if changed {
        state.dirty = true;
        state.undo_redo.push(pre_edit_snapshot);
    }
    state.lighting_live = lighting_live;

    // Write back waveform zoom/scroll (separate borrow to avoid conflict with cue editing)
    state.waveform_zoom = waveform_zoom;
    state.waveform_scroll = waveform_scroll;

    state.command_queue.extend(pending_commands);
}

/// Per-fixture look editor for lighting cues. Returns true if anything changed.
fn look_editor(ui: &mut egui::Ui, id: u32, look: &mut cuepool_core::FixtureLook) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("Dimmer:");
        changed |= ui
            .add(egui::Slider::new(&mut look.dimmer, 0.0..=1.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Color:");
        changed |= ui.color_edit_button_rgb(&mut look.color).changed();
        ui.label("White:");
        changed |= ui
            .add(
                egui::DragValue::new(&mut look.white)
                    .speed(0.01)
                    .range(0.0..=1.0),
            )
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Pan:");
        changed |= ui
            .add(egui::Slider::new(&mut look.pan, 0.0..=1.0))
            .changed();
    });
    ui.horizontal(|ui| {
        ui.label("Tilt:");
        changed |= ui
            .add(egui::Slider::new(&mut look.tilt, 0.0..=1.0))
            .changed();
    });
    egui::CollapsingHeader::new("Beam")
        .id_salt(("lx_beam", id))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Zoom:");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut look.zoom)
                            .speed(0.01)
                            .range(0.0..=1.0),
                    )
                    .changed();
                ui.label("Strobe:");
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut look.strobe)
                            .speed(0.01)
                            .range(0.0..=1.0),
                    )
                    .changed();
                ui.label("Gobo:");
                changed |= ui
                    .add(egui::DragValue::new(&mut look.gobo).speed(1).range(0..=255))
                    .changed();
            });
        });
    changed
}

/// Text field for editing a Decimal QID (target/stop references).
///
/// egui rewrites an externally-rebuilt buffer every frame, which wipes
/// in-progress keystrokes before the field loses focus — so the value appears
/// stuck. Stash the live text in egui temp storage and only commit on blur.
/// Returns `true` if the value changed.
fn qid_edit(ui: &mut egui::Ui, salt: &str, value: &mut Decimal) -> bool {
    let id = ui.make_persistent_id(salt);
    // Pending edit: (in-progress text, model value when the edit started).
    let pending = ui.ctx().data(|d| d.get_temp::<(String, Decimal)>(id));
    let mut text = pending
        .as_ref()
        .map(|(t, _)| t.clone())
        .unwrap_or_else(|| value.to_string());
    let response = ui.add(egui::TextEdit::singleline(&mut text).id(id));

    // A focused TextEdit only surrenders focus on Enter/Tab/Esc; blur it on a
    // click anywhere else so the edit ends.
    if response.has_focus() && response.clicked_elsewhere() {
        ui.memory_mut(|mem| mem.surrender_focus(id));
    }

    if response.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(id, (text, *value)));
        return false;
    }

    // Not focused: commit any pending edit. lost_focus() can't be used — when
    // another text field steals focus later in the same frame, this widget
    // never observes the transition.
    let Some((_, started_from)) = pending else {
        return false;
    };
    ui.ctx()
        .data_mut(|d| d.remove_temp::<(String, Decimal)>(id));
    let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // If the field was rebound mid-edit (selection changed), drop the edit
    // rather than commit one cue's text into another.
    if cancelled || started_from != *value {
        return false;
    }
    if let Ok(new) = text.parse::<Decimal>()
        && new != *value
    {
        *value = new;
        return true;
    }
    false
}

/// Text field for editing a time in `HH:MM:SS.FF` timecode (matching the show
/// clock display; also accepts plain seconds). Same commit-on-blur scheme as
/// [`qid_edit`] — see there for why the live text lives in egui temp storage.
/// Returns `true` if the value changed.
pub(crate) fn timecode_edit(ui: &mut egui::Ui, salt: &str, value: &mut f64, fps: f32) -> bool {
    let id = ui.make_persistent_id(salt);
    // Pending edit: (in-progress text, model value when the edit started).
    let pending = ui.ctx().data(|d| d.get_temp::<(String, f64)>(id));
    let mut text = pending
        .as_ref()
        .map(|(t, _)| t.clone())
        .unwrap_or_else(|| crate::transport::format_timecode(*value, fps));
    let response = ui
        .add(
            egui::TextEdit::singleline(&mut text)
                .id(id)
                .desired_width(100.0),
        )
        .on_hover_text(format!("HH:MM:SS.FF at {fps} fps, or plain seconds"));

    if response.has_focus() && response.clicked_elsewhere() {
        ui.memory_mut(|mem| mem.surrender_focus(id));
    }

    if response.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(id, (text, *value)));
        return false;
    }

    let Some((_, started_from)) = pending else {
        return false;
    };
    ui.ctx().data_mut(|d| d.remove_temp::<(String, f64)>(id));
    let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
    // If the field was rebound mid-edit (selection changed), drop the edit
    // rather than commit one cue's text into another.
    if cancelled || started_from != *value {
        return false;
    }
    if let Some(new) = crate::transport::parse_timecode(&text, fps)
        && new != *value
    {
        *value = new;
        return true;
    }
    false
}

/// Trigger editor — hotkey, MIDI, wall-clock and timecode firing methods.
fn triggers_editor(
    ui: &mut egui::Ui,
    triggers: &mut cuepool_core::CueTriggers,
    qid: Decimal,
    tc_fps: f32,
    changed: &mut bool,
    pending: &mut Vec<crate::app::AppCommand>,
) {
    ui.separator();
    ui.label(RichText::new("Triggers").strong().size(12.0));

    // Hotkey
    {
        let mut enabled = triggers.hotkey.is_some();
        if ui.checkbox(&mut enabled, "Hotkey").changed() {
            *changed = true;
            triggers.hotkey = if enabled {
                Some(cuepool_core::HotkeyTrigger { key: String::new() })
            } else {
                None
            };
        }
        if let Some(ref mut hotkey) = triggers.hotkey {
            ui.horizontal(|ui| {
                ui.label("Key:");
                let response = ui.text_edit_singleline(&mut hotkey.key);
                *changed |= response.changed();
            });
        }
    }

    // MIDI
    {
        let mut enabled = triggers.midi.is_some();
        if ui.checkbox(&mut enabled, "MIDI").changed() {
            *changed = true;
            triggers.midi = if enabled {
                Some(cuepool_core::MidiTrigger {
                    channel: 1,
                    kind: cuepool_core::MidiTriggerKind::NoteOn,
                    note_or_cc: 60,
                    velocity_min: 1,
                })
            } else {
                None
            };
        }
        if let Some(ref mut midi) = triggers.midi {
            ui.horizontal(|ui| {
                ui.label("Kind:");
                egui::ComboBox::from_id_salt("midi_kind")
                    .selected_text(format!("{:?}", midi.kind))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::MidiTriggerKind::NoteOn,
                            cuepool_core::MidiTriggerKind::NoteOff,
                            cuepool_core::MidiTriggerKind::CC,
                        ] {
                            if ui
                                .selectable_value(&mut midi.kind, variant, format!("{:?}", variant))
                                .clicked()
                            {
                                *changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Ch:");
                let mut ch = midi.channel as i32;
                let response = ui.add(egui::DragValue::new(&mut ch).range(1..=16));
                if response.changed() {
                    midi.channel = ch.clamp(1, 16) as u8;
                    *changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Note/CC:");
                let mut v = midi.note_or_cc as i32;
                let response = ui.add(egui::DragValue::new(&mut v).range(0..=127));
                if response.changed() {
                    midi.note_or_cc = v.clamp(0, 127) as u8;
                    *changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Vel ≥:");
                let mut v = midi.velocity_min as i32;
                let response = ui.add(egui::DragValue::new(&mut v).range(0..=127));
                if response.changed() {
                    midi.velocity_min = v.clamp(0, 127) as u8;
                    *changed = true;
                }
            });
            if ui.button("Capture next MIDI").clicked() {
                pending.push(crate::app::AppCommand::LearnMidiTrigger { qid });
            }
        }
    }

    // Wall Clock
    {
        let mut enabled = triggers.wall_clock.is_some();
        if ui.checkbox(&mut enabled, "Wall Clock").changed() {
            *changed = true;
            triggers.wall_clock = if enabled {
                Some(cuepool_core::WallClockTrigger {
                    time: "00:00:00".into(),
                    mode: cuepool_core::ClockMode::TwentyFourHour,
                    repeat: cuepool_core::RepeatMode::Daily,
                })
            } else {
                None
            };
        }
        if let Some(ref mut clock) = triggers.wall_clock {
            ui.horizontal(|ui| {
                ui.label("Time:");
                let response = ui.text_edit_singleline(&mut clock.time);
                *changed |= response.changed();
            });
            ui.horizontal(|ui| {
                ui.label("Mode:");
                egui::ComboBox::from_id_salt("clock_mode")
                    .selected_text(format!("{:?}", clock.mode))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::ClockMode::TwelveHour,
                            cuepool_core::ClockMode::TwentyFourHour,
                        ] {
                            if ui
                                .selectable_value(
                                    &mut clock.mode,
                                    variant,
                                    format!("{:?}", variant),
                                )
                                .clicked()
                            {
                                *changed = true;
                            }
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Repeat:");
                egui::ComboBox::from_id_salt("clock_repeat")
                    .selected_text(format!("{:?}", clock.repeat))
                    .show_ui(ui, |ui| {
                        for variant in [
                            cuepool_core::RepeatMode::Once,
                            cuepool_core::RepeatMode::Daily,
                        ] {
                            if ui
                                .selectable_value(
                                    &mut clock.repeat,
                                    variant,
                                    format!("{:?}", variant),
                                )
                                .clicked()
                            {
                                *changed = true;
                            }
                        }
                    });
            });
        }
    }

    // Timecode
    {
        let mut enabled = triggers.timecode.is_some();
        if ui.checkbox(&mut enabled, "Timecode").changed() {
            *changed = true;
            triggers.timecode = if enabled {
                Some(cuepool_core::TimecodeTrigger {
                    time: cuepool_core::Timespan::ZERO,
                })
            } else {
                None
            };
        }
        if let Some(ref mut tc) = triggers.timecode {
            ui.horizontal(|ui| {
                ui.label("Time:");
                let mut secs = tc.time.as_secs_f64();
                if timecode_edit(ui, "timecode_trigger_time", &mut secs, tc_fps) {
                    tc.time = cuepool_core::Timespan::from_secs_f64(secs);
                    *changed = true;
                }
            });
            if ui.button("Capture now").clicked() {
                pending.push(crate::app::AppCommand::CaptureTimecodeTrigger { qid });
            }
        }
    }
}

/// Lightweight per-cue output routing: destination pair + send fader.
fn routing_editor(ui: &mut egui::Ui, routing: &mut cuepool_core::AudioRouting, changed: &mut bool) {
    ui.separator();
    ui.label(egui::RichText::new("Output").strong().size(12.0));
    const PAIRS: [&str; 4] = ["1-2", "3-4", "5-6", "7-8"];
    ui.horizontal(|ui| {
        ui.label("Pair:");
        let cur = (routing.out_pair as usize).min(PAIRS.len() - 1);
        egui::ComboBox::from_id_salt("out_pair")
            .selected_text(PAIRS[cur])
            .show_ui(ui, |ui| {
                for (i, label) in PAIRS.iter().enumerate() {
                    if ui
                        .selectable_value(&mut routing.out_pair, i as u8, *label)
                        .clicked()
                    {
                        *changed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Send (dB):");
        let mut db = if routing.send > 0.0 {
            20.0 * routing.send.log10()
        } else {
            -60.0
        };
        let response = ui.add(egui::Slider::new(&mut db, -60.0..=6.0));
        if response.changed() {
            routing.send = if db <= -60.0 {
                0.0
            } else {
                10.0f32.powf(db / 20.0)
            };
            *changed = true;
        }
    });

    // Crosspoint matrix: when non-empty, overrides the pair/send route above.
    // Each row maps one source channel -> one output channel at a gain — this is
    // how a multichannel source (e.g. 5.1) routes its tracks to chosen outputs.
    if routing.crosspoints.is_empty() {
        if ui.button("+ Matrix routing (per-channel)").clicked() {
            routing
                .crosspoints
                .push(cuepool_core::Crosspoint::default());
            *changed = true;
        }
    } else {
        ui.label(
            egui::RichText::new("Matrix (overrides pair):")
                .italics()
                .size(11.0),
        );
        let mut remove: Option<usize> = None;
        for (i, cp) in routing.crosspoints.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                // Display channels 1-based; store 0-based.
                ui.label("in");
                let mut in_disp = cp.in_ch as i32 + 1;
                if ui
                    .add(egui::DragValue::new(&mut in_disp).range(1..=32))
                    .changed()
                {
                    cp.in_ch = (in_disp - 1).clamp(0, 31) as u8;
                    *changed = true;
                }
                ui.label("> out");
                let mut out_disp = cp.out_ch as i32 + 1;
                if ui
                    .add(egui::DragValue::new(&mut out_disp).range(1..=8))
                    .changed()
                {
                    cp.out_ch = (out_disp - 1).clamp(0, 7) as u8;
                    *changed = true;
                }
                let mut db = if cp.gain > 0.0 {
                    20.0 * cp.gain.log10()
                } else {
                    -60.0
                };
                if ui
                    .add(
                        egui::DragValue::new(&mut db)
                            .speed(0.5)
                            .range(-60.0..=6.0)
                            .suffix(" dB"),
                    )
                    .changed()
                {
                    cp.gain = if db <= -60.0 {
                        0.0
                    } else {
                        10.0f32.powf(db / 20.0)
                    };
                    *changed = true;
                }
                if ui.small_button("✕").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            routing.crosspoints.remove(i);
            *changed = true;
        }
        if ui.button("+ Add crosspoint").clicked() {
            routing
                .crosspoints
                .push(cuepool_core::Crosspoint::default());
            *changed = true;
        }
    }
}

fn eq_editor(ui: &mut egui::Ui, eq: &mut Option<cuepool_core::EQSettings>, changed: &mut bool) {
    ui.separator();
    ui.label(egui::RichText::new("EQ").strong().size(12.0));

    let mut enabled = eq.is_some();
    if ui.checkbox(&mut enabled, "Enabled").changed() {
        *changed = true;
        if enabled {
            // Some == EQ on. The inner `enabled` flag must agree, else the audio
            // EqProcessor (which reads it) builds no filters and EQ is silent.
            *eq = Some(cuepool_core::EQSettings {
                enabled: true,
                ..Default::default()
            });
        } else {
            *eq = None;
        }
    }

    let Some(eq) = eq else { return };

    ui.horizontal(|ui| {
        ui.label("HPF:");
        let mut hpf_freq = eq.hpf.frequency;
        let response = ui.add(
            egui::DragValue::new(&mut hpf_freq)
                .speed(1.0)
                .range(20.0..=20000.0)
                .suffix(" Hz"),
        );
        if response.changed() {
            eq.hpf.frequency = hpf_freq;
            *changed = true;
        }
        egui::ComboBox::from_id_salt("hpf_order")
            .width(80.0)
            .selected_text(format!("{:?}", eq.hpf.order))
            .show_ui(ui, |ui| {
                for variant in [
                    cuepool_core::EQFilterOrder::Disabled,
                    cuepool_core::EQFilterOrder::_12dBOct,
                    cuepool_core::EQFilterOrder::_24dBOct,
                ] {
                    if ui
                        .selectable_value(&mut eq.hpf.order, variant, format!("{:?}", variant))
                        .clicked()
                    {
                        *changed = true;
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("LPF:");
        let mut lpf_freq = eq.lpf.frequency;
        let response = ui.add(
            egui::DragValue::new(&mut lpf_freq)
                .speed(1.0)
                .range(20.0..=20000.0)
                .suffix(" Hz"),
        );
        if response.changed() {
            eq.lpf.frequency = lpf_freq;
            *changed = true;
        }
        egui::ComboBox::from_id_salt("lpf_order")
            .width(80.0)
            .selected_text(format!("{:?}", eq.lpf.order))
            .show_ui(ui, |ui| {
                for variant in [
                    cuepool_core::EQFilterOrder::Disabled,
                    cuepool_core::EQFilterOrder::_12dBOct,
                    cuepool_core::EQFilterOrder::_24dBOct,
                ] {
                    if ui
                        .selectable_value(&mut eq.lpf.order, variant, format!("{:?}", variant))
                        .clicked()
                    {
                        *changed = true;
                    }
                }
            });
    });

    let bands = [
        (&mut eq.band1, "Band 1"),
        (&mut eq.band2, "Band 2"),
        (&mut eq.band3, "Band 3"),
        (&mut eq.band4, "Band 4"),
    ];
    for (band, label) in bands {
        ui.horizontal(|ui| {
            ui.label(label);
            egui::ComboBox::from_id_salt(format!("eq_shape_{}", label))
                .width(80.0)
                .selected_text(format!("{:?}", band.shape))
                .show_ui(ui, |ui| {
                    for variant in [
                        cuepool_core::EQBandShape::Bell,
                        cuepool_core::EQBandShape::HighShelf,
                        cuepool_core::EQBandShape::LowShelf,
                        cuepool_core::EQBandShape::Notch,
                        cuepool_core::EQBandShape::LowPass,
                        cuepool_core::EQBandShape::HighPass,
                        cuepool_core::EQBandShape::AllPass,
                    ] {
                        if ui
                            .selectable_value(&mut band.shape, variant, format!("{:?}", variant))
                            .clicked()
                        {
                            *changed = true;
                        }
                    }
                });
            let mut freq = band.freq;
            let response = ui.add(
                egui::DragValue::new(&mut freq)
                    .speed(1.0)
                    .range(20.0..=20000.0)
                    .suffix(" Hz"),
            );
            if response.changed() {
                band.freq = freq;
                *changed = true;
            }
            let mut gain = band.gain;
            let response = ui.add(egui::Slider::new(&mut gain, -18.0..=18.0).text("dB"));
            if response.changed() {
                band.gain = gain;
                *changed = true;
            }
            let mut q = band.q;
            let response = ui.add(egui::DragValue::new(&mut q).speed(0.01).range(0.1..=10.0));
            if response.changed() {
                band.q = q;
                *changed = true;
            }
        });
    }
}
