//! A single deck: source + FX chain + opacity + blend mode.

use crate::sources::SourceKind;
use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_mixer::{BlendMode, EffectSlot};
use rustjay_render::Texture;

/// Tracks which texture holds the most recent render result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LastOutput {
    Texture,
    Ping,
}

/// One deck in the routing graph.
pub struct Deck {
    /// Stable identity.
    pub uuid: String,
    /// Display name.
    pub name: String,
    /// The source effect (ISF shader, video, image, etc.).
    pub source: Box<dyn EffectInstance>,
    /// Ordered post-source FX chain.
    pub chain: Vec<EffectSlot>,
    /// Mix opacity, 0.0–1.0. Base value; `opacity_key` is the modulated source.
    pub opacity: f32,
    /// How this deck blends onto the channel composite. Base value;
    /// `blend_key` is the modulated source.
    pub blend_mode: BlendMode,

    /// Fully-qualified parameter prefix (`<channel>deck_<uuid>_`). Set once the
    /// enclosing channel's prefix is known; defaults to the bare deck prefix.
    pub(crate) full_prefix: String,
    /// Cached lookup key for the deck opacity param (`<full_prefix>opacity`).
    pub(crate) opacity_key: String,
    /// Cached lookup key for the deck blend param (`<full_prefix>blend`).
    pub(crate) blend_key: String,

    // GPU resources — allocated lazily on first render.
    texture: Option<Texture>,
    ping: Option<Texture>,
    size: [u32; 2],
    last_output: LastOutput,
    /// Last-seen count of enabled FX; bumps compositor generation on parity change.
    pub(crate) last_enabled_count: usize,
    /// Path to the source shader (ISF), if applicable — used for hot-reload.
    pub source_path: Option<std::path::PathBuf>,
    /// The kind of source this deck was created from (for UI labeling).
    pub source_kind: SourceKind,
    /// The library descriptor this deck was instantiated from, if any. Carried
    /// so the routing graph can be serialized and rebuilt across restarts
    /// (camera device index, stream URL, etc. that aren't recoverable from
    /// `source_path` alone). `None` for decks built without a registry entry.
    pub source_entry: Option<crate::sources::SourceEntry>,
}

impl Deck {
    /// Create a deck from a source effect.
    pub fn new(
        uuid: impl Into<String>,
        name: impl Into<String>,
        mut source: Box<dyn EffectInstance>,
        source_kind: SourceKind,
    ) -> Self {
        let uuid = uuid.into();
        let name = name.into();
        // Bare default prefix; upgraded to include the channel component once the
        // deck is added to a channel (see `set_full_prefix`).
        let full_prefix = format!("deck_{}_", &uuid);
        source.set_param_prefix(&full_prefix);
        Self {
            opacity_key: format!("{full_prefix}opacity"),
            blend_key: format!("{full_prefix}blend"),
            uuid,
            name,
            source,
            chain: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::default(),
            full_prefix,
            texture: None,
            ping: None,
            size: [0, 0],
            last_output: LastOutput::Texture,
            last_enabled_count: 0,
            source_path: None,
            source_kind,
            source_entry: None,
        }
    }

    /// Assign the fully-qualified parameter prefix once the enclosing channel's
    /// prefix is known. Recomputes the cached opacity/blend keys and re-prefixes
    /// the source and every FX slot so all params resolve to a single canonical
    /// path shared by registration, GUI, MIDI, OSC, and modulation.
    ///
    /// Mirrors `rustjay_mixer::Channel`'s cached `opacity_key`/`blend_key`.
    pub(crate) fn set_full_prefix(&mut self, channel_prefix: &str) {
        self.full_prefix = format!("{channel_prefix}deck_{}_", self.uuid);
        self.opacity_key = format!("{}opacity", self.full_prefix);
        self.blend_key = format!("{}blend", self.full_prefix);
        self.source.set_param_prefix(&self.full_prefix);
        for slot in self.chain.iter_mut() {
            slot.effect
                .set_param_prefix(&format!("{}fx{}_", self.full_prefix, slot.uuid));
        }
    }

    /// Append an FX to this deck's chain, assigning its parameter prefix
    /// (`<full_prefix>fx<index>_`) so its params are reachable by control/modulation.
    pub fn add_effect(&mut self, effect: Box<dyn EffectInstance>) {
        self.chain.push(EffectSlot::new(effect));
        let slot = self.chain.last_mut().unwrap();
        let prefix = format!("{}fx{}_", self.full_prefix, &slot.uuid);
        slot.effect.set_param_prefix(&prefix);
    }

    /// Enable or disable the FX at `index` without removing it. Out-of-range
    /// indices are ignored.
    pub fn set_effect_enabled(&mut self, index: usize, enabled: bool) {
        if let Some(slot) = self.chain.get_mut(index) {
            slot.enabled = enabled;
        }
    }

    /// Reorder the FX chain: move the effect at `from` to `to`.
    /// Stable UUID-based prefixes mean parameter values stay wired.
    pub fn reorder_effect(&mut self, from: usize, to: usize) {
        if from >= self.chain.len() || from == to {
            return;
        }
        let to = to.min(self.chain.len() - 1);
        let slot = self.chain.remove(from);
        self.chain.insert(to, slot);
    }

    /// Ensure render-target textures match `size`.
    pub(crate) fn ensure_size(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size == size {
            return;
        }
        self.texture = Some(Texture::create_render_target(
            device,
            size[0],
            size[1],
            &format!("deck {} tex", self.name),
        ));
        self.ping = Some(Texture::create_render_target(
            device,
            size[0],
            size[1],
            &format!("deck {} ping", self.name),
        ));
        self.size = size;
        self.last_output = LastOutput::Texture;
    }

    /// Render source + chain, returning the output texture for this frame.
    pub(crate) fn render<'a>(
        &'a mut self,
        ctx: &mut RenderCtx<'_>,
        inputs: &[EffectInput<'_>],
        engine: &EngineState,
    ) -> Option<&'a Texture> {
        let tex = self.texture.as_ref()?;
        self.source.prepare(engine, ctx.device, ctx.queue);
        self.source.render_to(
            ctx,
            inputs,
            RenderTarget {
                view: &tex.view,
                size: self.size,
            },
            engine,
        );
        self.last_output = LastOutput::Texture;

        if self.chain.is_empty() {
            return Some(tex);
        }

        let ping = self.ping.as_ref()?;
        let mut is_ping = false;

        for slot in self.chain.iter_mut() {
            if !slot.enabled {
                continue;
            }
            let (src_tex, dst_tex) = if is_ping { (ping, tex) } else { (tex, ping) };
            let input = EffectInput {
                view: &src_tex.view,
                sampler: &src_tex.sampler,
                generation: src_tex.generation,
                texture: Some(&src_tex.texture),
            };
            slot.effect.prepare(engine, ctx.device, ctx.queue);
            slot.effect.render_to(
                ctx,
                &[input],
                RenderTarget {
                    view: &dst_tex.view,
                    size: self.size,
                },
                engine,
            );
            is_ping = !is_ping;
        }

        self.last_output = if is_ping {
            LastOutput::Ping
        } else {
            LastOutput::Texture
        };

        if is_ping {
            Some(ping)
        } else {
            Some(tex)
        }
    }

    /// The texture that holds the most recent render result.
    pub(crate) fn output_texture(&self) -> Option<&Texture> {
        match self.last_output {
            LastOutput::Texture => self.texture.as_ref(),
            LastOutput::Ping => self.ping.as_ref(),
        }
    }

    /// The fully-qualified parameter prefix for this deck.
    pub fn full_prefix(&self) -> &str {
        &self.full_prefix
    }
}
