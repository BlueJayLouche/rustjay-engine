//! LED Mapper Studio — a standalone CV LED-mapping tool.
//!
//! Webcam in, sACN out, one window. Flash a strip LED-by-LED, recover each
//! LED's position from the camera, export `ledmap.json`. All the CV/format
//! logic is reused from `rustjay-ledmap`; this binary is just the shell:
//! camera capture (nokhwa), sACN drive (rustjay-lighting), and an egui UI.
//!
//! `// ponytail:` the camera is captured on the UI thread (nokhwa's `Camera` is
//! `!Send` so it can't move to a worker). `frame()` blocks ~one camera frame,
//! which paces the UI at the camera's framerate — fine for a calibration tool.

use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{CameraIndex, RequestedFormat, RequestedFormatType};
use nokhwa::Camera;
use rustjay_ledmap::{CalibrationSession, LedMap, SequentialCalibrator};
use rustjay_lighting::{Dest, DmxSender, SacnTransport};

/// Most recent decoded camera frame (RGB8, row-major).
struct CamFrame {
    rgb: Vec<u8>,
    w: usize,
    h: usize,
}

/// A live calibration run: the sACN sender + the session FSM.
struct Run {
    dmx: DmxSender,
    session: CalibrationSession,
}

struct App {
    // config
    cam_index: u32,
    led_count: u32,
    start_universe: u16,
    start_channel: u16,
    order: String,
    threshold: u8,
    hold_frames: u32,
    priority: u8,
    out_path: String,

    // camera (lives on the UI thread — nokhwa Camera is !Send)
    cam: Option<Camera>,
    latest: Option<CamFrame>,
    tex: Option<egui::TextureHandle>,

    // calibration
    run: Option<Run>,
    result: Option<LedMap>,
    status: String,
    saved: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            cam_index: 0,
            led_count: 50,
            start_universe: 1,
            start_channel: 1,
            order: "GRB".into(),
            threshold: 64,
            hold_frames: 6,
            priority: 100,
            out_path: "ledmap.json".into(),
            cam: None,
            latest: None,
            tex: None,
            run: None,
            result: None,
            status: "Open a camera to begin.".into(),
            saved: None,
        }
    }
}

impl App {
    fn open_camera(&mut self) {
        let req = RequestedFormat::new::<RgbFormat>(RequestedFormatType::AbsoluteHighestFrameRate);
        match Camera::new(CameraIndex::Index(self.cam_index), req)
            .and_then(|mut c| c.open_stream().map(|_| c))
        {
            Ok(cam) => {
                self.cam = Some(cam);
                self.status = format!("Camera {} open.", self.cam_index);
            }
            Err(e) => self.status = format!("camera error: {e}"),
        }
    }

    /// Grab and decode the newest frame (blocking ~one camera frame).
    fn capture(&mut self) {
        let img = self
            .cam
            .as_mut()
            .and_then(|c| c.frame().ok())
            .and_then(|f| f.decode_image::<RgbFormat>().ok());
        if let Some(img) = img {
            self.latest = Some(CamFrame {
                w: img.width() as usize,
                h: img.height() as usize,
                rgb: img.into_raw(),
            });
        }
    }

    fn start(&mut self) {
        let transport = match SacnTransport::new(Dest::Multicast, self.priority, "ledmap-studio") {
            Ok(t) => t,
            Err(e) => {
                self.status = format!("sACN error: {e}");
                return;
            }
        };
        let dmx = DmxSender::spawn(Box::new(transport), 44.0);
        let cal = SequentialCalibrator::new(
            self.led_count,
            self.order.clone(),
            self.start_universe,
            self.start_channel,
            255,
            self.threshold,
        );
        self.run = Some(Run { dmx, session: CalibrationSession::new(cal, self.hold_frames) });
        self.result = None;
        self.saved = None;
        self.status = "Calibrating…".into();
    }

    fn stop(&mut self) {
        if let Some(run) = self.run.take() {
            run.dmx.submit(run.session.blackout());
        }
        self.status = "Stopped.".into();
    }

    /// Drive one step; returns true once the run is complete.
    fn pump(&mut self) -> bool {
        let luma = self.latest.as_ref().map(|f| (rgb_to_luma(&f.rgb), f.w, f.h));
        let Some(run) = self.run.as_mut() else { return false };
        let tick = run
            .session
            .tick(luma.as_ref().map(|(l, w, h)| (l.as_slice(), *w, *h)));
        run.dmx.submit(tick.frame);
        self.status = format!("Calibrating LED {} / {}", tick.step, tick.total);
        tick.done
    }

