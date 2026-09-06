//! Update default visual baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p kovvboj --test ui_kittest`.
//! Update projection baselines with
//! `UPDATE_SNAPSHOTS=1 cargo test -p kovvboj --features projection --test ui_kittest`.

#[cfg(any(feature = "projection", feature = "ffmpeg"))]
use egui::accesskit::Role;
#[cfg(any(feature = "projection", feature = "ffmpeg"))]
use egui_kittest::kittest::By;
use egui_kittest::{Harness, kittest::Queryable};
use rustjay_core::EngineState;
use rustjay_engine::prelude::AnyEguiTab;
#[cfg(feature = "projection")]
use kovvboj::ui::StageTab;
#[cfg(feature = "ffmpeg")]
use kovvboj::ui::EffectsTab;
use kovvboj::{
    KovvbojAppState,
    ui::{DeckTab, OutputsTab},
};

fn tab_harness<T: AnyEguiTab + 'static>(tab: T, size: [f32; 2]) -> Harness<'static> {
    tab_harness_with_app(tab, size, KovvbojAppState::default())
}

fn tab_harness_with_app<T: AnyEguiTab + 'static>(
    mut tab: T,
    size: [f32; 2],
    mut app: KovvbojAppState,
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
fn pad_surface_list_for_full_canvas(app: &mut KovvbojAppState) {
    for index in 1..=4 {
        let mut surface = kovvboj::stage::KovvbojSurface::full_frame(
            format!("Unused {index}"),
            format!("unused{index}"),
        );
        surface.vertices.clear();
        app.stage.surfaces.push(surface);
    }
}

/// A layer source that needs no GPU — enough to give the stack a row to draw.
struct StubSource;

