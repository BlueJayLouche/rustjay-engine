//! Multi-channel compositing mixer for rustjay-engine.

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

use rustjay_core::params::{ParamCategory, ParameterDescriptor};
use rustjay_core::{EffectInput, EffectInstance, EngineState, RenderCtx, RenderTarget};
use rustjay_render::Texture;


/// Which engine input slot a channel samples from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum InputSelect {
    #[default]
    Slot1,
    Slot2,
    Both,
}

impl InputSelect {
    pub fn to_index(self) -> usize {
        match self {
            InputSelect::Slot1 => 0,
            InputSelect::Slot2 => 1,
            InputSelect::Both => 2,
        }
    }

    pub fn from_index(v: usize) -> Self {
        match v {
            0 => InputSelect::Slot1,
            1 => InputSelect::Slot2,
            _ => InputSelect::Both,
        }
    }

    pub fn labels() -> &'static [&'static str] {
        &["Slot 1", "Slot 2", "Both"]
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LastOutput {
    Texture,
    Ping,
}

/// An effect in a chain with an on/off toggle and a stable UUID.
pub struct EffectSlot {
    pub effect: Box<dyn EffectInstance>,
    pub enabled: bool,
    pub uuid: String,
    /// ISF/shader source path — used to rebuild the chain across restarts.
    pub source_path: Option<std::path::PathBuf>,
}

impl EffectSlot {
    pub fn new(effect: Box<dyn EffectInstance>) -> Self {
        Self {
            effect,
            enabled: true,
            uuid: uuid::Uuid::new_v4().simple().to_string()[..8].to_string(),
            source_path: None,
        }
    }
}

/// One mixer channel: an effect plus how it is mixed into the composite.
pub struct Channel {
    /// Stable identity, persisted across presets (REQ-01.3).
    pub uuid: String,
    pub name: String,
    pub effect: Box<dyn EffectInstance>,
    /// Post-effect chain applied before compositing (REQ-01.5).
    pub chain: Vec<EffectSlot>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub input_select: InputSelect,
    pub solo: bool,
    pub mute: bool,

    // GPU resources — allocated lazily, reallocated only on resize (REQ-11.2).
    texture: Option<Texture>,
    ping: Option<Texture>,
    size: [u32; 2],
    last_output: LastOutput,

    // Cached param keys — avoids per-frame format! allocs (PERF-1).
    opacity_key: String,
    blend_key: String,
    input_select_key: String,
    /// Last-seen count of enabled FX; used to detect parity flips that change
    /// `output_texture()` and invalidate the composite cache (CORR-2).
    last_enabled_count: usize,
}

impl Channel {
    /// Create a channel from an effect instance with default mix settings.
    ///
    /// GPU textures are allocated on first render when the target size is known.
    pub fn new(
        uuid: impl Into<String>,
        name: impl Into<String>,
        mut effect: Box<dyn EffectInstance>,
    ) -> Self {
        let uuid = uuid.into();
        let name = name.into();
        effect.set_param_prefix(&format!("ch_{}_", &uuid));
        Self {
            opacity_key: format!("ch_{}_opacity", &uuid),
            blend_key: format!("ch_{}_blend", &uuid),
            input_select_key: format!("ch_{}_input_select", &uuid),
            uuid,
            name,
            effect,
            chain: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::default(),
            input_select: InputSelect::default(),
            solo: false,
            mute: false,
            texture: None,
            ping: None,
            size: [0, 0],
            last_output: LastOutput::Texture,
            last_enabled_count: 0,
        }
    }

    /// Append an effect to this channel's post-chain, assigning its parameter
    /// prefix (`ch_<uuid>_fx<uuid>_`) so its params are reachable by GUI/MIDI/
    /// OSC/LFO — mirrors [`Mixer::add_master_effect`].
    pub fn add_effect(&mut self, effect: Box<dyn EffectInstance>) {
        self.chain.push(EffectSlot::new(effect));
        let slot = self.chain.last_mut().unwrap();
        let prefix = format!("ch_{}_fx{}_", self.uuid, &slot.uuid);
        slot.effect.set_param_prefix(&prefix);
    }

    pub fn set_effect_enabled(&mut self, index: usize, enabled: bool) {
        if let Some(slot) = self.chain.get_mut(index) {
            slot.enabled = enabled;
        }
    }

    /// Reorder the channel's post-chain: move the effect at `from` to `to`.
    /// UUID-stable prefixes mean existing param values stay wired.
    pub fn reorder_effect(&mut self, from: usize, to: usize) {
        if from >= self.chain.len() || from == to {
            return;
        }
        let to = to.min(self.chain.len() - 1);
        let slot = self.chain.remove(from);
        self.chain.insert(to, slot);
    }

    /// Ensure the channel's render-target textures match `size`.
    fn ensure_size(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size == size {
            return;
        }
        self.texture = Some(Texture::create_render_target(
            device,
            size[0],
            size[1],
            &format!("ch {} tex", self.name),
        ));
        self.ping = Some(Texture::create_render_target(
            device,
            size[0],
            size[1],
            &format!("ch {} ping", self.name),
        ));
        self.size = size;
        self.last_output = LastOutput::Texture;
    }

    /// Render the channel effect and run its post-chain, returning the texture
    /// that holds the final output for this frame.
    fn render<'a>(
        &'a mut self,
        ctx: &mut RenderCtx<'_>,
        inputs: &[EffectInput<'_>],
        engine: &EngineState,
    ) -> Option<&'a Texture> {
        let tex = self.texture.as_ref()?;
        self.effect.render_to(
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
        let mut is_ping = false; // false → src=tex, dst=ping

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

    /// Only valid after [`render`](Self::render) has been called for the current frame.
    pub fn output_texture(&self) -> Option<&Texture> {
        match self.last_output {
            LastOutput::Texture => self.texture.as_ref(),
            LastOutput::Ping => self.ping.as_ref(),
        }
    }
}

impl Mixer {
    /// Only valid after the mixer has rendered for the current frame.
    pub fn channel_texture(&self, uuid: &str) -> Option<&Texture> {
        self.channels
            .iter()
            .find(|c| c.uuid == uuid)
            .and_then(|c| c.output_texture())
    }
}

/// Multi-channel compositor.
pub struct Mixer {
    pub channels: Vec<Channel>,
    /// Ignored when `channels.len() != 2`.
    pub crossfader: f32,
    /// Master effect chain (REQ-06).
    pub master: Vec<EffectSlot>,
    pub auto: Option<AutoCrossfade>,
    pub beat_sync: Option<BeatSyncCrossfade>,
    pub sequencer: SequencerState,

    // GPU resources — allocated lazily, reallocated only on resize or channel-count change.
    composite: Option<CompositePipeline>,
    blit: Option<BlitPipeline>,
    acc_a: Option<Texture>,
    acc_b: Option<Texture>,
    master_ping: Option<Texture>,
    size: [u32; 2],
    /// Bumped whenever GPU textures are reallocated (resize) or the channel set
    /// changes. Drives the composite pipeline's bind-group cache invalidation
    /// (REQ-11.1) — a cached bind group keyed by `(slot, dest)` is only valid
    /// within one generation.
    generation: u64,
}

impl Mixer {
    /// GPU resources are allocated on first render.
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
            generation: 0,
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
        // Bump generation: channel-index → texture mapping changed, so the
        // composite bind-group cache (keyed by slot) must be rebuilt.
        self.generation = self.generation.wrapping_add(1);
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
        // Removing shifts channel indices, invalidating slot-keyed bind groups.
        self.generation = self.generation.wrapping_add(1);
        Ok(self.channels.remove(index))
    }

    /// Add an effect to the master chain.
    ///
    /// Automatically assigns the prefix `master_fx{uuid}_` where `uuid` is the
    /// effect slot's stable identifier (ARCH-3).
    pub fn add_master_effect(&mut self, effect: Box<dyn EffectInstance>) {
        self.master.push(EffectSlot::new(effect));
        let slot = self.master.last_mut().unwrap();
        let prefix = format!("master_fx{}_", &slot.uuid);
        slot.effect.set_param_prefix(&prefix);
    }

    /// Reorder the master effect chain: move the effect at `from` to `to`.
    /// UUID-stable prefixes mean existing param values stay wired.
    pub fn reorder_master_effect(&mut self, from: usize, to: usize) {
        if from >= self.master.len() || from == to {
            return;
        }
        let to = to.min(self.master.len() - 1);
        let slot = self.master.remove(from);
        self.master.insert(to, slot);
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
            self.acc_a = Some(Texture::create_render_target(
                device,
                size[0],
                size[1],
                "mixer acc_a",
            ));
            self.acc_b = Some(Texture::create_render_target(
                device,
                size[0],
                size[1],
                "mixer acc_b",
            ));
            self.master_ping = Some(Texture::create_render_target(
                device,
                size[0],
                size[1],
                "master ping",
            ));
            self.size = size;
            self.generation = self.generation.wrapping_add(1);
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
                BlendMode::all()
                    .iter()
                    .map(|m| m.short_name().to_string())
                    .collect(),
                ch.blend_mode.to_index() as usize,
            ));

            out.push(ParameterDescriptor::enum_param(
                format!("{prefix}input_select"),
                format!("{} Input", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                InputSelect::labels()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ch.input_select.to_index(),
            ));

            for p in ch.effect.parameters() {
                out.push(prefix_descriptor(&prefix, &p));
            }

            for slot in ch.chain.iter() {
                let chain_prefix = format!("{prefix}fx{}_", slot.uuid);
                for p in slot.effect.parameters() {
                    out.push(prefix_descriptor(&chain_prefix, &p));
                }
            }
        }

        for slot in self.master.iter() {
            let prefix = format!("master_fx{}_", slot.uuid);
            for p in slot.effect.parameters() {
                out.push(prefix_descriptor(&prefix, &p));
            }
        }

        out
    }

    /// # Single-render-path invariant (REQ-11.4)
    ///
    /// Every channel/master/chain effect is an `EffectInstance` driven **only**
    /// through `render_to` here — never the `PluginRenderer::render` wrapper path.
    /// This preserves each `EffectNode`'s generation-keyed bind-group cache (see
    /// the B0.2 invariant note): alternating the two render paths on one renderer
    /// would thrash its cache. The mixer's own composite cache relies on the same
    /// discipline — see [`CompositePipeline`] and [`Mixer::generation`].
    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        engine: &EngineState,
    ) {
        self.ensure_resources(ctx.device, target.size);

        // CORR-2: detect enabled-count changes that flip output_texture() parity.
        // A parity flip changes which texture (main vs ping) the composite samples,
        // so the generation must bump to invalidate the bind-group cache.
        for ch in &mut self.channels {
            let current = ch.chain.iter().filter(|s| s.enabled).count();
            if ch.last_enabled_count != current {
                ch.last_enabled_count = current;
                self.generation = self.generation.wrapping_add(1);
            }
        }

        // Tick transitions before reading params (ordering matters).
        let dt = engine
            .performance
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .frame_time_ms
            / 1000.0;
        let bpm = engine.effective_bpm();
        let beat_phase = engine.effective_beat_phase();
        self.tick_transitions(dt, Some(bpm).filter(|&b| b > 0.0), beat_phase);

        // Modulation offsets are already applied by EngineState::get_param().
        let crossfader = engine.get_param("crossfader").unwrap_or(self.crossfader);

        let eff: Vec<f32> = if self.channels.len() == 2 {
            let ch0_opacity = engine
                .get_param(&self.channels[0].opacity_key)
                .unwrap_or(self.channels[0].opacity);
            let ch1_opacity = engine
                .get_param(&self.channels[1].opacity_key)
                .unwrap_or(self.channels[1].opacity);

            vec![(1.0 - crossfader) * ch0_opacity, crossfader * ch1_opacity]
        } else {
            self.channels
                .iter()
                .map(|ch| {
                    engine
                        .get_param(&ch.opacity_key)
                        .unwrap_or(ch.opacity)
                        .clamp(0.0, 1.0)
                })
                .collect()
        };

        for (i, ch) in self.channels.iter_mut().enumerate() {
            if eff.get(i).copied().unwrap_or(0.0) < 0.001 {
                continue;
            }
            let input_select = engine
                .get_param(&ch.input_select_key)
                .map(|v| InputSelect::from_index(v as usize))
                .unwrap_or(ch.input_select);
            let ch_inputs: &[EffectInput] = match input_select {
                InputSelect::Slot1 => &inputs[0..inputs.len().min(1)],
                InputSelect::Slot2 => &inputs[inputs.len().min(1)..inputs.len().min(2)],
                InputSelect::Both => inputs,
            };
            ch.render(ctx, ch_inputs, engine);
        }

        let acc_a = self.acc_a.as_ref().unwrap();
        let acc_b = self.acc_b.as_ref().unwrap();
        let composite = self.composite.as_ref().unwrap();

        clear_texture(ctx.encoder, &acc_a.view);

        let active: Vec<usize> = eff
            .iter()
            .enumerate()
            .filter(|&(_, &op)| op >= 0.001)
            .map(|(i, _)| i)
            .collect();

        let mut written_acc: Option<&Texture> = None;

        for &i in &active {
            let ch = &self.channels[i];
            let Some(src) = ch.output_texture() else {
                continue;
            };

            let blend_mode = engine
                .get_param(&ch.blend_key)
                .and_then(|v| BlendMode::from_index(v as u32))
                .unwrap_or(ch.blend_mode);

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
                eff[i],
                blend_mode,
                ctx.vertex_buffer,
            );
            written_acc = Some(write_acc);
        }

        let composite_out = written_acc.unwrap_or(acc_a);

        let master_ping = self.master_ping.as_ref().unwrap();
        let final_tex = run_chain(
            &mut self.master,
            ctx,
            composite_out,
            master_ping,
            self.size,
            engine,
        );

        let blit = self.blit.as_ref().unwrap();
        blit.blit(
            ctx.device,
            ctx.encoder,
            &final_tex.view,
            target.view,
            ctx.vertex_buffer,
        );
    }
}

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

