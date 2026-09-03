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
use crate::splash::{LAUNCH_HOLD, Presentation, backdrop_opacity, launch_opacity, splash};
use crate::ui::{DeckTab, EffectsTab, MixerTab, OutputsTab, SequencerTab, StageTab};
use rustjay_engine::prelude::{AnyEguiShell, AnyEguiTab, EguiControlGui, EngineState, GuiTab};
use rustjay_gui::egui_theme::{Palette, set_palette};
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
    /// Where the tempo flash is in its cycle, 0 on the beat.
    ///
    /// Accumulated per frame rather than derived from elapsed time × tempo:
    /// that form jumps discontinuously whenever the BPM changes, by more the
    /// longer the app has been running, so a tap sent the flash haywire.
    /// Advancing it by `dt × bpm` means a tempo change alters the rate and
    /// nothing else.
    beat_phase: f32,
    /// The tap we last saw, so a new one restarts the flash's cycle — the same
    /// event that resets the LFOs.
    last_tap_seen: f64,

    /// Launch splash: pending until the first frame gives it a clock, then
    /// timed from there so a slow startup doesn't eat the whole hold.
    splash_pending: bool,
    splash_started_at: Option<f64>,
    /// When Help → About was opened, for the same drawing on no timer.
    about_opened_at: Option<f64>,

    /// Persisted UI preferences (palette choice). Loaded on the first frame,
    /// because the shell is built before there is an egui context to theme.
    prefs: crate::persistence::UiPrefs,
    /// One-shot: install the display font and apply the saved palette.
    initialised: bool,
}

/// The KOVVBOJ display face, or monospace until it is available.
///
/// Workbench covers Latin only (217 codepoints), so the family lists egui's
/// default monospace behind it — anything Workbench lacks falls back per glyph
/// instead of rendering as tofu.
///
/// `Context::set_fonts` only takes effect at the start of the *next* pass, so on
/// the first frame this family is not bound yet and asking for it panics inside
/// epaint. Querying the context rather than tracking a "fonts installed" flag
/// also keeps the UI rendering if the font ever fails to load.
fn display_family(ctx: &egui::Context) -> egui::FontFamily {
    let want = egui::FontFamily::Name(WORKBENCH.into());
    if ctx.fonts(|f| f.families().contains(&want)) {
        want
    } else {
        egui::FontFamily::Monospace
    }
}

const WORKBENCH: &str = "workbench";

impl Default for KovvbojShell {
    fn default() -> Self {
        Self::new()
    }
}

