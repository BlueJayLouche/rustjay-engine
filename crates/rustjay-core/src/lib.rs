//! rustjay-core — shared types, traits, and state for the rustjay-engine ecosystem.

pub mod format;
pub mod instance;
pub mod lfo;
pub mod modulation;
pub mod params;
pub mod plugin;
pub mod routing;
pub mod state;
pub mod vertex;

pub use format::working_format;
pub use instance::{EffectInput, EffectInstance, RenderCtx, RenderTarget};
pub use lfo::{
    beat_division_to_hz, Lfo, LfoBank, LfoTarget, Waveform, BEAT_DIVISIONS,
    BEAT_DIVISION_NAMES,
};
pub use modulation::{
    ADSRStage, AudioBandPreset, AudioReactMode, AudioSourceValues, AudioValues, LFOWaveform,
    ModulationEngine, ModulationSource, ModulationSourceEntry, ParamModulation, StepInterpolation,
};
pub use params::{ParamCategory, ParamType, ParameterDescriptor};
pub use plugin::{
    EffectPlugin, MeshDescriptor, MeshTopology, Pass, PassInput, RenderGraph, RenderHookCtx,
};
pub use routing::{AudioRoute, AudioRoutingState, FftBand, ModulationTarget, RoutingMatrix};
pub use state::*;
pub use vertex::Vertex;
