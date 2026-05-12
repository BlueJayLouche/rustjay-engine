//! rustjay-engine — high-performance cross-platform VJ engine
//!
//! The engine facade. App authors implement `EffectPlugin` and call `run()`.

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

pub mod app;
pub mod config;

// Re-export the most useful types so app authors only need `rustjay_engine::*`
pub use rustjay_core::{EngineState, HsbParams, GuiTab, InputCommand, OutputCommand};
pub use rustjay_gui::{AnyGuiTab, BuiltinTab};

use anyhow::Result;
use std::sync::{Arc, Mutex};

pub fn run(app_name: &str) -> Result<()> {
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    app::run_app(shared_state, app_name)
}
