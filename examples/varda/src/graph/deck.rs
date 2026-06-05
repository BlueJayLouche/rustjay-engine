//! A single deck: source + FX chain + opacity + blend mode.

use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_mixer::BlendMode;
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
    pub chain: Vec<Box<dyn EffectInstance>>,
    /// Mix opacity, 0.0–1.0.
    pub opacity: f32,
    /// How this deck blends onto the channel composite.
    pub blend_mode: BlendMode,

    // GPU resources — allocated lazily on first render.
    texture: Option<Texture>,
    ping: Option<Texture>,
    size: [u32; 2],
    last_output: LastOutput,
    /// Last-seen chain length; bumps compositor generation on change.
    pub(crate) last_chain_len: usize,
}

impl Deck {
    /// Create a deck from a source effect.
    pub fn new(uuid: impl Into<String>, name: impl Into<String>, mut source: Box<dyn EffectInstance>) -> Self {
        let uuid = uuid.into();
        let name = name.into();
        source.set_param_prefix(&format!("deck_{}_", &uuid));
        Self {
            uuid,
            name,
            source,
            chain: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::default(),
            texture: None,
            ping: None,
            size: [0, 0],
            last_output: LastOutput::Texture,
            last_chain_len: 0,
        }
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

        for fx in self.chain.iter_mut() {
            let (src_tex, dst_tex) = if is_ping {
                (ping, tex)
            } else {
                (tex, ping)
            };
            let input = EffectInput {
                view: &src_tex.view,
                sampler: &src_tex.sampler,
                generation: src_tex.generation,
                texture: Some(&src_tex.texture),
            };
            fx.render_to(
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
}
