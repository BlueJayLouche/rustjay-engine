//! Varda's API state schema (app-owned).
//!
//! These DTOs serialize the deck/channel/effect structure + library registry
//! that this app publishes into `EngineState::app_state` (the generic opaque
//! JSON slot). The engine and `rustjay-api` know nothing about these types —
//! the schema lives entirely in the app. The snapshot is rebuilt each frame
//! with live (modulated) values, so HTTP reads and WebSocket deltas stay fresh.

use serde::Serialize;

/// App-level snapshot published by Varda.
#[derive(Debug, Clone, Serialize, Default)]
pub struct VardaStateSnapshot {
    /// Mixer crossfader value (live).
    pub crossfader: f32,
    /// All mixer channels.
    pub channels: Vec<VardaChannel>,
    /// Master-chain effects.
    pub master_effects: Vec<VardaEffect>,
    /// Library/registry snapshot.
    pub library: VardaLibrary,
}

/// One mixer channel.
#[derive(Debug, Clone, Serialize)]
pub struct VardaChannel {
    /// Channel short UUID.
    pub uuid: String,
    /// Display name (e.g. "Channel A").
    pub name: String,
    /// Canonical opacity parameter id (e.g. `ch_a_opacity`).
    pub opacity_key: String,
    /// Canonical blend parameter id.
    pub blend_key: String,
    /// Canonical input-select parameter id.
    pub input_select_key: String,
    /// Live opacity (base + modulation).
    pub opacity: f32,
    /// Live blend mode name.
    pub blend: String,
    /// Live input selection.
    pub input_select: String,
    /// Decks owned by this channel.
    pub decks: Vec<VardaDeck>,
    /// Channel-level FX chain.
    pub effects: Vec<VardaEffect>,
}

/// One deck inside a channel.
#[derive(Debug, Clone, Serialize)]
pub struct VardaDeck {
    /// Deck short UUID.
    pub uuid: String,
    /// Display name (e.g. "ColorCycle").
    pub name: String,
    /// Parent channel UUID.
    pub channel_uuid: String,
    /// Canonical opacity parameter id.
    pub opacity_key: String,
    /// Canonical blend parameter id.
    pub blend_key: String,
    /// Live opacity (base + modulation).
    pub opacity: f32,
    /// Live blend mode name.
    pub blend: String,
    /// Deck-level FX chain.
    pub effects: Vec<VardaEffect>,
}

/// One effect slot.
#[derive(Debug, Clone, Serialize)]
pub struct VardaEffect {
    /// Effect slot UUID.
    pub uuid: String,
    /// Display name.
    pub name: String,
    /// Whether the slot is currently enabled.
    pub enabled: bool,
    /// Full canonical parameter prefix (e.g. `ch_a_fxabc_`).
    pub param_prefix: String,
}

/// Library/registry contents.
#[derive(Debug, Clone, Serialize, Default)]
pub struct VardaLibrary {
    /// ISF shaders.
    pub shaders: Vec<VardaSourceEntry>,
    /// Static images.
    pub images: Vec<VardaSourceEntry>,
    /// Video files.
    pub videos: Vec<VardaSourceEntry>,
    /// Built-in generators.
    pub builtins: Vec<VardaSourceEntry>,
}

/// One entry in the Varda library.
#[derive(Debug, Clone, Serialize)]
pub struct VardaSourceEntry {
    /// Stable identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Source kind: `isf`, `image`, `video`, `solid_color`, `camera`, etc.
    pub kind: String,
    /// Absolute filesystem path, when applicable.
    pub path: Option<String>,
}
