//! Lighting configuration panel — DMX output settings + fixture patch table.

use cuepool_core::lighting::{
    Axis, ChannelRole, Corner, FixtureProfile, LightingProtocol, PatchedFixture, PixelMapSegment,
    SegmentSource,
};
use rustjay_lighting::{PatchSpan, find_overlaps};

use crate::app::SharedStateHandle;

pub fn show(ui: &mut egui::Ui, state: &SharedStateHandle) {
    let Ok(mut state) = state.lock() else { return };

    // ponytail: whole-state clone per panel frame so every edit is undoable.
    let pre_edit = crate::app::Snapshot::from_state(&state).with_merge_key("lighting");
    let mut changed = false;

    let lighting = &mut state.show_file.lighting;

    ui.heading("DMX Output");
    ui.separator();

    ui.horizontal(|ui| {
        changed |= ui
            .checkbox(&mut lighting.enabled, "Enabled")
            .on_hover_text("Output DMX over the network at the refresh rate below")
            .changed();
        egui::ComboBox::from_id_salt("lx_protocol")
            .selected_text(match lighting.protocol {
                LightingProtocol::Sacn => "sACN (E1.31)",
                LightingProtocol::ArtNet => "Art-Net",
            })
            .show_ui(ui, |ui| {
                changed |= ui
                    .selectable_value(
                        &mut lighting.protocol,
                        LightingProtocol::Sacn,
                        "sACN (E1.31)",
                    )
                    .clicked();
                changed |= ui
                    .selectable_value(&mut lighting.protocol, LightingProtocol::ArtNet, "Art-Net")
                    .clicked();
            })
            .response
            .on_hover_text("DMX output protocol");
    });
    ui.horizontal(|ui| {
        ui.label("Destination IP:");
        changed |= ui
            .text_edit_singleline(&mut lighting.dest_ip)
            .on_hover_text(
                "Unicast destination override — empty = sACN multicast / Art-Net broadcast",
            )
            .changed();
    });
    ui.label(
        egui::RichText::new("Empty = sACN multicast / Art-Net broadcast")
            .small()
            .weak(),
    );
    ui.horizontal(|ui| {
        ui.label("Refresh (fps):");
        changed |= ui
            .add(
                egui::DragValue::new(&mut lighting.fps)
                    .speed(1)
                    .range(1.0..=60.0),
            )
            .on_hover_text("DMX frames per second")
            .changed();
        ui.label("Look priority:").on_hover_text(
            "sACN-style merge priority of lighting-cue looks vs DMX Show cues (default 100)",
        );
        changed |= ui
            .add(
                egui::DragValue::new(&mut lighting.look_priority)
                    .speed(1)
                    .range(0..=255),
            )
            .on_hover_text(
                "sACN-style merge priority of lighting-cue looks vs DMX Show cues (default 100)",
            )
            .changed();
    });

    ui.separator();
    ui.horizontal(|ui| {
        ui.heading("Patch");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .button("Export sheet…")
                .on_hover_text("Save the patch as CSV: fixtures, per-channel detail, and segment spans, with overlap warnings")
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .set_file_name("patch-sheet.csv")
                    .save_file()
                {
                    let csv = cuepool_core::lighting::patch_sheet_csv(lighting);
                    if let Err(e) = std::fs::write(&path, csv) {
                        log::error!("Patch sheet export failed: {e}");
                    } else {
                        log::info!("Patch sheet exported to {}", path.display());
                    }
                }
            if ui
                .button("+ Add Fixture")
                .on_hover_text("Patch a new fixture at the next free address")
                .clicked()
            {
                let id = lighting.next_fixture_id();
                // Next free address after the last fixture in its universe.
                let (universe, address) = lighting
                    .fixtures
                    .last()
                    .map(|f| {
                        let footprint = lighting
                            .profile(&f.profile_id)
                            .map_or(1, |p| p.footprint() as u16);
                        (f.universe, f.address + footprint)
                    })
                    .unwrap_or((1, 1));
                lighting.fixtures.push(PatchedFixture {
                    id,
                    name: format!("Fixture {id}"),
                    profile_id: "dimmer".into(),
                    universe,
                    address,
                    dest_ip: String::new(),
                });
                changed = true;
            }
        });
    });

    let profiles = lighting.all_profiles();
    let mut remove: Option<usize> = None;
    for (i, fixture) in lighting.fixtures.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}", fixture.id))
                .on_hover_text("Fixture ID");
            ui.add(egui::TextEdit::singleline(&mut fixture.name).desired_width(110.0))
                .on_hover_text("Fixture name")
                .changed()
                .then(|| changed = true);
            let selected = profiles
                .iter()
                .find(|p| p.id == fixture.profile_id)
                .map_or_else(
                    || format!("{} (missing)", fixture.profile_id),
                    |p| p.name.clone(),
                );
            egui::ComboBox::from_id_salt(("lx_profile", i))
                .selected_text(selected)
                .width(150.0)
                .show_ui(ui, |ui| {
                    for p in &profiles {
                        if ui
                            .selectable_value(&mut fixture.profile_id, p.id.clone(), &p.name)
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("Fixture profile (channel layout)");
            ui.label("U:").on_hover_text("DMX universe");
            let mut u = fixture.universe as i32;
            if ui
                .add(egui::DragValue::new(&mut u).speed(1).range(1..=63999))
                .changed()
            {
                fixture.universe = u.max(1) as u16;
                changed = true;
            }
            ui.label("Ch:")
                .on_hover_text("Starting DMX channel (1–512)");
            let mut a = fixture.address as i32;
            if ui
                .add(egui::DragValue::new(&mut a).speed(1).range(1..=512))
                .changed()
            {
                fixture.address = a.clamp(1, 512) as u16;
                changed = true;
            }
            let footprint = profiles
                .iter()
                .find(|p| p.id == fixture.profile_id)
                .map_or(0, |p| p.footprint());
            ui.label(
                egui::RichText::new(format!("({footprint} ch)"))
                    .small()
                    .weak(),
            )
            .on_hover_text("DMX footprint of the selected profile");
            ui.label("IP:")
                .on_hover_text("Per-fixture unicast override — empty = use the global destination");
            ui.add(
                egui::TextEdit::singleline(&mut fixture.dest_ip)
                    .desired_width(100.0)
                    .hint_text("global"),
            )
            .changed()
            .then(|| changed = true);
            if ui
                .small_button("✕")
                .on_hover_text("Remove this fixture")
                .clicked()
            {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        lighting.fixtures.remove(i);
        changed = true;
    }

    // Overlap warnings — two fixtures claiming the same channels. Grouped per
    // destination: the same universe on different nodes doesn't collide.
    let mut spans_by_dest: std::collections::BTreeMap<String, Vec<PatchSpan>> = Default::default();
    for f in &lighting.fixtures {
        let Some(profile) = lighting.profile(&f.profile_id) else {
            continue;
        };
        let footprint = profile.footprint() as u16;
        if footprint == 0 {
            continue;
        }
        spans_by_dest
            .entry(f.effective_dest(&lighting.dest_ip).to_string())
            .or_default()
            .push(PatchSpan {
                owner: f.name.clone(),
                detail: f.profile_id.clone(),
                universe: f.universe,
                start: f.address,
                end: f.address + footprint - 1,
            });
    }
    for spans in spans_by_dest.values() {
        for o in find_overlaps(spans) {
            ui.colored_label(
                egui::Color32::from_rgb(230, 160, 60),
                format!(
                    "⚠ '{}' and '{}' overlap on universe {} ch {}–{}",
                    o.a.owner, o.b.owner, o.universe, o.start, o.end
                ),
            );
        }
    }

    ui.separator();
    // Painted after the mutable `lighting` borrow ends, from the same lock.
    let preview = state.lighting_preview.clone();
    let lighting = &mut state.show_file.lighting;
    egui::CollapsingHeader::new("Pixel Map").show(ui, |ui| {
        changed |= segments_editor(ui, lighting, &preview);
    });

    ui.separator();
    egui::CollapsingHeader::new("Profiles").show(ui, |ui| {
        changed |= profiles_editor(ui, lighting);
    });

    if changed {
        state.dirty = true;
        state.undo_redo.push(pre_edit);
    }
}

/// Pixel-map segment editor: source region → fixture grid → DMX patch.
/// `preview`: latest sampled pixels per segment id (cols, rows, RGBA).
fn segments_editor(
    ui: &mut egui::Ui,
    lighting: &mut cuepool_core::LightingConfig,
    preview: &std::collections::HashMap<u32, (u32, u32, Vec<u8>)>,
) -> bool {
    let mut changed = false;
    ui.label(
        egui::RichText::new(
            "Segments sample their source live; their channels override cue looks. \
             Source 'PixelMap' is fed by PixelMap cues, 'Canvas' mirrors the projector picture.",
        )
        .small()
        .weak(),
    );
    if ui
        .button("+ Add Segment")
        .on_hover_text("Add a pixel-map segment: samples a source region into fixture channels")
        .clicked()
    {
        let id = lighting.next_segment_id();
        lighting.segments.push(PixelMapSegment::new(id));
        changed = true;
    }

    let profiles = lighting.all_profiles();
    let mut remove: Option<usize> = None;
    for (i, seg) in lighting.segments.iter_mut().enumerate() {
        ui.separator();
        ui.horizontal(|ui| {
            changed |= ui
                .checkbox(&mut seg.enabled, "")
                .on_hover_text("Sample this segment live; its channels override cue looks")
                .changed();
            changed |= ui
                .add(egui::TextEdit::singleline(&mut seg.name).desired_width(110.0))
                .on_hover_text("Segment name")
                .changed();
            let selected = profiles
                .iter()
                .find(|p| p.id == seg.profile_id)
                .map_or_else(
                    || format!("{} (missing)", seg.profile_id),
                    |p| p.name.clone(),
                );
            egui::ComboBox::from_id_salt(("lx_seg_profile", i))
                .selected_text(selected)
                .width(130.0)
                .show_ui(ui, |ui| {
                    for p in &profiles {
                        if ui
                            .selectable_value(&mut seg.profile_id, p.id.clone(), &p.name)
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("Fixture profile for each grid cell");
            if ui
                .small_button("✕")
                .on_hover_text("Remove this segment")
                .clicked()
            {
                remove = Some(i);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Source:");
            egui::ComboBox::from_id_salt(("lx_seg_source", i))
                .selected_text(match seg.source {
                    SegmentSource::Canvas => "Canvas",
                    SegmentSource::PixelMap => "PixelMap",
                })
                .show_ui(ui, |ui| {
                    for (src, label) in [
                        (SegmentSource::Canvas, "Canvas"),
                        (SegmentSource::PixelMap, "PixelMap"),
                    ] {
                        if ui.selectable_value(&mut seg.source, src, label).clicked() {
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("What feeds the segment: 'PixelMap' is fed by PixelMap cues, 'Canvas' mirrors the projector picture");
            ui.label("Region:")
                .on_hover_text("Source region as a 0–1 fraction of the source (x, y, w, h)");
            for (label, v) in ["x", "y", "w", "h"].into_iter().zip(seg.region.iter_mut()) {
                ui.label(label);
                changed |= ui
                    .add(egui::DragValue::new(v).speed(0.01).range(0.0..=1.0))
                    .changed();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Grid:")
                .on_hover_text("Fixture grid: cols × rows cells, patched to consecutive channels");
            let mut cols = seg.cols as i32;
            if ui
                .add(egui::DragValue::new(&mut cols).speed(1).range(1..=512))
                .changed()
            {
                seg.cols = cols.max(1) as u32;
                changed = true;
            }
            ui.label("×");
            let mut rows = seg.rows as i32;
            if ui
                .add(egui::DragValue::new(&mut rows).speed(1).range(1..=512))
                .changed()
            {
                seg.rows = rows.max(1) as u32;
                changed = true;
            }
            ui.label("U:").on_hover_text("DMX universe");
            let mut u = seg.universe as i32;
            if ui
                .add(egui::DragValue::new(&mut u).speed(1).range(1..=63999))
                .changed()
            {
                seg.universe = u.max(1) as u16;
                changed = true;
            }
            ui.label("Ch:")
                .on_hover_text("Starting DMX channel (1–512)");
            let mut a = seg.address as i32;
            if ui
                .add(egui::DragValue::new(&mut a).speed(1).range(1..=512))
                .changed()
            {
                seg.address = a.clamp(1, 512) as u16;
                changed = true;
            }
            let footprint = profiles
                .iter()
                .find(|p| p.id == seg.profile_id)
                .map_or(0, |p| p.footprint());
            let count = (seg.cols * seg.rows) as usize;
            ui.label(
                egui::RichText::new(format!("{count} px × {footprint} ch"))
                    .small()
                    .weak(),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Scan:")
                .on_hover_text("Scan order: how grid cells map to consecutive DMX channels");
            egui::ComboBox::from_id_salt(("lx_seg_corner", i))
                .selected_text(seg.order.start_corner.label())
                .show_ui(ui, |ui| {
                    for c in [
                        Corner::TopLeft,
                        Corner::TopRight,
                        Corner::BottomLeft,
                        Corner::BottomRight,
                    ] {
                        if ui
                            .selectable_value(&mut seg.order.start_corner, c, c.label())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("Grid corner where channel 1 starts");
            egui::ComboBox::from_id_salt(("lx_seg_axis", i))
                .selected_text(seg.order.primary.label())
                .show_ui(ui, |ui| {
                    for a in [Axis::Horizontal, Axis::Vertical] {
                        if ui
                            .selectable_value(&mut seg.order.primary, a, a.label())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("Scan along rows first or columns first");
            changed |= ui
                .checkbox(&mut seg.order.serpentine, "Serpentine")
                .on_hover_text("Alternate the scan direction each row/column (zig-zag wiring)")
                .changed();
        });
        ui.horizontal(|ui| {
            ui.label("Brightness:");
            changed |= ui
                .add(egui::Slider::new(&mut seg.color.brightness, 0.0..=1.0))
                .on_hover_text("Output brightness for this segment")
                .changed();
            ui.label("Gamma:");
            changed |= ui
                .add(
                    egui::DragValue::new(&mut seg.gamma)
                        .speed(0.05)
                        .range(0.2..=4.0),
                )
                .on_hover_text("Gamma applied to the sampled colour")
                .changed();
            ui.label("White:");
            egui::ComboBox::from_id_salt(("lx_seg_white", i))
                .selected_text(seg.color.white.label())
                .show_ui(ui, |ui| {
                    use cuepool_core::lighting::WhiteMode;
                    for w in [
                        WhiteMode::Off,
                        WhiteMode::Min { amount: 1.0 },
                        WhiteMode::MinSubtract { amount: 1.0 },
                    ] {
                        if ui
                            .selectable_value(&mut seg.color.white, w, w.label())
                            .clicked()
                        {
                            changed = true;
                        }
                    }
                })
                .response
                .on_hover_text("White-channel derivation from the sampled RGB");
            changed |= ui
                .checkbox(&mut seg.color.derive_amber_uv, "Derive A/UV")
                .on_hover_text(
                    "Approximate Amber ((R+G)/2) and UV (0.8·B) from the sampled colour. \
                     Off: those channels stay at 0.",
                )
                .changed();
        });
        // Live preview: what each grid cell sampled (pre-gamma source colours).
        if let Some((cols, rows, rgba)) = preview.get(&seg.id) {
            let cell = 14.0_f32;
            let w = (*cols as f32 * cell).min(ui.available_width().max(cell));
            let h = (*rows as f32 * cell).min(200.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 2.0, egui::Color32::from_gray(25));
            let cw = rect.width() / *cols as f32;
            let ch = rect.height() / *rows as f32;
            for r in 0..*rows {
                for c in 0..*cols {
                    let idx = ((r * cols + c) * 4) as usize;
                    if idx + 3 < rgba.len() {
                        let col = egui::Color32::from_rgb(rgba[idx], rgba[idx + 1], rgba[idx + 2]);
                        let min = rect.min + egui::vec2(c as f32 * cw, r as f32 * ch);
                        painter.rect_filled(
                            egui::Rect::from_min_size(min, egui::vec2(cw - 1.0, ch - 1.0)),
                            1.0,
                            col,
                        );
                    }
                }
            }
        } else if seg.enabled {
            ui.label(
                egui::RichText::new("no samples yet — fire a PixelMap (or Video/Image) cue")
                    .small()
                    .weak(),
            );
        }
    }
    if let Some(i) = remove {
        lighting.segments.remove(i);
        changed = true;
    }
    changed
}

/// User fixture-profile editor. Builtins are fixed; user profiles are
/// name-editable with an append/remove channel-chip row (vjarda pattern).
fn import_gdtf_mode(
    lighting: &mut cuepool_core::LightingConfig,
    fixture: &cuepool_core::gdtf::GdtfFixture,
    mode_idx: usize,
    free_user_id: &dyn Fn(&cuepool_core::LightingConfig) -> String,
) {
    let mode = &fixture.modes[mode_idx];
    let name = if fixture.modes.len() > 1 {
        format!("{} — {}", fixture.name, mode.name)
    } else {
        fixture.name.clone()
    };
    log::info!(
        "Imported GDTF profile '{name}' ({} ch)",
        mode.channels.len()
    );
    lighting.profiles.push(FixtureProfile {
        id: free_user_id(lighting),
        name,
        channels: mode.channels.clone(),
    });
}

fn profiles_editor(ui: &mut egui::Ui, lighting: &mut cuepool_core::LightingConfig) -> bool {
    let mut changed = false;

    let free_user_id = |lighting: &cuepool_core::LightingConfig| -> String {
        (1..)
            .map(|n| format!("user_{n}"))
            .find(|id| lighting.all_profiles().iter().all(|p| &p.id != id))
            .unwrap()
    };

    ui.horizontal(|ui| {
        if ui
            .button("+ New Profile")
            .on_hover_text("Create an editable fixture profile (starts as RGB)")
            .clicked()
        {
            let id = free_user_id(lighting);
            let name = format!("User Profile {}", id.trim_start_matches("user_"));
            lighting.profiles.push(FixtureProfile {
                id,
                name,
                channels: vec![ChannelRole::Red, ChannelRole::Green, ChannelRole::Blue],
            });
            changed = true;
        }
        if ui
            .button("Import GDTF…")
            .on_hover_text(
                "Import a fixture profile from a .gdtf file (pick a DMX mode if it has several)",
            )
            .clicked()
            && let Some(path) = rfd::FileDialog::new()
                .add_filter("GDTF fixture", &["gdtf"])
                .pick_file()
        {
            match cuepool_core::gdtf::parse_gdtf(&path) {
                Ok(f) if f.modes.len() == 1 => {
                    import_gdtf_mode(lighting, &f, 0, &free_user_id);
                    changed = true;
                }
                Ok(f) => {
                    ui.data_mut(|d| d.insert_temp(egui::Id::new("gdtf_pending"), f));
                }
                Err(e) => log::error!("GDTF import failed: {e}"),
            }
        }
    });

    // Multi-mode GDTF: pick which DMX mode becomes the profile.
    let pending: Option<cuepool_core::gdtf::GdtfFixture> =
        ui.data(|d| d.get_temp(egui::Id::new("gdtf_pending")));
    if let Some(f) = pending {
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("{} ({}): mode →", f.name, f.manufacturer));
            let mut done = false;
            for (i, mode) in f.modes.iter().enumerate() {
                if ui
                    .button(format!("{} ({} ch)", mode.name, mode.channels.len()))
                    .clicked()
                {
                    import_gdtf_mode(lighting, &f, i, &free_user_id);
                    changed = true;
                    done = true;
                }
            }
            if ui.button("Cancel").clicked() {
                done = true;
            }
            if done {
                ui.data_mut(|d| {
                    d.remove::<cuepool_core::gdtf::GdtfFixture>(egui::Id::new("gdtf_pending"))
                });
            }
        });
    }

    let in_use: std::collections::HashSet<&str> = lighting
        .fixtures
        .iter()
        .map(|f| f.profile_id.as_str())
        .collect();

    let mut remove: Option<usize> = None;
    for (i, profile) in lighting.profiles.iter_mut().enumerate() {
        ui.separator();
        ui.horizontal(|ui| {
            changed |= ui
                .add(egui::TextEdit::singleline(&mut profile.name).desired_width(160.0))
                .on_hover_text("Profile name")
                .changed();
            ui.label(
                egui::RichText::new(format!("({} ch)", profile.channels.len()))
                    .small()
                    .weak(),
            );
            if in_use.contains(profile.id.as_str()) {
                ui.label(egui::RichText::new("in use").small().weak())
                    .on_hover_text("Patched fixtures use this profile — it can't be deleted");
            } else if ui
                .small_button("✕")
                .on_hover_text("Delete this profile")
                .clicked()
            {
                remove = Some(i);
            }
        });
        // Channel chips — drag to reorder (channel order = DMX layout),
        // right-click to delete. Left click is deliberately inert.
        ui.horizontal_wrapped(|ui| {
            let mut remove_ch: Option<usize> = None;
            let mut move_ch: Option<(usize, usize)> = None; // (from, insert-at)
            for (ci, role) in profile.channels.iter().enumerate() {
                let chip_id = egui::Id::new(("lx_chip", &profile.id, ci));
                let resp = ui
                    .dnd_drag_source(chip_id, (profile.id.clone(), ci), |ui| {
                        let _ = ui.small_button(format!("{}:{}", ci + 1, role.label()));
                    })
                    .response
                    .interact(egui::Sense::click())
                    .on_hover_text(format!(
                        "ch {}: {} — drag to reorder, right-click to delete",
                        ci + 1,
                        role.describe()
                    ));
                if resp.secondary_clicked() {
                    remove_ch = Some(ci);
                }
                if let Some(payload) = resp.dnd_release_payload::<(String, usize)>()
                    && payload.0 == profile.id
                    && payload.1 != ci
                {
                    move_ch = Some((payload.1, ci));
                }
            }
            if let Some((from, to)) = move_ch {
                let role = profile.channels.remove(from);
                profile
                    .channels
                    .insert(to.min(profile.channels.len()), role);
                changed = true;
            } else if let Some(ci) = remove_ch {
                profile.channels.remove(ci);
                changed = true;
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label("add:");
            let mut push = |role| {
                profile.channels.push(role);
                changed = true;
            };
            for (label, role) in [
                ("R", ChannelRole::Red),
                ("G", ChannelRole::Green),
                ("B", ChannelRole::Blue),
                ("W", ChannelRole::White),
                ("A", ChannelRole::Amber),
                ("UV", ChannelRole::Uv),
                ("D", ChannelRole::Dimmer),
                ("P", ChannelRole::Pan),
                ("Pf", ChannelRole::PanFine),
                ("T", ChannelRole::Tilt),
                ("Tf", ChannelRole::TiltFine),
                ("Z", ChannelRole::Zoom),
                ("St", ChannelRole::Strobe),
                ("Go", ChannelRole::Gobo),
                ("S", ChannelRole::Static(255)),
            ] {
                if ui
                    .small_button(label)
                    .on_hover_text(role.describe())
                    .clicked()
                {
                    push(role);
                }
            }
        });
        // Editable value when the last channel is Static (vjarda pattern).
        if let Some(ChannelRole::Static(v)) = profile.channels.last_mut() {
            ui.horizontal(|ui| {
                ui.label("static value:");
                changed |= ui
                    .add(egui::DragValue::new(v).speed(1).range(0..=255))
                    .on_hover_text("Constant DMX value sent on the Static channel")
                    .changed();
            });
        }
    }
    if let Some(i) = remove {
        lighting.profiles.remove(i);
        changed = true;
    }

    ui.separator();
    ui.label(egui::RichText::new("Builtins (fixed):").small().weak());
    for p in cuepool_core::lighting::builtin_profiles() {
        ui.label(
            egui::RichText::new(format!(
                "{} — {}",
                p.name,
                p.channels
                    .iter()
                    .map(|r| r.label())
                    .collect::<Vec<_>>()
                    .join(",")
            ))
            .small()
            .weak(),
        );
    }
    changed
}
