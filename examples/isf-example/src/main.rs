//! ISF (Interactive Shader Format) viewer.
//!
//! Opens an rfd file picker so you can load any `.fs` ISF shader at runtime.
//! The shader's JSON header is parsed to auto-generate parameter sliders.
//! The GLSL body is transpiled to WGSL via a template + macro-expansion engine.

mod isf_effect;
mod isf_tab;
mod isf_transpiler;

use isf_effect::IsfEffect;
use isf_tab::IsfTab;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga",            log::LevelFilter::Warn)
        .filter_module("wgpu_core",       log::LevelFilter::Warn)
        .filter_module("winit",           log::LevelFilter::Warn)
        .filter_module("tracing::span",   log::LevelFilter::Warn)
        .init();

    // Default to the bundled shaders directory for easy access to ghost-arcade collection
    let shaders_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("shaders");

    let path = rfd::FileDialog::new()
        .add_filter("ISF Shader", &["fs", "frag"])
        .set_title("Pick an ISF shader (.fs)")
        .set_directory(&shaders_dir)
        .pick_file()
        .ok_or_else(|| anyhow::anyhow!("No file selected — exiting."))?;

    log::info!("Loading ISF shader: {}", path.display());

    let effect = IsfEffect::from_path(&path)?;
    let tab = IsfTab {
        shader_name: effect.shader_name.clone(),
        pending_path: effect.pending_path.clone(),
        shaders_dir: shaders_dir.clone(),
    };

    rustjay_engine::run_with_tabs(effect, vec![Box::new(tab)])
}
