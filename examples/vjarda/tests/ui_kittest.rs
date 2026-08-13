//! Update default visual baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p vjarda --test ui_kittest`.
//! Update projection baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p vjarda --features projection --test ui_kittest`.

#[cfg(any(feature = "projection", feature = "ffmpeg"))]
use egui::accesskit::Role;
#[cfg(any(feature = "projection", feature = "ffmpeg"))]
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

fn tab_harness<T: AnyEguiTab + 'static>(tab: T, size: [f32; 2]) -> Harness<'static> {
    tab_harness_with_app(tab, size, VardaAppState::default())
}

fn tab_harness_with_app<T: AnyEguiTab + 'static>(
    mut tab: T,
    size: [f32; 2],
    mut app: VardaAppState,
) -> Harness<'static> {
    let mut engine = EngineState::default();
    assert!(engine.stage_preview_texture_id.is_none());
    assert!(engine.show_stage_preview);

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

#[cfg(feature = "projection")]
fn pad_surface_list_for_full_canvas(app: &mut VardaAppState) {
    for index in 1..=4 {
        let mut surface = vjarda::stage::VardaSurface::full_frame(
            format!("Unused {index}"),
            format!("unused{index}"),
        );
        surface.vertices.clear();
        app.stage.surfaces.push(surface);
    }
}

#[test]
fn deck_add_source_snapshot() {
    let mut harness = tab_harness(DeckTab::default(), [700.0, 500.0]);

    harness.get_by_label("Add Source");
    harness.get_by_label("📁 File");
    harness.get_by_label("📡 Stream");
    harness.get_by_label("📷 Camera / V4L2");
    harness.snapshot("deck_add_source");
}

#[cfg(feature = "ffmpeg")]
#[test]
fn deck_stream_paints_invalid_url_error() {
    let app = VardaAppState::default();
    app.mixer
        .lock()
        .unwrap()
        .add_channel(rustjay_mixer::Channel::new(
            "test",
            "Test",
            Box::new(vjarda::graph::DeckCompositor::new()),
        ))
        .unwrap();
    let mut harness = tab_harness_with_app(DeckTab::default(), [700.0, 500.0], app);

    harness.get_by_label("📡 Stream").click();
    harness.run();
    harness
        .get_all(By::new().role(Role::TextInput))
        .next()
        .expect("stream URL field")
        .click();
    harness.run();
    harness
        .get_all(By::new().role(Role::TextInput))
        .next()
        .expect("stream URL field")
        .type_text("ftp://stream.example/live");
    harness.run();

    harness.get_by_label("Unsupported stream URL scheme.");
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
fn stage_preview_disabled_snapshot() {
    let mut harness = tab_harness(StageTab::default(), [900.0, 700.0]);

    harness.get_by_label("Live preview").click();
    harness.run();
    harness.get_by_label("Stage");
    harness.get_by_label("Canvas: 1920×1080 px");
    harness.get_by_label("Edit mode");
    harness.snapshot("stage_without_preview");
}

#[cfg(feature = "projection")]
#[test]
fn stage_corner_pin_warp_snapshot() {
    let mut app = VardaAppState::default();
    pad_surface_list_for_full_canvas(&mut app);
    app.stage.surfaces[0].warp = rustjay_projection::WarpMode::corner_pin([
        [0.08, 0.14],
        [0.92, 0.08],
        [0.86, 0.88],
        [0.14, 0.94],
    ]);
    let mut harness = tab_harness_with_app(StageTab::default(), [900.0, 700.0], app);

    harness.get_by_label("Stage");
    harness.snapshot("stage_corner_pin_warp");
}

#[cfg(feature = "projection")]
#[test]
fn stage_edge_blend_preview_snapshot() {
    let mut app = VardaAppState::default();
    pad_surface_list_for_full_canvas(&mut app);
    let mut config = rustjay_projection::EdgeBlendConfig::default();
    config.left.enabled = true;
    config.left.width = 0.22;
    config.top.enabled = true;
    config.top.width = 0.16;
    app.stage.edge_blend_sync = Some(std::sync::Arc::new(std::sync::Mutex::new(
        vjarda::stage::EdgeBlendSync { config, version: 1 },
    )));
    let mut harness = tab_harness_with_app(StageTab::default(), [900.0, 700.0], app);

    harness.get_by_label("Stage");
    harness.snapshot("stage_edge_blend_preview");
}

#[cfg(feature = "projection")]
#[test]
fn outputs_projector_panel_snapshot() {
    let mut harness = tab_harness(OutputsTab::default(), [1200.0, 700.0]);

    harness.get_by_label("Projectors");
    harness.get(By::new().role(Role::TextInput).value("Projector"));
    harness.get_by_label("Fullscreen");
    harness.snapshot("outputs_projector_panel");
}
