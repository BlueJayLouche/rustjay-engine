//! LED calibration tab — flash a strip, recover per-LED positions from the
//! webcam, export `ledmap.json`.
//!
//! Self-contained: owns its own sACN sender (`rustjay-lighting`) and webcam
//! (`rustjay-io`), so it doesn't touch engine I/O. The per-frame loop lives in
//! [`LedMapTab::pump`]; the calibration logic is `rustjay-ledmap`. See
//! `crates/rustjay-ledmap/DESIGN.md`.

use std::sync::mpsc::Receiver;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rustjay_core::OutputCommand;
use rustjay_engine::prelude::*;
use rustjay_io::{WebcamCapture, WebcamFrame};
use rustjay_ledmap::{CalibrationSession, SequentialCalibrator};
use rustjay_lighting::{Dest, DmxSender, SacnTransport};

/// A live calibration run. Bundled behind a `Mutex` in the tab so the tab type
/// stays `Send + Sync` (the webcam `Receiver` is `!Sync` on its own).
struct Run {
    dmx: DmxSender,
    _cam: WebcamCapture, // kept alive; dropping it stops capture
    rx: Receiver<WebcamFrame>,
    session: CalibrationSession,
}

/// Calibration tab state: config + the optional live run.
pub struct LedMapTab {
    led_count: u32,
    start_universe: u16,
    start_channel: u16,
    order: String,
    on_level: u8,
    threshold: u8,
    hold_frames: u32,
    camera_index: usize,
    priority: u8,
    out_path: String,

    status: String,
    saved_path: Option<String>,
    /// Whether mapped-LED (sACN) playback output has been started.
    playback_on: bool,

    run: Mutex<Option<Run>>,
}

impl Default for LedMapTab {
    fn default() -> Self {
        Self {
            led_count: 50,
            start_universe: 1,
            start_channel: 1,
            order: "GRB".into(),
            on_level: 255,
            threshold: 64,
            hold_frames: 6,
            camera_index: 0,
            priority: 100,
            out_path: "./ledmap.json".into(),
            status: "Idle".into(),
            saved_path: None,
            playback_on: false,
            run: Mutex::new(None),
        }
    }
}

impl LedMapTab {
    pub fn new() -> Self {
        Self::default()
    }

    /// Spin up the sACN sender + webcam and begin a sequential flash.
    fn start(&mut self) {
        let transport = match SacnTransport::new(Dest::Multicast, self.priority, "rustjay-ledmap") {
            Ok(t) => t,
            Err(e) => {
                self.status = format!("sACN error: {e}");
                return;
            }
        };
        let dmx = DmxSender::spawn(Box::new(transport), 44.0);

        // ponytail: opens its own camera — clashes with the engine's webcam
        // input if that's already live. Tap the engine input texture instead
        // when this graduates from a calibration-only mode.
        let mut cam = match WebcamCapture::new(self.camera_index, 1280, 720, 30) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("camera error: {e}");
                return;
            }
        };
        let rx = match cam.start() {
            Ok(rx) => rx,
            Err(e) => {
                self.status = format!("camera start error: {e}");
                return;
            }
        };

        let cal = SequentialCalibrator::new(
            self.led_count,
            self.order.clone(),
            self.start_universe,
            self.start_channel,
            self.on_level,
            self.threshold,
        );
        // Subtract an ambient reference (LEDs held off first) so static room
        // lights don't dominate detection — matches ledmap-studio.
        let session = CalibrationSession::with_background_subtraction(cal, self.hold_frames);
        *self.run.lock().unwrap() = Some(Run { dmx, _cam: cam, rx, session });
        self.saved_path = None;
        self.status = "Calibrating…".into();
    }

    /// Stop and blackout the strip without exporting.
    fn stop(&mut self) {
        if let Some(run) = self.run.lock().unwrap().take() {
            run.dmx.submit(run.session.blackout());
        }
        self.status = "Stopped".into();
    }

    /// One render-frame step: drain to the newest webcam frame, tick the
    /// session, drive the result. Returns `(step, total, done, capturing_reference)`.
    fn pump(&self) -> (u32, u32, bool, bool) {
        let mut guard = self.run.lock().unwrap();
        let Some(run) = guard.as_mut() else {
            return (0, 0, false, false);
        };

        // Skip stale frames; only the most recent matters.
        let mut newest: Option<WebcamFrame> = None;
        while let Ok(f) = run.rx.try_recv() {
            newest = Some(f);
        }

        let luma_buf = newest.as_ref().map(|f| bgra_to_luma(&f.data));
        let luma = match (&luma_buf, &newest) {
            (Some(buf), Some(f)) => Some((buf.as_slice(), f.width as usize, f.height as usize)),
            _ => None,
        };

        let tick = run.session.tick(luma);
        run.dmx.submit(tick.frame);
        (tick.step, tick.total, tick.done, tick.capturing_reference)
    }

    /// Blackout, export the map, and clear the run.
    fn finish_and_save(&mut self) {
        let map = {
            let mut guard = self.run.lock().unwrap();
            guard.take().map(|run| {
                run.dmx.submit(run.session.blackout());
                run.session.finish(now_stamp())
            })
        };
        if let Some(map) = map {
            match map.save(&self.out_path) {
                Ok(()) => {
                    self.saved_path = Some(self.out_path.clone());
                    self.status = format!("Done — {} LEDs mapped", map.leds.len());
                }
                Err(e) => self.status = format!("save error: {e}"),
            }
        }
    }
}

