//! Routing graph — decks, channels, and the mixer spine.
//!
//! Phase 1 (this session) ports the deck-per-channel compositing model.
//! A `DeckCompositor` implements `EffectInstance` and is dropped into a
//! `rustjay_mixer::Channel` as the channel effect. The mixer then composites
//! channels exactly as before.

#[cfg(feature = "mixer")]
mod compositor;
#[cfg(feature = "mixer")]
mod deck;

#[cfg(feature = "mixer")]
pub use compositor::DeckCompositor;
#[cfg(feature = "mixer")]
pub use deck::Deck;

/// Channel — ordered deck list + compositing + FX.
///
/// Wraps `rustjay_mixer::Channel` by using `DeckCompositor` as the channel
/// effect. The deck-list ownership and per-channel sub-mix routing lives here.
pub struct Channel;
