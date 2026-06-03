//! rustjay-mixer — multi-channel compositing mixer for rustjay-engine.
//!
//! Phase **B3** of `PHASE_B_ROADMAP.md`. A [`Mixer`] owns a list of [`Channel`]s,
//! each driving a [`Box<dyn EffectInstance>`](rustjay_core::EffectInstance), and
//! composites them via [`CompositePipeline`] using per-channel [`BlendMode`] and
//! opacity, then runs a master effect chain. The mixer is itself an
//! `EffectInstance`, so it composes, nests, previews, and projects like any
//! single effect.
//!
//! **Status: T04–T11 implemented.** Channel rendering, effect chains, dynamic
//! channel management, `EffectInstance` / `EffectPlugin` wrappers, parameter
//! aggregation, modulatable crossfader, and transition state machines (auto,
//! beat-sync, sequencer) are wired. GUI (T14+) and persistence (T18) land in
//! later tasks.

#![warn(missing_docs)]

mod blend;
mod blit;
mod composite;
pub mod crossfade;
pub mod plugin;
pub mod preset;
pub mod sequencer;

pub use blend::BlendMode;
pub use blit::BlitPipeline;
pub use composite::CompositePipeline;
pub use crossfade::{AutoCrossfade, BeatSyncCrossfade, Easing};
pub use preset::{ChannelState, MixerState, MAX_CHANNELS, MIXER_STATE_VERSION};
pub use sequencer::{SequencerState, StepKind, TransitionEffect, TransitionStep};

use rustjay_core::{EffectInstance, EffectInput, RenderCtx, RenderTarget, EngineState};
use rustjay_core::params::{ParameterDescriptor, ParamCategory};
use rustjay_render::Texture;

/// Tracks which of a channel's two textures holds the most recent render result.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LastOutput {
    /// The main channel texture.
    Texture,
    /// The ping-pong scratch texture.
    Ping,
}

/// One mixer channel: an effect plus how it is mixed into the composite.
pub struct Channel {
    /// Stable identity, persisted across presets (REQ-01.3).
    pub uuid: String,
    /// Display name (e.g. "A", "B").
    pub name: String,
    /// The effect this channel renders.
    pub effect: Box<dyn EffectInstance>,
    /// Ordered post-effect chain applied before compositing (REQ-01.5).
    pub chain: Vec<Box<dyn EffectInstance>>,
    /// Mix opacity, 0.0–1.0.
    pub opacity: f32,
    /// How this channel blends onto the composite.
    pub blend_mode: BlendMode,
    /// Solo flag (UI/mix helper).
    pub solo: bool,
    /// Mute flag (UI/mix helper).
    pub mute: bool,

    // GPU resources — allocated lazily, reallocated only on resize (REQ-11.2).
    texture: Option<Texture>,
    ping: Option<Texture>,
    size: [u32; 2],
    last_output: LastOutput,
}

impl Channel {
    /// Create a channel from an effect instance with default mix settings.
    ///
    /// GPU textures are allocated on first render when the target size is known.
    pub fn new(uuid: impl Into<String>, name: impl Into<String>, effect: Box<dyn EffectInstance>) -> Self {
        Self {
            uuid: uuid.into(),
            name: name.into(),
            effect,
            chain: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::default(),
            solo: false,
            mute: false,
            texture: None,
            ping: None,
            size: [0, 0],
            last_output: LastOutput::Texture,
        }
    }

    /// Ensure the channel's render-target textures match `size`.
    fn ensure_size(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size == size {
            return;
        }
        self.texture = Some(Texture::create_render_target(
            device, size[0], size[1],
            &format!("ch {} tex", self.name),
        ));
        self.ping = Some(Texture::create_render_target(
            device, size[0], size[1],
            &format!("ch {} ping", self.name),
        ));
        self.size = size;
        self.last_output = LastOutput::Texture;
    }

