//! Update default visual baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p vjarda --test ui_kittest`.
//! Update projection baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p vjarda --features projection --test ui_kittest`.

#[cfg(feature = "projection")]
use egui::accesskit::Role;
#[cfg(feature = "projection")]
use egui_kittest::kittest::By;
use egui_kittest::{Harness, kittest::Queryable};
use rustjay_core::EngineState;
use rustjay_engine::prelude::AnyEguiTab;
#[cfg(feature = "projection")]
use vjarda::ui::StageTab;
use vjarda::{
    VardaAppState,
    ui::{DeckTab, OutputsTab},
};

fn tab_harness<T: AnyEguiTab + 'static>(mut tab: T, size: [f32; 2]) -> Harness<'static> {
    let mut app = VardaAppState::default();
    let mut engine = EngineState::default();
    assert!(engine.stage_preview_texture_id.is_none());

    let mut harness = Harness::builder()
        .with_size(size)
        .with_pixels_per_point(1.0)
        .with_theme(egui::Theme::Dark)
        .build_ui(move |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                tab.draw(ui, &mut app, &mut engine);
            });
        });

    harness.ctx.set_theme(egui::Theme::Dark);
    harness.ctx.global_style_mut(|style| {
        style.interaction.selectable_labels = false;
        style.interaction.multi_widget_text_select = false;
    });
    harness.run();
    harness
}

#[test]
fn deck_add_source_snapshot() {
    let mut harness = tab_harness(DeckTab::default(), [700.0, 500.0]);

    harness.get_by_label("Add Source");
    harness.get_by_label("📁 File");
    harness.get_by_label("📷 Camera / V4L2");
    harness.snapshot("deck_add_source");
}

#[cfg(not(feature = "projection"))]
#[test]
fn default_outputs_recording_snapshot() {
    let mut harness = tab_harness(OutputsTab::default(), [700.0, 400.0]);

    harness.get_by_label("Recording");
    harness.get_by_label("Browse…");
    harness.snapshot("outputs_recording");
}

#[cfg(feature = "projection")]
#[test]
fn stage_without_preview_texture_snapshot() {
    let mut harness = tab_harness(StageTab::default(), [900.0, 700.0]);

    harness.get_by_label("Stage");
    harness.snapshot("stage_without_preview");
}

#[cfg(feature = "projection")]
#[test]
fn outputs_projector_panel_snapshot() {
    let mut harness = tab_harness(OutputsTab::default(), [1200.0, 700.0]);

    harness.get_by_label("Projectors");
    harness.get(By::new().role(Role::TextInput).value("Projector"));
    harness.snapshot("outputs_projector_panel");
}
