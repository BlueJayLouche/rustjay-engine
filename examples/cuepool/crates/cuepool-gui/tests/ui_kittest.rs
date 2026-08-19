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

    // Key off the version banner, not the headline: the body is rewritten every
    // minor release, and this test guards when the modal shows, not what it says.
    let banner = format!("What's new · {RELEASE_NOTES_VERSION}");

    assert!(harness.query_by_label(&banner).is_none());
    let started_at = 1.0 / 60.0;
    harness.input_mut().time = Some(started_at + 2.5);
    harness.step();
    assert!(harness.query_by_label(&banner).is_some());

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
    assert!(harness.query_by_label(&banner).is_none());
}

#[test]
fn selecting_a_cue_updates_the_inspector() {
    let (mut harness, state) = demo_harness();

    select_row(&mut harness, "Arm Projection");

    assert_eq!(
        state.lock().unwrap().selected_cue_id,
        Some(Decimal::from(3))
    );
    assert_eq!(
        harness
            .get_all(By::new().role(Role::TextInput).value("Arm Projection"))
            .count(),
        1,
        "the inspector's name field follows the selection"
    );
    // The list shows the name as a label until it is double-clicked; `name_cell`
    // panics if there is no such row.
    name_cell(&harness, "Arm Projection");
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

/// The Remote Node field is free text, so each way it can silently misfire has
/// to be visible while the cue is being authored rather than discovered at GO.
#[test]
fn remote_node_field_warns_about_every_silent_misfire() {
    let (mut harness, state) = demo_harness();

    let address_to = |node: &str, remote_on: bool| {
        let mut state = state.lock().unwrap();
        state.show_file.show_settings.enable_remote_control = remote_on;
        state.show_file.show_settings.node_name = "sound-desk".into();
        let cue = state
            .selected_cue_mut()
            .expect("demo show should open with a cue selected");
        cue.base_mut().remote_node = node.into();
        cue.base().qid
    };

    // Remote control on, node never detected: the cue leaves and arrives nowhere.
    address_to("video-rig", true);
    harness.run();
    assert!(
        harness
            .query_by_label("⚠ No node named 'video-rig' detected — the cue will play nowhere")
            .is_some()
    );

    // Remote control off: the cue quietly plays out of this machine instead.
    let qid = address_to("video-rig", false);
    harness.run();
    let off_warning = format!("⚠ Remote Control is off — Q{qid} plays here, not on 'video-rig'");
    assert!(harness.query_by_label(off_warning.as_str()).is_some());

    // Addressed to this machine: legal, but not what "Remote Node" implies.
    address_to("sound-desk", true);
    harness.run();
    assert!(
        harness
            .query_by_label("⚠ 'sound-desk' is this machine — the cue plays here")
            .is_some()
    );

    // Known but gone quiet — the machine is off, or off the network.
    let known = |last_seen| {
        vec![cuepool_core::RemoteNode {
            name: "video-rig".into(),
            address: "10.0.0.9:9000".into(),
            last_seen,
        }]
    };
    state.lock().unwrap().show_file.show_settings.remote_nodes =
        known(Some(Instant::now() - Duration::from_secs(60)));
    address_to("video-rig", true);
    harness.run();
    assert!(
        harness
            .query_by_label(
                "⚠ 'video-rig' has not answered recently — check the machine is running"
            )
            .is_some()
    );

    // A node that is actually on the network draws no warning at all.
    state.lock().unwrap().show_file.show_settings.remote_nodes = known(Some(Instant::now()));
    address_to("video-rig", true);
    harness.run();
    assert!(harness.query(By::new().label_contains("⚠")).is_none());
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

/// Select a cue in the list and retype its number in the inspector's QID field.
///
/// The number shows twice — the cue list row and the inspector — and the
/// inspector panel is declared before the central list, so it comes first in
/// the accessibility tree.
fn retype_inspector_qid(harness: &mut Harness<'static>, name: &str, from: &str, to: &str) {
    select_row(harness, name);

    // The cue list shows the number in a label until it is double-clicked, so
    // the only text field holding it is the inspector's.
    assert_eq!(
        harness
            .get_all(By::new().role(Role::TextInput).value(from))
            .count(),
        1,
        "expected Q{from} in the inspector alone"
    );
    harness
        .get_all(By::new().role(Role::TextInput).value(from))
        .next()
        .expect("inspector QID field")
        .focus();
    harness.run();

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.run();
    harness
        .get_all(By::new().role(Role::TextInput).value(from))
        .next()
        .expect("inspector QID field")
        .type_text(to);
    harness.run();

    // Enter surrenders focus, which is when the edit commits.
    harness.key_press(egui::Key::Enter);
    harness.run();
    harness.step();
}

fn qids(state: &SharedStateHandle) -> Vec<Decimal> {
    state
        .lock()
        .unwrap()
        .show_file
        .cues
        .iter()
        .map(|cue| cue.base().qid)
        .collect()
}

#[test]
fn inspector_refuses_to_renumber_a_cue_onto_an_existing_qid() {
    let (mut harness, state) = demo_harness();
    let before = qids(&state);

    // Q2 is already taken by "House Lights Half".
    retype_inspector_qid(&mut harness, "Arm Projection", "3", "2");

    assert_eq!(
        qids(&state),
        before,
        "a QID collision must be refused, not create two cues sharing a number"
    );
}

#[test]
fn inspector_renumbering_carries_every_reference_to_the_old_qid() {
    let (mut harness, state) = demo_harness();

    // Q1 "Opening Sequence" is the parent of Q1.1/Q1.2/Q1.3 and the target of
    // the Q5 Stop cue.
    retype_inspector_qid(&mut harness, "Opening Sequence", "1", "7");

    let state = state.lock().unwrap();
    let cues = &state.show_file.cues;
    assert_eq!(cues[0].base().qid, Decimal::from(7));
    assert_eq!(state.selected_cue_id, Some(Decimal::from(7)));
    assert!(
        cues[1..4]
            .iter()
            .all(|cue| cue.base().parent == Some(Decimal::from(7))),
        "children must follow their group's new number"
    );
    assert!(
        matches!(
            &cues[7],
            cuepool_core::Cue::Stop { stop_qid, .. } if *stop_qid == Decimal::from(7)
        ),
        "the Stop cue must still point at the group it stops"
    );
}

/// The inspector's Name field for the cue called `name`.
///
/// The name shows twice when its cue is selected, in the inspector and in the
/// cue list row, and the inspector panel is declared first, so it comes first
/// in the accessibility tree (see [`retype_inspector_qid`]). Either field is
/// the same path through the global shortcut handler.
fn name_field<'t>(harness: &'t Harness<'static>, name: &'t str) -> egui_kittest::Node<'t> {
    harness
        .get_all(By::new().role(Role::TextInput).value(name))
        .next()
        .unwrap_or_else(|| panic!("no cue-name field holding {name:?}"))
}

