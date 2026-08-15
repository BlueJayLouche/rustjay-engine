//! Update visual baselines with `UPDATE_SNAPSHOTS=1 cargo test -p cuepool-gui`.

use cuepool_gui::app::RELEASE_NOTES_VERSION;
use cuepool_gui::{AppCommand, CuePoolApp, SharedStateHandle, ShowMode, build_identity, preview};
use egui::accesskit::Role;
use egui_kittest::{
    Harness, OsThreshold, SnapshotOptions,
    kittest::{By, Queryable},
};
use rust_decimal::Decimal;
use std::time::{Duration, Instant};

fn has_wgpu_adapter() -> bool {
    let instance = egui_wgpu::wgpu::Instance::new(
        egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle(),
    );
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn app_harness(mut app: CuePoolApp) -> (Harness<'static>, SharedStateHandle) {
    let state = app.state().clone();
    let mut harness = Harness::builder()
        .with_size([1280.0, 800.0])
        .with_pixels_per_point(1.0)
        .with_theme(egui::Theme::Dark)
        .build_ui(move |ui| app.update(ui));

    harness.ctx.set_theme(egui::Theme::Dark);
    harness.ctx.global_style_mut(|style| {
        style.interaction.selectable_labels = false;
        style.interaction.multi_widget_text_select = false;
    });
    harness.input_mut().time = Some(harness.ctx.input(|i| i.time));
    harness.step();
    (harness, state)
}

fn demo_harness() -> (Harness<'static>, SharedStateHandle) {
    app_harness(preview::demo_app())
}

#[test]
fn launch_splash_shows_the_build_and_blocks_the_workspace() {
    let (mut harness, state) = app_harness(CuePoolApp::new());

    assert!(harness.query_by_label(&build_identity()).is_some());
    harness.get_by_label("Edit Mode").click();
    harness.key_press(egui::Key::Space);
    harness.step();

    let state = state.lock().unwrap();
    assert_eq!(state.show_mode, ShowMode::Edit);
    assert!(state.command_queue.is_empty());
    drop(state);
    harness.remove_cursor();
    harness.step();
    // Keep the visual baseline stable across patch-version glyph changes. The
    // dynamic label assertion above still verifies the full build identity.
    // GitHub's Linux WGPU backend differs by one additional edge pixel.
    let pixel_threshold = if has_wgpu_adapter() {
        OsThreshold::new(9).linux(10)
    } else {
        OsThreshold::new(9)
    };
    let snapshot_options = SnapshotOptions::new().max_failed_pixels(pixel_threshold);
    harness.snapshot_options("launch_splash", &snapshot_options);

    let started_at = 1.0 / 60.0;
    harness.input_mut().time = Some(started_at + 2.29);
    harness.step();
    assert!(harness.query_by_label(&build_identity()).is_some());
    harness.snapshot_options("launch_splash_fade", &snapshot_options);

    harness.input_mut().time = Some(started_at + 2.5);
    harness.step();
    assert!(harness.query_by_label(&build_identity()).is_none());
}

#[test]
fn release_notes_follow_the_splash_and_are_acknowledged_once() {
    let app = CuePoolApp::new();
    let (mut harness, state) = app_harness(app);

    assert!(harness.query_by_label("GPU-native HAP playback").is_none());
    let started_at = 1.0 / 60.0;
    harness.input_mut().time = Some(started_at + 2.5);
    harness.step();
    assert!(harness.query_by_label("GPU-native HAP playback").is_some());

    harness.key_press(egui::Key::Space);
    harness.step();
    assert!(state.lock().unwrap().command_queue.is_empty());

    harness.remove_cursor();
    harness.step();
    if has_wgpu_adapter() {
        harness.snapshot_options(
            "release_notes",
            &SnapshotOptions::new().max_failed_pixels(OsThreshold::new(0).linux(1)),
        );
    }

    harness.get_by_label("Continue").click();
    harness.run();
    assert_eq!(
        state.lock().unwrap().last_seen_release_notes.as_deref(),
        Some(RELEASE_NOTES_VERSION)
    );
    assert!(harness.query_by_label("GPU-native HAP playback").is_none());
}

#[test]
fn selecting_a_cue_updates_the_inspector() {
    let (mut harness, state) = demo_harness();

    harness
        .get(By::new().role(Role::TextInput).value("Arm Projection"))
        .click();
    harness.run();
    harness.step();

    assert_eq!(
        state.lock().unwrap().selected_cue_id,
        Some(Decimal::from(3))
    );
    assert_eq!(
        harness
            .get_all(By::new().role(Role::TextInput).value("Arm Projection"))
            .count(),
        2,
        "the cue name should appear in both the list and inspector"
    );
}

#[test]
fn mode_button_toggles_show_mode() {
    let (mut harness, state) = demo_harness();

    harness.get_by_label("Edit Mode").click();
    harness.run();

    assert_eq!(state.lock().unwrap().show_mode, ShowMode::Show);
}

#[test]
fn go_button_queues_transport_command() {
    let (mut harness, state) = demo_harness();

    harness.get_by_label("▶ GO").click();
    harness.run();

    let commands: Vec<_> = state.lock().unwrap().command_queue.drain(..).collect();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, AppCommand::Go))
    );
}