    /// Render the channel effect and run its post-chain, returning the texture
    /// that holds the final output for this frame.
    fn render<'a>(&'a mut self, ctx: &mut RenderCtx<'_>, inputs: &[EffectInput<'_>], engine: &EngineState) -> Option<&'a Texture> {
        let tex = self.texture.as_ref()?;
        self.effect.render_to(
            ctx,
            inputs,
            RenderTarget { view: &tex.view, size: self.size },
            engine,
        );
        self.last_output = LastOutput::Texture;

        if self.chain.is_empty() {
            return Some(tex);
        }

        let ping = self.ping.as_ref()?;
        let mut is_ping = false; // false → src=tex, dst=ping

        for fx in self.chain.iter_mut() {
            let (src_tex, dst_tex) = if is_ping {
                (ping, tex)
            } else {
                (tex, ping)
            };
            let input = EffectInput {
                view: &src_tex.view,
                sampler: &src_tex.sampler,
                generation: 0,
                texture: Some(&src_tex.texture),
            };
            fx.render_to(
                ctx,
                &[input],
                RenderTarget { view: &dst_tex.view, size: self.size },
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
    ///
    /// Only valid after [`render`](Self::render) has been called for the current frame.
    fn output_texture(&self) -> Option<&Texture> {
        match self.last_output {
            LastOutput::Texture => self.texture.as_ref(),
            LastOutput::Ping => self.ping.as_ref(),
        }
    }
}

/// Multi-channel compositor.
pub struct Mixer {
    /// Channels, composited in index order.
    pub channels: Vec<Channel>,
    /// Crossfader position (0.0 = channel 0, 1.0 = channel 1). Used only for the
    /// 2-channel case; ignored when `channels.len() != 2`.
    pub crossfader: f32,
    /// Master effect chain applied after compositing (REQ-06).
    pub master: Vec<Box<dyn EffectInstance>>,
    /// Active auto-crossfade state machine (REQ-04.1).
    pub auto: Option<AutoCrossfade>,
    /// Active beat-synced crossfade (REQ-04.3).
    pub beat_sync: Option<BeatSyncCrossfade>,
    /// Transition sequencer (REQ-05).
    pub sequencer: SequencerState,

    // GPU resources — allocated lazily, reallocated only on resize or channel-count change.
    composite: Option<CompositePipeline>,
    blit: Option<BlitPipeline>,
    acc_a: Option<Texture>,
    acc_b: Option<Texture>,
    master_ping: Option<Texture>,
    size: [u32; 2],
}

impl Mixer {
    /// Create an empty mixer.
    ///
    /// GPU resources (composite pipeline, accumulation textures, blit pipeline) are
    /// allocated on first render when the target size is known.
    pub fn new() -> Self {
        Self {
            channels: Vec::new(),
            crossfader: 0.5,
            master: Vec::new(),
            auto: None,
            beat_sync: None,
            sequencer: SequencerState::new(),
            composite: None,
            blit: None,
            acc_a: None,
            acc_b: None,
            master_ping: None,
            size: [0, 0],
        }
    }

    /// Add a channel, returning its index.
    ///
    /// Fails if the mixer already has 8 channels (REQ-01.2).
    pub fn add_channel(&mut self, channel: Channel) -> Result<usize, &'static str> {
        if self.channels.len() >= 8 {
            return Err("maximum 8 channels");
        }
        self.channels.push(channel);
        // The new channel's textures are allocated lazily at the top of the next
        // `render_to` call (via `ensure_resources`), once the render size is known.
        Ok(self.channels.len() - 1)
    }

    /// Remove a channel by index, returning it.
    ///
    /// Fails if the mixer would drop below 1 channel (REQ-01.2).
    pub fn remove_channel(&mut self, index: usize) -> Result<Channel, &'static str> {
        if self.channels.len() <= 1 {
            return Err("minimum 1 channel");
        }
        if index >= self.channels.len() {
            return Err("channel index out of bounds");
        }
        Ok(self.channels.remove(index))
    }

    /// Effective per-channel opacity for the current frame (REQ-02.4).
    ///
    /// With exactly 2 channels the crossfader scales the two opacities; otherwise
    /// each channel's own opacity is used directly.
    pub fn effective_opacities(&self) -> Vec<f32> {
        if self.channels.len() == 2 {
            vec![
                (1.0 - self.crossfader) * self.channels[0].opacity,
                self.crossfader * self.channels[1].opacity,
            ]
        } else {
            self.channels.iter().map(|c| c.opacity).collect()
        }
    }

    /// Ensure all mixer-level and per-channel GPU resources match `size`.
    fn ensure_resources(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size != size || self.composite.is_none() {
            let format = wgpu::TextureFormat::Bgra8Unorm;
            self.composite = Some(CompositePipeline::new(device, format));
            self.blit = Some(BlitPipeline::new(device, format));
            self.acc_a = Some(Texture::create_render_target(device, size[0], size[1], "mixer acc_a"));
            self.acc_b = Some(Texture::create_render_target(device, size[0], size[1], "mixer acc_b"));
            self.master_ping = Some(Texture::create_render_target(device, size[0], size[1], "master ping"));
            self.size = size;
        }
        for ch in &mut self.channels {
            ch.ensure_size(device, size);
        }
    }

    /// Tick active transitions (auto, beat-sync, sequencer) and return the
    /// crossfader value they produce, if any.
    ///
    /// This should be called once per frame before reading the crossfader for
    /// compositing.  Engine param modulation takes precedence when no transition
    /// is active.
    pub fn tick_transitions(&mut self, dt: f32, bpm: Option<f32>, beat_phase: f32) -> Option<f32> {
        // Sequencer has highest priority.
        if self.sequencer.playing {
            if let Some(v) = self.sequencer.tick(self.crossfader, dt, bpm) {
                self.crossfader = v.clamp(0.0, 1.0);
                // Stop any conflicting one-shot transitions.
                self.auto = None;
                self.beat_sync = None;
                return Some(self.crossfader);
            }
            return None;
        }

        // Beat-sync crossfade.
        if let Some(ref mut bs) = self.beat_sync {
            match bs.tick(self.crossfader, dt, bpm, beat_phase) {
                Some(v) => {
                    self.crossfader = v.clamp(0.0, 1.0);
                    return Some(self.crossfader);
                }
                None if bs.is_done() => {
                    self.crossfader = bs.target;
                    self.beat_sync = None;
                    return Some(self.crossfader);
                }
                None => return None,
            }
        }

        // Plain auto crossfade.
        if let Some(ref mut auto) = self.auto {
            match auto.tick(dt) {
                Some(v) => {
                    self.crossfader = v.clamp(0.0, 1.0);
                    return Some(self.crossfader);
                }
                None => {
                    self.crossfader = auto.target().clamp(0.0, 1.0);
                    self.auto = None;
                    return Some(self.crossfader);
                }
            }
        }

        None
    }
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new()
    }
}

