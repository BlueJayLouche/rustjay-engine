//! Update visual baselines with `UPDATE_SNAPSHOTS=1 cargo test -p cuepool-gui`.

use cuepool_gui::app::{RELEASE_NOTES_VERSION, torus_colour};
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

/// The parts both presentations draw. `CardPresentation` promises About is the
/// same drawing as the launch splash, so both tests check this same list: one
/// asserts it is present, the other that the credits are not.
const SHARED_CARD_LABELS: [&str; 3] = [
    "Animated CuePool donut",
    "CUEPOOL",
    "AUDIO  /  VIDEO  /  LIGHTING  /  CONTROL",
];

/// What `CardPresentation::Invoked` adds on top of [`SHARED_CARD_LABELS`].
const ABOUT_ONLY_LABELS: [&str; 4] = [
    "A professional audio/video playback application",
    "GitHub",
    "License: GPL-3.0",
    "Close",
];

/// The card's donut is tinted per release series, so check the rendered pixels
/// really carry this build's palette entry. A snapshot cannot do this: every
/// palette entry saturates one channel, so any of them would survive a baseline
/// comparison identically.
///
/// A glyph pixel is `background + coverage * (colour - background)`. The
/// saturated channel makes the largest channel ratio equal `coverage`, which
/// then reconstructs the source colour from the brightest pixel in the rect.
/// Like the recolouring helper it replaces, this assumes a uniform backdrop
/// behind the donut, so it suits the launch presentation and not About.
fn assert_donut_uses_the_build_colour(harness: &mut Harness<'static>) {
    let donut = harness.get_by_label("Animated CuePool donut").rect();
    let image = harness.render().expect("splash should render");
    let x_range = donut.left().floor().max(0.0) as u32..donut.right().ceil() as u32;
    let y_range = donut.top().floor().max(0.0) as u32..donut.bottom().ceil() as u32;
    let background = *image.get_pixel(x_range.start, y_range.start);

    let mut brightest = background;
    let mut brightest_sum = 0_u32;
    for y in y_range {
        for x in x_range.clone() {
            let pixel = *image.get_pixel(x, y);
            let sum = u32::from(pixel[0]) + u32::from(pixel[1]) + u32::from(pixel[2]);
            if sum > brightest_sum {
                brightest_sum = sum;
                brightest = pixel;
            }
        }
    }

    let coverage = (0..3)
        .map(|channel| {
            f32::from(brightest[channel].saturating_sub(background[channel]))
                / f32::from(255 - background[channel])
        })
        .fold(0.0_f32, f32::max);
    assert!(
        coverage > 0.5,
        "expected solidly drawn donut glyphs, got peak coverage {coverage}"
    );

    let expected = torus_colour(
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
    );
    for (channel, expected) in [expected.r(), expected.g(), expected.b()]
        .into_iter()
        .enumerate()
    {
        let background = f32::from(background[channel]);
        let recovered = background + (f32::from(brightest[channel]) - background) / coverage;
        assert!(
            (recovered - f32::from(expected)).abs() <= 8.0,
            "donut channel {channel} rendered as {recovered:.1}, expected {expected}"
        );
    }
}

#[test]
fn launch_card_shows_the_build_then_fades_out() {
    let (mut harness, _state) = app_harness(CuePoolApp::new());

    assert!(harness.query_by_label(&build_identity()).is_some());
    for label in SHARED_CARD_LABELS {
        assert!(
            harness.query_by_label(label).is_some(),
            "the launch card should draw the shared card element {label:?}"
        );
    }
    // The credits belong to About alone; the launch card must stay uncluttered.
    for label in ABOUT_ONLY_LABELS {
        assert!(
            harness.query_by_label(label).is_none(),
            "the launch card should not carry the credit {label:?}"
        );
    }
    harness.remove_cursor();
    harness.step();
    if has_wgpu_adapter() {
        assert_donut_uses_the_build_colour(&mut harness);
    }

    let started_at = 1.0 / 60.0;
    harness.input_mut().time = Some(started_at + 2.29);
    harness.step();
    assert!(harness.query_by_label(&build_identity()).is_some());

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

    // Same drawing as the launch presentation...
    assert!(harness.query_by_label(&build_identity()).is_some());
    for label in SHARED_CARD_LABELS {
        assert!(
            harness.query_by_label(label).is_some(),
            "About should draw the shared card element {label:?}"
        );
    }
    // ...plus the credits only this presentation carries.
    for label in ABOUT_ONLY_LABELS {
        assert!(
            harness.query_by_label(label).is_some(),
            "About should add the credit {label:?}"
        );
    }

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

/// New / Open confirm in-app. A native modal here deadlocks the winit loop and
/// can open behind fullscreen output windows, which reads to the operator as
/// the app soft-locking with no dialog in sight.
#[test]
fn discarding_changes_is_confirmed_in_app_before_a_new_project() {
    let (mut harness, state) = demo_harness();
    {
        let mut state = state.lock().unwrap();
        state.dirty = true;
        state.command_queue.push(AppCommand::NewProject);
    }
    harness.run();

    // Parked behind the modal: the project is untouched until the operator answers.
    assert!(harness.query_by_label("Discard & Continue").is_some());
    assert!(!state.lock().unwrap().show_file.cues.is_empty());

    // Cancelling drops the command and leaves the project alone.
    harness.get_by_label("Cancel").click();
    harness.run();
    assert!(harness.query_by_label("Discard & Continue").is_none());
    assert!(!state.lock().unwrap().show_file.cues.is_empty());

    // Confirming re-queues it, and it runs without asking a second time.
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::NewProject);
    harness.run();
    harness.get_by_label("Discard & Continue").click();
    harness.run();
    assert!(harness.query_by_label("Discard & Continue").is_none());
    assert!(state.lock().unwrap().show_file.cues.is_empty());
}
