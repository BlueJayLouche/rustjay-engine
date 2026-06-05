//! Routing graph — decks, channels, and the mixer spine.
//!
//! Phase 1 (next session) will port the deck-per-channel compositing model
//! here. For now this module is a stub so the module tree exists.

/// Deck — source + FX chain + opacity + blend + scaling.
///
/// This is the single largest genuine port (no engine equivalent).
/// See VARDA_PORT.md Phase 1.
pub struct Deck;

/// Channel — ordered deck list + compositing + FX.
///
/// Wraps `rustjay_mixer::Channel` and adds deck-list ownership.
pub struct Channel;
