//! Laser tab — load a MadMapper laser material, watch the path it draws, and
//! stream it to a DAC.
//!
//! Laser decks are a pipeline of their own, parallel to the video mixer: a
//! laser material generates 2D paths rather than pixels, so it never enters a
//! channel or an FX chain. What appears here is the whole of it.
//!
//! The preview is not a nicety. Nothing in this pipeline can be checked by
//! pointing a projector at a wall to see what happens, so the path has to be
//! visible on screen — including the blanked jumps, which are where a laser
//! looks wrong long before the shapes do.
//!
//! The per-frame render lives in [`LaserTab::pump`], which needs a device and
//! an encoder and so is called from the render hook rather than from `draw`.

use std::sync::Mutex;

use rustjay_core::EngineState;
use rustjay_engine::prelude::*;
use rustjay_laser::{Budget, LaserDeck, LaserFrame};

/// Laser decks and the controls for them.
///
/// The decks sit behind a `Mutex` so the tab stays `Send + Sync`: a deck owns
/// a readback channel, which is `Send` but not `Sync`. Same reason the LED tab
/// does it.
pub struct LaserTab {
    decks: Mutex<Vec<LaserDeck>>,
    /// Which deck the panel is showing. Decks are a `Vec` from the start
    /// because a rig with two projectors is a rig with two decks, and the save
    /// format is the expensive thing to change later — but only one is shown
    /// until there is a reason for more.
    selected: usize,
    /// Path typed into the loader.
    path: String,
    /// Scanner settings, applied on Retune rather than live: the point count
    /// is the render target's width, and every feedback material indexes its
    /// history by it.
    pending: Budget,
    /// Show the blanked travel moves, which is usually what you want while
    /// building and never what you want while judging the picture.
    show_blanking: bool,
    /// Set by `draw`, acted on by `pump`. Loading a material and resizing its
    /// targets both need a device, which the UI pass does not have.
    pending_load: bool,
    pending_retune: bool,
    /// Made on the first pump and kept. A deck samples its own previous target
    /// as `mm_LastFrameData`, so the sampler has nothing to do with whatever
    /// the mixer happens to have bound — asking the render hook for one only
    /// made the laser depend on there being a video input at all.
    sampler: Option<wgpu::Sampler>,
    #[cfg(feature = "laser-dac")]
    dac: DacPanel,
    /// A calibration run, when one is going.
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    calib: Mutex<Option<Calibration>>,
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    camera_index: usize,
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    calib_status: Option<String>,
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    calib_error: Option<String>,
    /// A solved correction waiting to be collected by the stage, which owns
    /// the corner-pin and is what persists it.
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    calib_result: Option<rustjay_laser::Geometry>,
}

/// A camera-assisted 4-point calibration in progress.
///
/// Steps through the four ring targets, records where the camera saw each, and
/// solves the corner-pin that puts the field where it was asked to go. Every
/// target goes out through [`rustjay_laser::Safety`] unchanged — the rings are
/// shaped to pass that gate honestly, not to get around it.
#[cfg(all(feature = "laser-dac", feature = "webcam"))]
struct Calibration {
    _cam: rustjay_io::WebcamCapture, // dropping it stops capture
    rx: std::sync::mpsc::Receiver<rustjay_io::WebcamFrame>,
    /// Which target is showing, 0..4. Reaching 4 means all are measured.
    step: usize,
    /// Frames the ring has been up for. The galvos and the camera's exposure
    /// both need a moment to settle before the blob means anything.
    held: u32,
    observed: [[f32; 2]; 4],
    threshold: u8,
    hold_frames: u32,
}

#[cfg(feature = "laser-dac")]
#[derive(Default)]
struct DacPanel {
    /// Devices from the last scan. Network DACs answer a broadcast, so an
    /// empty list right after start-up may only mean nobody has replied.
    found: Vec<rustjay_laser::dac::DacInfo>,
    chosen: Option<String>,
    output: Option<rustjay_laser::dac::DacOutput>,
    error: Option<String>,
}

