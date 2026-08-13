//! Update visual baselines with `UPDATE_SNAPSHOTS=1 cargo test -p cuepool-gui`.

use cuepool_gui::{AppCommand, SharedStateHandle, ShowMode, preview};
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{By, Queryable},
};
use rust_decimal::Decimal;

fn has_wgpu_adapter() -> bool {
    let instance = egui_wgpu::wgpu::Instance::new(
        egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle(),
    );
    !pollster::block_on(instance.enumerate_adapters(egui_wgpu::wgpu::Backends::all())).is_empty()
}

fn demo_harness() -> (Harness<'static>, SharedStateHandle) {
    let mut app = preview::demo_app();
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
    harness.run();
    (harness, state)
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
    assert!((*secs - 135.0).abs() < 0.01, "unexpected seek target: {secs}");

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

    harness.run();
    harness.snapshot("edit_mode");

    harness.get_by_label("Edit Mode").click();
    harness.run();
    harness.run();
    harness.snapshot("show_mode");
}