/// A synthesized click. `Node::click()` cannot produce a double click — it moves
/// the pointer before each press, and egui does not pair the two — so the raw
/// events go in directly, with the clock advanced between them.
fn click_at(harness: &mut Harness<'static>, pos: egui::Pos2, at: f64) {
    harness.input_mut().time = Some(at);
    harness
        .input_mut()
        .events
        .push(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.input_mut().events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
    }
    harness.step();
}

/// The cue list's name cell, told from the Active Cues entry of the same name by
/// which panel it sits in: cues are the central column.
fn name_cell(harness: &Harness<'static>, name: &str) -> egui::Rect {
    harness
        .get_all(By::new().label(name))
        .map(|node| node.rect())
        .find(|rect| rect.left() > 240.0 && rect.right() < 870.0)
        .unwrap_or_else(|| panic!("no cue-list row named {name:?}"))
}

/// Select a cue by clicking its row. One click selects and no longer opens an
/// editor, so this is a plain click.
fn select_row(harness: &mut Harness<'static>, name: &str) {
    let pos = name_cell(harness, name).center();
    let now = harness.ctx.input(|i| i.time);
    click_at(harness, pos, now + 0.10);
    harness.run();
    harness.step();
}

/// Open a cue's name for editing. A single click only selects the row; the cells
/// are labels until double-clicked, so the arrow keys keep walking the playhead.
fn open_name_editor(harness: &mut Harness<'static>, name: &str) {
    let pos = name_cell(harness, name).center();
    let now = harness.ctx.input(|i| i.time);
    click_at(harness, pos, now + 0.10);
    click_at(harness, pos, now + 0.18);
    harness.run();
}