impl Default for LaserTab {
    fn default() -> Self {
        Self::new()
    }
}

impl LaserTab {
    pub fn new() -> Self {
        Self {
            decks: Mutex::new(Vec::new()),
            selected: 0,
            path: String::new(),
            pending: Budget::default(),
            show_blanking: true,
            pending_load: false,
            pending_retune: false,
            sampler: None,
            #[cfg(all(feature = "laser-dac", feature = "webcam"))]
            calib: Mutex::new(None),
            #[cfg(all(feature = "laser-dac", feature = "webcam"))]
            camera_index: 0,
            #[cfg(all(feature = "laser-dac", feature = "webcam"))]
            calib_status: None,
            #[cfg(all(feature = "laser-dac", feature = "webcam"))]
            calib_error: None,
            #[cfg(all(feature = "laser-dac", feature = "webcam"))]
            calib_result: None,
            #[cfg(feature = "laser-dac")]
            dac: DacPanel::default(),
        }
    }

    /// Render every deck that is due and collect finished readbacks.
    ///
    /// Call once per engine frame from the render hook. Decks keep the
    /// scanner's clock, not the engine's, so most frames this does nothing but
    /// poll — rendering faster than the beam draws would run the material's
    /// time and feedback history ahead of what anybody sees.
    pub fn pump(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        engine: &EngineState,
        quad: &wgpu::Buffer,
    ) {
        // Cloned out rather than borrowed: `wgpu::Sampler` is a handle, and
        // holding a borrow of `self` here would block the deck list below.
        let sampler = self
            .sampler
            .get_or_insert_with(|| {
                device.create_sampler(&wgpu::SamplerDescriptor {
                    label: Some("Laser feedback sampler"),
                    ..Default::default()
                })
            })
            .clone();

        if std::mem::take(&mut self.pending_load) {
            self.load(device, queue);
        }
        if std::mem::take(&mut self.pending_retune) {
            let budget = self.pending;
            let selected = self.selected;
            let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(deck) = decks.get_mut(selected) {
                deck.retune(budget, device, queue);
            }
        }

        let now = std::time::Instant::now();
        let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
        for deck in decks.iter_mut() {
            if deck.due(now) {
                deck.render(device, queue, encoder, engine, quad, &sampler);
            }
            deck.poll(device);
        }

        // A calibration owns the DAC while it runs — it is showing its own
        // targets, and the deck's material would fight it for the beam.
        #[cfg(all(feature = "laser-dac", feature = "webcam"))]
        let calibrating = self.calib.lock().map(|g| g.is_some()).unwrap_or(false);
        #[cfg(all(feature = "laser-dac", not(feature = "webcam")))]
        let calibrating = false;

        #[cfg(feature = "laser-dac")]
        if let Some(output) = &self.dac.output
            && !calibrating
        {
            // The gate decides; this only carries. A deck holding output dark
            // sends nothing, and the DAC replays its last frame — so a blanked
            // frame goes out explicitly rather than by omission.
            match decks.get_mut(self.selected).and_then(LaserDeck::output) {
                Some(frame) => output.send(frame),
                None => output.blank(),
            }
        }
    }

    /// Load a laser material into a new deck.
    fn load(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let path = std::path::PathBuf::from(self.path.trim());
        match LaserDeck::from_path(&path, self.pending) {
            Ok(mut deck) => {
                deck.init(device, queue);
                if let Some(err) = &deck.error {
                    log::error!("[laser] {}: {err}", deck.name);
                }
                let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
                self.selected = decks.len();
                decks.push(deck);
            }
            Err(e) => log::error!("[laser] could not load {}: {e}", path.display()),
        }
    }
}

