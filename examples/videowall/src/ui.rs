//! The "Matrix" control tab: pick an output, set its grid, tweak each cell
//! (enable / aspect / orientation / source-rect nudge), calibrate, save/load.

#[cfg(feature = "videowall")]
use std::sync::atomic::Ordering;

use rustjay_engine::prelude::*;
use rustjay_projection::{AspectRatio, GridSize, Orientation, VideoMatrixConfig};

use crate::app::{OutputSync, Outputs};

const ASPECTS: [(AspectRatio, &str); 5] = [
    (AspectRatio::Ratio16_9, "16:9"),
    (AspectRatio::Ratio4_3, "4:3"),
    (AspectRatio::Ratio16_10, "16:10"),
    (AspectRatio::Ratio1_1, "1:1"),
    (AspectRatio::Ratio21_9, "21:9"),
];
const ORIENTS: [(Orientation, &str); 4] = [
    (Orientation::Normal, "0°"),
    (Orientation::Rotated90, "90°"),
    (Orientation::Rotated180, "180°"),
    (Orientation::Rotated270, "270°"),
];

#[derive(serde::Serialize, serde::Deserialize)]
struct Profile {
    names: Vec<String>,
    configs: Vec<VideoMatrixConfig>,
}

pub struct MatrixTab {
    outputs: Outputs,
    names: Vec<String>,
    selected: usize,
    /// Auto-enhance (darken+contrast) detection images before AprilTag detection.
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    enhance: bool,
    /// Detected tag size ÷ screen short side (lower → bigger cell).
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    tag_fill: f32,
    /// Last photo used for detection, so "Re-detect" can re-run after tuning.
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    last_photo: Option<std::path::PathBuf>,
    /// Loaded calibration photo as an egui texture, shown behind the overlay so
    /// bezels can be verified against the actual screens.
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    photo_tex: Option<egui::TextureHandle>,
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    photo_aspect: f32,
    /// Show the calibration photo (vs the live master output) behind the overlay.
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    show_photo: bool,
}

impl MatrixTab {
    pub fn new(outputs: Outputs) -> Self {
        let n = outputs.lock().map(|o| o.len()).unwrap_or(0);
        Self {
            outputs,
            names: (1..=n).map(|i| format!("Output {i}")).collect(),
            selected: 0,
            enhance: true,
            tag_fill: 0.5,
            last_photo: None,
            photo_tex: None,
            photo_aspect: 16.0 / 9.0,
            show_photo: true,
        }
    }