/// Renaming a cue must not operate the show. Space used to fire GO on every
/// word break, and Escape — the gesture that closes the editor — stopped
/// everything that was playing.
#[test]
fn typing_into_a_cue_name_does_not_reach_the_show() {
    let (mut harness, state) = demo_harness();

    open_name_editor(&mut harness, "Lobby Ambience");
    assert!(
        harness.ctx.text_edit_focused(),
        "a double click should open the name for editing and put the caret in it"
    );
    state.lock().unwrap().command_queue.clear();

    // A real space is a Key event as well as a Text event; kittest's
    // `type_text` sends only the latter, so press the key itself.
    for key in [
        egui::Key::Space,
        egui::Key::Delete,
        egui::Key::Backspace,
        egui::Key::ArrowUp,
        egui::Key::ArrowDown,
        egui::Key::Home,
        egui::Key::End,
    ] {
        harness.key_press(key);
        harness.step();
        assert!(
            state.lock().unwrap().command_queue.is_empty(),
            "{key:?} while renaming leaked into the show"
        );
    }

    // The field runs its own undo, so Cmd+Z belongs to it, and Cmd+arrows are
    // start/end-of-text on macOS rather than "move the cue".
    for key in [egui::Key::Z, egui::Key::ArrowUp, egui::Key::ArrowDown] {
        harness.key_press_modifiers(egui::Modifiers::COMMAND, key);
        harness.step();
        assert!(
            state.lock().unwrap().command_queue.is_empty(),
            "Cmd+{key:?} while renaming leaked into the show"
        );
    }

    // Escape closes the editor. egui has already dropped focus by the time the
    // shortcut handler sees this frame, which is why it needs a frame's memory.
    harness.key_press(egui::Key::Escape);
    harness.step();
    assert!(
        state.lock().unwrap().command_queue.is_empty(),
        "Escape cancelling a rename stopped the show"
    );
    assert!(!harness.ctx.text_edit_focused());

    // ...and the very next Escape, with the editor closed, does stop it.
    harness.key_press(egui::Key::Escape);
    harness.step();
    let commands: Vec<_> = state.lock().unwrap().command_queue.drain(..).collect();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, AppCommand::Stop)),
        "Escape outside a field should stop the show, got {commands:?}"
    );
}

/// Save has to keep working mid-rename: nothing a text caret does collides
/// with it, and losing it is how an operator loses a cue sheet.
#[test]
fn menu_shortcuts_stay_live_while_renaming() {
    let (mut harness, state) = demo_harness();

    // Give the project a path, or Save falls through to a native Save As
    // dialog, which cannot open off the main thread under a test harness.
    let path = std::env::temp_dir().join("cuepool_shortcut_while_renaming.qproj");
    let _ = std::fs::remove_file(&path);
    state.lock().unwrap().project_path = Some(path.clone());

    open_name_editor(&mut harness, "Lobby Ambience");

    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    harness.step();
    assert!(
        path.exists(),
        "Cmd+S should still save while a field is focused"
    );
    let _ = std::fs::remove_file(&path);
}

/// The other half of the gate: with no field focused, Space is still GO. The
/// cue list draws plain labels in Show mode, so nothing holds focus there.
#[test]
fn space_still_fires_go_in_show_mode() {
    let (mut harness, state) = demo_harness();

    harness.get_by_label("Edit Mode").click();
    harness.run();
    assert_eq!(state.lock().unwrap().show_mode, ShowMode::Show);

    harness
        .get(By::new().role(Role::Label).value("Q1.1  Lobby Ambience"))
        .click();
    harness.run();
    state.lock().unwrap().command_queue.clear();

    harness.key_press(egui::Key::Space);
    harness.step();
    let commands: Vec<_> = state.lock().unwrap().command_queue.drain(..).collect();
    assert!(
        commands
            .iter()
            .any(|command| matches!(command, AppCommand::Go)),
        "Space should fire GO with no field focused, got {commands:?}"
    );
}