impl rustjay_core::EffectInstance for StubSource {
    fn render_to(
        &mut self,
        _ctx: &mut rustjay_core::RenderCtx<'_>,
        _inputs: &[rustjay_core::EffectInput<'_>],
        _target: rustjay_core::RenderTarget<'_>,
        _engine: &rustjay_core::EngineState,
    ) {
    }
}

/// The centre column is the layer stack now — adding sources moved to the
/// Library panel. A deck itself needs a `wgpu::Device` to build, so this
/// covers the channel row; the strip's own logic is unit-tested below.
///
/// No pixel baseline: every committed snapshot here is rendered by CI on
/// lavapipe, so one generated on a dev machine would fail the moment it landed.
#[test]
fn layer_row_is_drawn() {
    let app = KovvbojAppState::default();
    app.mixer
        .lock()
        .unwrap()
        .add_channel(rustjay_mixer::Channel::new(
            "test",
            "Test",
            Box::new(StubSource),
        ))
        .unwrap();
    let harness = tab_harness_with_app(DeckTab::default(), [700.0, 500.0], app);

    harness.get_by_label("Test");
}

/// On a narrow panel the row-1 controls (S, M, opacity slider, blend, ✖)
/// must squash the opacity slider and ellipsize the layer name rather than
/// slide over the name or out of frame. Regression test: the slider ignores
/// `add_sized` (it always allocates `spacing.slider_width`), so the squash
/// has to go through `spacing_mut()`.
#[test]
fn layer_controls_do_not_cover_the_name_on_narrow_panels() {
    fn app_with_layer() -> KovvbojAppState {
        let app = KovvbojAppState::default();
        app.mixer
            .lock()
            .unwrap()
            .add_channel(rustjay_mixer::Channel::new(
                "test",
                "A rather long layer name",
                Box::new(StubSource),
            ))
            .unwrap();
        app
    }

    for w in [700.0f32, 400.0, 300.0] {
        let harness = tab_harness_with_app(DeckTab::default(), [w, 500.0], app_with_layer());
        // The name appears on the row-1 button and the row-2 strip chip;
        // the row-1 one is drawn first.
        let name = harness
            .get_all(egui_kittest::kittest::By::new().label_contains("A rather long"))
            .next()
            .expect("layer name");
        let solo = harness.get_by_label("S");
        let mute = harness.get_by_label("M");
        assert!(
            solo.rect().min.x >= name.rect().max.x - 1.0,
            "panel {w}: S ({:?}) overlaps the layer name ({:?})",
            solo.rect(),
            name.rect()
        );
        assert!(
            solo.rect().min.x >= 0.0 && mute.rect().min.x >= 0.0,
            "panel {w}: S/M out of frame (S={:?}, M={:?})",
            solo.rect(),
            mute.rect()
        );
    }
}

/// Fifteen stub layers, the last one carrying one FX chip to drag.
fn app_with_layers() -> KovvbojAppState {
    let app = KovvbojAppState::default();
    let mut mixer = app.mixer.lock().unwrap();
    for i in 0..15 {
        let mut ch = rustjay_mixer::Channel::new(
            format!("l{i}"),
            format!("Layer {i}"),
            Box::new(StubSource),
        );
        if i == 14 {
            ch.chain
                .push(rustjay_mixer::EffectSlot::new(Box::new(StubSource)));
        }
        mixer.add_channel(ch).unwrap();
    }
    drop(mixer);
    app
}

/// Dragging an FX chip to the bottom edge of the layer list scrolls it, so
/// off-screen layers stay reachable as drop targets. The scroll offset lives
/// in the shell's `decks_scroll` area, so this drives the real shell rather
/// than the tab alone.
#[test]
fn chip_drag_at_the_edge_scrolls_the_layer_list() {
    use rustjay_engine::prelude::AnyEguiShell;

    let mut app = app_with_layers();
    let mut engine = EngineState::default();
    engine.audio.enabled = false;
    let shared = std::sync::Arc::new(std::sync::Mutex::new(engine));
    let mut host = rustjay_engine::prelude::EguiControlGui::new(shared).unwrap();
    let mut shell = kovvboj::shell::KovvbojShell::new();

    let mut harness = Harness::builder()
        .with_size([900.0, 600.0])
        .with_pixels_per_point(1.0)
        .with_theme(egui::Theme::Dark)
        .build_ui(move |ui| shell.draw(ui, &mut app, &mut host));
    harness.ctx.set_theme(egui::Theme::Dark);
    harness.ctx.global_style_mut(|style| {
        style.interaction.selectable_labels = false;
        style.interaction.multi_widget_text_select = false;
    });
    // The splash animates (requests repaints until dismissed), so this harness
    // steps a fixed frame count instead of running to quiescence.
    harness.run_steps(2);

    // Dismiss the launch splash (any press dismisses it).
    harness.drag_at(egui::pos2(450.0, 300.0));
    harness.drop_at(egui::pos2(450.0, 300.0));
    harness.run_steps(4);

    let before = harness.get_by_label("Layer 0").rect();

    // The list's bottom edge sits just above the MASTER panel; hover a few
    // points above it, inside the scroll margin. ("MASTER" labels more than
    // one node; the bottom panel's is the lowest on screen.)
    let master_top = harness
        .get_all(egui_kittest::kittest::By::new().label("MASTER"))
        .map(|n| n.rect().top())
        .fold(f32::MIN, f32::max);
    let chip = harness.get_by_label("effect");
    let c = chip.rect().center();
    harness.drag_at(c);
    harness.run_steps(2);
    harness.hover_at(egui::pos2(c.x, master_top - 8.0));
    harness.run_steps(8);

    let after = harness.get_by_label("Layer 0").rect();
    assert!(
        after.min.y < before.min.y,
        "dragging to the bottom edge should scroll the list (before={before:?}, after={after:?})"
    );

    harness.drop_at(egui::pos2(c.x, 300.0));
    harness.run_steps(2);
}

/// Dragging an FX chip to the right edge of a layer's strip scrolls the strip
/// horizontally, so drop gaps past the panel's edge stay reachable.
#[test]
fn chip_drag_at_the_strip_edge_scrolls_horizontally() {
    let app = KovvbojAppState::default();
    {
        let mut mixer = app.mixer.lock().unwrap();
        let mut ch = rustjay_mixer::Channel::new("l0", "Layer 0", Box::new(StubSource));
        for _ in 0..8 {
            ch.chain
                .push(rustjay_mixer::EffectSlot::new(Box::new(StubSource)));
        }
        mixer.add_channel(ch).unwrap();
    }
    let mut harness = tab_harness_with_app(DeckTab::default(), [400.0, 500.0], app);

    let chips = |h: &Harness| {
        h.get_all(egui_kittest::kittest::By::new().label("effect"))
            .map(|n| n.rect())
            .collect::<Vec<_>>()
    };
    let before = chips(&harness);
    let last = before.last().expect("eight chips");
    assert!(
        last.min.x > 400.0,
        "the last chip should start off the right edge: {last:?}"
    );

    let c = before[0].center();
    harness.drag_at(c);
    // Auto-scroll requests a repaint every frame, so `run()`'s
    // must-stop-repainting guard doesn't apply while dragging.
    harness.run_steps(2);
    harness.hover_at(egui::pos2(370.0, c.y));
    harness.step();
    // The drop zones widen the content once the drag starts, so compare chip
    // positions across the scroll window only.
    let x0 = chips(&harness)[0].min.x;
    harness.run_steps(7);
    let x1 = chips(&harness)[0].min.x;

    assert!(
        x1 < x0 - 20.0,
        "dragging to the right edge should scroll the strip (before={x0}, after={x1})"
    );

    harness.drop_at(egui::pos2(200.0, c.y));
    harness.run_steps(2);
}

/// Selecting a node must survive a chain reorder, which is why `Selection`
/// addresses nodes by UUID rather than by index.
#[test]
fn selection_identifies_a_node_by_uuid_not_position() {
    use kovvboj::Selection;

    let first = Selection::LayerFx {
        layer: "layer-1".into(),
        fx: "fx-a".into(),
    };
    let same_slot_moved = Selection::LayerFx {
        layer: "layer-1".into(),
        fx: "fx-a".into(),
    };
    let different_fx = Selection::LayerFx {
        layer: "layer-1".into(),
        fx: "fx-b".into(),
    };
    let same_fx_other_layer = Selection::LayerFx {
        layer: "layer-2".into(),
        fx: "fx-a".into(),
    };

    assert_eq!(first, same_slot_moved, "position is not part of identity");
    assert_ne!(first, different_fx);
    assert_ne!(
        first, same_fx_other_layer,
        "an fx uuid alone must not match across layers"
    );
    assert_eq!(Selection::default(), Selection::None);
}

#[cfg(feature = "ffmpeg")]
#[test]
fn deck_stream_paints_invalid_url_error() {
    let app = KovvbojAppState::default();
    app.mixer
        .lock()
        .unwrap()
        .add_channel(rustjay_mixer::Channel::new(
            "test",
            "Test",
            Box::new(kovvboj::sources::testing::StubSource),
        ))
        .unwrap();
    let mut harness = tab_harness_with_app(EffectsTab::default(), [700.0, 500.0], app);

    harness.get_by_label("Add Stream URL").click();
    harness.run();
    harness
        .get_all(By::new().role(Role::TextInput))
        // The library's search box is the first text field in this tab; the
        // stream URL is the next one.
        .nth(1)
        .expect("stream URL field")
        .click();
    harness.run();
    harness
        .get_all(By::new().role(Role::TextInput))
        // The library's search box is the first text field in this tab; the
        // stream URL is the next one.
        .nth(1)
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
    let mut app = KovvbojAppState::default();
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
    let mut app = KovvbojAppState::default();
    pad_surface_list_for_full_canvas(&mut app);
    let mut config = rustjay_projection::EdgeBlendConfig::default();
    config.left.enabled = true;
    config.left.width = 0.22;
    config.top.enabled = true;
    config.top.width = 0.16;
    app.stage.edge_blend_sync = Some(std::sync::Arc::new(std::sync::Mutex::new(
        kovvboj::stage::EdgeBlendSync { config, version: 1 },
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

/// The launch splash and the About box are one drawing shown two ways, so the
/// About-only credits are what tells them apart.
///
/// No pixel baseline, for the reason given on `layer_row_is_drawn`.
#[test]
fn the_splash_grows_credits_only_when_invoked() {
    fn harness_for(presentation: kovvboj::splash::Presentation) -> Harness<'static> {
        let mut harness = Harness::builder()
            .with_size([720.0, 400.0])
            .with_theme(egui::Theme::Dark)
            .build_ui(move |ui| {
                kovvboj::splash::splash(ui, 0.0, presentation);
            });
        harness.run();
        harness
    }

    let launch = harness_for(kovvboj::splash::Presentation::Launch);
    launch.get_by_label("KOVVBOJ");
    assert!(launch.query_by_label("Close").is_none());

    let about = harness_for(kovvboj::splash::Presentation::About);
    about.get_by_label("KOVVBOJ");
    about.get_by_label("Close");
}
