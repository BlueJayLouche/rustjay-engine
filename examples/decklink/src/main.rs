//! Blackmagic DeckLink input example.
//!
//! Opens the first detected DeckLink device and renders its live feed
//! through rustjay-engine.

#[cfg(windows)]
fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .init();

    log::info!("Starting DeckLink example");

    rustjay_engine::run(decklink_input::DecklinkApp::new())
}

/// The DeckLink SDK is Windows-only; on other platforms this example does nothing
/// but still builds so `cargo build --workspace` stays green everywhere.
#[cfg(not(windows))]
fn main() {
    eprintln!("The decklink example requires Windows (Blackmagic DeckLink SDK).");
}
