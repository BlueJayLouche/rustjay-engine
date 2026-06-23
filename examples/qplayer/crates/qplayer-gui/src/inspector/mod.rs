//! Cue inspector — right-side panel showing details for the selected cue.

use crate::app::SharedStateHandle;
use egui::RichText;

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    ui.heading("Inspector");
    ui.separator();

    // Spawn waveform generation for the selected cue if needed
    let waveform_path = {
        let Ok(state) = state.lock() else { return };
        if let Some(cue) = state.selected_cue() {
            let path = match cue {
                qplayer_core::Cue::Sound { path, .. } | qplayer_core::Cue::Video { path, .. } => path.clone(),
                _ => String::new(),
            };
            if !path.is_empty() && !state.waveform_cache.contains_key(&path) && !state.pending_waveforms.contains(&path) {
                Some(path)
            } else {
                None
            }
        } else {
            None
        }
    };
    if let Some(path) = waveform_path {
        let state_clone = std::sync::Arc::clone(state);
        std::thread::spawn(move || {
            if let Some(peaks) = crate::waveform::generate_peaks(&path, 200) {
                if let Ok(mut state) = state_clone.lock() {
                    state.waveform_cache.insert(path.clone(), peaks);
                    state.pending_waveforms.remove(&path);
                }
            } else if let Ok(mut state) = state_clone.lock() {
                state.pending_waveforms.remove(&path);
            }
        });
    }

    let Ok(mut state) = state.lock() else { return };

    // Pre-fetch waveform data and zoom/scroll before taking mutable cue reference
    let waveform_data = if let Some(cue) = state.selected_cue() {
        let path = match cue {
            qplayer_core::Cue::Sound { path, .. } | qplayer_core::Cue::Video { path, .. } => path.clone(),
            _ => String::new(),
        };
        let peaks = state.waveform_cache.get(&path).cloned();
        let pending = state.pending_waveforms.contains(&path);
        Some((peaks, pending))
    } else {
        None
    };
    let (mut waveform_zoom, mut waveform_scroll) = (state.waveform_zoom, state.waveform_scroll);

    let Some(cue) = state.selected_cue_mut() else {
        ui.label("Select a cue to edit its properties.");
        return;
    };

    let base = cue.base_mut();
    let mut changed = false;

    ui.label(RichText::new(format!("Q{}", base.qid)).strong().size(18.0));
    ui.add_space(8.0);

    // Common fields
    ui.horizontal(|ui| {
        ui.label("Name:");
        let response = ui.text_edit_singleline(&mut base.name);
        changed |= response.changed();
    });
    ui.horizontal(|ui| {
        ui.label("QID:");
        // Use a persistent ID so egui can maintain focus and cursor state.
        let id = ui.make_persistent_id("inspector_qid");
        // Read any in-progress edit from egui's temp storage so typing isn't
        // overwritten every frame by base.qid.to_string().
        let mut qid_str = ui.ctx().data(|data| {
            data.get_temp::<String>(id)
        }).unwrap_or_else(|| base.qid.to_string());

        let response = ui.add(
            egui::TextEdit::singleline(&mut qid_str)
                .id(id),
        );

        if response.has_focus() {
            // While editing, store the live text in egui temp data.
            ui.ctx().data_mut(|data| {
                data.insert_temp(id, qid_str.clone());
            });
        } else {
            // Focus lost — clear temp data so next time we start from base.qid.
            ui.ctx().data_mut(|data| {
                data.remove_temp::<String>(id);
            });
        }

        if response.lost_focus() {
            if let Ok(new_qid) = qid_str.parse::<rust_decimal::Decimal>() {
                if new_qid != base.qid {
                    base.qid = new_qid;
                    changed = true;
                }
            }
        }
    });
    ui.horizontal(|ui| {
        let mut enabled = base.enabled;
        let response = ui.checkbox(&mut enabled, "Enabled");
        if response.changed() {
            base.enabled = enabled;
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
                for variant in [qplayer_core::TriggerMode::Go, qplayer_core::TriggerMode::WithLast, qplayer_core::TriggerMode::AfterLast] {
                    if ui.selectable_value(&mut base.trigger, variant, format!("{:?}", variant)).clicked() {
                        changed = true;
                    }
                }
            });
    });
    ui.horizontal(|ui| {
        ui.label("Delay (s):");
        let mut delay_secs = base.delay.as_secs_f64();
        let response = ui.add(egui::DragValue::new(&mut delay_secs).speed(0.1).range(0.0..=60.0));
        if response.changed() {
            base.delay = qplayer_core::Timespan::from_secs_f64(delay_secs);
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        ui.label("Remote Node:");
        let response = ui.text_edit_singleline(&mut base.remote_node);
        changed |= response.changed();
    });
    ui.horizontal(|ui| {
        ui.label("Loop:");
        egui::ComboBox::from_id_salt("loop_mode")
            .selected_text(format!("{:?}", base.loop_mode))
            .show_ui(ui, |ui| {
                for variant in [qplayer_core::LoopMode::OneShot, qplayer_core::LoopMode::Looped, qplayer_core::LoopMode::LoopedInfinite, qplayer_core::LoopMode::HoldLast] {
                    if ui.selectable_value(&mut base.loop_mode, variant, format!("{:?}", variant)).clicked() {
                        changed = true;
                    }
                }
            });
    });
    if base.loop_mode == qplayer_core::LoopMode::Looped {
        ui.horizontal(|ui| {
            ui.label("Loop Count:");
            let response = ui.add(egui::DragValue::new(&mut base.loop_count).speed(1).range(1..=999));
            if response.changed() {
                changed = true;
            }
        });
    }

    ui.separator();

    match cue {
        qplayer_core::Cue::Sound { path, volume, pan, fade_in, fade_out, fade_type, eq, .. } => {
            ui.label(RichText::new("Sound Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked() {
                    if let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("Audio", &["wav", "mp3", "flac", "ogg", "aiff", "wma"])
                        .pick_file()
                    {
                        *path = new_path.to_string_lossy().to_string();
                        changed = true;
                    }
                }
            });
            if let Some((Some(ref peaks), _)) = waveform_data {
                let (new_zoom, new_scroll) = crate::waveform::draw(ui, peaks, waveform_zoom, waveform_scroll, 48.0);
                waveform_zoom = new_zoom;
                waveform_scroll = new_scroll;
            } else if let Some((None, true)) = waveform_data {
                ui.label(egui::RichText::new("Generating waveform…").italics().color(egui::Color32::GRAY));
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
                        for variant in [qplayer_core::FadeType::Linear, qplayer_core::FadeType::SCurve, qplayer_core::FadeType::Square, qplayer_core::FadeType::InverseSquare] {
                            if ui.selectable_value(fade_type, variant, format!("{:?}", variant)).clicked() {
                                changed = true;
                            }
                        }
                    });
            });
            eq_editor(ui, eq, &mut changed);
        }
        qplayer_core::Cue::Video { path, volume, pan, fade_in, fade_out, fade_type, eq, .. } => {
            ui.label(RichText::new("Video Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("File:");
                let response = ui.text_edit_singleline(path);
                changed |= response.changed();
                if ui.button("Browse…").clicked() {
                    if let Some(new_path) = rfd::FileDialog::new()
                        .add_filter("Video", &["mp4", "mov", "mkv", "avi"])
                        .pick_file()
                    {
                        *path = new_path.to_string_lossy().to_string();
                        changed = true;
                    }
                }
            });
            if let Some((Some(ref peaks), _)) = waveform_data {
                let (new_zoom, new_scroll) = crate::waveform::draw(ui, peaks, waveform_zoom, waveform_scroll, 48.0);
                waveform_zoom = new_zoom;
                waveform_scroll = new_scroll;
            } else if let Some((None, true)) = waveform_data {
                ui.label(egui::RichText::new("Generating waveform…").italics().color(egui::Color32::GRAY));
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
                        for variant in [qplayer_core::FadeType::Linear, qplayer_core::FadeType::SCurve, qplayer_core::FadeType::Square, qplayer_core::FadeType::InverseSquare] {
                            if ui.selectable_value(fade_type, variant, format!("{:?}", variant)).clicked() {
                                changed = true;
                            }
                        }
                    });
            });
            eq_editor(ui, eq, &mut changed);
        }
        qplayer_core::Cue::Group { .. } => {
            ui.label(RichText::new("Group Cue").monospace().size(12.0));
        }
        qplayer_core::Cue::Stop { stop_qid, .. } => {
            ui.label(RichText::new("Stop Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Stops Q#:");
                let mut qid_str = stop_qid.to_string();
                let response = ui.text_edit_singleline(&mut qid_str);
                if response.lost_focus() {
                    if let Ok(new_qid) = qid_str.parse::<rust_decimal::Decimal>() {
                        if new_qid != *stop_qid {
                            *stop_qid = new_qid;
                            changed = true;
                        }
                    }
                }
            });
        }
        qplayer_core::Cue::Volume { sound_qid, volume, .. } => {
            ui.label(RichText::new("Volume Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Target Q#:");
                let mut qid_str = sound_qid.to_string();
                let response = ui.text_edit_singleline(&mut qid_str);
                if response.lost_focus() {
                    if let Ok(new_qid) = qid_str.parse::<rust_decimal::Decimal>() {
                        if new_qid != *sound_qid {
                            *sound_qid = new_qid;
                            changed = true;
                        }
                    }
                }
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
        }
        qplayer_core::Cue::Dummy { .. } => {
            ui.label(RichText::new("Dummy Cue").monospace().size(12.0));
        }
        qplayer_core::Cue::TimeCode { start_time, duration, .. } => {
            ui.label(RichText::new("TimeCode Cue").monospace().size(12.0));
            ui.horizontal(|ui| {
                ui.label("Start (s):");
                let mut secs = start_time.as_secs_f64();
                let response = ui.add(egui::DragValue::new(&mut secs).speed(0.1));
                if response.changed() {
                    *start_time = qplayer_core::Timespan::from_secs_f64(secs);
                    changed = true;
                }
            });
            ui.horizontal(|ui| {
                ui.label("Duration (s):");
                let mut secs = duration.as_secs_f64();
                let response = ui.add(egui::DragValue::new(&mut secs).speed(0.1));
                if response.changed() {
                    *duration = qplayer_core::Timespan::from_secs_f64(secs);
                    changed = true;
                }
            });
        }
        qplayer_core::Cue::Osc { command, .. } => {
            ui.label(RichText::new("OSC Cue").monospace().size(12.0));
            ui.label("Command format: /address,arg1,arg2,…");
            ui.horizontal(|ui| {
                ui.label("Command:");
                let response = ui.text_edit_singleline(command);
                changed |= response.changed();
            });
        }
    }

    if changed {
        state.dirty = true;
    }

    // Write back waveform zoom/scroll (separate borrow to avoid conflict with cue editing)
    state.waveform_zoom = waveform_zoom;
    state.waveform_scroll = waveform_scroll;
}