/// Show mode hides the editing widgets, but the shortcuts and the right-click
/// menu raise the same commands regardless — so the lock lives in the handler.
#[test]
fn show_mode_refuses_cue_edits_however_they_are_raised() {
    let (mut harness, state) = demo_harness();
    {
        let mut state = state.lock().unwrap();
        state.show_mode = ShowMode::Show;
        state.selected_cue_id = Some(Decimal::new(11, 1));
    }
    let before = qids(&state);

    // The bare Delete key reached DeleteSelectedCue with nothing in its way.
    harness.key_press(egui::Key::Backspace);
    harness.run();
    assert_eq!(
        qids(&state),
        before,
        "Backspace must not delete in Show mode"
    );

    for command in [
        AppCommand::AddCue {
            cue_type: cuepool_gui::app::CueType::Sound,
        },
        AppCommand::DeleteCue { qid: None },
        AppCommand::DuplicateCue { qid: None },
        AppCommand::MoveCueUp { qid: None },
        AppCommand::UpdateCueName {
            qid: Decimal::new(11, 1),
            name: "Renamed mid-show".into(),
        },
    ] {
        state.lock().unwrap().command_queue.push(command.clone());
        harness.run();
        assert_eq!(
            qids(&state),
            before,
            "{command:?} must be refused in Show mode"
        );
    }
    assert_eq!(
        state.lock().unwrap().show_file.cues[1].base().name,
        "Lobby Ambience",
        "a refused rename must not reach the model either"
    );

    // Back in Edit mode the same command lands, so the guard is the mode and not
    // something incidental about the harness.
    state.lock().unwrap().show_mode = ShowMode::Edit;
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::DeleteCue { qid: None });
    harness.run();
    assert_ne!(qids(&state), before, "Edit mode still deletes");
}

/// Snapshots hold the state *before* an edit, so a merged run has to keep its
/// first. Keeping the last made one Cmd+Z after typing a name give the name back
/// minus its final keystroke, with the rest of the run unreachable either way.
#[test]
fn a_run_of_merged_edits_undoes_as_one_step() {
    let (mut harness, state) = demo_harness();
    let qid = Decimal::new(11, 1);
    state.lock().unwrap().undo_redo = Default::default();

    // The queue window's Name cell commits per keystroke by design.
    for name in ["L", "Lo", "Lob", "Lobb", "Lobby"] {
        state
            .lock()
            .unwrap()
            .command_queue
            .push(AppCommand::UpdateCueName {
                qid,
                name: name.into(),
            });
        harness.run();
    }
    assert_eq!(state.lock().unwrap().show_file.cues[1].base().name, "Lobby");

    state.lock().unwrap().command_queue.push(AppCommand::Undo);
    harness.run();
    assert_eq!(
        state.lock().unwrap().show_file.cues[1].base().name,
        "Lobby Ambience",
        "one undo should rewind the whole typing run, not one keystroke"
    );
}

/// Moving the standby playhead is not an edit. It used to push an undo entry per
/// mouse click, which buried real edits under a 50-deep stack of selections and
/// threw away redo — a push clears it — just for clicking a cue.
#[test]
fn selecting_a_cue_is_not_an_undo_step() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().undo_redo = Default::default();

    for qid in [Decimal::ONE, Decimal::new(12, 1), Decimal::new(13, 1)] {
        state
            .lock()
            .unwrap()
            .command_queue
            .push(AppCommand::SelectCue(qid));
        harness.run();
    }
    assert!(
        !state.lock().unwrap().undo_redo.can_undo(),
        "selections must not accumulate undo entries"
    );
    // Keyboard navigation takes the same path.
    harness.key_press(egui::Key::ArrowUp);
    harness.run();
    assert!(
        !state.lock().unwrap().undo_redo.can_undo(),
        "arrow-key navigation must not accumulate undo entries either"
    );

    // And redo survives a selection change.
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::UpdateCueName {
            qid: Decimal::ONE,
            name: "Changed".into(),
        });
    harness.run();
    state.lock().unwrap().command_queue.push(AppCommand::Undo);
    harness.run();
    assert!(state.lock().unwrap().undo_redo.can_redo());
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::SelectCue(Decimal::new(12, 1)));
    harness.run();
    assert!(
        state.lock().unwrap().undo_redo.can_redo(),
        "selecting a cue must not discard the redo stack"
    );
}