/// Move the tempo flash on by one frame, restarting the cycle on a beat.
///
/// Accumulating beats deriving the phase from elapsed time × tempo: the derived
/// form jumps whenever the BPM changes — further the longer the app has run —
/// which made a single tap send the flash haywire.
fn advance_beat_phase(phase: f32, dt: f32, bpm: f32, beat: bool) -> f32 {
    if beat {
        return 0.0;
    }
    if bpm <= 0.0 {
        return phase;
    }
    (phase + dt * bpm / 60.0).fract()
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
            beat_phase: 0.0,
            last_tap_seen: 0.0,
            splash_pending: true,
            splash_started_at: None,
            about_opened_at: None,
            prefs: crate::persistence::UiPrefs::default(),
            initialised: false,
        }
    }

    /// Install the display font and apply the saved palette. Runs once, on the
    /// first frame — `set_fonts` is cheap to call but not free, and the palette
    /// only needs applying when it changes.
    fn initialise(&mut self, ctx: &egui::Context) {
        if self.initialised {
            return;
        }
        self.initialised = true;

        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            WORKBENCH.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(include_bytes!(
                "../assets/fonts/Workbench.ttf"
            ))),
        );
        let mut chain = vec![WORKBENCH.to_owned()];
        chain.extend(
            fonts
                .families
                .get(&egui::FontFamily::Monospace)
                .cloned()
                .unwrap_or_default(),
        );
        fonts
            .families
            .insert(egui::FontFamily::Name(WORKBENCH.into()), chain);
        ctx.set_fonts(fonts);

        self.prefs = crate::persistence::default_workspace().load_ui();
        self.show_library = self.prefs.library_open;
        self.show_preview = self.prefs.inspector_open;
        self.show_outputs = self.prefs.outputs_open;
        self.show_sequencer = self.prefs.sequencer_open;
        for (i, tab) in VIEW_TABS.iter().enumerate() {
            self.builtin_open[i] = self.prefs.open_windows.iter().any(|n| n == tab.name());
        }
        set_palette(Palette::by_id(&self.prefs.palette));
    }

    /// A drag handle along one edge of a panel, returning the new width.
    ///
    /// Call this *after* the panel's contents: egui gives the pointer to the
    /// last widget drawn over a spot, so a handle registered first is shadowed
    /// by whatever is painted on top of it.
    ///
    /// egui's own `resizable(true)` does nothing here: the whole app is drawn
    /// inside `run_ui`'s root `Ui` through the deprecated top-level
    /// `Panel::show`, and the resize response never sees the pointer. The
    /// built-in host sidesteps this by making its sidebar a fixed width. Owning
    /// the interaction is simpler than changing how the host mounts its UI, and
    /// it puts the width somewhere we can persist.
    fn edge_drag(ui: &mut egui::Ui, id: &str, side: egui::Align, width: f32) -> Option<f32> {
        use rustjay_gui::egui_theme::colors::*;
        const GRIP: f32 = 6.0;
        let panel = ui.max_rect();
        let handle = match side {
            // A right-hand panel is dragged by its left edge, and vice versa.
            egui::Align::Min => egui::Rect::from_min_max(
                egui::pos2(panel.right() - GRIP, panel.top()),
                panel.right_bottom(),
            ),
            _ => egui::Rect::from_min_max(
                panel.left_top(),
                egui::pos2(panel.left() + GRIP, panel.bottom()),
            ),
        };
        let resp = ui.interact(handle, ui.id().with(id), egui::Sense::drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            ui.painter().rect_filled(handle, 0.0, amber().gamma_multiply(0.5));
        }
        if !resp.dragged() {
            return resp.drag_stopped().then_some(width);
        }
        let delta = resp.drag_delta().x;
        let moved = match side {
            egui::Align::Min => width + delta,
            _ => width - delta,
        };
        Some(moved.clamp(140.0, (ui.ctx().content_rect().width() - 360.0).max(200.0)))
    }

    /// Note which windows and panels are open, saving only when it changes.
    ///
    /// Toggles come from menu checkboxes and window close buttons, so there is
    /// no single place to hook; comparing against what was last written is
    /// cheaper than threading a callback through every one of them.
    fn remember_window_state(&mut self) {
        let open: Vec<String> = VIEW_TABS
            .iter()
            .enumerate()
            .filter(|(i, _)| self.builtin_open[*i])
            .map(|(_, t)| t.name().to_string())
            .collect();
        let changed = open != self.prefs.open_windows
            || self.show_outputs != self.prefs.outputs_open
            || self.show_sequencer != self.prefs.sequencer_open
            || self.show_library != self.prefs.library_open
            || self.show_preview != self.prefs.inspector_open;
        if !changed {
            return;
        }
        self.prefs.open_windows = open;
        self.prefs.outputs_open = self.show_outputs;
        self.prefs.sequencer_open = self.show_sequencer;
        self.prefs.library_open = self.show_library;
        self.prefs.inspector_open = self.show_preview;
        self.save_prefs();
    }

    /// Write UI preferences out.
    fn save_prefs(&mut self) {
        if let Err(e) = crate::persistence::default_workspace().save_ui(&self.prefs) {
            log::warn!("[Shell] could not save UI prefs: {e}");
        }
    }

    /// Persist the chosen palette and apply it. The repaint picks it up next
    /// frame, when the host re-runs `apply_professional_theme`.
    fn choose_palette(&mut self, id: &str) {
        self.prefs.palette = id.to_string();
        set_palette(Palette::by_id(id));
        if let Err(e) = crate::persistence::default_workspace().save_ui(&self.prefs) {
            log::warn!("[Shell] could not save UI prefs: {e}");
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
        self.initialise(ui.ctx());

        // Layer thumbnails: the render hook fills the textures, but only the
        // shell is handed the host that can turn them into egui ids.
        #[cfg(feature = "mixer")]
        if let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>() {
            state.thumbs.sync(host);
        }

        // ⌘Z / ⇧⌘Z. Structural edits only — see `KovvbojAppState::push_undo_from`.
        let (undo_pressed, redo_pressed) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z),
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::Z,
                ),
            )
        });
        if (undo_pressed || redo_pressed)
            && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
        {
            if redo_pressed {
                state.redo();
            } else {
                state.undo();
            }
        }

        let engine = host.engine().clone();

        self.menu_and_status_row(ui, app_state, &engine);
        self.remember_window_state();
        self.view_windows(ui, app_state, host, &engine);

        if self.show_library {
            #[allow(deprecated)] // top-level Panel::show, as in the built-in host
            let mut new_width = None;
            egui::Panel::left("kovvboj_library")
                .exact_size(self.prefs.library_width)
                .resizable(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .small_button("◀")
                            .on_hover_text("Collapse the library")
                            .clicked()
                        {
                            self.show_library = false;
                        }
                    });
                    egui::ScrollArea::vertical()
                        .show(ui, |ui| tab(&mut self.library, ui, app_state, &engine));
                    new_width = Self::edge_drag(
                        ui,
                        "library_resize",
                        egui::Align::Min,
                        self.prefs.library_width,
                    );
                });
            if let Some(w) = new_width {
                if (w - self.prefs.library_width).abs() > 0.5 {
                    self.prefs.library_width = w;
                } else {
                    self.save_prefs();
                }
            }
        } else {
            // A thin strip to bring it back, so hiding it is not a trip to the
            // View menu.
            #[allow(deprecated)]
            egui::Panel::left("kovvboj_library_collapsed")
                .exact_size(22.0)
                .resizable(false)
                .show(ui, |ui| {
                    if ui
                        .small_button("▶")
                        .on_hover_text("Show the library")
                        .clicked()
                    {
                        self.show_library = true;
                    }
                });
        }

        let mut inspector_left = None;
        if self.show_preview {
            #[allow(deprecated)]
            let mut new_width = None;
            egui::Panel::right("kovvboj_inspector")
                .exact_size(self.prefs.inspector_width)
                .resizable(false)
                .show(ui, |ui| {
                    inspector_left = Some(ui.clip_rect().left());
                    Self::preview(ui, host.output_preview_texture_id, &engine);
                    egui::ScrollArea::vertical()
                        .id_salt("inspector_scroll")
                        .show(ui, |ui| {
                            let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
                            else {
                                return;
                            };
                            let mut guard =
                                engine.lock().unwrap_or_else(|e| e.into_inner());
                            crate::ui::draw_inspector(ui, state, &mut guard);
                        });
                    new_width = Self::edge_drag(
                        ui,
                        "inspector_resize",
                        egui::Align::Max,
                        self.prefs.inspector_width,
                    );
                });
            if let Some(w) = new_width {
                if (w - self.prefs.inspector_width).abs() > 0.5 {
                    self.prefs.inspector_width = w;
                } else {
                    self.save_prefs();
                }
            }
        }

        // Not a `CentralPanel`: nested inside `run_ui`'s root Ui it is handed
        // ~34px more width than the right panel left free, and paints over the
        // inspector's left edge — burying the resize grip and clipping
        // "MASTER" to "TER". It cannot simply be narrowed either, because a
        // CentralPanel stores its size in egui memory, so constraining it feeds
        // back and it shrinks again every frame. A plain child Ui keeps no such
        // memory, so the bound holds.
        let mut central = ui.available_rect_before_wrap();
        if let Some(x) = inspector_left {
            central.max.x = central.max.x.min(x);
        }
        ui.painter()
            .rect_filled(central, 0.0, ui.style().visuals.panel_fill);
        ui.scope_builder(egui::UiBuilder::new().max_rect(central), |ui| {
            egui::Frame::central_panel(ui.style()).show(ui, |ui| {
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
        });

        self.splash(ui.ctx());
    }
}

