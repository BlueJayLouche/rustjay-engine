//! Parameter router — maps hierarchical Varda control paths to flat engine ids.
//!
//! Varda exposes a hierarchical namespace to external controllers:
//!   `deck/<deck_uuid>/param/<name>`        → `ch_<channel_uuid>_deck_<deck_uuid>_<name>`
//!   `channel/<channel_uuid>/param/<name>`  → `ch_<channel_uuid>_<name>`
//!   `crossfader` (and any bare id)          → returned unchanged
//!
//! Resolution is **structural**: any `<name>` after `param/` is mapped, so
//! source and FX-chain params (`fx<uuid>_<param>`) resolve too — not just an
//! opacity/blend allowlist. The router only stores the deck→channel association
//! and the set of known channels (the one fact a path can't encode). The flat
//! ids are the bare canonical engine ids `rustjay_mixer` registers, so they flow
//! through the engine's existing `WebCommand::Set` / MIDI param paths without
//! forking the param system.

use std::collections::{HashMap, HashSet};

/// Maps hierarchical Varda parameter paths to flat engine parameter ids.
#[derive(Debug, Clone, Default)]
pub struct ParamRouter {
    /// `deck_uuid` → owning `channel_uuid` (the only fact not encodable in a path).
    deck_channel: HashMap<String, String>,
    /// Known channel uuids (so `channel/<uuid>/...` only resolves for real channels).
    channels: HashSet<String>,
}

impl ParamRouter {
    /// Create an empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a channel so `channel/<uuid>/param/<name>` paths resolve.
    pub fn register_channel(&mut self, uuid: &str, _name: &str) {
        self.channels.insert(uuid.to_string());
    }

    /// Register a deck and its owning channel so `deck/<uuid>/param/<name>`
    /// paths resolve to the fully-qualified engine id.
    pub fn register_deck(&mut self, channel_uuid: &str, deck_uuid: &str, _name: &str) {
        self.deck_channel
            .insert(deck_uuid.to_string(), channel_uuid.to_string());
    }

    /// Resolve a hierarchical path to a flat canonical engine parameter id.
    ///
    /// Returns `None` for an unknown deck/channel. Bare ids (no slash) and
    /// two-segment `category/id` paths pass through unchanged, so the router is
    /// idempotent over already-flat ids (e.g. `crossfader`, `mixer/crossfader`).
    pub fn resolve(&self, path: &str) -> Option<String> {
        let segs: Vec<&str> = path.split('/').collect();
        match segs.as_slice() {
            // deck/<deck_uuid>/param/<name> → ch_<channel_uuid>_deck_<deck_uuid>_<name>
            ["deck", deck_uuid, "param", name] => self
                .deck_channel
                .get(*deck_uuid)
                .map(|ch| format!("ch_{ch}_deck_{deck_uuid}_{name}")),
            // channel/<channel_uuid>/param/<name> → ch_<channel_uuid>_<name>
            ["channel", ch_uuid, "param", name] => self
                .channels
                .contains(*ch_uuid)
                .then(|| format!("ch_{ch_uuid}_{name}")),
            // bare id, or two-segment category/id → already flat, pass through
            [_] | [_, _] => Some(path.to_string()),
            _ => None,
        }
    }

    /// Clear all registrations.
    pub fn clear(&mut self) {
        self.deck_channel.clear();
        self.channels.clear();
    }

    /// Number of registered decks + channels.
    pub fn len(&self) -> usize {
        self.deck_channel.len() + self.channels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures use bare uuids exactly as the real call sites do
    // (`ch.uuid`/`deck.uuid`), so the asserted ids pin the true canonical
    // scheme `ch_<uuid>_deck_<uuid>_<name>` — not a double-prefixed artifact.

    #[test]
    fn test_resolve_deck_param_any_name() {
        let mut router = ParamRouter::new();
        router.register_deck("c1", "d1", "Deck A");
        // opacity, blend, and an FX-chain param all resolve structurally.
        assert_eq!(
            router.resolve("deck/d1/param/opacity"),
            Some("ch_c1_deck_d1_opacity".to_string())
        );
        assert_eq!(
            router.resolve("deck/d1/param/blend"),
            Some("ch_c1_deck_d1_blend".to_string())
        );
        assert_eq!(
            router.resolve("deck/d1/param/fx9_intensity"),
            Some("ch_c1_deck_d1_fx9_intensity".to_string())
        );
    }

    #[test]
    fn test_resolve_channel_param_any_name() {
        let mut router = ParamRouter::new();
        router.register_channel("c1", "Channel A");
        assert_eq!(
            router.resolve("channel/c1/param/opacity"),
            Some("ch_c1_opacity".to_string())
        );
        assert_eq!(
            router.resolve("channel/c1/param/input_select"),
            Some("ch_c1_input_select".to_string())
        );
    }

    #[test]
    fn test_pass_through_bare_and_flat() {
        let router = ParamRouter::new();
        // bare id (crossfader is handled by pass-through, no registration needed)
        assert_eq!(router.resolve("crossfader"), Some("crossfader".to_string()));
        // two-segment category/id flat path
        assert_eq!(
            router.resolve("mixer/crossfader"),
            Some("mixer/crossfader".to_string())
        );
    }

    #[test]
    fn test_unknown_deck_or_channel_returns_none() {
        let router = ParamRouter::new();
        assert!(router.resolve("deck/unknown/param/opacity").is_none());
        assert!(router.resolve("channel/unknown/param/opacity").is_none());
    }

    /// Cross-check: the router's hierarchical resolution must land on an id the
    /// mixer ACTUALLY registers — builds the real Mixer/DeckCompositor/Deck graph
    /// and asserts membership, so a future scheme divergence (e.g. a double
    /// prefix) fails here instead of silently breaking external control.
    #[cfg(feature = "mixer")]
    #[test]
    fn resolver_output_matches_registered_param_ids() {
        use crate::graph::{Deck, DeckCompositor};
        use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
        use rustjay_mixer::{Channel, Mixer};

        struct StubSource;
        impl EffectInstance for StubSource {
            fn render_to(
                &mut self,
                _ctx: &mut RenderCtx<'_>,
                _inputs: &[EffectInput<'_>],
                _target: RenderTarget<'_>,
                _engine: &EngineState,
            ) {
            }
        }

        let mut comp = DeckCompositor::new();
        comp.decks
            .push(Deck::new("d1", "Deck", Box::new(StubSource)));
        let mut mixer = Mixer::new();
        mixer
            .add_channel(Channel::new("c1", "Ch", Box::new(comp)))
            .unwrap();
        let registered: Vec<String> = mixer.parameters().into_iter().map(|d| d.id).collect();

        let mut router = ParamRouter::new();
        router.register_channel("c1", "Ch");
        router.register_deck("c1", "d1", "Deck");

        let deck_op = router
            .resolve("deck/d1/param/opacity")
            .expect("deck opacity resolves");
        assert!(
            registered.contains(&deck_op),
            "router produced `{deck_op}`, not in registered ids: {registered:?}"
        );

        let ch_op = router
            .resolve("channel/c1/param/opacity")
            .expect("channel opacity resolves");
        assert!(
            registered.contains(&ch_op),
            "router produced `{ch_op}`, not in registered ids: {registered:?}"
        );
    }
}
