//! ISF (Interactive Shader Format) viewer.
//!
//! Starts immediately with the last-loaded shader (persisted across runs).
//! Falls back to the bundled ColorCycle.fs on first launch or if the saved
//! path no longer exists. Use the "Load Shader..." button in the UI to switch.

mod isf_tab;

use std::path::{Path, PathBuf};

use isf_tab::IsfTab;
use rustjay_isf::{last_shader_config_path, IsfEffect};

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    let shaders_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");
    let path = startup_shader_path(&shaders_dir);

    log::info!("Loading ISF shader: {}", path.display());

    let effect = IsfEffect::from_path(&path)?;
    let tab = IsfTab {
        cached_name: effect.shader_name.clone(),
        shader_name: effect.shader_name_shared.clone(),
        pending_path: effect.pending_path.clone(),
        shaders_dir,
    };

    rustjay_engine::run_with_tabs(effect, vec![Box::new(tab)])
}

/// Returns the path to start with:
/// 1. Last-used shader (from ~/.config/rustjay/isf-last-shader.txt) if the file still exists.
/// 2. Bundled ColorCycle.fs as the default.
fn startup_shader_path(shaders_dir: &Path) -> PathBuf {
    if let Ok(saved) = std::fs::read_to_string(last_shader_config_path()) {
        let p = PathBuf::from(saved.trim());
        if p.exists() {
            return p;
        }
    }
    shaders_dir.join("ColorCycle.fs")
}
