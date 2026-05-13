//! rustjay-core — shared types, traits, and state for the rustjay-engine ecosystem.
//!
//! This crate contains the vocabulary every other crate agrees on:
//! - [`EffectPlugin`] — the trait app authors implement
//! - [`EngineState`] — the engine's live state
//! - [`RenderGraph`] — multi-pass pipeline descriptor
//! - [`Vertex`] — full-screen quad geometry
//! - LFO, audio routing, and command enums
//!
//! It has no internal workspace dependencies.

#![warn(missing_docs)]

pub mod lfo;
pub mod plugin;
pub mod routing;
pub mod state;
pub mod vertex;

pub use lfo::{LfoState, LfoBank, Lfo, Waveform, LfoTarget, beat_division_to_hz, BEAT_DIVISIONS, BEAT_DIVISION_NAMES};
pub use plugin::{EffectPlugin, RenderGraph, Pass, PassInput, MeshDescriptor, MeshTopology};
pub use routing::{
    FftBand, ModulationTarget, AudioRoute, RoutingMatrix, AudioRoutingState,
};
pub use state::*;
pub use vertex::Vertex;
