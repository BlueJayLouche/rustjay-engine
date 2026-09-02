//! KOVVBOJ's window shell — menu bar, workspace modes, three-column layout.
//!
//! Replaces the engine's built-in sidebar-and-tabs host (see
//! [`AnyEguiShell`](rustjay_engine::prelude::AnyEguiShell)). The centre column
//! switches between the three full-screen workflows; everything else is either
//! a permanent side panel or a window opened from the View menu.
//!
//! This phase is structural only: each region hosts an existing tab body
//! unmodified. Rebuilding the deck card as a chain strip, and moving params
//! into the inspector, is the next phase — see `KOVVBOJ_UI.md`.

#[cfg(feature = "webcam")]
use crate::ui::LedMapTab;
use crate::ui::{DeckTab, EffectsTab, MixerTab, OutputsTab, SequencerTab, StageTab};
use rustjay_engine::prelude::{AnyEguiShell, AnyEguiTab, EguiControlGui, EngineState, GuiTab};
use rustjay_gui::egui_widgets::{PillState, status_pill};
use std::sync::{Arc, Mutex};

/// Which full-screen workflow the centre column shows.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Mode {
    /// Decks, chains and the master row.
    Mix,
    /// Projection surfaces and warp.
    Stage,
    /// LED pixel mapping.
    Map,
}

impl Mode {
    const ALL: [(Self, &'static str); 3] = [
        (Self::Mix, "MIX"),
        (Self::Stage, "STAGE"),
        (Self::Map, "MAP"),
    ];
}

/// Built-in tabs the View menu can open as windows, in menu order.
///
/// `Settings` is deliberately absent — it lives under Edit. `Sync` is folded
/// into Audio by the host and has no body of its own.
const VIEW_TABS: [GuiTab; 10] = [
    GuiTab::Input,
    GuiTab::Output,
    GuiTab::Color,
    GuiTab::Motion,
    GuiTab::Audio,
    GuiTab::Modulation,
    GuiTab::Midi,
    GuiTab::Osc,
    GuiTab::Web,
    GuiTab::Presets,
];

/// The KOVVBOJ control window.
pub struct KovvbojShell {
    mode: Mode,

    // Region occupants — existing tab bodies, drawn in their new homes.
    library: EffectsTab,
    decks: DeckTab,
    master: MixerTab,
    stage: StageTab,
    #[cfg(feature = "webcam")]
    ledmap: LedMapTab,
    outputs: OutputsTab,
    sequencer: SequencerTab,

    /// Open state per entry in [`VIEW_TABS`].
    builtin_open: [bool; VIEW_TABS.len()],
    show_settings: bool,
    show_outputs: bool,
    show_sequencer: bool,
    show_library: bool,
    show_preview: bool,
}

impl Default for KovvbojShell {
    fn default() -> Self {
        Self::new()
    }
}

impl KovvbojShell {
    /// Build the shell with every window closed and MIX selected.
    pub fn new() -> Self {
        Self {
            mode: Mode::Mix,
            library: EffectsTab::default(),
            decks: DeckTab::default(),
            master: MixerTab::default(),
            stage: StageTab::new(),
            #[cfg(feature = "webcam")]
            ledmap: LedMapTab::new(),
            outputs: OutputsTab::new(),
            sequencer: SequencerTab,
            builtin_open: [false; VIEW_TABS.len()],
            show_settings: false,
            show_outputs: false,
            show_sequencer: false,
            show_library: true,
            show_preview: true,
        }
    }
}

/// Draw one tab body with the engine locked for exactly as long as it takes.
fn tab(
    body: &mut dyn AnyEguiTab,
    ui: &mut egui::Ui,
    app_state: &mut dyn std::any::Any,
    engine: &Arc<Mutex<EngineState>>,
) {
    let mut guard = engine.lock().unwrap_or_else(|e| e.into_inner());
    body.draw(ui, app_state, &mut guard);
}

impl AnyEguiShell for KovvbojShell {
    fn draw(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        host: &mut EguiControlGui,
    ) {
        // Cloned once so nothing below has to keep `host` borrowed to reach the
        // engine — `host` is needed mutably for `draw_builtin_tab`.
        let engine = host.engine().clone();

        self.menu_and_status_row(ui, app_state, &engine);
        self.view_windows(ui, app_state, host, &engine);

        if self.show_library {
            #[allow(deprecated)] // top-level Panel::show, as in the built-in host
            egui::Panel::left("kovvboj_library")
                .default_size(200.0)
                .min_size(140.0)
                .resizable(true)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .show(ui, |ui| tab(&mut self.library, ui, app_state, &engine));
                });
        }