/// The mode is an operator stance, not project content: it is never saved, so it
/// has no business in a snapshot or in the dirty flag.
#[test]
fn show_mode_is_neither_dirtying_nor_undoable() {
    let (mut harness, state) = demo_harness();
    {
        let mut state = state.lock().unwrap();
        state.dirty = false;
        state.undo_redo = Default::default();
    }

    harness.get_by_label("Edit Mode").click(); // into Show mode
    harness.run();
    {
        let state = state.lock().unwrap();
        assert_eq!(state.show_mode, ShowMode::Show);
        assert!(
            !state.dirty,
            "a mode switch must not mark the project unsaved"
        );
        assert!(
            !state.undo_redo.can_undo(),
            "a mode switch must not take an undo entry"
        );
    }

    // An edit made before the show, then undone during it: the mode must hold,
    // and Show mode refuses the undo outright.
    harness.get_by_label("Show Mode").click(); // back to Edit
    harness.run();
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::UpdateCueName {
            qid: Decimal::ONE,
            name: "Edited before the show".into(),
        });
    harness.run();
    harness.get_by_label("Edit Mode").click(); // into Show mode
    harness.run();

    state.lock().unwrap().command_queue.push(AppCommand::Undo);
    harness.run();
    let state = state.lock().unwrap();
    assert_eq!(
        state.show_mode,
        ShowMode::Show,
        "undo must not drop the operator out of Show mode"
    );
    assert_eq!(
        state.show_file.cues[0].base().name,
        "Edited before the show",
        "Show mode refuses the undo, so the edit stands"
    );
}

/// `choose_qid` numbers a new cue *after* the selection, but the cue was appended
/// to the end of the show — so the list read Q1, Q2, Q3, Q1.11 and the number
/// stopped meaning anything. Position, group and number now share one anchor.
#[test]
fn a_new_cue_lands_where_its_number_says_and_is_selected() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().selected_cue_id = Some(Decimal::new(11, 1));
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::AddCue {
            cue_type: cuepool_gui::app::CueType::Sound,
        });
    harness.run();

    let state = state.lock().unwrap();
    let added = state.show_file.cues[2].base();
    assert_eq!(
        qids_of(&state.show_file.cues),
        vec![
            Decimal::ONE,
            Decimal::new(11, 1),
            Decimal::new(111, 2),
            Decimal::new(12, 1),
            Decimal::new(13, 1),
            Decimal::from(2),
            Decimal::from(3),
            Decimal::from(4),
            Decimal::from(5),
        ],
        "the new cue belongs directly after the cue it was numbered against"
    );
    assert_eq!(
        added.parent,
        Some(Decimal::ONE),
        "adding beside a group member joins that group"
    );
    assert_eq!(
        state.selected_cue_id,
        Some(added.qid),
        "the new cue should be selected, not left to be hunted for"
    );
}

/// A group is drawn as a block and fires as a block, so it copies and deletes as
/// one. Copying the header alone produced something that looked like a group and
/// fired nothing; deleting it alone orphaned every member.
#[test]
fn a_group_duplicates_and_deletes_whole() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().selected_cue_id = Some(Decimal::ONE);
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::DuplicateCue { qid: None });
    harness.run();

    {
        let state = state.lock().unwrap();
        assert_eq!(state.show_file.cues.len(), 12, "header plus three members");
        let copied_header = state.show_file.cues[4].base().qid;
        assert_eq!(state.selected_cue_id, Some(copied_header));
        assert!(state.show_file.cues[4].base().name.ends_with(" (copy)"));
        for offset in 5..8 {
            assert_eq!(
                state.show_file.cues[offset].base().parent,
                Some(copied_header),
                "a copied member must follow the copied header, not the original"
            );
        }
        // Every number is still unique, which is what makes delete safe.
        let numbers = qids_of(&state.show_file.cues);
        let unique: std::collections::HashSet<_> = numbers.iter().collect();
        assert_eq!(
            unique.len(),
            numbers.len(),
            "duplicate QIDs must be unreachable"
        );
    }

    // Now delete the original group: it takes its members and moves the standby.
    state.lock().unwrap().selected_cue_id = Some(Decimal::ONE);
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::DeleteCue { qid: None });
    harness.run();

    let state = state.lock().unwrap();
    assert_eq!(
        state.show_file.cues.len(),
        8,
        "four cues went with the group"
    );
    assert!(
        !state
            .show_file
            .cues
            .iter()
            .any(|cue| cue.base().parent == Some(Decimal::ONE)),
        "no member may outlive its group"
    );
    assert!(
        state.selected_cue_id.is_some(),
        "deleting must move the standby on, not clear it"
    );
}