#[test]
fn status_window_is_not_embedded_in_control_viewport() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().show_status_window = true;
    harness.run();

    assert!(harness.query_by_label("Copy to Clipboard").is_none());
    assert!(state.lock().unwrap().show_status_window);
}

#[test]
fn operator_alert_is_visible_dismissible_and_expires() {
    let (mut harness, state) = demo_harness();
    const MESSAGE: &str = "Projector 2 failed; 2 of 3 outputs remain active. See Window → Log.";

    state.lock().unwrap().report_operator_error(MESSAGE);
    harness.step();
    assert!(harness.query_by_label(MESSAGE).is_some());

    harness
        .get(By::new().role(Role::Button).label("Dismiss"))
        .click();
    harness.run();
    harness.step();
    assert!(state.lock().unwrap().operator_alert.is_none());
    assert!(harness.query_by_label(MESSAGE).is_none());

    state.lock().unwrap().report_operator_error(MESSAGE);
    state
        .lock()
        .unwrap()
        .operator_alert
        .as_mut()
        .unwrap()
        .expires_at = Instant::now() - Duration::from_millis(1);
    harness.step();
    assert!(state.lock().unwrap().operator_alert.is_none());
    assert!(harness.query_by_label(MESSAGE).is_none());
}

#[test]
fn active_progress_scrubs_only_in_edit_mode() {
    let (mut harness, state) = demo_harness();
    let bar = harness.get_by_label("Scrub active cue Q1.1").rect();
    let drag = |harness: &mut Harness<'static>| {
        let from = egui::pos2(bar.left() + bar.width() * 0.2, bar.center().y);
        let to = egui::pos2(bar.left() + bar.width() * 0.75, bar.center().y);
        harness.drag_at(from);
        harness.step();
        harness.hover_at(to);
        harness.step();
        harness.drop_at(to);
        harness.run();
    };

    drag(&mut harness);
    let commands: Vec<_> = state.lock().unwrap().command_queue.drain(..).collect();
    let Some(AppCommand::SeekCue { instance_id, secs }) = commands
        .iter()
        .rev()
        .find(|command| matches!(command, AppCommand::SeekCue { .. }))
    else {
        panic!("edit-mode drag should queue SeekCue");
    };
    assert_eq!(*instance_id, 1);
    assert!(
        (*secs - 135.0).abs() < 0.01,
        "unexpected seek target: {secs}"
    );

    harness.get_by_label("Edit Mode").click();
    harness.run();
    drag(&mut harness);
    assert!(
        state
            .lock()
            .unwrap()
            .command_queue
            .iter()
            .all(|command| !matches!(command, AppCommand::SeekCue { .. })),
        "show-mode drag must not queue SeekCue"
    );
}

#[test]
fn edit_and_show_mode_snapshots() {
    if !has_wgpu_adapter() {
        eprintln!("skipping UI snapshots: no WGPU adapter available");
        return;
    }
    let (mut harness, _) = demo_harness();
    // ponytail: Linux WGPU differs by one edge pixel; use per-platform
    // baselines if the renderer drift grows beyond that.
    let snapshot_options = SnapshotOptions::new().max_failed_pixels(OsThreshold::new(0).linux(1));

    harness.run();
    harness.snapshot_options("edit_mode", &snapshot_options);

    harness.get_by_label("Edit Mode").click();
    harness.run();
    harness.run();
    harness.snapshot_options("show_mode", &snapshot_options);
}
