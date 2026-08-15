//! Update visual baselines with `UPDATE_SNAPSHOTS=1 cargo test -p cuepool-gui`.

use cuepool_gui::app::RELEASE_NOTES_VERSION;
use cuepool_gui::{AppCommand, CuePoolApp, SharedStateHandle, ShowMode, build_identity, preview};
use egui::accesskit::Role;
use egui_kittest::{
    Harness, OsThreshold, SnapshotOptions, image_snapshot_options,
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

// Keep the visual baselines stable across version glyph and accent changes.
// Semantic and unit assertions still verify the exact dynamic values.
// GitHub's Linux WGPU backend differs by one additional edge pixel.
fn card_snapshot_options() -> SnapshotOptions {
    let pixel_threshold = if has_wgpu_adapter() {
        OsThreshold::new(9).linux(10)
    } else {
        OsThreshold::new(9)
    };
    SnapshotOptions::new().max_failed_pixels(pixel_threshold)
}

// Recolours the donut back to the 0.5-series blue so the baseline survives a
// release-series accent change. Only valid where the backdrop behind the donut
// is uniform, which is true of the launch presentation and not of About.
fn snapshot_version_neutral_card(
    harness: &mut Harness<'static>,
    name: &str,
    options: &SnapshotOptions,
) {
    let donut = harness.get_by_label("Animated CuePool donut").rect();
    let mut image = harness.render().expect("splash snapshot should render");
    let x_range = donut.left().floor().max(0.0) as u32..donut.right().ceil() as u32;
    let y_range = donut.top().floor().max(0.0) as u32..donut.bottom().ceil() as u32;
    let background = *image.get_pixel(x_range.start, y_range.start);
    let baseline_colour = [92_u8, 168, 255];

    for y in y_range {
        for x in x_range.clone() {
            let pixel = image.get_pixel_mut(x, y);
            let coverage = (0..3)
                .map(|channel| {
                    f32::from(pixel[channel].saturating_sub(background[channel]))
                        / f32::from(255 - background[channel])
                })
                .fold(0.0_f32, f32::max);
            if coverage > 0.0 {
                for channel in 0..3 {
                    let background = f32::from(background[channel]);
                    pixel[channel] = (background
                        + coverage * (f32::from(baseline_colour[channel]) - background))
                        .round() as u8;
                }
            }
        }
    }

    image_snapshot_options(&image, name, options);
}

#[test]
fn launch_card_shows_the_build_then_fades_out() {
    let (mut harness, _state) = app_harness(CuePoolApp::new());

    assert!(harness.query_by_label(&build_identity()).is_some());
    harness.remove_cursor();
    harness.step();
    let snapshot_options = card_snapshot_options();
    snapshot_version_neutral_card(&mut harness, "launch_splash", &snapshot_options);

    let started_at = 1.0 / 60.0;
    harness.input_mut().time = Some(started_at + 2.29);
    harness.step();
    assert!(harness.query_by_label(&build_identity()).is_some());
    snapshot_version_neutral_card(&mut harness, "launch_splash_fade", &snapshot_options);

    harness.input_mut().time = Some(started_at + 2.5);
    harness.step();
    assert!(harness.query_by_label(&build_identity()).is_none());
}

#[test]
fn launch_card_is_skippable_and_swallows_the_keypress() {
    let (mut harness, state) = app_harness(CuePoolApp::new());

    assert!(harness.query_by_label(&build_identity()).is_some());
    harness.key_press(egui::Key::Space);
    harness.step();

    // The card goes early, but Space must not have reached the show behind it.
    assert!(harness.query_by_label(&build_identity()).is_none());
    let state = state.lock().unwrap();
    assert_eq!(state.show_mode, ShowMode::Edit);
    assert!(state.command_queue.is_empty());
}

#[test]
fn about_reopens_the_same_card_with_credits() {
    let (mut harness, state) = demo_harness();

    assert!(harness.query_by_label(&build_identity()).is_none());
    state.lock().unwrap().show_about_window = true;
    harness.step();

    // Same identity as the launch presentation, plus the credits it adds.
    assert!(harness.query_by_label(&build_identity()).is_some());
    assert!(harness.query_by_label("License: GPL-3.0").is_some());
    harness.remove_cursor();
    harness.step();
    // Snapshotted as rendered, not version-neutralised: that helper assumes a
    // uniform backdrop behind the donut, and About deliberately leaves the
    // workspace visible, so it would rewrite real pixels inside the donut's
    // bounding box. This baseline moves when the release series changes.
    harness.snapshot_options("about_card", &card_snapshot_options());

    harness.get_by_label("Close").click();
    harness.run();
    assert!(!state.lock().unwrap().show_about_window);
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
fn status_bar_tracks_the_active_video_decode_path() {
    let (mut harness, state) = demo_harness();

    // Demo telemetry decodes on the GPU with nothing given up.
    assert!(
        harness
            .query_by_label("Video: hardware (VideoToolbox)")
            .is_some()
    );

    // Losing acceleration mid-show has to reach the status bar, and be marked.
    {
        let mut state = state.lock().unwrap();
        let video = state.diagnostics.video.as_mut().unwrap();
        video.decode_path = "software".into();
        video.fallback_reason = Some("shareable D3D12VA pool rejected".into());
    }
    harness.run();
    assert!(harness.query_by_label("Video: software ⚠").is_some());

    // The ⚠ only flags it — the *reason* has to be one hover away. Tooltips
    // wait out egui's delay and then self-animate, so advance the harness
    // clock and step() rather than run() (which trips its repaint budget).
    harness.get_by_label("Video: software ⚠").hover();
    for _ in 0..12 {
        let time = harness.input().time.unwrap_or_default() + 0.1;
        harness.input_mut().time = Some(time);
        harness.step();
    }
    assert!(
        harness
            .query_by_label_contains("shareable D3D12VA pool rejected")
            .is_some()
    );

    // Playback ending clears the badge rather than leaving a stale claim up.
    harness.remove_cursor();
    state.lock().unwrap().diagnostics.video = None;
    harness.run();
    assert!(harness.query_by_label("Video: idle").is_some());
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
    const MESSAGE: &str = "Projector 2 failed; 2 of 3 outputs remain active. See Window > Log.";

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