/// Nudging used to be a bare `swap`, which moved a group header off its members
/// and stepped into the middle of a neighbouring group.
#[test]
fn nudging_a_group_moves_it_as_a_block() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().selected_cue_id = Some(Decimal::ONE);
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::MoveCueDown { qid: None });
    harness.run();

    let state = state.lock().unwrap();
    assert_eq!(
        qids_of(&state.show_file.cues),
        vec![
            Decimal::from(2),
            Decimal::ONE,
            Decimal::new(11, 1),
            Decimal::new(12, 1),
            Decimal::new(13, 1),
            Decimal::from(3),
            Decimal::from(4),
            Decimal::from(5),
        ],
        "the group and its members move together, over the whole neighbour"
    );
}

/// Dropping a cue on the group header directly above it changes no position, only
/// the parent — and the old "did the index change?" guard threw that away, so the
/// commonest way to put the first cue into a group did nothing at all.
#[test]
fn dropping_a_cue_on_the_group_directly_above_joins_it() {
    let (mut harness, state) = demo_harness();
    // Free Q1.1 so joining is observable.
    state.lock().unwrap().show_file.cues[1].base_mut().parent = None;
    // The cue list computes `to_idx = group_idx + 1` for a drop on a group row.
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::MoveCue {
            from_idx: 1,
            to_idx: 1,
            parent: Some(Decimal::ONE),
        });
    harness.run();

    let state = state.lock().unwrap();
    assert_eq!(
        state.show_file.cues[1].base().parent,
        Some(Decimal::ONE),
        "the drop must join the group even though nothing moved"
    );
    assert_eq!(
        state.show_file.cues[1].base().qid,
        Decimal::new(11, 1),
        "and must not shuffle the list to do it"
    );
}

fn qids_of(cues: &[cuepool_core::Cue]) -> Vec<Decimal> {
    cues.iter().map(|cue| cue.base().qid).collect()
}

/// A duplicate inherits its original's alternate triggers, so both cues answer to
/// the same key or note. Clearing them silently would be worse — a trigger the
/// operator wanted and lost is a cue that does not fire during the show — so the
/// copy keeps them and the operator is told.
#[test]
fn duplicating_a_triggered_cue_warns_that_the_trigger_is_shared() {
    let (mut harness, state) = demo_harness();
    {
        let mut state = state.lock().unwrap();
        state.show_file.cues[4].base_mut().triggers.hotkey =
            Some(cuepool_core::HotkeyTrigger { key: "G".into() });
        state.selected_cue_id = Some(Decimal::from(2));
    }
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::DuplicateCue { qid: None });
    harness.run();

    let state = state.lock().unwrap();
    assert!(
        state.show_file.cues[5].base().triggers.hotkey.is_some(),
        "the copy keeps the trigger"
    );
    assert!(
        state
            .operator_alert
            .as_ref()
            .is_some_and(|alert| alert.message.contains("hotkey")),
        "and the operator is told both cues now fire on it"
    );
}

/// Dropping a group on the trailing "move to end" strip must reach the end, not
/// snap back to the boundary of whatever group happens to sit last.
#[test]
fn a_group_dragged_to_the_end_lands_at_the_end() {
    let (mut harness, state) = demo_harness();
    let len = state.lock().unwrap().show_file.cues.len();
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::MoveCue {
            from_idx: 0,
            to_idx: len,
            parent: None,
        });
    harness.run();

    let state = state.lock().unwrap();
    assert_eq!(
        qids_of(&state.show_file.cues),
        vec![
            Decimal::from(2),
            Decimal::from(3),
            Decimal::from(4),
            Decimal::from(5),
            Decimal::ONE,
            Decimal::new(11, 1),
            Decimal::new(12, 1),
            Decimal::new(13, 1),
        ],
        "the group and its members should end up last, still contiguous"
    );
}

