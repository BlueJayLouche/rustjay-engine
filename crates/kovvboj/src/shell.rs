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

use crate::splash::{LAUNCH_HOLD, Presentation, backdrop_opacity, launch_opacity, splash};
#[cfg(feature = "webcam")]
use crate::ui::LedMapTab;
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
    /// Projection surfaces, lighting placement and warp.
    Stage,
    /// The webcam calibration wizard: flash a strip, recover positions.
    Calibrate,
}

impl Mode {
    const ALL: [(Self, &'static str); 3] = [
        (Self::Mix, "MIX"),
        (Self::Stage, "STAGE"),
        (Self::Calibrate, "CALIBRATE"),
    ];
}

/// Built-in tabs the View menu can open as windows, in menu order.
///
/// `Settings` is deliberately absent — it lives under Edit. `Sync` is folded
/// into Audio by the host and has no body of its own. `Input` and `Output`
/// are absent too: the Library's DEVICES rows and the Outputs window own
/// those jobs, and the engine's versions opened a second capture session on
/// the same camera and a second NDI/Syphon sender from the hidden main window.
const VIEW_TABS: [GuiTab; 8] = [
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
    /// The two deck columns. `decks` above stays as the whole-stack view, used
    /// when a set has no decks.
    deck_a: DeckTab,
    deck_b: DeckTab,
    /// What each deck is about to be saved as, while it is being typed.
    /// Cleared on save, and not persisted — a name in flight is not state.
    deck_name: [String; 2],
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
    /// Extra focus *within* MIX: drop the library and inspector there too.
    ///
    /// Outside MIX they are hidden regardless — the effects library and the
    /// deck inspector have nothing to say about a projection surface, and the
    /// width they cost is exactly what stage mapping needs. This flag only
    /// covers the case where MIX itself wants the room. Not persisted.
    focus: bool,
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
    /// Global recording settings, edited in Settings. Loaded with `prefs`.
    recording: crate::persistence::RecordingPrefs,
    /// Folder picked by the recording folder dialog thread.
    pending_recording_folder: Arc<Mutex<Option<std::path::PathBuf>>>,
    /// One-shot: install the display font and apply the saved palette.
    initialised: bool,

    /// Folder picked by the File menu's dialog thread, waiting to be applied.
    /// The dialog is modal and blocking, so it cannot run on the render thread —
    /// same pattern as the source and recording pickers.
    pending_workspace: Arc<Mutex<Option<(WorkspaceAction, std::path::PathBuf)>>>,
    /// What a background file job is doing, shown in the menu bar. Packing a
    /// set copies its video, which takes long enough that silence reads as a
    /// hang.
    busy: Arc<Mutex<Option<String>>>,
}

/// What the file the user is picking is for.
#[derive(Copy, Clone)]
enum WorkspaceAction {
    New,
    Open,
    SaveAs,
    Export,
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
            deck_a: DeckTab::for_deck(0),
            deck_b: DeckTab::for_deck(1),
            deck_name: [String::new(), String::new()],
            master: MixerTab::default(),
            stage: StageTab::new(),
            #[cfg(feature = "webcam")]
            ledmap: LedMapTab::new(),
            outputs: OutputsTab::new(),
            sequencer: SequencerTab,
            pending_workspace: Arc::new(Mutex::new(None)),
            busy: Arc::new(Mutex::new(None)),
            builtin_open: [false; VIEW_TABS.len()],
            show_settings: false,
            show_outputs: false,
            show_sequencer: false,
            show_library: true,
            show_preview: true,
            focus: false,
            beat_phase: 0.0,
            last_tap_seen: 0.0,
            splash_pending: true,
            splash_started_at: None,
            about_opened_at: None,
            prefs: crate::persistence::UiPrefs::default(),
            recording: crate::persistence::RecordingPrefs::default(),
            pending_recording_folder: Arc::new(Mutex::new(None)),
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
        self.recording = crate::persistence::default_workspace().load_recording();
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
            ui.painter()
                .rect_filled(handle, 0.0, amber().gamma_multiply(0.5));
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
            // A deck is a group, so its preview is that group's thumbnail. The
            // ids go where every other live preview publishes one, so the
            // crossfader strip reads them the same way the Stage canvas reads
            // the master output.
            let ids = [crate::DECK_A, crate::DECK_B].map(|uuid| {
                state.thumbs.ids.get(uuid).map(|id| match id {
                    egui::TextureId::Managed(n) | egui::TextureId::User(n) => *n,
                })
            });
            if let Ok(mut eng) = host.engine().lock()
                && eng.deck_preview_texture_ids != ids
            {
                eng.deck_preview_texture_ids = ids;
            }
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