/// Paint a frame's path into `rect`, scan field mapped to fit.
///
/// Lit segments are drawn in their own colour; blanked ones — the beam
/// travelling between strokes — faintly, because where the jumps go is most of
/// what makes a laser picture look right or wrong.
fn paint_path(painter: &egui::Painter, rect: egui::Rect, frame: &LaserFrame, show_blanking: bool) {
    let half = rect.width().min(rect.height()) * 0.5;
    let centre = rect.center();
    // The scan field is 2 across, y up; screen y runs down.
    let to_screen = |p: &rustjay_laser::LaserPoint| {
        egui::pos2(centre.x + p.x * half, centre.y - p.y * half)
    };

    for pair in frame.points.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        // A segment is only lit if both ends are: the beam is off for the whole
        // move into a blanked point.
        let lit = !a.is_blank() && !b.is_blank();
        if !lit && !show_blanking {
            continue;
        }
        let stroke = if lit {
            egui::Stroke::new(
                1.5,
                egui::Color32::from_rgb(
                    (b.r * 255.0) as u8,
                    (b.g * 255.0) as u8,
                    (b.b * 255.0) as u8,
                ),
            )
        } else {
            egui::Stroke::new(0.5, egui::Color32::from_rgb(48, 48, 60))
        };
        painter.line_segment([to_screen(a), to_screen(b)], stroke);
    }
}

impl AnyEguiTab for LaserTab {
    fn name(&self) -> &str {
        "Laser"
    }

    fn draw(&mut self, ui: &mut egui::Ui, _app: &mut dyn std::any::Any, _engine: &mut EngineState) {
        self.ui(ui);
    }
}

#[cfg(all(feature = "laser-dac", feature = "webcam"))]
impl LaserTab {
    /// Begin a calibration run.
    ///
    /// Refuses rather than arming anything itself: the beam only ever goes live
    /// because a person said so, and a calibration is no more entitled to that
    /// than a show is.
    fn calib_start(&mut self, camera_index: usize) -> Result<(), String> {
        if self.dac.output.is_none() {
            return Err("no DAC connected — calibration needs the beam on a wall".into());
        }
        let armed = {
            let decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
            decks.get(self.selected).is_some_and(|d| d.safety.is_armed())
        };
        if !armed {
            return Err("arm the output first — calibration will not arm it for you".into());
        }

        let mut cam = rustjay_io::WebcamCapture::new(camera_index, 1280, 720, 30)
            .map_err(|e| format!("camera error: {e}"))?;
        let rx = cam.start().map_err(|e| format!("camera start error: {e}"))?;

        // Measure the raw rig. The rings are emitted through whatever
        // correction is already loaded, so calibrating on top of one would
        // measure the pair and solve for the wrong thing.
        {
            let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(deck) = decks.get_mut(self.selected) {
                deck.geometry = rustjay_laser::Geometry::identity();
            }
        }

        *self.calib.lock().unwrap_or_else(|e| e.into_inner()) = Some(Calibration {
            _cam: cam,
            rx,
            step: 0,
            held: 0,
            observed: [[0.0; 2]; 4],
            threshold: 64,
            hold_frames: 8,
        });
        Ok(())
    }

    fn calib_stop(&self) {
        self.calib.lock().unwrap_or_else(|e| e.into_inner()).take();
    }

