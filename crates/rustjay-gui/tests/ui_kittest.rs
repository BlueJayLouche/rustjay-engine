//! Update visual baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p rustjay-gui --features egui`.

#![cfg(feature = "egui")]

use egui::{Rect, accesskit::Role, epaint::Shape};
use egui_kittest::{
    Harness,
    kittest::{NodeT as _, Queryable as _},
};
use rustjay_core::EngineState;
use rustjay_gui::{AnyEguiTab, EguiControlGui};
use std::sync::{Arc, Mutex};

struct DummyAppTab;

impl AnyEguiTab for DummyAppTab {
    fn name(&self) -> &str {
        "Dummy App"
    }

    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        _app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        ui.heading("Dummy app tab");
    }
}

#[allow(deprecated)] // Context harness is required for the control GUI's top-level panels.
fn control_harness(size: [f32; 2], pixels_per_point: f32) -> Harness<'static> {
    let mut engine = EngineState::default();
    engine.audio.enabled = false;
    let shared_state = Arc::new(Mutex::new(engine));
    let mut gui = EguiControlGui::new(shared_state).expect("default engine state is valid");
    gui.custom_tabs.push(Box::new(DummyAppTab));
    let mut app_state = ();

    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .with_theme(egui::Theme::Dark)
        // egui 0.36 / kittest 0.36: the app closure receives a root Ui.
        .build_ui(move |ui| gui.build_ui(ui, &mut app_state));

    harness.ctx.set_theme(egui::Theme::Dark);
    harness.ctx.global_style_mut(|style| {
        style.interaction.selectable_labels = false;
        style.interaction.multi_widget_text_select = false;
    });
    harness.run();
    harness
}

fn collect_text_rects(shape: &Shape, clip_rect: Rect, label: &str, rects: &mut Vec<Rect>) {
    match shape {
        Shape::Text(text) if text.galley.job.text == label => {
            let rect = text.visual_bounding_rect();
            if clip_rect.intersects(rect) {
                rects.push(rect);
            }
        }
        Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text_rects(shape, clip_rect, label, rects);
            }
        }
        _ => {}
    }
}

fn painted_text_rects(harness: &Harness<'_>, label: &str) -> Vec<Rect> {
    let mut rects = Vec::new();
    for clipped in &harness.output().shapes {
        collect_text_rects(&clipped.shape, clipped.clip_rect, label, &mut rects);
    }
    rects
}

fn assert_painted(harness: &Harness<'_>, label: &str, expected: bool) {
    assert_eq!(
        !painted_text_rects(harness, label).is_empty(),
        expected,
        "expected {label:?} painted={expected}"
    );
}

fn click_painted_text(harness: &mut Harness<'_>, label: &str) {
    let rects = painted_text_rects(harness, label);
    assert_eq!(
        rects.len(),
        1,
        "expected one painted {label:?}, got {rects:?}"
    );
    let pos = rects[0].center();
    harness.hover_at(pos);
    harness.drag_at(pos);
    harness.drop_at(pos);
    harness.run();
}

#[test]
fn expanded_sidebar_snapshot() {
    let mut harness = control_harness([800.0, 1400.0], 0.5);

    harness.snapshot("sidebar_expanded");
}

#[test]
fn every_sidebar_section_collapses_and_expands() {
    let mut harness = control_harness([400.0, 900.0], 1.0);

    for (header, child) in [
        ("SIGNAL", "OUTPUT"),
        ("PARAMS", "AUDIO"),
        ("CONTROL", "MIDI"),
        ("MANAGE", "SETTINGS"),
        ("APP", "DUMMY APP"),
    ] {
        assert_painted(&harness, child, true);
        assert_eq!(chevrons_beside(&harness, header, "▼"), 1, "{header} expanded");
        click_painted_text(&mut harness, header);
        assert_painted(&harness, child, false);
        // A collapsed section's own header shows ▶, not ▼.
        assert_eq!(chevrons_beside(&harness, header, "▶"), 1, "{header} collapsed");
        assert_eq!(chevrons_beside(&harness, header, "▼"), 0, "{header} collapsed");
        click_painted_text(&mut harness, header);
        assert_painted(&harness, child, true);
    }
}

/// Count chevron glyphs painted on the same row as `header` (the sidebar
/// paints each section's chevron at the left edge of its header row). Other
/// UI regions also use ▶ (start buttons, index markers), so counting globally
/// is meaningless.
fn chevrons_beside(harness: &Harness<'_>, header: &str, glyph: &str) -> usize {
    let headers = painted_text_rects(harness, header);
    assert_eq!(headers.len(), 1, "expected one painted {header:?}");
    let row_y = headers[0].center().y;
    painted_text_rects(harness, glyph)
        .iter()
        .filter(|r| (r.center().y - row_y).abs() < 6.0)
        .count()
}