        if self.show_preview {
            #[allow(deprecated)]
            egui::Panel::right("kovvboj_inspector")
                .default_size(300.0)
                .min_size(200.0)
                .resizable(true)
                .show(ui, |ui| {
                    Self::preview(ui, host.output_preview_texture_id, &engine);
                    // The inspector proper lands in the next phase; nothing is
                    // drawn here rather than a "coming soon" placeholder.
                });
        }

        #[allow(deprecated)]
        egui::CentralPanel::default().show(ui, |ui| {
            self.mode_switcher(ui);
            ui.separator();

            match self.mode {
                Mode::Mix => {
                    // MASTER is pinned to the bottom so the crossfader stays
                    // reachable however far the deck list scrolls.
                    #[allow(deprecated)]
                    egui::Panel::bottom("kovvboj_master")
                        .default_size(180.0)
                        .min_size(80.0)
                        .resizable(true)
                        .show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("master_scroll")
                                .show(ui, |ui| tab(&mut self.master, ui, app_state, &engine));
                        });
                    egui::ScrollArea::vertical()
                        .id_salt("decks_scroll")
                        .show(ui, |ui| tab(&mut self.decks, ui, app_state, &engine));
                }
                Mode::Stage => {
                    egui::ScrollArea::vertical()
                        .id_salt("stage_scroll")
                        .show(ui, |ui| tab(&mut self.stage, ui, app_state, &engine));
                }
                Mode::Map => {
                    #[cfg(feature = "webcam")]
                    egui::ScrollArea::vertical()
                        .id_salt("map_scroll")
                        .show(ui, |ui| tab(&mut self.ledmap, ui, app_state, &engine));
                    #[cfg(not(feature = "webcam"))]
                    ui.label("LED mapping needs the `webcam` feature.");
                }
            }
        });
    }
}

