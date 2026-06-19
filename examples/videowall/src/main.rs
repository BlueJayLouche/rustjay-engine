//! Video-wall / HDMI-matrix mapper.
//!
//! N projector outputs, each compositing regions of one live source onto a grid
//! of screens (`MatrixStage`), with AprilTag auto-calibration (`videowall`
//! feature). The control window's "Matrix" tab manages outputs and per-cell
//! mapping; the built-in Input tab picks the source.

mod app;
mod ui;

use std::sync::{Arc, Mutex};

use app::{Outputs, OutputSync, VideoWallEffect};
use rustjay_projection::{GridSize, ProjectionStage};
use ui::MatrixTab;
use winit::window::WindowAttributes;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .init();

    // Seed one 3×3 output; more can be added at runtime from the UI.
    let outputs: Outputs = Arc::new(Mutex::new(vec![OutputSync::new(GridSize::new(3, 3))]));
    let plugin = VideoWallEffect::new(outputs.clone());
    let tab = MatrixTab::new(outputs.clone());

    // Snapshot the Arcs for the setup closure (registers the initial windows).
    let initial: Vec<OutputSync> =
        outputs.lock().unwrap_or_else(|e| e.into_inner()).clone();

    rustjay_engine::run_with_projection_egui_tabs(plugin, vec![Box::new(tab)], move |sub| {
        for (i, o) in initial.iter().enumerate() {
            let m = o.matrix.clone();
            #[cfg(feature = "videowall")]
            let c = o.calib.clone();
            let attrs = WindowAttributes::default()
                .with_title(format!("Video Wall {}", i + 1))
                .with_inner_size(winit::dpi::LogicalSize::new(960.0, 540.0));
            sub.add_projector(attrs, None, move |device, format| {
                #[allow(unused_mut)]
                let mut stages: Vec<Box<dyn ProjectionStage>> =
                    vec![Box::new(rustjay_projection::MatrixStage::new(device, format, m.clone()))];
                #[cfg(feature = "videowall")]
                stages.push(Box::new(rustjay_projection::TagGridStage::new(device, format, c.clone())));
                stages
            });
        }
    })
}
