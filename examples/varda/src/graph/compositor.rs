//! Deck compositor — implements `EffectInstance` by compositing multiple decks.
//!
//! Reuses `rustjay_mixer::CompositePipeline` and `BlitPipeline` so deck blending
//! uses the same shader path as channel blending.

use rustjay_core::{
    EffectInput, EffectInstance, EngineState, ParameterDescriptor, ParamCategory, RenderCtx,
    RenderTarget,
};
use rustjay_mixer::{BlendMode, BlitPipeline, CompositePipeline};
use rustjay_render::Texture;

use crate::graph::Deck;

/// Composites an ordered list of decks into a single output texture.
///
/// Implements `EffectInstance` so it can be dropped into a `rustjay_mixer::Channel`
/// as the channel's effect. The channel's post-chain then applies FX to the
/// deck composite.
pub struct DeckCompositor {
    /// Decks, composited in index order.
    pub decks: Vec<Deck>,
    /// Parameter prefix (set by enclosing `Channel`).
    param_prefix: String,

    // GPU resources — allocated lazily on first render.
    composite: Option<CompositePipeline>,
    blit: Option<BlitPipeline>,
    acc_a: Option<Texture>,
    acc_b: Option<Texture>,
    size: [u32; 2],
    generation: u64,
}

impl DeckCompositor {
    /// Create an empty compositor.
    pub fn new() -> Self {
        Self {
            decks: Vec::new(),
            param_prefix: String::new(),
            composite: None,
            blit: None,
            acc_a: None,
            acc_b: None,
            size: [0, 0],
            generation: 0,
        }
    }

    /// Ensure GPU resources match `size`.
    fn ensure_resources(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size != size || self.composite.is_none() {
            let format = wgpu::TextureFormat::Bgra8Unorm;
            self.composite = Some(CompositePipeline::new(device, format));
            self.blit = Some(BlitPipeline::new(device, format));
            self.acc_a = Some(Texture::create_render_target(
                device,
                size[0],
                size[1],
                "deck acc_a",
            ));
            self.acc_b = Some(Texture::create_render_target(
                device,
                size[0],
                size[1],
                "deck acc_b",
            ));
            self.size = size;
            self.generation = self.generation.wrapping_add(1);
        }
        for deck in &mut self.decks {
            deck.ensure_size(device, size);
        }
    }
}

impl EffectInstance for DeckCompositor {
    fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
    }

    fn label(&self) -> &str {
        "deck-compositor"
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let mut out = Vec::new();
        for deck in &self.decks {
            let prefix = format!("{}deck_{}_", self.param_prefix, deck.uuid);

            out.push(ParameterDescriptor::float(
                format!("{prefix}opacity"),
                format!("{} Opacity", deck.name),
                ParamCategory::Custom("Deck".to_string()),
                0.0,
                1.0,
                deck.opacity,
                0.01,
            ));

            out.push(ParameterDescriptor::enum_param(
                format!("{prefix}blend"),
                format!("{} Blend", deck.name),
                ParamCategory::Custom("Deck".to_string()),
                BlendMode::all()
                    .iter()
                    .map(|m| m.short_name().to_string())
                    .collect(),
                deck.blend_mode.to_index() as usize,
            ));

            // Source params
            for p in deck.source.parameters() {
                out.push(prefix_descriptor(&prefix, &p));
            }

            // Deck FX chain params
            for (k, fx) in deck.chain.iter().enumerate() {
                let chain_prefix = format!("{prefix}fx{k}_");
                for p in fx.parameters() {
                    out.push(prefix_descriptor(&chain_prefix, &p));
                }
            }
        }
        out
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        engine: &EngineState,
    ) {
        self.ensure_resources(ctx.device, target.size);

        // Detect chain length changes that flip output_texture() parity.
        for deck in &mut self.decks {
            let current = deck.chain.len();
            if deck.last_chain_len != current {
                deck.last_chain_len = current;
                self.generation = self.generation.wrapping_add(1);
            }
        }

        // Temporarily set param prefix so nested decks read prefixed params.
        let old_prefix = engine.param_lookup_prefix.borrow().clone();
        if !self.param_prefix.is_empty() {
            *engine.param_lookup_prefix.borrow_mut() = Some(self.param_prefix.clone());
        }

        // 1. Render each active deck into its own texture.
        for deck in &mut self.decks {
            if deck.opacity < 0.001 {
                continue;
            }
            deck.render(ctx, inputs, engine);
        }

        // 2. Composite decks onto the running accumulation.
        let acc_a = self.acc_a.as_ref().unwrap();
        let acc_b = self.acc_b.as_ref().unwrap();
        let composite = self.composite.as_ref().unwrap();

        clear_texture(ctx.encoder, &acc_a.view);

        let active: Vec<usize> = self
            .decks
            .iter()
            .enumerate()
            .filter(|(_, d)| d.opacity >= 0.001)
            .map(|(i, _)| i)
            .collect();

        let mut written_acc: Option<&Texture> = None;

        for &i in &active {
            let deck = &self.decks[i];
            let Some(src) = deck.output_texture() else { continue };

            let (read_acc, write_acc) = match written_acc {
                None => (acc_a, acc_b),
                Some(w) if std::ptr::eq(w as *const _, acc_a as *const _) => (acc_a, acc_b),
                _ => (acc_b, acc_a),
            };
            let dest_is_a = std::ptr::eq(read_acc as *const _, acc_a as *const _);

            composite.blend(
                ctx.device,
                ctx.queue,
                ctx.encoder,
                self.generation,
                i,
                dest_is_a,
                &src.view,
                &read_acc.view,
                &write_acc.view,
                deck.opacity,
                deck.blend_mode,
                ctx.vertex_buffer,
            );
            written_acc = Some(write_acc);
        }

        let composite_out = written_acc.unwrap_or(acc_a);

        // 3. Blit the composite to the target.
        let blit = self.blit.as_ref().unwrap();
        blit.blit(
            ctx.device,
            ctx.encoder,
            &composite_out.view,
            target.view,
            ctx.vertex_buffer,
        );

        // Restore prefix.
        *engine.param_lookup_prefix.borrow_mut() = old_prefix;
    }
}

/// Clear a texture to transparent black.
fn clear_texture(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("DeckCompositor Clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}

/// Prefix every field of a `ParameterDescriptor`.
fn prefix_descriptor(prefix: &str, desc: &ParameterDescriptor) -> ParameterDescriptor {
    ParameterDescriptor {
        id: format!("{prefix}{}", desc.id),
        name: format!("{} [{}]", desc.name, prefix.trim_end_matches('_')),
        category: desc.category.clone(),
        param_type: desc.param_type.clone(),
        min: desc.min,
        max: desc.max,
        default: desc.default,
        step: desc.step,
    }
}