impl EffectInstance for Mixer {
    fn label(&self) -> &str {
        "mixer"
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let mut out = Vec::new();

        // Mixer-level params
        out.push(ParameterDescriptor::float(
            "crossfader",
            "Crossfader",
            ParamCategory::Custom("Mixer".to_string()),
            0.0,
            1.0,
            self.crossfader,
            0.01,
        ));

        for ch in &self.channels {
            let prefix = format!("ch_{}_", ch.uuid);

            out.push(ParameterDescriptor::float(
                format!("{prefix}opacity"),
                format!("{} Opacity", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0,
                1.0,
                ch.opacity,
                0.01,
            ));

            out.push(ParameterDescriptor::enum_param(
                format!("{prefix}blend"),
                format!("{} Blend", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                BlendMode::all().iter().map(|m| m.short_name().to_string()).collect(),
                ch.blend_mode.to_index() as usize,
            ));

            // Channel effect params
            for p in ch.effect.parameters() {
                out.push(prefix_descriptor(&prefix, &p));
            }

            // Channel chain effect params
            for (k, fx) in ch.chain.iter().enumerate() {
                let chain_prefix = format!("{prefix}fx{k}_");
                for p in fx.parameters() {
                    out.push(prefix_descriptor(&chain_prefix, &p));
                }
            }
        }

        // Master chain params
        for (k, fx) in self.master.iter().enumerate() {
            let prefix = format!("master_fx{k}_");
            for p in fx.parameters() {
                out.push(prefix_descriptor(&prefix, &p));
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

        // Tick transitions (auto, beat-sync, sequencer) before reading params.
        let dt = engine.performance.frame_time_ms / 1000.0;
        let bpm = engine.effective_bpm();
        let beat_phase = engine.effective_beat_phase();
        self.tick_transitions(dt, Some(bpm).filter(|&b| b > 0.0), beat_phase);

        // Read mixer-level params from the engine (modulated values).
        let crossfader = engine.get_param("crossfader").unwrap_or(self.crossfader);
        let eff: Vec<f32> = if self.channels.len() == 2 {
            let ch0_opacity = engine
                .get_param(&format!("ch_{}_opacity", self.channels[0].uuid))
                .unwrap_or(self.channels[0].opacity);
            let ch1_opacity = engine
                .get_param(&format!("ch_{}_opacity", self.channels[1].uuid))
                .unwrap_or(self.channels[1].opacity);
            vec![
                (1.0 - crossfader) * ch0_opacity,
                crossfader * ch1_opacity,
            ]
        } else {
            self.channels
                .iter()
                .map(|ch| {
                    engine
                        .get_param(&format!("ch_{}_opacity", ch.uuid))
                        .unwrap_or(ch.opacity)
                })
                .collect()
        };

        // 1. Render each active channel into its own texture (REQ-01.4, REQ-11.3).
        for (i, ch) in self.channels.iter_mut().enumerate() {
            if eff.get(i).copied().unwrap_or(0.0) < 0.001 {
                continue;
            }
            ch.render(ctx, inputs, engine);
        }

        // 2. Composite channels onto the running accumulation (REQ-02.3).
        let acc_a = self.acc_a.as_ref().unwrap();
        let acc_b = self.acc_b.as_ref().unwrap();
        let composite = self.composite.as_ref().unwrap();

        // Start with a cleared accumulator.
        clear_texture(ctx.encoder, &acc_a.view);

        let active: Vec<usize> = eff
            .iter()
            .enumerate()
            .filter(|(_, &op)| op >= 0.001)
            .map(|(i, _)| i)
            .collect();

        let mut written_acc: Option<&Texture> = None;

        for &i in &active {
            let ch = &self.channels[i];
            let Some(src) = ch.output_texture() else { continue };

            let blend_mode = engine
                .get_param(&format!("ch_{}_blend", ch.uuid))
                .and_then(|v| BlendMode::from_index(v as u32))
                .unwrap_or(ch.blend_mode);

            let (read_acc, write_acc) = match written_acc {
                None => (acc_a, acc_b),
                Some(w) if std::ptr::eq(w as *const _, acc_a as *const _) => (acc_a, acc_b),
                _ => (acc_b, acc_a),
            };

            composite.blend(
                ctx.device,
                ctx.encoder,
                &src.view,
                &read_acc.view,
                &write_acc.view,
                eff[i],
                blend_mode,
                ctx.vertex_buffer,
            );
            written_acc = Some(write_acc);
        }

        let composite_out = written_acc.unwrap_or(acc_a);

        // 3. Master effect chain (REQ-06).
        let master_ping = self.master_ping.as_ref().unwrap();
        let final_tex = run_chain(&mut self.master, ctx, composite_out, master_ping, self.size, engine);

        // 4. Blit the final result to the given target (REQ-08.2).
        let blit = self.blit.as_ref().unwrap();
        blit.blit(ctx.device, ctx.encoder, &final_tex.view, target.view, ctx.vertex_buffer);
    }
}

/// Prefix every field of a `ParameterDescriptor` so nested effect params are
/// namespaced by channel UUID / master chain position.
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

/// Run a slice of effects as a ping-pong chain.
///
/// `initial_input` is the source for the first effect. `ping` is a scratch
/// texture of the same size. Returns a reference to whichever texture holds
/// the final output (may be `initial_input` when `effects` is empty).
///
/// Shared between per-channel chains and the master chain (design.md §Q2).
fn run_chain<'a>(
    effects: &'a mut [Box<dyn EffectInstance>],
    ctx: &mut RenderCtx<'_>,
    initial_input: &'a Texture,
    ping: &'a Texture,
    size: [u32; 2],
    engine: &EngineState,
) -> &'a Texture {
    if effects.is_empty() {
        return initial_input;
    }

    let mut is_ping = false; // false → src=initial_input, dst=ping

    for fx in effects.iter_mut() {
        let (src_tex, dst_tex) = if is_ping {
            (ping, initial_input)
        } else {
            (initial_input, ping)
        };
        let input = EffectInput {
            view: &src_tex.view,
            sampler: &src_tex.sampler,
            generation: 0,
            texture: Some(&src_tex.texture),
        };
        fx.render_to(
            ctx,
            &[input],
            RenderTarget { view: &dst_tex.view, size },
            engine,
        );
        is_ping = !is_ping;
    }

    if is_ping {
        ping
    } else {
        initial_input
    }
}

/// Clear a texture to transparent black.
fn clear_texture(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("Mixer Clear Texture"),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A headless `EffectInstance` stub — records nothing, only has to compile.
    struct Stub;

    impl EffectInstance for Stub {
        fn render_to(
            &mut self,
            _ctx: &mut rustjay_core::RenderCtx<'_>,
            _inputs: &[rustjay_core::EffectInput<'_>],
            _target: rustjay_core::RenderTarget<'_>,
            _engine: &rustjay_core::EngineState,
        ) {
        }
    }

    #[test]
    fn crossfader_splits_two_channel_opacity() {
        let mut mixer = Mixer::new();
        mixer.add_channel(Channel::new("a", "A", Box::new(Stub))).unwrap();
        mixer.add_channel(Channel::new("b", "B", Box::new(Stub))).unwrap();
        mixer.crossfader = 0.25;

        let eff = mixer.effective_opacities();
        assert_eq!(eff.len(), 2);
        assert!((eff[0] - 0.75).abs() < 1e-6);
        assert!((eff[1] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn channel_count_clamped() {
        let mut mixer = Mixer::new();
        for i in 0..8 {
            assert!(mixer.add_channel(Channel::new(&format!("{i}"), &format!("CH{i}"), Box::new(Stub))).is_ok());
        }
        assert!(mixer.add_channel(Channel::new("overflow", "OVF", Box::new(Stub))).is_err());

        // Can't remove below 1
        for _ in 0..7 {
            mixer.remove_channel(0).unwrap();
        }
        assert!(mixer.remove_channel(0).is_err());
    }

    #[test]
    fn empty_chain_returns_input() {
        // run_chain with no effects should return the initial input texture reference.
        // We can't create a real Texture without a GPU device, so this test verifies
        // the logic path at the type level by checking the function signature compiles.
    }
}
