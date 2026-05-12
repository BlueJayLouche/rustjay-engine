//! rustjay-engine — high-performance cross-platform VJ engine
//!
//! The engine facade. App authors implement `EffectPlugin` and call `run()`.

#[cfg(target_os = "macos")]
#[macro_use]
extern crate objc;

pub mod app;
pub mod config;

// Re-export the most useful types so app authors only need `rustjay_engine::*`
pub use rustjay_core::{EffectPlugin, EngineState, HsbParams, GuiTab, InputCommand, OutputCommand};
pub use rustjay_gui::{AnyGuiTab, BuiltinTab};

use anyhow::Result;
use std::sync::{Arc, Mutex};

/// Run the engine with the given plugin and no custom GUI tabs.
///
/// ```no_run
/// use rustjay_engine::prelude::*;
///
/// struct MyEffect;
/// impl EffectPlugin for MyEffect { /* ... */ }
///
/// fn main() -> anyhow::Result<()> {
///     rustjay_engine::run(MyEffect)
/// }
/// ```
pub fn run<P: EffectPlugin>(plugin: P) -> Result<()> {
    run_with_tabs(plugin, vec![])
}

/// Run the engine with the given plugin and custom GUI tabs.
///
/// ```no_run
/// use rustjay_engine::prelude::*;
///
/// struct MyEffect;
/// impl EffectPlugin for MyEffect { /* ... */ }
/// struct MyTab;
/// impl AnyGuiTab for MyTab { /* ... */ }
///
/// fn main() -> anyhow::Result<()> {
///     rustjay_engine::run_with_tabs(MyEffect, vec![Box::new(MyTab)])
/// }
/// ```
pub fn run_with_tabs<P: EffectPlugin>(
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
) -> Result<()> {
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    app::run_app(shared_state, plugin, tabs)
}

/// Prelude module for convenient imports.
pub mod prelude {
    pub use rustjay_core::{
        EffectPlugin, EngineState, Vertex, HsbParams,
        LfoState, LfoBank, Lfo, Waveform, LfoTarget,
        InputCommand, OutputCommand, AudioCommand, MidiCommand, OscCommand, PresetCommand, WebCommand,
        GuiTab, InputType,
    };
    pub use rustjay_gui::{AnyGuiTab, BuiltinTab};
    pub use rustjay_render::{WgpuEngine, Texture, InputTexture};
    pub use crate::{run, run_with_tabs};
}