    fn finish(&mut self) {
        if let Some(run) = self.run.take() {
            run.dmx.submit(run.session.blackout());
            self.result = Some(run.session.finish(now_stamp()));
        }
        self.export();
    }

    fn export(&mut self) {
        if let Some(map) = &self.result {
            match map.save(&self.out_path) {
                Ok(()) => {
                    self.saved = Some(self.out_path.clone());
                    self.status = format!("Exported {} LEDs → {}", map.leds.len(), self.out_path);
                }
                Err(e) => self.status = format!("save error: {e}"),
            }
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("LED Mapper Studio");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Camera");
            ui.add(egui::DragValue::new(&mut self.cam_index).range(0..=16));
            if ui.button("Open").clicked() {
                self.open_camera();
            }
        });
        ui.separator();

        let running = self.run.is_some();
        ui.add_enabled_ui(!running, |ui| {
            egui::Grid::new("cfg").num_columns(2).show(ui, |ui| {
                ui.label("LED count");
                ui.add(egui::DragValue::new(&mut self.led_count).range(1..=20000));
                ui.end_row();
                ui.label("Universe");
                ui.add(egui::DragValue::new(&mut self.start_universe).range(1..=63999));
                ui.end_row();
                ui.label("Channel");
                ui.add(egui::DragValue::new(&mut self.start_channel).range(1..=512));
                ui.end_row();
                ui.label("Color order");
                ui.text_edit_singleline(&mut self.order);
                ui.end_row();
                ui.label("Threshold");
                ui.add(egui::Slider::new(&mut self.threshold, 0..=255));
                ui.end_row();
                ui.label("Hold frames");
                ui.add(egui::DragValue::new(&mut self.hold_frames).range(1..=60));
                ui.end_row();
                ui.label("sACN priority");
                ui.add(egui::Slider::new(&mut self.priority, 0..=200));
                ui.end_row();
                ui.label("Output file");
                ui.text_edit_singleline(&mut self.out_path);
                ui.end_row();
            });
        });

        ui.add_space(8.0);
        if !running {
            let can = self.cam.is_some();
            if ui.add_enabled(can, egui::Button::new("▶ Start calibration")).clicked() {
                self.start();
            }
            if self.result.is_some() && ui.button("💾 Export again").clicked() {
                self.export();
            }
        } else if ui.button("⏹ Stop").clicked() {
            self.stop();
        }

        ui.add_space(8.0);
        ui.label(&self.status);
        if let Some(p) = &self.saved {
            ui.colored_label(egui::Color32::from_rgb(0, 200, 0), format!("Saved → {p}"));
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui) {
        if let Some(f) = &self.latest {
            let img = egui::ColorImage::from_rgb([f.w, f.h], &f.rgb);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(ui.ctx().load_texture("cam", img, egui::TextureOptions::LINEAR)),
            }
        }
        if let Some(tex) = &self.tex {
            let avail = ui.available_size();
            let (iw, ih) = (tex.size()[0] as f32, tex.size()[1] as f32);
            let scale = (avail.x / iw).min(avail.y / ih).max(0.0);
            let size = egui::vec2(iw * scale, ih * scale);
            let resp = ui.image(egui::load::SizedTexture::new(tex.id(), size));
            if let Some(map) = &self.result {
                let r = resp.rect;
                let p = ui.painter_at(r);
                for led in &map.leds {
                    if led.conf <= 0.0 {
                        continue;
                    }
                    let pos = r.min + egui::vec2(led.u * r.width(), led.v * r.height());
                    p.circle_filled(pos, 3.0, egui::Color32::from_rgb(0, 255, 0));
                }
            }
        } else {
            ui.centered_and_justified(|ui| ui.label("No camera. Open one on the left."));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.capture();
        if self.run.is_some() && self.pump() {
            self.finish();
        }

        egui::SidePanel::left("controls")
            .default_width(240.0)
            .show_inside(ui, |ui| self.controls(ui));
        egui::CentralPanel::default().show_inside(ui, |ui| self.preview(ui));

        ui.ctx().request_repaint();
    }
}

/// RGB8 → Rec.601 luma (77/150/29 ≈ /256).
fn rgb_to_luma(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks_exact(3)
        .map(|p| ((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8)
        .collect()
}

/// Free-form capture timestamp (Unix seconds) for the exported map.
fn now_stamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

fn main() -> eframe::Result<()> {
    env_logger::init();
    eframe::run_native(
        "LED Mapper Studio",
        eframe::NativeOptions::default(),
        Box::new(|_cc| Ok(Box::new(App::default()))),
    )
}