/// The QID field reverts to the old number on its own when a rename is refused,
/// which without a word on screen reads as the edit never having registered.
#[test]
fn a_refused_renumber_is_reported_to_the_operator() {
    let (mut harness, state) = demo_harness();
    let before = qids(&state);
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::UpdateCueQid {
            qid: Decimal::new(11, 1),
            new_qid: Decimal::new(12, 1), // taken by the Video cue
        });
    harness.run();

    let state = state.lock().unwrap();
    assert_eq!(qids_of(&state.show_file.cues), before);
    assert!(
        state
            .operator_alert
            .as_ref()
            .is_some_and(|alert| alert.message.contains("already in use")),
        "a refused renumber must say so, not only log it"
    );
}

/// The live push is the whole feature: without the queued command the slider
/// still edits the cue and the playing cue never hears it. Covers Video too,
/// since both cue types share `level_editor`.
#[test]
fn level_edits_queue_a_live_push_for_sound_and_video() {
    let (mut harness, state) = demo_harness();

    for (qid, expected_volume) in [(Decimal::new(11, 1), 0.8_f32), (Decimal::new(12, 1), 0.7)] {
        state.lock().unwrap().selected_cue_id = Some(qid);
        harness.run();
        state.lock().unwrap().command_queue.clear();

        // Drag the volume slider a little way left of where it sits.
        let slider = harness.get_by_label("Volume (dB):").rect();
        harness.drag_at(slider.center());
        harness.step();
        let to = egui::pos2(slider.center().x - slider.width() * 0.2, slider.center().y);
        harness.hover_at(to);
        harness.step();
        harness.drop_at(to);
        harness.run();

        let commands: Vec<_> = state.lock().unwrap().command_queue.drain(..).collect();
        let Some(AppCommand::SetCueLevel {
            qid: pushed_qid,
            volume,
            ..
        }) = commands
            .iter()
            .rev()
            .find(|command| matches!(command, AppCommand::SetCueLevel { .. }))
        else {
            panic!("dragging Volume on Q{qid} should queue SetCueLevel");
        };
        assert_eq!(*pushed_qid, qid);
        assert!(
            *volume < expected_volume,
            "Q{qid} dragged left should push a quieter level than {expected_volume}, got {volume}"
        );
        // The cue itself is edited as before, so the change survives a re-fire.
        let stored = match state
            .lock()
            .unwrap()
            .show_file
            .cues
            .iter()
            .find(|cue| cue.base().qid == qid)
        {
            Some(
                cuepool_core::Cue::Sound { volume, .. } | cuepool_core::Cue::Video { volume, .. },
            ) => *volume,
            other => panic!("expected a Sound or Video cue at Q{qid}, got {other:?}"),
        };
        assert!(
            (stored - *volume).abs() < f32::EPSILON,
            "pushed level {volume} should match the cue's stored {stored}"
        );
    }
}
/// "Pause at the moment, add cue" is the case the pre-filled timecode trigger
/// exists for. Applying it from a running clock armed every cue added mid-show
/// with a trigger nobody asked for, showing only as a 10px badge.
#[test]
fn a_pre_filled_timecode_trigger_needs_a_paused_clock() {
    let (mut harness, state) = demo_harness();

    for (paused, expected) in [(false, None), (true, Some(42.5))] {
        {
            let mut state = state.lock().unwrap();
            state.show_time = Some(42.5);
            state.show_paused = paused;
            state.selected_cue_id = None;
        }
        state
            .lock()
            .unwrap()
            .command_queue
            .push(AppCommand::AddCue {
                cue_type: cuepool_gui::app::CueType::Stop,
            });
        harness.run();

        let state = state.lock().unwrap();
        let added = state.show_file.cues.last().expect("the added cue").base();
        assert_eq!(
            added
                .triggers
                .timecode
                .as_ref()
                .map(|t| t.time.as_secs_f64()),
            expected,
            "clock paused={paused}"
        );
    }
}

