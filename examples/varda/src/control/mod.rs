//! Control — MIDI / OSC / HTTP parameter routing.
//!
//! Bridges incoming control messages to Varda's hierarchical parameter paths
//! (`deck/<uuid>/param/<name>`, `crossfader`, etc.) and routes them through
//! `engine.set_param_base` so MIDI/OSC/LFO/HTTP stay co-equal consumers.
//!
//! See VARDA_PORT.md Phase 5.

pub mod param_router;

/// Keyboard bindings layer.
pub struct Keymap;