#[test]
fn settings_and_output_snapshots_via_sidebar() {
    let mut harness = control_harness([800.0, 700.0], 1.0);

    click_painted_text(&mut harness, "SETTINGS");

    let disabled_dimension_inputs = harness
        .query_all_by_role(Role::SpinButton)
        .filter(|node| node.accesskit_node().is_disabled())
        .count();
    assert_eq!(disabled_dimension_inputs, 4);

    harness
        .query_all_by_role(Role::ComboBox)
        .next()
        .expect("internal resolution preset dropdown")
        .click();
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "NTSC (720x480)")
            .is_some()
    );
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "PAL (720x576)")
            .is_some()
    );
    harness
        .query_all_by_role(Role::ComboBox)
        .next()
        .expect("internal resolution preset dropdown")
        .click();
    harness.run();

    harness.snapshot("settings_tab");

    click_painted_text(&mut harness, "OUTPUT");
    harness.snapshot("output_tab");
}

/// Binding a modulation source used to mean going to the Modulation window,
/// making one, then coming back to the parameter. The map-mode popup creates
/// and assigns in a single click.
#[test]
fn map_mode_popup_creates_and_assigns_a_source() {
    let mut engine = EngineState::default();
    engine.audio.enabled = false;
    engine.lfo_assign_mode = true;
    let shared = Arc::new(Mutex::new(engine));

    let drawn = shared.clone();
    let mut harness = Harness::builder()
        .with_size([420.0, 320.0])
        .with_theme(egui::Theme::Dark)
        .build_ui(move |ui| {
            let mut engine = drawn.lock().unwrap_or_else(|e| e.into_inner());
            let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(200.0, 24.0));
            rustjay_gui::apply_param_map_overlay(
                ui,
                &mut engine,
                rect,
                "color/brightness",
                "Brightness",
                "color/brightness",
                0.0,
                1.0,
            );
        });
    harness.run();

    // A default engine already ships modulation sources, so count the change
    // rather than the total.
    let before = {
        let e = shared.lock().unwrap_or_else(|p| p.into_inner());
        let m = e.modulation.lock().unwrap_or_else(|p| p.into_inner());
        m.sources.len()
    };

    // Nothing is offered until the parameter itself is clicked.
    assert_painted(&harness, "+ Audio · Bass", false);

    let pos = egui::pos2(120.0, 32.0);
    harness.hover_at(pos);
    harness.drag_at(pos);
    harness.drop_at(pos);
    harness.run();

    assert_painted(&harness, "+ New LFO", true);
    assert_painted(&harness, "+ Audio · Bass", true);

    click_painted_text(&mut harness, "+ Audio · Bass");

    let engine = shared.lock().unwrap_or_else(|p| p.into_inner());
    let mod_eng = engine.modulation.lock().unwrap_or_else(|p| p.into_inner());
    assert_eq!(
        mod_eng.sources.len(),
        before + 1,
        "one click makes exactly one source"
    );
    assert!(
        matches!(
            mod_eng.sources.last().map(|s| &s.source),
            Some(rustjay_core::modulation::ModulationSource::AudioBand { .. })
        ),
        "and it is the audio band that was picked"
    );
    let bound = mod_eng
        .assignments
        .get("color/brightness")
        .expect("the parameter is bound");
    assert_eq!(bound.len(), 1, "one active source per parameter");
}

