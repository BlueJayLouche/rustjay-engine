//! rustjay-engine — high-performance cross-platform VJ engine
//!
//! The engine facade. App authors implement [`EffectPlugin`] and call [`run()`].
//!
//! # Getting Started
//!
#![doc = include_str!("../GUIDE.md")]

pub mod app;
pub mod config;
#[cfg(feature = "gles2")]
pub mod gles2;

#[cfg(feature = "projection")]
pub use app::projection::ProjectionSubsystem;

// Re-export the most useful types so app authors only need `rustjay_engine::*`
#[cfg(feature = "prodj")]
pub use rustjay_core::{CdjDevice, ProDjCommand, ProDjState};
pub use rustjay_core::{
    EffectPlugin, EngineState, GuiTab, HsbParams, InputCommand, MeshDescriptor, MeshTopology,
    OutputCommand, ParamCategory, ParamType, ParameterDescriptor, Pass, PassInput, RenderGraph,
    RenderHookCtx,
};
#[cfg(feature = "link")]
pub use rustjay_core::{LinkCommand, LinkState};
#[cfg(feature = "egui")]
pub use rustjay_gui::AnyEguiTab;
pub use rustjay_gui::{AnyGuiTab, BuiltinTab};
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

/// Parse `--web-token <token>` and `--bind <host>` from `std::env::args()` and
/// apply them to the engine state before the app starts.
fn apply_cli_args(state: &mut EngineState) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--web-token" => {
                i += 1;
                if i < args.len() {
                    state.web_token = args[i].clone();
                }
            }
            "--bind" => {
                i += 1;
                if i < args.len() {
                    state.web_host = args[i].clone();
                }
            }
            _ => {}
        }
        i += 1;
    }
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
pub fn run_with_tabs<P: EffectPlugin>(plugin: P, tabs: Vec<Box<dyn AnyGuiTab>>) -> Result<()> {
    let mut state = EngineState::new();
    apply_cli_args(&mut state);
    let shared_state = Arc::new(Mutex::new(state));
    app::run_app(shared_state, plugin, tabs, false)
}

/// Run the engine with projection mapping enabled.
///
/// The `setup` closure receives a [`ProjectionSubsystem`] where you can
/// register extra projector windows and their post-processing stage chains.
///
/// ```ignore
/// use rustjay_engine::{run_with_projection, ProjectionSubsystem};
/// use rustjay_projection::IdentityStage;
///
/// fn main() -> anyhow::Result<()> {
///     run_with_projection(MyEffect, vec![], |sub| {
///         sub.add_projector(
///             WindowAttributes::default().with_title("Projector 1"),
///             vec![Box::new(IdentityStage::new(&device, format))],
///         );
///     })
/// }
/// ```
#[cfg(feature = "projection")]
pub fn run_with_projection<P: EffectPlugin, F: FnOnce(&mut ProjectionSubsystem)>(
    plugin: P,
    tabs: Vec<Box<dyn AnyGuiTab>>,
    setup: F,
) -> Result<()> {
    let mut state = EngineState::new();
    apply_cli_args(&mut state);
    let shared_state = Arc::new(Mutex::new(state));
    app::run_app_with_projection(shared_state, plugin, tabs, false, setup)
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
    let mut state = EngineState::new();
    apply_cli_args(&mut state);
    let shared_state = Arc::new(Mutex::new(state));
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
    let mut state = EngineState::new();
    apply_cli_args(&mut state);
    let shared_state = Arc::new(Mutex::new(state));
    app::run_egui_app(shared_state, plugin, tabs)
}

/// Run the engine with egui custom tabs **and** projection mapping.
///
/// The `setup` closure receives a [`ProjectionSubsystem`] where you register
/// extra projector windows and their post-processing stage chains.
///
/// ```ignore
/// use rustjay_engine::prelude::*;
/// use rustjay_projection::IdentityStage;
/// use winit::window::WindowAttributes;
///
/// fn main() -> anyhow::Result<()> {
///     rustjay_engine::run_with_projection_egui_tabs(
///         MyEffect,
///         vec![Box::new(MyTab)],
///         |sub| {
///             sub.add_projector(
///                 WindowAttributes::default().with_title("Projector 1"),
///                 |device, format| vec![Box::new(IdentityStage::new(device, format))],
///             );
///         },
///     )
/// }
/// ```
#[cfg(all(feature = "projection", feature = "egui"))]
pub fn run_with_projection_egui_tabs<P: EffectPlugin, F: FnOnce(&mut ProjectionSubsystem)>(
    plugin: P,
    tabs: Vec<Box<dyn AnyEguiTab>>,
    setup: F,
) -> Result<()> {
    let mut state = EngineState::new();
    apply_cli_args(&mut state);
    let shared_state = Arc::new(Mutex::new(state));
    app::run_egui_app_with_projection(shared_state, plugin, tabs, false, setup)
}

/// Run using a Wayland-backed GLES 2.0 context (compositor required).
#[cfg(feature = "gles2")]
pub fn run_gles2_headless_with_tabs<P, G>(plugin: P, gles2: G) -> Result<()>
where
    P: EffectPlugin,
    G: gles2::Gles2Effect,
{
    gles2::run_gles2_headless_with_tabs(plugin, gles2)
}

/// Run using a DRM/GBM GLES 2.0 context — renders directly to `/dev/dri/card0`.
///
/// No compositor (weston, X11) is needed. The process must have access to the
/// DRM device (user in `video` group, or run under a seat session).
#[cfg(feature = "drm-gles2")]
pub fn run_drm_gles2_headless_with_tabs<P, G>(plugin: P, gles2: G) -> Result<()>
where
    P: EffectPlugin,
    G: gles2::Gles2Effect,
{
    gles2::run_drm_gles2_headless_with_tabs(plugin, gles2)
}

pub mod prelude {
    #[cfg(feature = "gles2")]
    pub use crate::gles2::{run_gles2_headless_with_tabs, Gles2Effect};
    #[cfg(feature = "drm-gles2")]
    pub use crate::run_drm_gles2_headless_with_tabs;
    #[cfg(feature = "egui")]
    pub use crate::run_with_egui_tabs;
    #[cfg(all(feature = "projection", feature = "egui"))]
    pub use crate::run_with_projection_egui_tabs;
    pub use crate::{run, run_headless, run_headless_with_tabs, run_with_tabs};
    pub use rustjay_core::{
        beat_division_to_hz, working_format, AudioCommand, EffectPlugin, EngineState, GuiTab,
        HsbParams, InputCommand, InputType, Lfo, LfoBank, LfoTarget, LinkCommand, MeshDescriptor,
        MeshTopology, MidiCommand, ModulationSource, OscCommand, OutputCommand, ParamCategory,
        ParamType, ParameterDescriptor, Pass, PassInput, PresetCommand, ProDjCommand, RenderGraph,
        RenderHookCtx, Vertex, Waveform, WebCommand, BEAT_DIVISIONS, BEAT_DIVISION_NAMES,
    };
    #[cfg(feature = "egui")]
    pub use rustjay_gui::{param_slider, param_slider_int, AnyEguiTab};
    pub use rustjay_gui::{AnyGuiTab, BuiltinTab};
    pub use rustjay_render::{InputTexture, PreviousFrameTexture, Texture, WgpuEngine};
}