    /// One frame of a calibration run: hold the ring up, then measure it.
    ///
    /// Returns the step reached, and whether all four are done.
    fn calib_pump(&self) -> Option<(usize, bool)> {
        use rustjay_laser::calibrate::{TARGET_RADIUS, target_frame};

        let mut guard = self.calib.lock().unwrap_or_else(|e| e.into_inner());
        let run = guard.as_mut()?;
        if run.step >= 4 {
            return Some((4, true));
        }

        // Show the ring. Through the gate, every frame, like anything else.
        let frame = target_frame(run.step, TARGET_RADIUS);
        {
            let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
            let passed = decks
                .get_mut(self.selected)
                .map(|d| d.safety.gate(&frame, true).is_some())
                .unwrap_or(false);
            match (passed, &self.dac.output) {
                (true, Some(out)) => out.send(&frame),
                (false, Some(out)) => out.blank(),
                _ => {}
            }
        }

        // Newest camera frame only — a backlog is all stale exposures.
        let mut latest = None;
        while let Ok(f) = run.rx.try_recv() {
            latest = Some(f);
        }
        let Some(f) = latest else { return Some((run.step, false)) };

        run.held += 1;
        if run.held <= run.hold_frames {
            return Some((run.step, false));
        }

        let luma = bgra_to_luma(&f.data);
        let (w, h) = (f.width as usize, f.height as usize);
        if let Some(blob) = rustjay_ledmap::detect::brightest_blob(&luma, w, h, run.threshold) {
            run.observed[run.step] = [blob.x, blob.y];
            run.step += 1;
            run.held = 0;
        }
        Some((run.step, run.step >= 4))
    }

    /// Solve from what was measured and load the result onto the deck.
    fn calib_finish(&mut self) -> String {
        use rustjay_laser::calibrate::{TARGET_RADIUS, solve, squared_up};

        let observed = {
            let guard = self.calib.lock().unwrap_or_else(|e| e.into_inner());
            match guard.as_ref() {
                Some(run) if run.step >= 4 => run.observed,
                _ => return "not finished — all four targets have to be seen".into(),
            }
        };
        // Square up what the laser already reaches: undo the keystone, keep the
        // coverage. The corners stay editable afterwards.
        let desired = squared_up(&observed);
        let geometry = solve(&observed, &desired, TARGET_RADIUS);

        self.calib_result = Some(geometry);
        self.calib_stop();
        if let Some(out) = &self.dac.output {
            out.blank();
        }
        "Calibrated — corners loaded, edit them below if the fit is off".into()
    }
}

/// Luma from a BGRA webcam frame, for blob detection.
#[cfg(all(feature = "laser-dac", feature = "webcam"))]
fn bgra_to_luma(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4)
        .map(|p| {
            // Rec. 601, the same weighting `rustjay-ledmap` calibrates against.
            ((p[2] as f32 * 0.299) + (p[1] as f32 * 0.587) + (p[0] as f32 * 0.114)) as u8
        })
        .collect()
}

impl LaserTab {
    /// Push the corner-pin the stage owns onto the selected deck.
    pub fn set_geometry(&self, geometry: rustjay_laser::Geometry) {
        let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(deck) = decks.get_mut(self.selected) {
            deck.geometry = geometry;
        }
    }

    /// A calibration's answer, once. The stage collects it, stores it, and
    /// pushes it back down as the owner of the value.
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    pub fn take_calibration(&mut self) -> Option<rustjay_laser::Geometry> {
        self.calib_result.take()
    }

    /// Run `f` over the selected deck's latest path, if there is one.
    ///
    /// A closure rather than a returned `Vec`: a frame is thousands of points
    /// and this is called every time the stage canvas repaints, so handing out
    /// a copy would be a per-frame allocation for a preview.
    pub fn with_preview<R>(&self, f: impl FnOnce(&LaserFrame) -> R) -> Option<R> {
        let decks = self.decks.lock().ok()?;
        decks.get(self.selected).map(|d| f(d.frame()))
    }