        // ⌘T — TAKE. A modifier combo, so it cannot fire while a name is being
        // typed into a text field.
        #[cfg(feature = "mixer")]
        {
            let take_pressed = ui
                .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::T));
            if take_pressed {
                let secs = host
                    .engine()
                    .lock()
                    .ok()
                    .and_then(|e| e.get_param_base(crate::TAKE_SECONDS))
                    .unwrap_or(1.0);
                if let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
                    && let Ok(mut m) = state.mixer.lock()
                {
                    crate::take(&mut m, secs);
                }
            }
        }

        // A folder picked by the File menu last frame.
        if let Some((action, dir)) = self
            .pending_workspace
            .lock()
            .ok()
            .and_then(|mut g| g.take())
            && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
        {
            match action {
                WorkspaceAction::New => state.new_workspace(dir),
                WorkspaceAction::Open => state.open_workspace(dir),
                WorkspaceAction::SaveAs => state.save_workspace_as(dir),
                #[cfg(feature = "mixer")]
                WorkspaceAction::Export => {
                    // Bundles what is on disk, so write the live set out first.
                    state.save_workspace();
                    export_set(state.workspace.dir.clone(), dir, &self.busy, ui.ctx());
                }
                #[cfg(not(feature = "mixer"))]
                WorkspaceAction::Export => {}
            }
        }

        // ⌘S from anywhere, not just the mixer tab.
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S))
            && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
        {
            state.save_workspace();
        }

        let engine = host.engine().clone();

        self.menu_and_status_row(ui, app_state, &engine);
        self.remember_window_state();
        self.view_windows(ui, app_state, host, &engine);

        // The library and inspector belong to MIX. STAGE and CALIBRATE want the
        // width far more than they want a shader list, and a stage canvas is
        // aspect-fit — every pixel taken off its width comes off its height too.
        let chrome = self.mode == Mode::Mix && !self.focus;

        if self.show_library && chrome {
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
                    // egui's scroll bar floats over the content by default, and
                    // a library row lays out right-to-left — so the rightmost
                    // button sat under the bar and could not be clicked at all.
                    // Taking layout space costs a few pixels of width and makes
                    // every row's last control reachable.
                    ui.spacing_mut().scroll.floating = false;
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
        } else if chrome {
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
        if self.show_preview && chrome {
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
                            let mut guard = engine.lock().unwrap_or_else(|e| e.into_inner());
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
                        // The floor, not the default, is what actually holds this
                        // open: `default_size` only applies on the frame before the
                        // panel has stored a size, and something narrower than the
                        // content was winning from then on — a drag was the only
                        // thing that rewrote it. egui clamps the size it loads to
                        // this range every frame, so a floor survives that.
                        // ponytail: 200 clears the master row, the chain-name row
                        // and the FX strip. If the master area ever needs to shrink
                        // below that, it wants the inspector's arrangement — own the
                        // resize, keep the height in `UiPrefs` — not a lower floor.
                        egui::Panel::bottom("kovvboj_master")
                            .default_size(220.0)
                            .min_size(200.0)
                            .resizable(true)
                            .show(ui, |ui| {
                                egui::ScrollArea::vertical()
                                    .id_salt("master_scroll")
                                    .show(ui, |ui| tab(&mut self.master, ui, app_state, &engine));
                            });
                        // The crossfader sits between the stacks and MASTER:
                        // below what it transitions, above what it feeds.
                        let decked = app_state
                            .downcast_ref::<crate::KovvbojAppState>()
                            .and_then(|s| s.mixer.lock().ok().map(|m| m.decks.is_some()))
                            .unwrap_or(false);
                        if decked {
                            #[allow(deprecated)]
                            egui::Panel::bottom("kovvboj_crossfader")
                                .default_size(96.0)
                                .min_size(84.0)
                                .resizable(false)
                                .show(ui, |ui| {
                                    Self::crossfader_strip(ui, app_state, &engine);
                                });
                        }

                        let mut decks = if decked {
                            // Two stacks, A left and B right, each scrolling on
                            // its own so a long deck does not drag the other.
                            let full = ui.available_rect_before_wrap();
                            let mid = full.center().x;
                            let mut left = full;
                            left.max.x = mid - 4.0;
                            let mut right = full;
                            right.min.x = mid + 4.0;

                            // Both axes: a layer row has a minimum width — the
                            // blend picker and the mix buttons do not shrink —
                            // and half a window can be under it. Scrolling
                            // sideways keeps a narrow column's rows reachable
                            // *and* keeps them inside their own column. A
                            // vertical-only area clips to its parent's edge, not
                            // its own, so an overflowing row paints across deck B.
                            ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
                                self.deck_heading(ui, "DECK A", app_state, 0);
                                egui::ScrollArea::both()
                                    .id_salt("deck_a_scroll")
                                    .show(ui, |ui| tab(&mut self.deck_a, ui, app_state, &engine));
                            });
                            ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
                                self.deck_heading(ui, "DECK B", app_state, 1);
                                egui::ScrollArea::both()
                                    .id_salt("deck_b_scroll")
                                    .show(ui, |ui| tab(&mut self.deck_b, ui, app_state, &engine))
                            })
                            .inner
                        } else {
                            egui::ScrollArea::vertical()
                                .id_salt("decks_scroll")
                                .show(ui, |ui| tab(&mut self.decks, ui, app_state, &engine))
                        };
                        // While a chip or layer drag is in flight, hovering the
                        // top/bottom edge scrolls the list, so off-screen layers
                        // stay reachable as drop targets.
                        if egui::DragAndDrop::has_any_payload(ui.ctx())
                            && let Some(pos) = ui.ctx().pointer_interact_pos()
                            && decks.inner_rect.contains(pos)
                        {
                            let delta = crate::ui::drag_edge_scroll_delta(
                                pos.y,
                                decks.inner_rect.top(),
                                decks.inner_rect.bottom(),
                            );
                            if delta != 0.0 {
                                decks.state.offset.y += delta;
                                decks.state.store(ui.ctx(), decks.id);
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                    Mode::Stage => {
                        // No outer scroll area: the stage canvas sizes itself from
                        // the space it is given, and inside a vertical scroll that
                        // space is unbounded, so it came out a strip. The tab owns
                        // its own layout — a bottom panel for the inspector, the
                        // canvas filling the rest — and scrolls where it needs to.
                        tab(&mut self.stage, ui, app_state, &engine);
                    }
                    Mode::Calibrate => {
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
        let (
            has_color,
            has_motion,
            fps,
            bpm,
            clock,
            web,
            osc,
            master_recording,
            outputs_recording,
        ) = {
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
                // Projector and headless recordings, as published by `prepare`.
                state
                    .output_sinks
                    .lock()
                    .map(|s| s.iter().any(|l| l == "REC"))
                    .unwrap_or(false),
            )
        };
        let recording = master_recording || outputs_recording;

        #[allow(deprecated)]
        egui::Panel::top("kovvboj_menubar")
            .exact_size(32.0)
            .frame(
                egui::Frame::NONE
                    .fill(surface_2())
                    .stroke(egui::Stroke::new(1.0_f32, hair_2())),
            )
            .show(ui, |ui| {
                let pending_workspace = self.pending_workspace.clone();
                let busy = self.busy.clone();
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
                        // Which set is loaded — the only place it is visible,
                        // since a workspace is just a directory.
                        let dir = app_state
                            .downcast_ref::<crate::KovvbojAppState>()
                            .map(|s| s.workspace.dir.clone())
                            .unwrap_or_default();
                        ui.add_enabled(
                            false,
                            egui::Label::new(
                                egui::RichText::new(workspace_label(&dir)).monospace().size(11.0),
                            ),
                        );
                        ui.separator();

                        if ui.button("New Workspace…").clicked() {
                            pick_workspace_dir(
                                WorkspaceAction::New,
                                "New Workspace",
                                &pending_workspace,
                                ui.ctx(),
                            );
                            ui.close();
                        }
                        if ui.button("Open Workspace…").clicked() {
                            pick_workspace_dir(
                                WorkspaceAction::Open,
                                "Open Workspace",
                                &pending_workspace,
                                ui.ctx(),
                            );
                            ui.close();
                        }
                        ui.menu_button("Open Recent", |ui| {
                            let recent = crate::persistence::load_recent();
                            if recent.is_empty() {
                                ui.add_enabled(false, egui::Button::new("Nothing yet"));
                            }
                            for path in &recent {
                                if ui
                                    .button(workspace_label(path))
                                    .on_hover_text(path.display().to_string())
                                    .clicked()
                                {
                                    if let Some(state) =
                                        app_state.downcast_mut::<crate::KovvbojAppState>()
                                    {
                                        state.open_workspace(path.clone());
                                    }
                                    ui.close();
                                }
                            }
                            ui.separator();
                            if ui.button("Clear Menu").clicked() {
                                crate::persistence::clear_recent();
                                ui.close();
                            }
                        });

                        ui.separator();
                        if ui
                            .add(egui::Button::new("Save Workspace").shortcut_text("⌘S"))
                            .clicked()
                            && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
                        {
                            state.save_workspace();
                        }
                        if ui.button("Save Workspace As…").clicked() {
                            pick_workspace_dir(
                                WorkspaceAction::SaveAs,
                                "Save Workspace As",
                                &pending_workspace,
                                ui.ctx(),
                            );
                            ui.close();
                        }
                        if ui
                            .button("Revert to Saved")
                            .on_hover_text("Discard everything since the last save")
                            .clicked()
                            && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>()
                        {
                            state.revert_workspace();
                        }

                        #[cfg(feature = "mixer")]
                        {
                            ui.separator();
                            if ui
                                .button("Export Set…")
                                .on_hover_text("Pack the set and every shader, clip and image it uses into one file")
                                .clicked()
                            {
                                export_dialog(&dir, &pending_workspace, ui.ctx());
                                ui.close();
                            }
                            if ui.button("Import Set…").clicked() {
                                import_dialog(&pending_workspace, &busy, ui.ctx());
                                ui.close();
                            }
                        }

                        ui.separator();
                        if ui.button(reveal_label()).clicked() {
                            reveal(&dir);
                            ui.close();
                        }
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

                    // Library analysis. A scan is a pre-show job, like a DJ
                    // analysing a crate: never automatic, because rendering an
                    // unknown shader costs GPU time the live output wants.
                    ui.menu_button("Library", |ui| {
                        let scanning = app_state
                            .downcast_ref::<crate::KovvbojAppState>()
                            .is_some_and(|s| s.scan.is_some());
                        if scanning {
                            if let Some(state) =
                                app_state.downcast_mut::<crate::KovvbojAppState>()
                                && let Some(job) = state.scan.as_mut()
                            {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}/{} analysed",
                                        job.done, job.total
                                    ))
                                    .size(11.0),
                                );
                                if ui.button("Cancel scan").clicked() {
                                    job.cancel();
                                    ui.close();
                                }
                            }
                        } else {
                            let start = ui
                                .button("Scan library")
                                .on_hover_text(
                                    "Analyse shaders not yet seen: thumbnail, weight and \
                                     whether they compile",
                                )
                                .clicked();
                            let rescan = ui
                                .button("Rescan everything")
                                .on_hover_text(
                                    "Re-measure the whole library — needed after changing \
                                     internal resolution",
                                )
                                .clicked();
                            if (start || rescan)
                                && let Some(state) =
                                    app_state.downcast_mut::<crate::KovvbojAppState>()
                            {
                                let paths = state.library_shader_paths();
                                // The size the analyzer will actually render
                                // at, so an already-scanned shader is only
                                // skipped when its weight still applies.
                                let at = engine
                                    .lock()
                                    .map(|e| {
                                        [
                                            e.resolution.internal_width,
                                            e.resolution.internal_height,
                                        ]
                                    })
                                    .unwrap_or([1920, 1080]);
                                state.scan = Some(if rescan {
                                    crate::previs::ScanJob::rescan(paths)
                                } else {
                                    crate::previs::ScanJob::new(paths, &state.previs, at)
                                });
                                ui.close();
                            }
                        }
                        ui.separator();
                        if ui
                            .button("Add folder…")
                            .on_hover_text("Another directory for the library to scan")
                            .clicked()
                            && let Some(state) =
                                app_state.downcast_ref::<crate::KovvbojAppState>()
                        {
                            let pending = state.pending_library_folder.clone();
                            let ctx = ui.ctx().clone();
                            std::thread::spawn(move || {
                                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                    if let Ok(mut g) = pending.lock() {
                                        *g = Some(dir);
                                    }
                                    ctx.request_repaint();
                                }
                            });
                            ui.close();
                        }
                    });

                    ui.menu_button("Help", |ui| {
                        if ui.button("About KOVVBOJ").clicked() {
                            self.about_opened_at = Some(ui.input(|i| i.time));
                            ui.close();
                        }
                    });

                    // Scan progress sits with the other long-job readouts, so
                    // there is one place to look for "the app is working".
                    if let Some(text) = app_state
                        .downcast_ref::<crate::KovvbojAppState>()
                        .and_then(|s| s.scan.as_ref())
                        .map(|j| j.label())
                    {
                        ui.add_space(10.0);
                        ui.label(egui::RichText::new(text).size(11.0).monospace().color(amber()));
                    }

                    if let Some(job) = busy.lock().ok().and_then(|b| b.clone()) {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new(job).size(11.0).monospace().color(amber()),
                        );
                    }

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
                                "Stop all recordings"
                            } else {
                                "Start recording"
                            });
                        if rec.clicked() {
                            if recording {
                                // Stop everything the button is lit for, not
                                // just the main output.
                                if master_recording && let Ok(mut e) = engine.lock() {
                                    e.output_command = rustjay_core::OutputCommand::StopRecording;
                                }
                                #[cfg(feature = "projection")]
                                if let Some(s) = app_state.downcast_mut::<crate::KovvbojAppState>()
                                {
                                    for p in &mut s.stage.projectors {
                                        p.recording = false;
                                    }
                                    for h in &mut s.stage.headless_outputs {
                                        h.recording = false;
                                    }
                                }
                            } else if let Some(s) =
                                app_state.downcast_ref::<crate::KovvbojAppState>()
                                && let Ok(mut e) = engine.lock()
                            {
                                let (path, codec) = s.workspace.next_recording("kovvboj");
                                e.output_command = rustjay_core::OutputCommand::StartRecording {
                                    path: path.to_string_lossy().into_owned(),
                                    codec,
                                    audio_device: None,
                                };
                            }
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

    /// MIX / STAGE / CALIBRATE, and MIX's focus toggle.
    ///
    /// The switcher always stays visible: it costs one row, and hiding it would
    /// mean leaving focus just to change mode.
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
            // Only MIX has anything to hide — the other modes drop the library
            // and inspector on their own, so the button would be inert there.
            if self.mode == Mode::Mix {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let hint = if self.focus {
                        "Show the library and inspector (Esc)"
                    } else {
                        "Hide the library and inspector"
                    };
                    if ui
                        .selectable_label(self.focus, "\u{26F6}")
                        .on_hover_text(hint)
                        .clicked()
                    {
                        self.focus = !self.focus;
                    }
                });
            }
        });

        // Escape leaves focus, but only while the About box is down — that
        // reads Escape too, and it is drawn after this.
        if self.focus
            && self.about_opened_at.is_none()
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.focus = false;
        }
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
            let recording_before = self.recording.clone();
            if let Ok(mut g) = self.pending_recording_folder.lock()
                && let Some(dir) = g.take()
            {
                self.recording.folder = Some(dir);
            }
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
                    ui.label(egui::RichText::new("Recording").strong());
                    egui::ComboBox::from_label("Codec")
                        .selected_text(self.recording.codec.label())
                        .show_ui(ui, |ui| {
                            for c in rustjay_core::RecorderCodec::ALL {
                                ui.selectable_value(&mut self.recording.codec, c, c.label());
                            }
                        });
                    let folder = match &self.recording.folder {
                        Some(dir) => dir.display().to_string(),
                        None => "recordings/ inside the open set".to_string(),
                    };
                    // Truncated on its own line: a long path in a horizontal row
                    // would widen the whole window.
                    ui.add(egui::Label::new(format!("Folder: {folder}")).truncate())
                        .on_hover_text(&folder);
                    ui.horizontal(|ui| {
                        if ui.button("Choose folder…").clicked() {
                            let pending = self.pending_recording_folder.clone();
                            let ctx = ui.ctx().clone();
                            std::thread::spawn(move || {
                                if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                                    if let Ok(mut g) = pending.lock() {
                                        *g = Some(dir);
                                    }
                                    ctx.request_repaint();
                                }
                            });
                        }
                        if self.recording.folder.is_some()
                            && ui.button("Use the set's folder").clicked()
                        {
                            self.recording.folder = None;
                        }
                    });
                    ui.label(
                        egui::RichText::new("Applies to every set, from the next recording.")
                            .weak(),
                    );
                    ui.separator();
                    host.draw_builtin_tab(ui, GuiTab::Settings);
                });
            self.show_settings = open;
            if let Some(id) = chosen {
                self.choose_palette(id);
            }
            if self.recording != recording_before
                && let Err(e) =
                    crate::persistence::default_workspace().save_recording(&self.recording)
            {
                log::warn!("[Shell] could not save recording settings: {e}");
            }
        }

        // App-owned windows. The open flag is copied into a local so the
        // window can borrow it mutably while the body borrows `self`.
        if self.show_outputs {
            let mut open = true;
            egui::Window::new("Outputs")
                .open(&mut open)
                .default_width(520.0)
                // A hint, not a bound — the rows are what keep the window
                // from growing to the screen, see `OutputsTab`.
                .max_width(640.0)
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

    /// A deck column's heading, with the one verb a deck has of its own.
    ///
    /// A deck draws no group row — the column *is* the header — so saving lives
    /// here instead, where a group's 💾 would be. Named on the way out, the way
    /// the master chain is: a deck's own name is always "Deck A", so saving
    /// under it wrote the same file every time and replaced the last look
    /// silently. Typing a name and saving is one gesture, and a name already
    /// taken says "replaces" *before* the click, not after.
    #[cfg_attr(not(feature = "mixer"), allow(unused_variables))]
    fn deck_heading(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        app_state: &mut dyn std::any::Any,
        deck: usize,
    ) {
        #[cfg(feature = "mixer")]
        let uuid = if deck == 1 { crate::DECK_B } else { crate::DECK_A };
        #[cfg(feature = "mixer")]
        let selected = app_state
            .downcast_ref::<crate::KovvbojAppState>()
            .is_some_and(|s| {
                matches!(&s.selection, crate::Selection::Group { group } if group == uuid)
            });
        #[cfg(not(feature = "mixer"))]
        let selected = false;
        let mut select = false;
        ui.horizontal(|ui| {
            // The heading selects the deck: its level, blend and chain then
            // go in the inspector, and ➕ in the library adds an effect to it.
            if ui
                .selectable_label(
                    selected,
                    egui::RichText::new(label)
                        .strong()
                        .monospace()
                        .color(rustjay_gui::egui_theme::colors::ink_4()),
                )
                .on_hover_text("Select the deck — edit it in the inspector, add effects from the library")
                .clicked()
            {
                select = true;
            }
            #[cfg(feature = "mixer")]
            {
                let typed = &mut self.deck_name[deck.min(1)];
                let entry = ui.add(
                    egui::TextEdit::singleline(typed)
                        .hint_text("deck name")
                        .desired_width(110.0),
                );
                let name = typed.trim().to_string();
                let named = !name.is_empty();
                // Enter saves, so naming and saving is one gesture.
                let entered =
                    entry.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && named;
                let clicked = ui
                    .add_enabled(named, egui::Button::new("💾").small())
                    .on_hover_text("Save this deck to the library, layers and all")
                    .on_disabled_hover_text("Give the deck a name first")
                    .clicked();
                if let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>() {
                    if entered || clicked {
                        state.pending_group_save = Some((uuid.to_string(), name.clone()));
                        self.deck_name[deck.min(1)].clear();
                    } else if named && state.saved_groups.iter().any(|g| g.name == name) {
                        // One name is one file, deck or group: say so first.
                        ui.label(
                            egui::RichText::new("replaces")
                                .size(10.0)
                                .color(rustjay_gui::egui_theme::colors::amber()),
                        );
                    }
                }
            }
        });
        #[cfg(feature = "mixer")]
        if select && let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>() {
            state.selection = crate::Selection::Group {
                group: uuid.to_string(),
            };
        }
        #[cfg(not(feature = "mixer"))]
        let _ = select;
        ui.separator();
    }

    /// The widest the block between the deck previews gets — the fader and
    /// the transition row under it — in points.
    #[cfg(feature = "mixer")]
    const CROSSFADER_MAX: f32 = 480.0;

    /// The crossfader, flanked by what it is fading between.
    ///
    /// The fader position *is* the transition's progress, so there is one
    /// control here and not two: picking a different shader changes what the
    /// same movement looks like.
    #[cfg(feature = "mixer")]
    fn crossfader_strip(
        ui: &mut egui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &Arc<Mutex<EngineState>>,
    ) {
        let Some(state) = app_state.downcast_mut::<crate::KovvbojAppState>() else {
            return;
        };
        // The strip is on screen, so the deck thumbnails are being drawn: say so,
        // or they go idle and freeze the moment the layer list is closed.
        state.thumbs.mark_wanted();

        // Which transitions are on offer, and which is loaded.
        let current = state
            .mixer
            .lock()
            .ok()
            .and_then(|m| m.transition.as_ref().and_then(|s| s.source_path.clone()));
        let current_name = current
            .as_deref()
            .map(crate::transition_name)
            .unwrap_or_else(|| "none".to_string());

        let mut pick: Option<std::path::PathBuf> = None;
        let mut edit_transition = false;
        let mut add_transition = false;

        // Deck A's output, the fader, deck B's output: one unit, centred on the
        // seam between the deck columns at any width. The fader stretches with
        // the window up to `CROSSFADER_MAX` and stops there — past that a longer
        // throw is only more mouse travel, and the previews drift away from the
        // fader they flank.
        let full = ui.available_rect_before_wrap();
        let side = (full.width() * 0.22).clamp(80.0, 200.0);
        let gap = ui.spacing().item_spacing.x;
        let block = (full.width() - 2.0 * (side + gap)).min(Self::CROSSFADER_MAX);
        let column = |x: f32, w: f32| {
            egui::Rect::from_min_size(egui::pos2(x, full.min.y), egui::vec2(w, full.height()))
        };
        let a = column(full.center().x - block / 2.0 - gap - side, side);
        let middle = column(a.max.x + gap, block);
        let b = column(middle.max.x + gap, side);
        let clicked = [
            ui.scope_builder(egui::UiBuilder::new().max_rect(a), |ui| {
                Self::deck_preview(ui, engine, 0, side)
            })
            .inner
            .clicked(),
            ui.scope_builder(egui::UiBuilder::new().max_rect(b), |ui| {
                Self::deck_preview(ui, engine, 1, side)
            })
            .inner
            .clicked(),
        ];
        ui.scope_builder(egui::UiBuilder::new().max_rect(middle), |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("A").strong().monospace());
                // B goes in first, right-to-left, so the fader takes exactly
                // the span between the two letters.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("B").strong().monospace());
                    // `slider_width`, not `add_sized`: a Slider ignores the
                    // size it is handed, so this sat at the theme's 200pt.
                    ui.spacing_mut().slider_width = ui.available_width().max(40.0);
                    let mut eng = engine.lock().unwrap_or_else(|e| e.into_inner());
                    let mut x = eng.get_param_base("crossfader").unwrap_or(0.0);
                    let map_mode = rustjay_engine::prelude::map_mode_active(&eng);
                    let resp = ui
                        .add_enabled(
                            !map_mode,
                            egui::Slider::new(&mut x, 0.0..=1.0).show_value(false),
                        )
                        .on_hover_text(
                            "Crossfade between the decks — this is the transition's progress",
                        );
                    rustjay_engine::prelude::param_map_overlay(ui, &mut eng, resp.rect, "crossfader");
                    if resp.changed() {
                        eng.set_param_base("crossfader", x);
                    }
                });
            });
            ui.horizontal(|ui| {
                // The label opens the transition's own settings in the
                // inspector: softness, seed, direction, whatever it declares.
                // A clickable label rather than a button, whose padding pushed
                // the picker under the TAKE length at a 1200pt window.
                let editing = state.selection == crate::Selection::Transition;
                let ink = if editing {
                    rustjay_gui::egui_theme::colors::amber()
                } else {
                    rustjay_gui::egui_theme::colors::ink_4()
                };
                if ui
                    .add(
                        egui::Label::new(egui::RichText::new("transition").size(10.0).color(ink))
                            .sense(egui::Sense::click()),
                    )
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .on_hover_text("Edit the transition's settings")
                    .clicked()
                {
                    edit_transition = true;
                }
                egui::ComboBox::from_id_salt("transition_pick")
                    .selected_text(current_name)
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        // First, not last: a library full of transitions puts
                        // the end of this list a long scroll away.
                        if ui
                            .button("Add transition\u{2026}")
                            .on_hover_text(
                                "Any ISF transition: two image inputs and a float named progress",
                            )
                            .clicked()
                        {
                            add_transition = true;
                        }
                        ui.separator();
                        for path in state.transitions() {
                            if ui
                                .selectable_label(
                                    current.as_deref() == Some(path.as_path()),
                                    crate::transition_name(path),
                                )
                                .clicked()
                            {
                                pick = Some(path.clone());
                            }
                        }
                    });

                // Right-to-left, so TAKE sits under B.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // TAKE: an action, not a value. It arms the auto-crossfade,
                    // which writes the fader's *base* as it runs, so anything
                    // modulating the crossfader still applies once and not
                    // twice. The length is a parameter, so it is mappable.
                    let mut secs = engine
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_param_base(crate::TAKE_SECONDS)
                        .unwrap_or(1.0);
                    if ui
                        .button(egui::RichText::new("TAKE").strong().monospace())
                        .on_hover_text("Crossfade to the other deck (⌘T)")
                        .clicked()
                        && let Ok(mut m) = state.mixer.lock()
                    {
                        crate::take(&mut m, secs);
                    }
                    if ui
                        .add(
                            egui::DragValue::new(&mut secs)
                                .speed(0.05)
                                .range(0.05..=10.0)
                                .suffix("s"),
                        )
                        .on_hover_text("How long a TAKE runs")
                        .changed()
                    {
                        engine
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .set_param_base(crate::TAKE_SECONDS, secs);
                    }
                });
            });
        });

        if let Some(path) = pick {
            state.pending_transition = Some(path);
        }
        if edit_transition {
            state.selection = crate::Selection::Transition;
        }
        if add_transition {
            Self::add_transition(state, engine);
        }

        // A click on a deck's preview pops that deck out into its own window.
        #[cfg(feature = "projection")]
        for deck in (0..2).filter(|&d| clicked[d]) {
            Self::pop_out_deck(state, deck);
        }
        #[cfg(not(feature = "projection"))]
        let _ = clicked;
    }

    /// Pick a shader file and add it to the transitions: checked for the ISF
    /// transition shape, copied into the shader library — so it is there on
    /// the next launch — and loaded straight away.
    #[cfg(feature = "mixer")]
    fn add_transition(state: &mut crate::KovvbojAppState, engine: &Arc<Mutex<EngineState>>) {
        let Some(picked) = rfd::FileDialog::new()
            .set_title("Add a transition")
            .add_filter("ISF shader", &["fs"])
            .pick_file()
        else {
            return;
        };
        let notify = |message: String, level: rustjay_core::NotificationLevel| {
            engine.lock().unwrap_or_else(|e| e.into_inner()).notify(
                message,
                level,
                std::time::Duration::from_secs(6),
            );
        };
        let name = crate::transition_name(&picked);
        if !std::fs::read_to_string(&picked).is_ok_and(|src| crate::is_transition(&src)) {
            notify(
                format!(
                    "{name} is not a transition: it needs exactly two image inputs and a float named progress."
                ),
                rustjay_core::NotificationLevel::Warning,
            );
            return;
        }
        match crate::sources::registry::install_shader(&picked, &crate::shaders_dir()) {
            Ok(installed) => {
                state.pending_transition = Some(installed);
                // Into the list now, not whenever the folder watcher next looks.
                state.rescan_library();
            }
            Err(e) => notify(
                format!("Could not add {name}: {e}"),
                rustjay_core::NotificationLevel::Error,
            ),
        }
    }

    /// Open a deck's image in its own small OS window.
    ///
    /// The engine builds it as it builds a projector — surface, stage chain,
    /// close button — but keeps it on its preview list, so it never takes a
    /// projector's index. The deck is downsampled straight into the window:
    /// low resolution by window size, not by an extra texture.
    // ponytail: the image stretches if the window is resized off 16:9;
    // letterbox in `DeckPopoutStage` if that ever matters.
    #[cfg(all(feature = "projection", feature = "mixer"))]
    fn pop_out_deck(state: &mut crate::KovvbojAppState, deck: usize) {
        // An open window's stage holds the slot's only other handle, so a
        // second click on an open deck opens nothing.
        if Arc::strong_count(&state.deck_popouts[deck]) > 1 {
            return;
        }
        let slot = state.deck_popouts[deck].clone();
        let Some(handle) = state.projection_handle.as_ref() else {
            return;
        };
        let mut guard = handle.lock().unwrap_or_else(|e| e.into_inner());
        let Some(sub) = guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>() else {
            return;
        };
        let attrs = rustjay_engine::window_attributes()
            .with_title(format!("KOVVBOJ — Deck {}", ["A", "B"][deck]))
            .with_inner_size(winit::dpi::LogicalSize::new(480.0, 270.0));
        sub.add_preview(attrs, move |device, format| {
            vec![Box::new(crate::stage::DeckPopoutStage::new(device, format, slot))]
        });
    }

    /// One deck's live output, aspect-fit into `width`. Clicking it pops the
    /// deck out into its own window.
    #[cfg(feature = "mixer")]
    fn deck_preview(
        ui: &mut egui::Ui,
        engine: &Arc<Mutex<EngineState>>,
        deck: usize,
        width: f32,
    ) -> egui::Response {
        let (id, w, h) = {
            let state = engine.lock().unwrap_or_else(|e| e.into_inner());
            (
                state.deck_preview_texture_ids[deck],
                state.output_width,
                state.output_height,
            )
        };
        let aspect = if w > 0 && h > 0 {
            w as f32 / h as f32
        } else {
            16.0 / 9.0
        };
        let size = egui::vec2(width, width / aspect);
        let resp = match id {
            Some(raw) => ui.add(
                egui::Image::new((egui::TextureId::User(raw), size))
                    .fit_to_exact_size(size)
                    .sense(egui::Sense::click()),
            ),
            // Nothing published yet: hold the space so the fader does not jump
            // sideways on the frame the previews arrive. Dark and outlined —
            // a light slab reads as a blown-out image rather than an empty one.
            None => {
                let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                let painter = ui.painter();
                painter.rect_filled(rect, 2.0, ui.style().visuals.extreme_bg_color);
                painter.rect_stroke(
                    rect,
                    2.0,
                    egui::Stroke::new(1.0, rustjay_gui::egui_theme::colors::ink_2()),
                    egui::StrokeKind::Inside,
                );
                resp
            }
        };
        if cfg!(feature = "projection") {
            resp.on_hover_text("Open in its own window")
                .on_hover_cursor(egui::CursorIcon::PointingHand)
        } else {
            resp
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

#[cfg(test)]
mod tests {
    #[test]
    fn drag_edge_scroll_only_fires_in_the_margins() {
        // Middle of the span: no scroll.
        assert_eq!(crate::ui::drag_edge_scroll_delta(300.0, 100.0, 500.0), 0.0);
        // Outside the span entirely: no scroll.
        assert_eq!(crate::ui::drag_edge_scroll_delta(50.0, 100.0, 500.0), 0.0);
        assert_eq!(crate::ui::drag_edge_scroll_delta(550.0, 100.0, 500.0), 0.0);
        // Near the minimum scrolls one way, near the maximum the other,
        // faster at the very edge.
        let top_edge = crate::ui::drag_edge_scroll_delta(101.0, 100.0, 500.0);
        let top_margin = crate::ui::drag_edge_scroll_delta(120.0, 100.0, 500.0);
        assert!(top_edge < top_margin && top_margin < 0.0);
        let bottom_edge = crate::ui::drag_edge_scroll_delta(499.0, 100.0, 500.0);
        let bottom_margin = crate::ui::drag_edge_scroll_delta(480.0, 100.0, 500.0);
        assert!(bottom_edge > bottom_margin && bottom_margin > 0.0);
    }
}

/// A workspace is a directory, so name it by that directory. A dot-directory
/// (`.kovvboj`) is named for the project folder holding it — the hidden name is
/// the same for every set.
fn workspace_label(dir: &std::path::Path) -> String {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    match dir
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        Some(parent) if name.starts_with('.') => format!("{parent}/{name}"),
        _ => name.to_string(),
    }
}

/// Ask for a folder off the render thread; the result lands in `pending` for
/// the next frame to apply. A modal dialog on the UI thread would freeze the
/// output for as long as it is open.
fn pick_workspace_dir(
    action: WorkspaceAction,
    title: &str,
    pending: &Arc<Mutex<Option<(WorkspaceAction, std::path::PathBuf)>>>,
    ctx: &egui::Context,
) {
    let pending = pending.clone();
    let ctx = ctx.clone();
    let title = title.to_string();
    std::thread::spawn(move || {
        if let Some(dir) = rfd::FileDialog::new().set_title(&title).pick_folder() {
            if let Ok(mut guard) = pending.lock() {
                *guard = Some((action, dir));
            }
            ctx.request_repaint();
        }
    });
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Reveal in Finder"
    } else if cfg!(target_os = "windows") {
        "Show in Explorer"
    } else {
        "Open Folder"
    }
}

