//! The plugin: renders the live source to the main target (the projectors then
//! composite it per output) and services auto-detect requests from the UI.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use rustjay_engine::prelude::*;
use rustjay_projection::{GridSize, MatrixSync, VideoMatrixConfig};

#[cfg(feature = "videowall")]
use rustjay_projection::{AprilTagAutoDetector, CalibSync, texture_to_gray_image};

/// Per-output shared handles. Cloned into each projector's stage chain (writer:
/// the UI) and read by the plugin (auto-detect). One `OutputSync` == one wall.
#[derive(Clone)]
pub struct OutputSync {
    pub matrix: Arc<Mutex<MatrixSync>>,
    #[cfg(feature = "videowall")]
    pub calib: Arc<Mutex<CalibSync>>,
    /// UI → plugin: run auto-detect against the live input next frame.
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    pub detect_req: Arc<AtomicBool>,
}

impl OutputSync {
    pub fn new(grid: GridSize) -> Self {
        let mut cfg = VideoMatrixConfig::new(grid);
        cfg.input_grid.create_default_mapping();
        let mut sync = MatrixSync::default();
        sync.set_config(cfg);
        Self {
            matrix: Arc::new(Mutex::new(sync)),
            #[cfg(feature = "videowall")]
            calib: Arc::new(Mutex::new(CalibSync::default())),
            detect_req: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Shared, growable list of outputs (UI adds; plugin reads).
pub type Outputs = Arc<Mutex<Vec<OutputSync>>>;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct WallState;

pub struct VideoWallEffect {
    #[cfg_attr(not(feature = "videowall"), allow(dead_code))]
    outputs: Outputs,
}

impl VideoWallEffect {
    pub fn new(outputs: Outputs) -> Self {
        Self { outputs }
    }
}

impl EffectPlugin for VideoWallEffect {
    type State = WallState;
    type Uniforms = [f32; 4];

    fn app_name(&self) -> &str {
        "videowall"
    }

    fn shader_source(&self) -> &'static str {
        include_str!("passthrough.wgsl")
    }

    fn build_uniforms(&self, _state: &Self::State, _engine: &EngineState) -> Self::Uniforms {
        [0.0; 4]
    }

    // Service auto-detect requests, then let the engine draw the passthrough
    // (source → target) — `false` means "engine, run your default pass".
    #[cfg(feature = "videowall")]
    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, _state: &mut Self::State) -> bool {
        use std::sync::atomic::Ordering;
        let outs = self.outputs.lock().unwrap_or_else(|e| e.into_inner());
        for o in outs.iter() {
            if !o.detect_req.swap(false, Ordering::SeqCst) {
                continue;
            }
            let Some(inp) = ctx.input.as_ref() else { continue };
            let Some(tex) = inp.texture else { continue };
            let (w, h) = (tex.width(), tex.height());
            match texture_to_gray_image(ctx.device, ctx.queue, tex, w, h) {
                Ok(gray) => {
                    let grid = o
                        .matrix
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .config
                        .output_grid;
                    let det = AprilTagAutoDetector::new();
                    let screens = det.detect_screens(&gray);
                    log::info!("auto-detect: {} screen(s) found", screens.len());
                    let cfg = det.suggest_config(&screens, grid);
                    o.matrix.lock().unwrap_or_else(|e| e.into_inner()).set_config(cfg);
                }
                Err(e) => log::warn!("auto-detect readback failed: {e}"),
            }
        }
        false
    }
}