    /// Load a calibration photo into an egui texture for the preview background.
    #[cfg(feature = "videowall")]
    fn load_photo_tex(&mut self, ui: &egui::Ui, path: &std::path::Path) {
        match image::open(path) {
            Ok(img) => {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                let ci = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], rgba.as_raw());
                self.photo_tex = Some(ui.ctx().load_texture("calib_photo", ci, egui::TextureOptions::LINEAR));
                self.photo_aspect = w as f32 / h.max(1) as f32;
            }
            Err(e) => log::error!("load photo texture failed: {e}"),
        }
    }

    /// Run AprilTag detection on an image file → suggested config.
    #[cfg(feature = "videowall")]
    fn run_photo_detect(
        &self,
        path: &std::path::Path,
        grid: GridSize,
    ) -> Option<VideoMatrixConfig> {
        match image::open(path) {
            Ok(img) => {
                let det = rustjay_projection::AprilTagAutoDetector::with_config(
                    rustjay_projection::AutoDetectConfig {
                        enhance: self.enhance,
                        tag_fill: self.tag_fill,
                        ..Default::default()
                    },
                );
                let screens = det.detect_screens(&img.to_luma8());
                log::info!("photo detect: {} screen(s)", screens.len());
                Some(det.suggest_config(&screens, grid))
            }
            Err(e) => {
                log::error!("open image failed: {e}");
                None
            }
        }
    }

    fn add_output(&mut self, engine: &mut EngineState) {
        let out = OutputSync::new(GridSize::new(3, 3));
        self.outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(out.clone());
        let idx = self.names.len();
        self.names.push(format!("Output {}", idx + 1));
        self.selected = idx;

        // Spawn the projector window at runtime via the engine's projection handle.
        if let Some(handle) = engine.projection_handle.as_ref() {
            let mut guard = handle.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(sub) =
                guard.downcast_mut::<rustjay_engine::ProjectionSubsystem>()
            {
                let m = out.matrix.clone();
                #[cfg(feature = "videowall")]
                let c = out.calib.clone();
                let attrs = winit::window::WindowAttributes::default()
                    .with_title(format!("Video Wall {}", idx + 1))
                    .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
                sub.add_projector(attrs, None, move |device, format| {
                    #[allow(unused_mut)]
                    let mut stages: Vec<Box<dyn rustjay_projection::ProjectionStage>> =
                        vec![Box::new(rustjay_projection::MatrixStage::new(
                            device,
                            format,
                            m.clone(),
                        ))];
                    #[cfg(feature = "videowall")]
                    stages.push(Box::new(rustjay_projection::TagGridStage::new(
                        device,
                        format,
                        c.clone(),
                    )));
                    stages
                });
            }
        }
    }

    fn save_profile(&self) {
        let outs = self.outputs.lock().unwrap_or_else(|e| e.into_inner());
        let profile = Profile {
            names: self.names.clone(),
            configs: outs
                .iter()
                .map(|o| o.matrix.lock().unwrap_or_else(|e| e.into_inner()).config.clone())
                .collect(),
        };
        drop(outs);
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name("videowall.json")
            .save_file()
        {
            match serde_json::to_string_pretty(&profile) {
                Ok(s) => {
                    if let Err(e) = std::fs::write(&path, s) {
                        log::error!("save failed: {e}");
                    }
                }
                Err(e) => log::error!("serialize failed: {e}"),
            }
        }
    }

    fn load_profile(&mut self) {
        let Some(path) = rfd::FileDialog::new().add_filter("JSON", &["json"]).pick_file() else {
            return;
        };
        let profile: Profile = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(p) => p,
            None => {
                log::error!("load failed: {}", path.display());
                return;
            }
        };
        // Apply to existing outputs by index (does not add/remove windows).
        let outs = self.outputs.lock().unwrap_or_else(|e| e.into_inner());
        for (o, cfg) in outs.iter().zip(profile.configs) {
            o.matrix.lock().unwrap_or_else(|e| e.into_inner()).set_config(cfg);
        }
        for (n, name) in self.names.iter_mut().zip(profile.names) {
            *n = name;
        }
    }

    fn draw_inner(&mut self, ui: &mut egui::Ui, engine: &mut EngineState) {
        // ── Output selector ───────────────────────────────────────────────
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("output_sel")
                .selected_text(
                    self.names.get(self.selected).cloned().unwrap_or_default(),
                )
                .show_ui(ui, |ui| {
                    for (i, name) in self.names.iter().enumerate() {
                        ui.selectable_value(&mut self.selected, i, name);
                    }
                });
            if ui.button("➕ Add output").clicked() {
                self.add_output(engine);
            }
            ui.label("(close a wall window to remove it)");
        });
        if let Some(name) = self.names.get_mut(self.selected) {
            ui.horizontal(|ui| {
                ui.label("Name:");
                ui.text_edit_singleline(name);
            });
        }
        ui.separator();

        let Some(out) = self
            .outputs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(self.selected)
            .cloned()
        else {
            ui.label("No outputs.");
            return;
        };

        let mut cfg = out.matrix.lock().unwrap_or_else(|e| e.into_inner()).config.clone();
        let mut dirty = false;

        // ── Grid ──────────────────────────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Grid:");
            let mut cols = cfg.output_grid.columns;
            let mut rows = cfg.output_grid.rows;
            let c = ui.add(egui::DragValue::new(&mut cols).range(1..=8));
            ui.label("×");
            let r = ui.add(egui::DragValue::new(&mut rows).range(1..=8));
            if c.changed() || r.changed() {
                cfg.output_grid = GridSize::new(cols, rows);
                cfg.input_grid.grid_size = cfg.output_grid;
                cfg.input_grid.create_default_mapping();
                dirty = true;
            }
            if ui.button("Reset cells").clicked() {
                cfg.input_grid.create_default_mapping();
                dirty = true;
            }
        });

        // ── Output aspect (matrix framebuffer) — letterboxed into the window ──
        ui.horizontal(|ui| {
            ui.label("Output aspect:");
            let mut a = cfg.output_aspect;
            for (label, val) in [
                ("16:9", 16.0 / 9.0),
                ("4:3", 4.0 / 3.0),
                ("16:10", 16.0 / 10.0),
                ("1:1", 1.0),
                ("21:9", 21.0 / 9.0),
            ] {
                if ui.selectable_label((a - val).abs() < 1e-3, label).clicked() {
                    a = val;
                }
            }
            if (a - cfg.output_aspect).abs() > 1e-6 {
                cfg.output_aspect = a;
                dirty = true;
            }
        });

        // ── Calibration + auto-detect ─────────────────────────────────────
        #[cfg(feature = "videowall")]
        {
            ui.horizontal(|ui| {
                let mut active =
                    out.calib.lock().unwrap_or_else(|e| e.into_inner()).active;
                if ui.checkbox(&mut active, "Calibrate (show tag grid)").changed() {
                    out.calib
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .set(active, cfg.output_grid);
                }
                if ui.button("🔍 Auto-detect (live input)").clicked() {
                    out.detect_req.store(true, Ordering::SeqCst);
                }
                ui.label("(point a camera input at the wall)");
            });
            ui.horizontal(|ui| {
                ui.label("Tag fill:");
                ui.add(egui::Slider::new(&mut self.tag_fill, 0.1..=1.0).fixed_decimals(2))
                    .on_hover_text("detected tag size ÷ screen short side — lower makes cells bigger");
                ui.checkbox(&mut self.enhance, "Enhance");
            });
            ui.horizontal(|ui| {
                if ui.button("🖼 Detect from photo…").clicked()
                    && let Some(path) = rfd::FileDialog::new()
                        .add_filter("image", &["png", "jpg", "jpeg", "bmp"])
                        .pick_file()
                {
                    self.load_photo_tex(ui, &path);
                    if let Some(new_cfg) = self.run_photo_detect(&path, cfg.output_grid) {
                        out.matrix.lock().unwrap_or_else(|e| e.into_inner()).set_config(new_cfg.clone());
                        cfg = new_cfg;
                    }
                    self.last_photo = Some(path);
                }
                if self.last_photo.is_some() && ui.button("↻ Re-detect").clicked() {
                    let path = self.last_photo.clone().unwrap();
                    self.load_photo_tex(ui, &path);
                    if let Some(new_cfg) = self.run_photo_detect(&path, cfg.output_grid) {
                        out.matrix.lock().unwrap_or_else(|e| e.into_inner()).set_config(new_cfg.clone());
                        cfg = new_cfg;
                    }
                }
            });
        }

        // ── Preview with sampled-region overlay ───────────────────────────
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Sampled regions:");
            #[cfg(feature = "videowall")]
            if self.photo_tex.is_some() {
                ui.checkbox(&mut self.show_photo, "calibration photo");
            }
        });
        let master_aspect = engine.resolution.internal_width as f32
            / engine.resolution.internal_height.max(1) as f32;
        #[cfg(feature = "videowall")]
        let (ptex, paspect) = if self.show_photo && self.photo_tex.is_some() {
            (self.photo_tex.as_ref().map(|t| t.id()), self.photo_aspect)
        } else {
            (engine.stage_preview_texture_id.map(egui::TextureId::User), master_aspect)
        };
        #[cfg(not(feature = "videowall"))]
        let (ptex, paspect) = (
            engine.stage_preview_texture_id.map(egui::TextureId::User),
            master_aspect,
        );
        draw_preview(ui, ptex, paspect, &cfg);
        ui.separator();

        // ── Per-cell table ────────────────────────────────────────────────
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (i, m) in cfg.input_grid.mappings.iter_mut().enumerate() {
                ui.push_id(i, |ui| {
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut m.enabled, "").changed() {
                            dirty = true;
                        }
                        ui.label(format!(
                            "cell {i} → ({:.0},{:.0})",
                            m.output_position.col, m.output_position.row
                        ));

                        // aspect
                        let mut aspect = m.aspect_ratio;
                        egui::ComboBox::from_id_salt("aspect")
                            .width(60.0)
                            .selected_text(aspect_label(aspect))
                            .show_ui(ui, |ui| {
                                for (a, lbl) in ASPECTS {
                                    ui.selectable_value(&mut aspect, a, lbl);
                                }
                            });
                        if aspect != m.aspect_ratio {
                            m.aspect_ratio = aspect;
                            dirty = true;
                        }

                        // orientation
                        let mut orient = m.orientation;
                        egui::ComboBox::from_id_salt("orient")
                            .width(55.0)
                            .selected_text(orient_label(orient))
                            .show_ui(ui, |ui| {
                                for (o, lbl) in ORIENTS {
                                    ui.selectable_value(&mut orient, o, lbl);
                                }
                            });
                        if orient != m.orientation {
                            m.orientation = orient;
                            dirty = true;
                        }
                    });

                    // Wall-geometry nudge (aspect-neutral; aspect-locked size). Edits
                    // wall_center (cx/cy) and a single uniform size scale so the
                    // display aspect is preserved and the box never reforms.
                    ui.horizontal(|ui| {
                        ui.label("    wall:");
                        let (mut cx, mut cy) = match m.wall_center {
                            Some(c) => (c[0], c[1]),
                            None => (0.5, 0.5),
                        };
                        let (mut wx, mut wy) = match m.wall_size {
                            Some(s) => (s[0], s[1]),
                            None => (0.3, 0.3),
                        };
                        let mut ch = false;
                        ui.label("cx");
                        ch |= ui
                            .add(egui::DragValue::new(&mut cx).speed(0.002).range(-2.0..=3.0))
                            .changed();
                        ui.label("cy");
                        ch |= ui
                            .add(egui::DragValue::new(&mut cy).speed(0.002).range(-2.0..=3.0))
                            .changed();
                        // Aspect-locked size: scale both axes by the same factor.
                        let aspect = if wy > 0.0 { wx / wy } else { 1.0 };
                        ui.label("size");
                        if ui
                            .add(egui::DragValue::new(&mut wy).speed(0.002).range(0.001..=3.0))
                            .changed()
                        {
                            wx = wy * aspect;
                            ch = true;
                        }
                        if ch {
                            m.wall_center = Some([cx, cy]);
                            m.wall_size = Some([wx, wy]);
                            // A manual nudge supersedes any stale override.
                            m.custom_source_rect = None;
                            dirty = true;
                        }
                    });

                    // per-display adjustments (brightness / contrast / gamma)
                    ui.horizontal(|ui| {
                        ui.label("    adj:");
                        ui.label("b");
                        dirty |= ui
                            .add(egui::DragValue::new(&mut m.brightness).speed(0.01).range(0.0..=2.0))
                            .changed();
                        ui.label("c");
                        dirty |= ui
                            .add(egui::DragValue::new(&mut m.contrast).speed(0.01).range(0.0..=2.0))
                            .changed();
                        ui.label("g");
                        dirty |= ui
                            .add(egui::DragValue::new(&mut m.gamma).speed(0.01).range(0.1..=3.0))
                            .changed();
                    });
                });
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("💾 Save profile").clicked() {
                // write back any pending edit first
                if dirty {
                    out.matrix.lock().unwrap_or_else(|e| e.into_inner()).set_config(cfg.clone());
                    dirty = false;
                }
                self.save_profile();
            }
            if ui.button("📂 Load profile").clicked() {
                self.load_profile();
            }
        });

        if dirty {
            out.matrix.lock().unwrap_or_else(|e| e.into_inner()).set_config(cfg);
        }
    }
}