fn reveal(dir: &std::path::Path) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(opener).arg(dir).spawn() {
        log::warn!("[Workspace] could not reveal {}: {}", dir.display(), e);
    }
}

/// The set's name — the workspace folder, or the project folder holding it when
/// the workspace is the hidden `.kovvboj` inside one.
fn set_name(dir: &std::path::Path) -> String {
    let dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("set");
    if name.starts_with('.') {
        return dir
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .unwrap_or("set")
            .to_string();
    }
    name.to_string()
}

/// Ask where to write a bundle. The pack itself waits for the next frame, which
/// has the app state needed to save the live set first.
fn export_dialog(
    dir: &std::path::Path,
    pending: &Arc<Mutex<Option<(WorkspaceAction, std::path::PathBuf)>>>,
    ctx: &egui::Context,
) {
    let pending = pending.clone();
    let ctx = ctx.clone();
    let name = format!("{}.kovvbojset", set_name(dir));
    std::thread::spawn(move || {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Export Set")
            .add_filter("KOVVBOJ set", &["kovvbojset"])
            .set_file_name(&name)
            .save_file()
        {
            if let Ok(mut guard) = pending.lock() {
                *guard = Some((WorkspaceAction::Export, path));
            }
            ctx.request_repaint();
        }
    });
}