fn eq_editor(ui: &mut egui::Ui, eq: &mut Option<qplayer_core::EQSettings>, changed: &mut bool) {
    ui.separator();
    ui.label(egui::RichText::new("EQ").strong().size(12.0));

    let mut enabled = eq.is_some();
    if ui.checkbox(&mut enabled, "Enabled").changed() {
        *changed = true;
        if enabled {
            *eq = Some(qplayer_core::EQSettings::default());
        } else {
            *eq = None;
        }
    }

    let Some(eq) = eq else { return };

    ui.horizontal(|ui| {
        ui.label("HPF:");
        let mut hpf_freq = eq.hpf.frequency;
        let response = ui.add(egui::DragValue::new(&mut hpf_freq).speed(1.0).range(20.0..=20000.0).suffix(" Hz"));
        if response.changed() {
            eq.hpf.frequency = hpf_freq;
            *changed = true;
        }
        egui::ComboBox::from_id_salt("hpf_order")
            .width(80.0)
            .selected_text(format!("{:?}", eq.hpf.order))
            .show_ui(ui, |ui| {
                for variant in [qplayer_core::EQFilterOrder::Disabled, qplayer_core::EQFilterOrder::_12dBOct, qplayer_core::EQFilterOrder::_24dBOct] {
                    if ui.selectable_value(&mut eq.hpf.order, variant, format!("{:?}", variant)).clicked() {
                        *changed = true;
                    }
                }
            });
    });

    ui.horizontal(|ui| {
        ui.label("LPF:");
        let mut lpf_freq = eq.lpf.frequency;
        let response = ui.add(egui::DragValue::new(&mut lpf_freq).speed(1.0).range(20.0..=20000.0).suffix(" Hz"));
        if response.changed() {
            eq.lpf.frequency = lpf_freq;
            *changed = true;
        }
        egui::ComboBox::from_id_salt("lpf_order")
            .width(80.0)
            .selected_text(format!("{:?}", eq.lpf.order))
            .show_ui(ui, |ui| {
                for variant in [qplayer_core::EQFilterOrder::Disabled, qplayer_core::EQFilterOrder::_12dBOct, qplayer_core::EQFilterOrder::_24dBOct] {
                    if ui.selectable_value(&mut eq.lpf.order, variant, format!("{:?}", variant)).clicked() {
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
                        qplayer_core::EQBandShape::Bell,
                        qplayer_core::EQBandShape::HighShelf,
                        qplayer_core::EQBandShape::LowShelf,
                        qplayer_core::EQBandShape::Notch,
                        qplayer_core::EQBandShape::LowPass,
                        qplayer_core::EQBandShape::HighPass,
                        qplayer_core::EQBandShape::AllPass,
                    ] {
                        if ui.selectable_value(&mut band.shape, variant, format!("{:?}", variant)).clicked() {
                            *changed = true;
                        }
                    }
                });
            let mut freq = band.freq;
            let response = ui.add(egui::DragValue::new(&mut freq).speed(1.0).range(20.0..=20000.0).suffix(" Hz"));
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