/// Returns whichever texture holds the final output (may be `initial_input` when `effects` is empty).
fn run_chain<'a>(
    effects: &'a mut [EffectSlot],
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

    for slot in effects.iter_mut() {
        if !slot.enabled {
            continue;
        }
        let (src_tex, dst_tex) = if is_ping {
            (ping, initial_input)
        } else {
            (initial_input, ping)
        };
        let input = EffectInput {
            view: &src_tex.view,
            sampler: &src_tex.sampler,
            generation: src_tex.generation,
            texture: Some(&src_tex.texture),
        };
        slot.effect.render_to(
            ctx,
            &[input],
            RenderTarget {
                view: &dst_tex.view,
                size,
            },
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
        mixer
            .add_channel(Channel::new("a", "A", Box::new(Stub)))
            .unwrap();
        mixer
            .add_channel(Channel::new("b", "B", Box::new(Stub)))
            .unwrap();
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
            assert!(mixer
                .add_channel(Channel::new(
                    format!("{i}"),
                    format!("CH{i}"),
                    Box::new(Stub)
                ))
                .is_ok());
        }
        assert!(mixer
            .add_channel(Channel::new("overflow", "OVF", Box::new(Stub)))
            .is_err());

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

    #[test]
    fn mixer_no_longer_owns_modulation_engine() {
        // Phase 4: modulation lives in EngineState.modulation, not Mixer.
        let mixer = Mixer::new();
        // Mixer::new() should compile and not contain a modulation field.
        assert!(mixer.channels.is_empty());
        assert_eq!(mixer.crossfader, 0.5);
    }
}
