#![warn(missing_docs)]

//! rustjay-engine — high-performance cross-platform VJ engine
//!
//! The engine facade. App authors implement [`EffectPlugin`] and call [`run()`].
//!
//! # Getting Started
//!
#![doc = include_str!("../GUIDE.md")]

pub mod app;
pub mod config;

// Re-export the most useful types so app authors only need `rustjay_engine::*`
pub use rustjay_core::{EffectPlugin, EngineState, HsbParams, GuiTab, InputCommand, OutputCommand, RenderGraph, Pass, PassInput, MeshDescriptor, MeshTopology, ParameterDescriptor, ParamCategory, ParamType};
#[cfg(feature = "link")]
pub use rustjay_core::{LinkState, LinkCommand};
#[cfg(feature = "prodj")]
pub use rustjay_core::{ProDjState, CdjDevice, ProDjCommand};
pub use rustjay_gui::{AnyGuiTab, BuiltinTab};
#[cfg(feature = "egui")]
pub use rustjay_gui::AnyEguiTab;
pub use rustjay_render::PreviousFrameTexture;

use anyhow::Result;
use std::sync::{Arc, Mutex};

/// Run the engine with the given plugin and no custom GUI tabs.
///
/// ```ignore
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
/// ```ignore
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
    app::run_app(shared_state, plugin, tabs, false)
}

/// Run the engine in headless mode (no control window).
///
/// Opens only the output window, fullscreen. GUI is suppressed; the effect
/// is still controllable via OSC, MIDI, and the Web UI. Intended for
/// embedded or single-output deployments such as a Raspberry Pi.
pub fn run_headless<P: EffectPlugin>(plugin: P) -> Result<()> {
    run_headless_with_tabs(plugin, vec![])
}

/// Run the engine in headless mode with custom GUI tabs.
///
/// The tabs are registered but not displayed (no control window is created).
/// Parameters declared by the tabs remain accessible via OSC/MIDI/Web.
pub fn run_headless_with_tabs<P: EffectPlugin>(
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
) -> Result<()> {
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    app::run_app(shared_state, plugin, tabs, true)
}

/// Run the engine with the given plugin and custom egui tabs.
///
/// ```ignore
/// use rustjay_engine::prelude::*;
///
/// struct MyEffect;
/// impl EffectPlugin for MyEffect { /* ... */ }
/// struct MyTab;
/// impl AnyEguiTab for MyTab { /* ... */ }
///
/// fn main() -> anyhow::Result<()> {
///     rustjay_engine::run_with_egui_tabs(MyEffect, vec![Box::new(MyTab)])
/// }
/// ```
#[cfg(feature = "egui")]
pub fn run_with_egui_tabs<P: EffectPlugin>(
    plugin: P,
    tabs: Vec<Box<dyn AnyEguiTab>>,
) -> Result<()> {
    let shared_state = Arc::new(Mutex::new(EngineState::new()));
    app::run_egui_app(shared_state, plugin, tabs)
}

/// Prelude module for convenient imports.
pub mod prelude {
    pub use rustjay_core::{
        EffectPlugin, EngineState, Vertex, HsbParams,
        LfoState, LfoBank, Lfo, Waveform, LfoTarget, beat_division_to_hz, BEAT_DIVISIONS, BEAT_DIVISION_NAMES,
        InputCommand, OutputCommand, AudioCommand, MidiCommand, OscCommand, PresetCommand, WebCommand,
        LinkCommand, ProDjCommand,
        GuiTab, InputType,
        RenderGraph, Pass, PassInput, MeshDescriptor, MeshTopology,
        ParameterDescriptor, ParamCategory, ParamType,
    };
    pub use rustjay_gui::{AnyGuiTab, BuiltinTab};
    #[cfg(feature = "egui")]
    pub use rustjay_gui::{AnyEguiTab, param_slider, param_slider_int};
    pub use rustjay_render::{WgpuEngine, Texture, InputTexture, PreviousFrameTexture};
    pub use crate::{run, run_with_tabs, run_headless, run_headless_with_tabs};
    #[cfg(feature = "egui")]
    pub use crate::run_with_egui_tabs;
}