/// The only way to leave a group was the trailing strip, which also moved the cue
/// to the end of the show. Clearing `parent` in place is not enough on its own: a
/// group's span stops at the first row that is not a member, so the cues after
/// this one would fall outside it while still pointing at it.
#[test]
fn ungrouping_moves_the_cue_clear_of_its_group() {
    let (mut harness, state) = demo_harness();
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::UngroupCue {
            qid: Decimal::new(11, 1),
        });
    harness.run();

    let state = state.lock().unwrap();
    assert_eq!(
        qids_of(&state.show_file.cues),
        vec![
            Decimal::ONE,
            Decimal::new(12, 1),
            Decimal::new(13, 1),
            Decimal::new(11, 1),
            Decimal::from(2),
            Decimal::from(3),
            Decimal::from(4),
            Decimal::from(5),
        ],
        "the freed cue steps past the group, not to the end of the show"
    );
    assert_eq!(state.show_file.cues[3].base().parent, None);
    assert!(
        state.show_file.cues[1..3]
            .iter()
            .all(|cue| cue.base().parent == Some(Decimal::ONE)),
        "the members left behind stay contiguous with their header"
    );
}

/// The row menu labels its items "Delete Q3" but used to issue selection-scoped
/// commands. They agreed only because right-click selects first, and two menus
/// can be open at once.
#[test]
fn a_cue_command_naming_its_target_ignores_the_selection() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().selected_cue_id = Some(Decimal::from(2));
    state
        .lock()
        .unwrap()
        .command_queue
        .push(AppCommand::DeleteCue {
            qid: Some(Decimal::new(11, 1)),
        });
    harness.run();

    let state = state.lock().unwrap();
    assert!(
        !qids_of(&state.show_file.cues).contains(&Decimal::new(11, 1)),
        "the named cue goes"
    );
    assert!(
        qids_of(&state.show_file.cues).contains(&Decimal::from(2)),
        "the selected one stays"
    );
}

/// The Q# and Name cells were live text fields, so one click on a row put a caret
/// in one and the arrow keys then walked the caret rather than the standby
/// playhead, with nothing on screen to say why.
#[test]
fn a_single_click_selects_without_opening_the_editor() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().selected_cue_id = None;
    harness.run();

    let pos = name_cell(&harness, "Lobby Ambience").center();
    let now = harness.ctx.input(|i| i.time);
    click_at(&mut harness, pos, now + 0.10);
    harness.run();

    assert_eq!(
        state.lock().unwrap().selected_cue_id,
        Some(Decimal::new(11, 1)),
        "a click on the name still selects the row"
    );
    assert!(
        !harness.ctx.text_edit_focused(),
        "but it must not open the editor"
    );

    // So the arrows still belong to the list.
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    assert_eq!(
        state.lock().unwrap().selected_cue_id,
        Some(Decimal::new(12, 1)),
        "the arrow keys should still walk the playhead after a click"
    );
}

/// Double click is the mouse route into the editor; Enter is the keyboard one,
/// without which a keyboard-only operator could no longer rename a cue at all.
#[test]
fn enter_opens_the_selected_cue_for_renaming() {
    let (mut harness, state) = demo_harness();
    state.lock().unwrap().selected_cue_id = Some(Decimal::new(11, 1));
    harness.run();
    assert!(!harness.ctx.text_edit_focused());

    harness.key_press(egui::Key::Enter);
    harness.run();
    assert!(
        harness.ctx.text_edit_focused(),
        "Enter should open the selected cue's name"
    );
    assert_eq!(
        name_field(&harness, "Lobby Ambience").value().as_deref(),
        Some("Lobby Ambience")
    );

    // Escape closes it again and hands the keys back to the list.
    harness.key_press(egui::Key::Escape);
    harness.run();
    assert!(!harness.ctx.text_edit_focused());
    harness.key_press(egui::Key::ArrowDown);
    harness.run();
    assert_eq!(
        state.lock().unwrap().selected_cue_id,
        Some(Decimal::new(12, 1))
    );
}