/// Pack in the background — a set with video in it is gigabytes, and the render
/// thread has frames to draw. The result is only a log line; failure leaves the
/// live set untouched.
#[cfg(feature = "mixer")]
fn export_set(
    workspace: std::path::PathBuf,
    out: std::path::PathBuf,
    busy: &Arc<Mutex<Option<String>>>,
    ctx: &egui::Context,
) {
    let busy = busy.clone();
    let ctx = ctx.clone();
    *busy.lock().unwrap_or_else(|e| e.into_inner()) = Some("exporting set…".to_string());
    std::thread::spawn(move || {
        if let Err(e) = crate::persistence::bundle::export(&workspace, &out) {
            log::warn!("[Bundle] export failed: {e}");
        }
        *busy.lock().unwrap_or_else(|e| e.into_inner()) = None;
        ctx.request_repaint();
    });
}

/// Pick a bundle, unpack it beside itself, and queue the result to be opened —
/// an imported set you then have to go and find would be half a feature.
#[cfg(feature = "mixer")]
fn import_dialog(
    pending: &Arc<Mutex<Option<(WorkspaceAction, std::path::PathBuf)>>>,
    busy: &Arc<Mutex<Option<String>>>,
    ctx: &egui::Context,
) {
    let pending = pending.clone();
    let busy = busy.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let Some(archive) = rfd::FileDialog::new()
            .set_title("Import Set")
            .add_filter("KOVVBOJ set", &["kovvbojset"])
            .pick_file()
        else {
            return;
        };
        *busy.lock().unwrap_or_else(|e| e.into_inner()) = Some("importing set…".to_string());
        ctx.request_repaint();
        match crate::persistence::bundle::import(&archive) {
            Ok(dir) => {
                if let Ok(mut guard) = pending.lock() {
                    *guard = Some((WorkspaceAction::Open, dir));
                }
            }
            Err(e) => log::warn!("[Bundle] import failed: {e}"),
        }
        *busy.lock().unwrap_or_else(|e| e.into_inner()) = None;
        ctx.request_repaint();
    });
}