impl AnyEguiTab for MatrixTab {
    fn name(&self) -> &str {
        "Matrix"
    }

    fn draw(&mut self, ui: &mut egui::Ui, _app_state: &mut dyn std::any::Any, engine: &mut EngineState) {
        ui.push_id("videowall_matrix_tab", |ui| self.draw_inner(ui, engine));
    }
}

/// Live master-output preview with the sampled source-rects drawn over it, so
/// you can see which areas each cell pulls from. The master output (passthrough
/// of the source) backs the canvas; input UV maps 1:1 to it.
fn draw_preview(ui: &mut egui::Ui, preview: Option<egui::TextureId>, aspect: f32, cfg: &VideoMatrixConfig) {
    // Fit width, cap height, keep aspect.
    let mut cw = ui.available_width().min(640.0);
    let mut ch = cw / aspect;
    if ch > 360.0 {
        ch = 360.0;
        cw = ch * aspect;
    }
    let (rect, _) = ui.allocate_exact_size(egui::vec2(cw, ch), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    if let Some(tex) = preview {
        painter.image(
            tex,
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            egui::Color32::from_gray(150), // dim a touch so overlays read
        );
    } else {
        painter.rect_filled(rect, egui::CornerRadius::ZERO, egui::Color32::from_gray(18));
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "no preview",
            egui::FontId::proportional(12.0),
            egui::Color32::GRAY,
        );
    }

    // Resolve source rects via the SAME uniform bbox fit the GPU uses, for the
    // shown background's aspect — so the layout reads undistorted on both the 4:3
    // calibration photo and a 16:9 webcam (only uniformly scaled/centred).
    let rects = rustjay_projection::resolve_source_rects(cfg, aspect);
    for ((i, m), s) in cfg
        .input_grid
        .mappings
        .iter()
        .enumerate()
        .filter(|(_, m)| m.enabled)
        .zip(rects)
    {
        let r = egui::Rect::from_min_size(
            egui::pos2(rect.min.x + s.x * rect.width(), rect.min.y + s.y * rect.height()),
            egui::vec2(s.width * rect.width(), s.height * rect.height()),
        );
        let color = egui::Color32::from_rgb(0, 220, 120);
        painter.rect_stroke(r, egui::CornerRadius::ZERO, egui::Stroke::new(2.0, color), egui::StrokeKind::Inside);
        let label = match m.display_id {
            Some(id) => format!("{i}·#{id}"),
            None => format!("{i}"),
        };
        painter.text(
            r.min + egui::vec2(3.0, 2.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::monospace(12.0),
            color,
        );
    }
}

fn aspect_label(a: AspectRatio) -> &'static str {
    ASPECTS.iter().find(|(x, _)| *x == a).map(|(_, l)| *l).unwrap_or("?")
}
fn orient_label(o: Orientation) -> &'static str {
    ORIENTS.iter().find(|(x, _)| *x == o).map(|(_, l)| *l).unwrap_or("?")
}