    /// The panel body.
    ///
    /// Inherent as well as on the trait, so the Stage tab's LASERS layer can
    /// draw it while already holding the app state this lives in — going
    /// through [`AnyEguiTab::draw`] there would want that state a second time.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Laser");
        ui.label(
            egui::RichText::new("MadMapper laser materials — 2D paths, not pixels")
                .size(11.0)
                .weak(),
        );
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Material");
            ui.text_edit_singleline(&mut self.path);
            if ui.button("Browse…").clicked()
                && let Some(picked) = rfd::FileDialog::new()
                    .add_filter("ISF shader", &["fs"])
                    .pick_file()
            {
                self.path = picked.display().to_string();
            }
            // Loading needs a device, which `draw` has no access to; the render
            // hook does it on the next pump. See `pending_load`.
            if ui.button("Load").clicked() {
                self.pending_load = true;
            }
        });

        let mut decks = self.decks.lock().unwrap_or_else(|e| e.into_inner());
        if decks.is_empty() {
            ui.label(egui::RichText::new("No laser deck loaded.").weak());
            return;
        }
        self.selected = self.selected.min(decks.len() - 1);

        ui.horizontal(|ui| {
            for (i, deck) in decks.iter().enumerate() {
                ui.selectable_value(&mut self.selected, i, &deck.name);
            }
        });
        ui.separator();

        let deck = &mut decks[self.selected];

        // ── safety ────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            let armed = deck.safety.is_armed();
            let label = if armed { "DISARM" } else { "ARM" };
            let colour = if armed {
                egui::Color32::from_rgb(200, 60, 60)
            } else {
                egui::Color32::from_rgb(60, 140, 60)
            };
            if ui
                .add(egui::Button::new(egui::RichText::new(label).strong()).fill(colour))
                .clicked()
            {
                if armed {
                    deck.safety.disarm();
                } else {
                    deck.safety.arm();
                }
            }
            if ui.button("BLACKOUT").clicked() {
                deck.safety.disarm();
            }
            if let Some(blocked) = deck.safety.blocked {
                ui.label(
                    egui::RichText::new(blocked.reason())
                        .color(egui::Color32::from_rgb(220, 160, 60)),
                );
            }
        });
        if let Some(err) = &deck.error {
            ui.colored_label(egui::Color32::from_rgb(220, 90, 90), err);
        }
        ui.separator();

        // ── preview ───────────────────────────────────────────────────────
        let size = ui.available_width().min(360.0);
        let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, egui::Color32::from_rgb(12, 12, 16));
        paint_path(&painter, rect, deck.frame(), self.show_blanking);
        ui.checkbox(&mut self.show_blanking, "Show blanked travel");
        ui.label(
            egui::RichText::new(format!(
                "{} points asked of the material, {} drawn after optimisation",
                deck.point_count(),
                deck.frame().points.len()
            ))
            .size(11.0)
            .weak(),
        );
        ui.separator();

        // ── scanner ───────────────────────────────────────────────────────
        ui.label(egui::RichText::new("Scanner").strong());
        ui.add(
            egui::Slider::new(&mut self.pending.points_per_second, 5_000..=60_000)
                .text("points / second"),
        );
        ui.add(egui::Slider::new(&mut self.pending.refresh_hz, 10.0..=60.0).text("refresh (Hz)"));
        ui.label(
            egui::RichText::new(format!(
                "budget: {} points a pass",
                self.pending.points(None)
            ))
            .size(11.0)
            .weak(),
        );
        if self.pending != deck.budget() && ui.button("Retune").clicked() {
            self.pending_retune = true;
        }

        // ── optimiser ─────────────────────────────────────────────────────
        ui.separator();
        ui.collapsing("Path settling", |ui| {
            ui.label(
                egui::RichText::new(
                    "Extra points so the mirrors can keep up. Unverified against \
                     hardware — set these against a real projector.",
                )
                .size(11.0)
                .weak(),
            );
            ui.add(egui::Slider::new(&mut deck.optimiser.blank_dwell, 0..=16).text("blank dwell"));
            ui.add(egui::Slider::new(&mut deck.optimiser.corner_dwell, 0..=16).text("corner dwell"));
            ui.add(
                egui::Slider::new(&mut deck.optimiser.stroke_repeat, 0..=16).text("stroke repeat"),
            );
            ui.add(
                egui::Slider::new(&mut deck.optimiser.corner_threshold, 0.05..=1.5)
                    .text("corner angle (rad)"),
            );
            ui.checkbox(&mut deck.optimiser.skip_black, "Skip dark strokes");
        });

        // The DAC panel touches `self`, so the decks guard has to go first.
        drop(decks);
        #[cfg(feature = "laser-dac")]
        {
            ui.separator();
            self.draw_dac(ui);
        }
    }
}

