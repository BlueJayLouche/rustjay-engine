//! Kovvboj's API state schema (app-owned).
//!
//! These DTOs serialize the deck/channel/effect structure + library registry
//! that this app publishes into `EngineState::app_state` (the generic opaque
//! JSON slot). The engine and `rustjay-api` know nothing about these types —
//! the schema lives entirely in the app. The snapshot is rebuilt each frame
//! with live (modulated) values, so HTTP reads and WebSocket deltas stay fresh.

use serde::Serialize;

/// App-level snapshot published by Kovvboj.
#[derive(Debug, Clone, Serialize, Default)]
pub struct KovvbojStateSnapshot {
    /// Mixer crossfader value (live).
    pub crossfader: f32,
    /// All mixer channels.
    pub channels: Vec<KovvbojChannel>,
    /// Master-chain effects.
    pub master_effects: Vec<KovvbojEffect>,
    /// Library/registry snapshot.
    pub library: KovvbojLibrary,
}

/// One mixer channel.
#[derive(Debug, Clone, Serialize)]
pub struct KovvbojChannel {
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
    pub decks: Vec<KovvbojDeck>,
    /// Channel-level FX chain.
    pub effects: Vec<KovvbojEffect>,
}

/// One deck inside a channel.
#[derive(Debug, Clone, Serialize)]
pub struct KovvbojDeck {
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
    pub effects: Vec<KovvbojEffect>,
}

/// One effect slot.
#[derive(Debug, Clone, Serialize)]
pub struct KovvbojEffect {
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
pub struct KovvbojLibrary {
    /// ISF shaders.
    pub shaders: Vec<KovvbojSourceEntry>,
    /// Static images.
    pub images: Vec<KovvbojSourceEntry>,
    /// Video files.
    pub videos: Vec<KovvbojSourceEntry>,
    /// Built-in generators.
    pub builtins: Vec<KovvbojSourceEntry>,
}

/// One entry in the Kovvboj library.
#[derive(Debug, Clone, Serialize)]
pub struct KovvbojSourceEntry {
    /// Stable identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Source kind: `isf`, `image`, `video`, `solid_color`, `camera`, etc.
    pub kind: String,
    /// Absolute filesystem path, when applicable.
    pub path: Option<String>,
    /// Device index for camera/NDI sources.
    pub device_index: usize,
}