#[cfg(all(test, feature = "mixer"))]
mod crossfader_tests {
    use super::*;
    use egui_kittest::kittest::Queryable as _;

    /// The fader sits on the seam between the two deck columns at any window
    /// width, and stretches with the window up to `CROSSFADER_MAX`. It used to
    /// be packed in from the left, and the width it was handed through
    /// `add_sized` was ignored, so it stayed at the theme's 200pt and drifted
    /// off-centre as the window grew.
    #[test]
    fn crossfader_is_centred_on_the_decks_and_stretches() {
        for width in [700.0_f32, 1600.0] {
            let engine = Arc::new(Mutex::new(EngineState::new()));
            let mut app = crate::KovvbojAppState::default();
            let strip = Arc::new(Mutex::new(egui::Rect::NOTHING));
            let seen = strip.clone();
            let mut harness = egui_kittest::Harness::builder()
                .with_size([width, 160.0])
                .build_ui(move |ui| {
                    *seen.lock().unwrap() = ui.available_rect_before_wrap();
                    KovvbojShell::crossfader_strip(ui, &mut app, &engine);
                });
            rustjay_gui::egui_theme::apply_professional_theme(&harness.ctx);
            harness.run();

            let strip = *strip.lock().unwrap();
            let fader = harness.get_by_role(egui::accesskit::Role::Slider).rect();
            assert!(
                (fader.center().x - strip.center().x).abs() < 1.0,
                "at {width}pt the fader centres on {} but the decks meet at {}",
                fader.center().x,
                strip.center().x
            );
            assert!(
                fader.width() > 200.0 && fader.width() <= KovvbojShell::CROSSFADER_MAX,
                "at {width}pt the fader is {}pt: past the old fixed 200, within the cap",
                fader.width()
            );
        }
    }
}