/// An LFO and an audio band on one parameter is the point, not a mistake: the
/// engine sums every assignment, so the popup must add rather than replace.
#[test]
fn map_mode_popup_stacks_sources_on_one_param() {
    let mut engine = EngineState::default();
    engine.audio.enabled = false;
    engine.lfo_assign_mode = true;
    let shared = Arc::new(Mutex::new(engine));

    let drawn = shared.clone();
    let mut harness = Harness::builder()
        .with_size([420.0, 420.0])
        .with_theme(egui::Theme::Dark)
        .build_ui(move |ui| {
            let mut engine = drawn.lock().unwrap_or_else(|e| e.into_inner());
            let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(200.0, 24.0));
            rustjay_gui::apply_param_map_overlay(
                ui,
                &mut engine,
                rect,
                "color/brightness",
                "Brightness",
                "color/brightness",
                0.0,
                1.0,
            );
        });
    harness.run();

    let open = |h: &mut Harness<'_>| {
        let pos = egui::pos2(120.0, 32.0);
        h.hover_at(pos);
        h.drag_at(pos);
        h.drop_at(pos);
        h.run();
    };

    open(&mut harness);
    click_painted_text(&mut harness, "+ New LFO");
    open(&mut harness);
    click_painted_text(&mut harness, "+ Audio · Bass");

    let engine = shared.lock().unwrap_or_else(|p| p.into_inner());
    let mod_eng = engine.modulation.lock().unwrap_or_else(|p| p.into_inner());
    let bound = mod_eng
        .assignments
        .get("color/brightness")
        .expect("the parameter is bound");
    assert_eq!(bound.len(), 2, "the second source adds to the first");

    let kinds: Vec<&str> = bound
        .iter()
        .filter_map(|m| {
            mod_eng
                .sources
                .iter()
                .find(|e| e.uuid == m.source_id)
                .map(|e| match e.source {
                    rustjay_core::modulation::ModulationSource::LFO { .. } => "LFO",
                    rustjay_core::modulation::ModulationSource::AudioBand { .. } => "Audio",
                    _ => "other",
                })
        })
        .collect();
    assert!(kinds.contains(&"LFO") && kinds.contains(&"Audio"), "{kinds:?}");
}

/// U4: the routing window is a view over the modulation engine. Adding a
/// route through it must land in the shared `ModulationEngine` as an
/// `AudioBand` source plus an assignment — not in the legacy matrix.
#[test]
fn routing_window_add_route_creates_source_and_assignment() {
    let mut engine = EngineState::default();
    // The audio tab hides everything below the device picker (including the
    // routing section) while audio analysis is off.
    engine.audio.enabled = true;
    // And the routing section hides its window button while routing is off.
    engine.audio_routing.enabled = true;
    let shared = Arc::new(Mutex::new(engine));

    let mut gui =
        EguiControlGui::new(shared.clone()).expect("default engine state is valid");
    let mut app_state = ();
    let mut harness = Harness::builder()
        .with_size([1100.0, 1700.0])
        .with_theme(egui::Theme::Dark)
        .build_ui(move |ui| gui.build_ui(ui, &mut app_state));
    // With audio enabled a status pill repaints continuously, so `run()`
    // never settles; drive a fixed number of frames instead.
    harness.run_steps(2);

    // click_painted_text ends in `run()`; same click, but stepped. A label can
    // be painted twice (sidebar entry + main-area title); the leftmost is the
    // sidebar button.
    fn click_steps(harness: &mut Harness<'_>, label: &str) {
        let mut rects = painted_text_rects(harness, label);
        assert!(!rects.is_empty(), "expected a painted {label:?}");
        rects.sort_by(|a, b| a.left().partial_cmp(&b.left()).unwrap());
        let pos = rects[0].center();
        harness.hover_at(pos);
        harness.drag_at(pos);
        harness.drop_at(pos);
        harness.run_steps(2);
    }

    // A default engine ships 8 seeded LFO sources, so count the change.
    let before = {
        let e = shared.lock().unwrap_or_else(|p| p.into_inner());
        let m = e.modulation.lock().unwrap_or_else(|p| p.into_inner());
        m.sources.len()
    };

    click_steps(&mut harness, "AUDIO");
    click_steps(&mut harness, "Open Routing Matrix");
    assert_painted(&harness, "Add Route", true);

    click_steps(&mut harness, "Add Route");

    let engine = shared.lock().unwrap_or_else(|p| p.into_inner());
    let mod_eng = engine.modulation.lock().unwrap_or_else(|p| p.into_inner());
    assert_eq!(
        mod_eng.sources.len(),
        before + 1,
        "one route adds exactly one source"
    );
    // Defaults: band index 1 (Bass, 60–120 Hz), target index 1 (Saturation).
    let entry = mod_eng
        .find_source_by_uuid("route_0")
        .expect("the route is a source in the shared engine");
    assert!(
        matches!(
            &entry.source,
            rustjay_core::modulation::ModulationSource::AudioBand {
                freq_low,
                freq_high,
                ..
            } if *freq_low == 60.0 && *freq_high == 120.0
        ),
        "the source is an AudioBand for the chosen band"
    );
    let bound = mod_eng
        .assignments
        .get("saturation")
        .expect("the chosen target is bound");
    // The default engine already assigns a seeded LFO to saturation; the route
    // adds to it rather than replacing it.
    let route_mods: Vec<_> = bound.iter().filter(|m| m.source_id == "route_0").collect();
    assert_eq!(route_mods.len(), 1, "exactly one assignment for the new route");
    assert_eq!(route_mods[0].amount, 0.5);
}