impl AnyEguiTab for LedMapTab {
    fn name(&self) -> &str {
        "LED Map"
    }

    fn draw(&mut self, ui: &mut egui::Ui, _app: &mut dyn std::any::Any, engine: &mut EngineState) {
        ui.heading("LED Calibration");
        ui.label(
            egui::RichText::new("Flash a strip, capture per-LED positions, export ledmap.json")
                .size(11.0)
                .weak(),
        );
        ui.separator();

        let running = self.run.lock().unwrap().is_some();

        if !running {
            egui::Grid::new("ledmap_cfg").num_columns(2).show(ui, |ui| {
                ui.label("LED count");
                ui.add(egui::DragValue::new(&mut self.led_count).range(1..=20000));
                ui.end_row();
                ui.label("Start universe");
                ui.add(egui::DragValue::new(&mut self.start_universe).range(1..=63999));
                ui.end_row();
                ui.label("Start channel");
                ui.add(egui::DragValue::new(&mut self.start_channel).range(1..=512));
                ui.end_row();
                ui.label("Color order");
                ui.text_edit_singleline(&mut self.order);
                ui.end_row();
                ui.label("On level");
                ui.add(egui::DragValue::new(&mut self.on_level));
                ui.end_row();
                ui.label("Detect threshold");
                ui.add(egui::Slider::new(&mut self.threshold, 0..=255));
                ui.end_row();
                ui.label("Hold frames");
                ui.add(egui::DragValue::new(&mut self.hold_frames).range(1..=60));
                ui.end_row();
                ui.label("Camera index");
                ui.add(egui::DragValue::new(&mut self.camera_index).range(0..=16));
                ui.end_row();
                ui.label("sACN priority");
                ui.add(egui::Slider::new(&mut self.priority, 0..=200));
                ui.end_row();
                ui.label("Output file");
                ui.text_edit_singleline(&mut self.out_path);
                ui.end_row();
            });
            ui.add_space(8.0);
            if ui.button("▶ Start calibration").clicked() {
                self.start();
            }
        } else {
            let (step, total, done, capturing_ref) = self.pump();
            let frac = if total > 0 { step as f32 / total as f32 } else { 0.0 };
            let label = if capturing_ref {
                "Hold still — capturing reference…".to_string()
            } else {
                format!("LED {step} / {total}")
            };
            ui.add(egui::ProgressBar::new(frac).text(label));
            ui.add_space(8.0);
            if ui.button("⏹ Stop").clicked() {
                self.stop();
            }
            if done {
                self.finish_and_save();
            }
            // Keep the capture/drive loop ticking while a run is active.
            ui.ctx().request_repaint();
        }

        ui.add_space(8.0);
        ui.label(&self.status);
        if let Some(p) = &self.saved_path {
            ui.label(egui::RichText::new(format!("Saved → {p}")).color(egui::Color32::GREEN));
        }

        // --- Playback (sACN) -------------------------------------------------
        ui.add_space(12.0);
        ui.separator();
        ui.label(egui::RichText::new("Playback (sACN)").strong());
        ui.label(
            egui::RichText::new("Drive the strip from rendered output using the map file above")
                .size(11.0)
                .weak(),
        );
        if !self.playback_on {
            if ui.button("▶ Start LED output").clicked() {
                engine.output_command = OutputCommand::StartLed {
                    path: self.out_path.clone(),
                    priority: self.priority,
                };
                self.playback_on = true;
            }
        } else {
            if ui.button("⏹ Stop LED output").clicked() {
                engine.output_command = OutputCommand::StopLed;
                self.playback_on = false;
            }
            ui.label(egui::RichText::new("● sACN LIVE").color(egui::Color32::GREEN).strong());
        }
    }
}

/// BGRA8 → Rec.601 luma. Weights 77/150/29 sum to 256 (shift by 8).
fn bgra_to_luma(bgra: &[u8]) -> Vec<u8> {
    bgra.chunks_exact(4)
        .map(|p| {
            let (b, g, r) = (p[0] as u32, p[1] as u32, p[2] as u32);
            ((r * 77 + g * 150 + b * 29) >> 8) as u8
        })
        .collect()
}

/// Free-form capture timestamp — Unix seconds. `ledmap.json` treats it as an
/// opaque string, so no date crate needed.
fn now_stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}
