//! ShaderGlass — a rustjay-engine example.
//!
//! Runs a live source (Syphon / NDI / webcam / … — picked in the built-in Input
//! tab) through a deck-style chain of ISF shaders, shown in the output window.
//! The separate egui control window carries the engine's built-in Input/Output/
//! Preset tabs plus a custom "Shader" tab: shader library, FX chain editor,
//! per-slot parameters, and profile save/load.
//!
//! Defaults to a single CRT-Glass slot; add ShaderBeam (or any ISF shader) on top.

mod chain;
mod profile;
mod shader_tab;

use std::path::PathBuf;

use chain::ChainEffect;
use shader_tab::ShaderTab;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    let shaders_dir =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../crates/rustjay-isf/shaders");

    let effect = ChainEffect::new(vec![shaders_dir.join("CRT-Glass.fs")]);
    let tab = ShaderTab {
        shaders_dir: shaders_dir.clone(),
        library: ShaderTab::scan_library(&shaders_dir),
        search: String::new(),
        chain: effect.handle(),
        pending_params: None,
    };

    rustjay_engine::run_with_egui_tabs(effect, vec![Box::new(tab)])
}
