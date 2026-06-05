//! Persistence — `.varda/` workspace layout.
//!
//! - `scene.json`  — channels, decks, effects, modulation, crossfader, sequences
//! - `stage.json`  — surface layout, outputs, warp calibration
//! - `midi.json`   — MIDI controller mappings
//! - `keymap.json` — keyboard shortcut bindings
//! - `presets/`    — saved deck/channel presets
//!
//! See VARDA_PORT.md Phase 11.

/// Workspace loader/saver.
pub struct Workspace;