impl KovvbojShell {
    /// One 32px row: name, menus, then right-aligned status.
    fn menu_and_status_row(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &Arc<Mutex<EngineState>>,
    ) {
        use rustjay_gui::egui_theme::colors::*;

        // Which optional built-ins have anything to show, so the View menu does
        // not offer empty panels. Mirrors the built-in host's own filter.
        let (has_color, has_motion, fps, bpm, web, osc) = {
            let state = engine.lock().unwrap_or_else(|e| e.into_inner());
            let perf = state
                .performance
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .fps;
            (
                state
                    .param_descriptors
                    .iter()
                    .any(|d| d.category == rustjay_core::ParamCategory::Color),
                state
                    .param_descriptors
                    .iter()
                    .any(|d| d.category == rustjay_core::ParamCategory::Motion),
                perf,
                state.effective_bpm(),
                state.web_enabled,
                state.osc_enabled,
            )
        };

        #[allow(deprecated)]
        egui::Panel::top("kovvboj_menubar")
            .exact_size(32.0)
            .frame(
                egui::Frame::NONE
                    .fill(SURFACE_2)
                    .stroke(egui::Stroke::new(1.0_f32, HAIR_2)),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("KOVVBOJ")
                            .strong()
                            .size(13.0)
                            .color(AMBER)
                            .monospace(),
                    );
                    ui.add_space(10.0);

                    ui.menu_button("File", |ui| {
                        if ui.button("Save Workspace").clicked()
                            && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
                        {
                            state.save_workspace();
                        }
                        ui.separator();
                        if ui.button("Quit").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });

                    ui.menu_button("Edit", |ui| {
                        // Undo/Redo arrive with the structural-edit stack.
                        if ui.button("Settings").clicked() {
                            self.show_settings = true;
                        }
                    });

                    ui.menu_button("View", |ui| {
                        for (i, t) in VIEW_TABS.iter().enumerate() {
                            let usable = match t {
                                GuiTab::Color => has_color,
                                GuiTab::Motion => has_motion,
                                _ => true,
                            };
                            if usable {
                                ui.checkbox(&mut self.builtin_open[i], t.name());
                            }
                        }
                        ui.separator();
                        ui.checkbox(&mut self.show_outputs, "Outputs");
                        ui.checkbox(&mut self.show_sequencer, "Sequencer");
                        ui.separator();
                        ui.checkbox(&mut self.show_library, "Library");
                        ui.checkbox(&mut self.show_preview, "Preview");
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        status_pill(
                            ui,
                            "OSC",
                            if osc {
                                PillState::Online
                            } else {
                                PillState::Neutral
                            },
                        );
                        status_pill(
                            ui,
                            "WEB",
                            if web {
                                PillState::Online
                            } else {
                                PillState::Neutral
                            },
                        );
                        ui.label(
                            egui::RichText::new(format!("{:.0} BPM", bpm))
                                .size(11.0)
                                .monospace()
                                .color(INK_2),
                        );
                        ui.label(
                            egui::RichText::new(format!("{:.0} FPS", fps))
                                .size(11.0)
                                .monospace()
                                .color(INK_2),
                        );
                    });
                });
            });
    }

    /// MIX / STAGE / MAP.
    fn mode_switcher(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (mode, label) in Mode::ALL {
                if ui.selectable_label(self.mode == mode, label).clicked() {
                    self.mode = mode;
                }
            }
        });
    }

    /// Every window the View and Edit menus can open.
    fn view_windows(
        &mut self,
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        host: &mut EguiControlGui,
        engine: &Arc<Mutex<EngineState>>,
    ) {
        let ctx = ui.ctx().clone();

        for (i, t) in VIEW_TABS.iter().enumerate() {
            if !self.builtin_open[i] {
                continue;
            }
            egui::Window::new(t.name())
                .open(&mut self.builtin_open[i])
                .default_width(420.0)
                .vscroll(true)
                .show(&ctx, |ui| host.draw_builtin_tab(ui, *t));
        }

        if self.show_settings {
            egui::Window::new("Settings")
                .open(&mut self.show_settings)
                .default_width(460.0)
                .vscroll(true)
                .show(&ctx, |ui| host.draw_builtin_tab(ui, GuiTab::Settings));
        }

        // App-owned windows. The open flag is copied into a local so the
        // window can borrow it mutably while the body borrows `self`.
        if self.show_outputs {
            let mut open = true;
            egui::Window::new("Outputs")
                .open(&mut open)
                .default_width(520.0)
                .vscroll(true)
                .show(&ctx, |ui| tab(&mut self.outputs, ui, app_state, engine));
            self.show_outputs = open;
        }

        if self.show_sequencer {
            let mut open = true;
            egui::Window::new("Sequencer")
                .open(&mut open)
                .default_width(420.0)
                .vscroll(true)
                .show(&ctx, |ui| tab(&mut self.sequencer, ui, app_state, engine));
            self.show_sequencer = open;
        }
    }

    /// Aspect-fit the master output into the panel width.
    fn preview(
        ui: &mut egui::Ui,
        texture_id: Option<egui::TextureId>,
        engine: &Arc<Mutex<EngineState>>,
    ) {
        let Some(id) = texture_id else {
            return;
        };
        let (w, h) = {
            let state = engine.lock().unwrap_or_else(|e| e.into_inner());
            (state.output_width, state.output_height)
        };
        let aspect = if w > 0 && h > 0 {
            w as f32 / h as f32
        } else {
            16.0 / 9.0
        };
        let width = ui.available_width();
        let size = egui::vec2(width, width / aspect);
        ui.add(egui::Image::new((id, size)).fit_to_exact_size(size));
        ui.separator();
    }
}
