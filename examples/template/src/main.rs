//! HSB color template — reference example for rustjay-engine.
//!
//! Demonstrates single video input with HSB color manipulation,
//! audio reactivity, LFO, MIDI, OSC, and web server.

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Template v{}", env!("CARGO_PKG_VERSION"));

    rustjay_engine::run("template")
}
