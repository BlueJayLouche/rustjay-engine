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
        .build(move |ctx| gui.build_ui(ctx, &mut app_state));

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