impl KovvbojShell {
    /// The launch splash, then the About box — drawn over everything else, so
    /// they go last.
    fn splash(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);

        if let Some((elapsed, remaining)) = self.launch_timing(now) {
            let opacity = launch_opacity(remaining);
            ctx.request_repaint_after(remaining.min(crate::splash::FRAME_INTERVAL));
            egui::Modal::new(egui::Id::new("kovvboj_splash"))
                .backdrop_color(
                    Presentation::Launch
                        .backdrop()
                        .gamma_multiply(backdrop_opacity(remaining)),
                )
                .frame(Presentation::Launch.frame(&ctx.style_of(ctx.theme())))
                .show(ctx, |ui| {
                    ui.multiply_opacity(opacity);
                    splash(ui, elapsed, Presentation::Launch);
                });
            // Cut, rather than jumping into the fade: at this length the tail
            // of the curve is already invisible, so easing a skip would only
            // read as a delay.
            if ctx.input(|i| i.pointer.any_pressed() || !i.keys_down.is_empty()) {
                self.splash_started_at = None;
            }
        } else if let Some(opened_at) = self.about_opened_at {
            ctx.request_repaint_after(crate::splash::FRAME_INTERVAL);
            let card = egui::Modal::new(egui::Id::new("kovvboj_splash"))
                .backdrop_color(Presentation::About.backdrop())
                .frame(Presentation::About.frame(&ctx.style_of(ctx.theme())))
                .show(ctx, |ui| {
                    splash(ui, (now - opened_at).max(0.0) as f32, Presentation::About)
                });
            // Escape is consumed rather than read, so dismissing the About box
            // does not also drop the engine out of fullscreen.
            if card.inner
                || card.should_close()
                || ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                self.about_opened_at = None;
            }
        }
    }

    /// How far into the launch hold we are, or `None` once it is over.
    ///
    /// The clock starts on the first frame that asks rather than at
    /// construction, so the splash gets its full run however long the engine
    /// took to come up behind it.
    fn launch_timing(&mut self, now: f64) -> Option<(f32, std::time::Duration)> {
        if self.splash_pending {
            self.splash_pending = false;
            self.splash_started_at = Some(now);
        }
        let elapsed = (now - self.splash_started_at?).max(0.0);
        if elapsed >= LAUNCH_HOLD.as_secs_f64() {
            self.splash_started_at = None;
            return None;
        }
        Some((
            elapsed as f32,
            std::time::Duration::from_secs_f64(LAUNCH_HOLD.as_secs_f64() - elapsed),
        ))
    }

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
        let (has_color, has_motion, fps, bpm, clock, web, osc, recording) = {
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
                // Follow the clock the LFO follows. Link and ProDJ give a stable
                // ramp to lock onto; under Audio the detector fires at irregular
                // intervals, which is why `stable_beat_phase` refuses it — so
                // there we free-run and let a tap set the downbeat, exactly as a
                // tap resets the LFOs.
                (
                    state.stable_beat_phase(),
                    !matches!(state.sync_source, rustjay_core::SyncSource::Audio),
                    state.audio.last_tap_time,
                ),
                state.web_enabled,
                state.osc_enabled,
                state.recording_active,
            )
        };

        #[allow(deprecated)]
        egui::Panel::top("kovvboj_menubar")
            .exact_size(32.0)
            .frame(
                egui::Frame::NONE
                    .fill(surface_2())
                    .stroke(egui::Stroke::new(1.0_f32, hair_2())),
            )
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("KOVVBOJ")
                            .size(15.0)
                            .color(amber())
                            .family(display_family(ui.ctx())),
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
                        let (can_undo, can_redo) = app_state
                            .downcast_ref::<crate::KovvbojAppState>()
                            .map(|s| (!s.undo_stack.is_empty(), !s.redo_stack.is_empty()))
                            .unwrap_or((false, false));
                        if ui
                            .add_enabled(can_undo, egui::Button::new("Undo").shortcut_text("⌘Z"))
                            .clicked()
                            && let Some(state) =
                                app_state.downcast_mut::<crate::KovvbojAppState>()
                        {
                            state.undo();
                        }
                        if ui
                            .add_enabled(can_redo, egui::Button::new("Redo").shortcut_text("⇧⌘Z"))
                            .clicked()
                            && let Some(state) =
                                app_state.downcast_mut::<crate::KovvbojAppState>()
                        {
                            state.redo();
                        }
                        ui.separator();
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

                    ui.menu_button("Help", |ui| {
                        if ui.button("About KOVVBOJ").clicked() {
                            self.about_opened_at = Some(ui.input(|i| i.time));
                            ui.close();
                        }
                    });

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(10.0);
                        // The one output control you reach for mid-set; the rest
                        // of the routing lives in the Outputs window.
                        // Map modes. These live only on the built-in host's top
                        // bar, which this shell replaces — without them there is
                        // no way into either mode here at all.
                        let (mod_map, midi_map) = engine
                            .lock()
                            .map(|e| (e.lfo_assign_mode, e.midi_learn_mode))
                            .unwrap_or((false, false));
                        // A button rather than a `selectable_label`: selection
                        // fills with the accent colour, and the state colour then
                        // sits on top of it unreadably.
                        if ui
                            .button(
                                egui::RichText::new("MIDI")
                                    .monospace()
                                    .size(11.0)
                                    .color(if midi_map { amber() } else { ink_3() }),
                            )
                            .on_hover_text("Click a parameter, then move a MIDI control")
                            .clicked()
                            && let Ok(mut e) = engine.lock()
                        {
                            e.midi_learn_mode = !e.midi_learn_mode;
                            if e.midi_learn_mode {
                                e.lfo_assign_mode = false;
                            } else {
                                e.midi_command = rustjay_core::MidiCommand::CancelLearn;
                            }
                        }
                        if ui
                            .button(
                                egui::RichText::new("MOD")
                                    .monospace()
                                    .size(11.0)
                                    .color(if mod_map { amber() } else { ink_3() }),
                            )
                            .on_hover_text("Click a parameter to bind an LFO or audio band")
                            .clicked()
                            && let Ok(mut e) = engine.lock()
                        {
                            e.lfo_assign_mode = !e.lfo_assign_mode;
                            if e.lfo_assign_mode {
                                e.midi_learn_mode = false;
                            }
                        }

                        let rec = ui
                            .selectable_label(
                                recording,
                                egui::RichText::new("● REC")
                                    .monospace()
                                    .size(11.0)
                                    .color(if recording { alert() } else { ink_3() }),
                            )
                            .on_hover_text(if recording {
                                "Stop recording"
                            } else {
                                "Start recording"
                            });
                        if rec.clicked()
                            && let Ok(mut e) = engine.lock()
                        {
                            e.output_command = if recording {
                                rustjay_core::OutputCommand::StopRecording
                            } else {
                                rustjay_core::OutputCommand::StartRecording {
                                    path: crate::ui::next_recording_path(),
                                    codec: rustjay_core::RecorderCodec::H264,
                                    audio_device: None,
                                }
                            };
                        }
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
                        // Flash on the beat: brightest as the phase wraps, faded
                        // by a quarter of the way through. Driven by the phase
                        // rather than the audio beat flag so it still pulses on a
                        // tapped or Link tempo with no audio coming in.
                        // Advance the flash by this frame's share of a beat, and
                        // restart the cycle on a detected beat. Accumulating keeps
                        // a tempo change from moving where we are in the bar.
                        let (stable_phase, has_stable_clock, last_tap) = clock;
                        let phase = if has_stable_clock {
                            // A real ramp from Link or ProDJ: just read it.
                            stable_phase
                        } else {
                            let dt = ui.input(|i| i.stable_dt).min(0.1);
                            let tapped = last_tap > self.last_tap_seen;
                            self.last_tap_seen = last_tap;
                            self.beat_phase =
                                advance_beat_phase(self.beat_phase, dt, bpm, tapped);
                            if bpm > 0.0 { self.beat_phase } else { 1.0 }
                        };
                        let flash = (1.0 - phase * 4.0).clamp(0.0, 1.0);
                        let beat_color = if bpm > 0.0 {
                            let dim = ink_2();
                            let lit = amber();
                            egui::Color32::from_rgb(
                                (dim.r() as f32 + (lit.r() as f32 - dim.r() as f32) * flash) as u8,
                                (dim.g() as f32 + (lit.g() as f32 - dim.g() as f32) * flash) as u8,
                                (dim.b() as f32 + (lit.b() as f32 - dim.b() as f32) * flash) as u8,
                            )
                        } else {
                            ink_2()
                        };
                        ui.label(
                            egui::RichText::new(format!("{:.0} BPM", bpm))
                                .size(11.0)
                                .monospace()
                                .color(beat_color),
                        )
                        .on_hover_text("Flashes on the beat");
                        ui.label(
                            egui::RichText::new(format!("{:.0} FPS", fps))
                                .size(11.0)
                                .monospace()
                                .color(ink_2()),
                        );
                    });
                });
            });
    }

    /// MIX / STAGE / MAP.
    fn mode_switcher(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (mode, label) in Mode::ALL {
                let text = egui::RichText::new(label)
                    .size(14.0)
                    .family(display_family(ui.ctx()));
                if ui.selectable_label(self.mode == mode, text).clicked() {
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
                // These tab bodies were laid out for the host's fixed-width
                // sidebar. Their widgets are bounded now, but a window that
                // auto-sizes still creeps outward over the first frames as the
                // content settles; the ceiling stops it at the width the
                // widgets themselves top out at.
                .max_width(560.0)
                .vscroll(true)
                .show(&ctx, |ui| host.draw_builtin_tab(ui, *t));
        }

        if self.show_settings {
            let mut open = true;
            let mut chosen: Option<&'static str> = None;
            egui::Window::new("Settings")
                .open(&mut open)
                .default_width(460.0)
                .vscroll(true)
                .show(&ctx, |ui| {
                    ui.label(egui::RichText::new("Appearance").strong());
                    let current = Palette::PRESETS
                        .iter()
                        .find(|(id, _)| *id == self.prefs.palette)
                        .map(|(_, name)| *name)
                        .unwrap_or("HUD Amber");
                    egui::ComboBox::from_label("Palette")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            for (id, name) in Palette::PRESETS {
                                if ui
                                    .selectable_label(self.prefs.palette == id, name)
                                    .clicked()
                                {
                                    chosen = Some(id);
                                }
                            }
                        });
                    ui.separator();
                    host.draw_builtin_tab(ui, GuiTab::Settings);
                });
            self.show_settings = open;
            if let Some(id) = chosen {
                self.choose_palette(id);
            }
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

#[cfg(test)]
mod beat_flash_tests {
    use super::advance_beat_phase;

    #[test]
    fn a_beat_restarts_the_cycle() {
        assert_eq!(advance_beat_phase(0.73, 0.016, 120.0, true), 0.0);
    }

    #[test]
    fn it_advances_a_beat_per_beat() {
        // 120 BPM is half a second a beat, so a quarter second is half a cycle.
        let mut phase = 0.0;
        for _ in 0..25 {
            phase = advance_beat_phase(phase, 0.01, 120.0, false);
        }
        assert!((phase - 0.5).abs() < 0.01, "phase was {phase}");
    }

    /// The bug: deriving the phase from elapsed time × tempo moved the flash to
    /// an unrelated point in the bar whenever the tempo changed. Accumulating
    /// changes the rate and leaves the position alone.
    #[test]
    fn a_tempo_change_does_not_move_the_phase() {
        let phase = 0.4;
        let slow = advance_beat_phase(phase, 0.016, 60.0, false);
        let fast = advance_beat_phase(phase, 0.016, 180.0, false);
        assert!(slow > phase && fast > phase, "both move forward");
        assert!(fast - phase > slow - phase, "faster tempo moves further");
        assert!(fast < 0.5, "but neither jumps across the bar: {fast}");
    }

    #[test]
    fn a_stopped_clock_leaves_it_where_it_is() {
        assert_eq!(advance_beat_phase(0.31, 0.016, 0.0, false), 0.31);
    }
}