#[cfg(feature = "laser-dac")]
impl LaserTab {
    fn draw_dac(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new("Output").strong());
        ui.horizontal(|ui| {
            if ui.button("Scan for DACs").clicked() {
                self.dac.found = rustjay_laser::dac::list();
                self.dac.error = None;
            }
            ui.label(
                egui::RichText::new(format!("{} found", self.dac.found.len()))
                    .size(11.0)
                    .weak(),
            );
        });
        for info in &self.dac.found {
            let chosen = self.dac.chosen.as_deref() == Some(info.id.as_str());
            if ui
                .selectable_label(chosen, format!("{} — {}", info.name, info.kind.display_name()))
                .clicked()
            {
                self.dac.chosen = Some(info.id.clone());
            }
        }
        if let Some(id) = self.dac.chosen.clone() {
            ui.horizontal(|ui| {
                if self.dac.output.is_none() && ui.button("Connect").clicked() {
                    match rustjay_laser::dac::DacOutput::open(id.as_str(), self.pending.points_per_second)
                    {
                        Ok(output) => {
                            // The DAC's own arm mirrors the deck's gate; both
                            // have to agree before light leaves the projector.
                            if let Err(e) = output.arm() {
                                self.dac.error = Some(e.to_string());
                            }
                            self.dac.output = Some(output);
                        }
                        Err(e) => self.dac.error = Some(e.to_string()),
                    }
                }
                if self.dac.output.is_some() && ui.button("Disconnect").clicked() {
                    self.dac.output = None; // dropping stops the stream
                }
                if let Some(output) = &self.dac.output {
                    let state = if output.connected() { "connected" } else { "lost" };
                    ui.label(egui::RichText::new(state).size(11.0).weak());
                }
            });
        }
        if let Some(err) = &self.dac.error {
            ui.colored_label(egui::Color32::from_rgb(220, 90, 90), err);
        }

        #[cfg(all(feature = "laser-dac", feature = "webcam"))]
        self.calibration_ui(ui);
    }

    /// The camera-assisted 4-point wizard.
    #[cfg(all(feature = "laser-dac", feature = "webcam"))]
    fn calibration_ui(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.label(egui::RichText::new("Calibrate with a camera").strong());

        let running = self.calib.lock().unwrap_or_else(|e| e.into_inner()).is_some();
        if !running {
            ui.label(
                egui::RichText::new(
                    "Points the beam at four corners in turn and watches where they land. \
                     Targets are small traced rings, never parked dots — a stationary beam \
                     is the one thing the safety gate exists to stop.",
                )
                .size(11.0)
                .weak(),
            );
            ui.horizontal(|ui| {
                ui.label("Camera");
                ui.add(egui::DragValue::new(&mut self.camera_index).range(0..=8));
                if ui.button("Start").clicked()
                    && let Err(e) = self.calib_start(self.camera_index)
                {
                    self.calib_error = Some(e);
                }
            });
            if let Some(err) = &self.calib_error {
                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), err);
            }
            if let Some(msg) = &self.calib_status {
                ui.label(msg);
            }
            return;
        }

        // A run is live: drive it, and repaint so it keeps stepping.
        ui.ctx().request_repaint();
        let (step, done) = self.calib_pump().unwrap_or((0, false));
        ui.label(format!("Target {} of 4", (step + 1).min(4)));
        ui.add(egui::ProgressBar::new(step as f32 / 4.0).show_percentage());

        ui.horizontal(|ui| {
            if done && ui.button("Solve").clicked() {
                self.calib_status = Some(self.calib_finish());
                self.calib_error = None;
            }
            if ui.button("Cancel").clicked() {
                self.calib_stop();
                if let Some(out) = &self.dac.output {
                    out.blank();
                }
            }
        });
    }
}
