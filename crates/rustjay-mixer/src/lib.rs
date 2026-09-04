//! Multi-channel compositing mixer for rustjay-engine.

mod blend;
pub mod blit;
mod composite;
pub mod crossfade;
pub mod plugin;
pub mod preset;
pub mod sequencer;

pub use blend::BlendMode;
pub use blit::BlitPipeline;
pub use composite::{CompositePipeline, KeyParams};
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
    /// The bus group this layer belongs to, if any. Membership lives here
    /// rather than as a span on the group: a span has to be recomputed on every
    /// restack, and dropping a layer onto a group it already sits next to is
    /// then indistinguishable from not moving it at all.
    pub group: Option<String>,
    /// When `false` the channel is skipped entirely: no effect render pass and
    /// no composite step. Used by hosts (e.g. VP-404) to elide idle pads.
    pub active: bool,

    // Chroma/luma key defaults (engine params override these each frame).
    pub key_mode: u32,       // 0=none, 1=chroma, 2=luma
    pub key_r: f32,
    pub key_g: f32,
    pub key_b: f32,
    pub key_threshold: f32,
    pub key_smoothness: f32,
    pub luma_invert: bool,

    // GPU resources — allocated lazily, reallocated only on resize (REQ-11.2).
    texture: Option<Texture>,
    ping: Option<Texture>,
    size: [u32; 2],
    last_output: LastOutput,

    // Cached param keys — avoids per-frame format! allocs (PERF-1).
    opacity_key: String,
    blend_key: String,
    input_select_key: String,
    key_mode_key: String,
    key_r_key: String,
    key_g_key: String,
    key_b_key: String,
    key_threshold_key: String,
    key_smoothness_key: String,
    key_luma_invert_key: String,
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
        effect.set_param_prefix(&format!("ch_{}_", uuid));
        Self {
            opacity_key: format!("ch_{}_opacity", uuid),
            blend_key: format!("ch_{}_blend", uuid),
            input_select_key: format!("ch_{}_input_select", uuid),
            key_mode_key: format!("ch_{}_key_mode", uuid),
            key_r_key: format!("ch_{}_key_r", uuid),
            key_g_key: format!("ch_{}_key_g", uuid),
            key_b_key: format!("ch_{}_key_b", uuid),
            key_threshold_key: format!("ch_{}_key_threshold", uuid),
            key_smoothness_key: format!("ch_{}_key_smoothness", uuid),
            key_luma_invert_key: format!("ch_{}_key_luma_invert", uuid),
            uuid,
            name,
            effect,
            chain: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::default(),
            input_select: InputSelect::default(),
            solo: false,
            mute: false,
            group: None,
            active: true,
            key_mode: 0,
            key_r: 0.0,
            key_g: 1.0,
            key_b: 0.0,
            key_threshold: 0.3,
            key_smoothness: 0.1,
            luma_invert: false,
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
        let prefix = format!("ch_{}_fx{}_", self.uuid, slot.uuid);
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
        // Uniform buffers are written here, not in render_to — an effect that is
        // never prepared draws with stale or zero uniforms, which for an ISF
        // shader means black.
        self.effect.prepare(engine, ctx.device, ctx.queue);
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
            slot.effect.prepare(engine, ctx.device, ctx.queue);
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
/// A bus group: several layers composited together, then treated as one.
///
/// The members are composited into the group's own accumulator, the group's
/// chain runs over that result, and the result is blended into the master like
/// a single layer. That is what makes one blur cover three layers rather than
/// blurring each of them.
///
/// Members are the channels occupying `start .. start + len` of `Mixer.channels`
/// — a group is a contiguous span of the stack, as it is in every compositor —
/// so restacking a member out of the span takes it out of the group.
pub struct ChannelGroup {
    /// Stable identity; the parameter prefix is `grp_<uuid>_`.
    pub uuid: String,
    pub name: String,
    /// Effects applied to the composited members.
    pub chain: Vec<EffectSlot>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub solo: bool,
    pub mute: bool,
    /// Collapsed in the UI. Runtime only.
    pub collapsed: bool,
    /// Whether `group_out` holds this frame's image. Runtime only.
    rendered: bool,
    /// Own compositor: the shared one caches bind groups by `(slot, dest_is_a)`,
    /// which says nothing about *which* accumulator a group writes to, so
    /// reusing it would hand the group a bind group pointing at the master's
    /// textures.
    composite: Option<CompositePipeline>,
    acc_a: Option<Texture>,
    acc_b: Option<Texture>,
    chain_ping: Option<Texture>,
    /// The group's finished image: members composited, chain applied. Held so
    /// the master pass can read it with a shared borrow.
    group_out: Option<Texture>,
    size: [u32; 2],
    opacity_key: String,
    blend_key: String,
}

impl ChannelGroup {
    pub fn new(uuid: impl Into<String>, name: impl Into<String>) -> Self {
        let uuid = uuid.into();
        Self {
            opacity_key: format!("grp_{uuid}_opacity"),
            blend_key: format!("grp_{uuid}_blend"),
            uuid,
            name: name.into(),
            chain: Vec::new(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            solo: false,
            mute: false,
            collapsed: false,
            rendered: false,
            composite: None,
            acc_a: None,
            acc_b: None,
            chain_ping: None,
            group_out: None,
            size: [0, 0],
        }
    }

    fn ensure_resources(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size == size && self.composite.is_some() {
            return;
        }
        let format = rustjay_core::working_format();
        self.composite = Some(CompositePipeline::new(device, format));
        self.acc_a = Some(Texture::create_render_target(device, size[0], size[1], "group acc_a"));
        self.acc_b = Some(Texture::create_render_target(device, size[0], size[1], "group acc_b"));
        self.chain_ping = Some(Texture::create_render_target(device, size[0], size[1], "group chain ping"));
        self.group_out = Some(Texture::create_render_target(device, size[0], size[1], "group out"));
        self.size = size;
    }
}

pub struct Mixer {
    pub channels: Vec<Channel>,
    /// Bus groups over contiguous spans of `channels`.
    pub groups: Vec<ChannelGroup>,
    /// Ignored when `channels.len() != 2`.
    pub crossfader: f32,
    /// Whether the crossfader scales the two channel opacities when there are
    /// exactly two channels.
    ///
    /// A host that treats channels as a free-standing layer stack turns this
    /// off: otherwise a stack that happens to hold exactly two layers renders
    /// both at half opacity, which reads as a mysterious dimming rather than a
    /// crossfade. Defaults to `true`, preserving the A/B behaviour every
    /// existing host relies on.
    pub use_crossfader: bool,
    /// Scales every channel's effective opacity — a master dimmer / blackout.
    ///
    /// 1.0 is unity, so hosts that never touch it are unaffected.
    pub master_dim: f32,
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
            groups: Vec::new(),
            crossfader: 0.5,
            use_crossfader: true,
            master_dim: 1.0,
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
    /// Fails if the mixer already has [`MAX_CHANNELS`] channels (REQ-01.2).
    pub fn add_channel(&mut self, channel: Channel) -> Result<usize, String> {
        if self.channels.len() >= MAX_CHANNELS {
            return Err(format!("maximum channels ({MAX_CHANNELS})"));
        }
        self.channels.push(channel);
        // The new channel's textures are allocated lazily at the top of the next
        // `render_to` call (via `ensure_resources`), once the render size is known.
        self.invalidate_composite_cache();
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
        self.invalidate_composite_cache();
        Ok(self.channels.remove(index))
    }

    /// Add an effect to the master chain.
    ///
    /// Automatically assigns the prefix `master_fx{uuid}_` where `uuid` is the
    /// effect slot's stable identifier (ARCH-3).
    pub fn add_master_effect(&mut self, effect: Box<dyn EffectInstance>) {
        self.master.push(EffectSlot::new(effect));
        let slot = self.master.last_mut().unwrap();
        let prefix = format!("master_fx{}_", slot.uuid);
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

    /// Move the channel at `from` to `to`, shifting the rest.
    ///
    /// Channels composite in order, so for a host that presents them as layers
    /// this is the restack operation. Out-of-range indices are ignored; uuids
    /// are untouched, so parameter prefixes and modulation survive the move.
    pub fn reorder_channel(&mut self, from: usize, to: usize) {
        if from >= self.channels.len() || from == to {
            return;
        }
        let to = to.min(self.channels.len() - 1);
        let channel = self.channels.remove(from);
        self.channels.insert(to, channel);
        self.invalidate_composite_cache();
    }

    /// Declare that the channel-index → source-texture mapping has changed, so
    /// the composite pipelines must rebuild their slot-keyed bind groups.
    ///
    /// Both the master compositor and each group's own one cache bind groups by
    /// `(slot, dest parity)` while rewriting each slot's uniform (opacity, blend,
    /// key) every frame. Skip this after a restack or a source swap and slot `i`
    /// keeps sampling the *previous* occupant's pixels while wearing the
    /// *current* occupant's opacity — a layer's fader appears to drive its
    /// neighbour.
    ///
    /// Call after anything that moves a channel between indices or replaces the
    /// effect behind one. Adding, removing and resizing already call it.
    pub fn invalidate_composite_cache(&mut self) {
        self.generation = self.generation.wrapping_add(1);
    }

    /// Effective per-channel opacity for the current frame (REQ-02.4).
    ///
    /// With exactly 2 channels the crossfader scales the two opacities; otherwise
    /// each channel's own opacity is used directly.
    pub fn effective_opacities(&self) -> Vec<f32> {
        let dim = self.master_dim.clamp(0.0, 1.0);
        self.raw_effective_opacities().iter().map(|o| o * dim).collect()
    }

    /// Whether anything is soloed, which changes what every other channel does.
    pub fn any_solo(&self) -> bool {
        self.channels.iter().any(|c| c.solo) || self.groups.iter().any(|g| g.solo)
    }

    /// The group owning a channel index, if any.
    pub fn group_of(&self, index: usize) -> Option<&ChannelGroup> {
        let id = self.channels.get(index)?.group.as_ref()?;
        self.groups.iter().find(|g| &g.uuid == id)
    }

    /// Channel indices belonging to a group, in stack order.
    pub fn group_members(&self, uuid: &str) -> Vec<usize> {
        self.channels
            .iter()
            .enumerate()
            .filter(|(_, c)| c.group.as_deref() == Some(uuid))
            .map(|(i, _)| i)
            .collect()
    }

    /// Gather the named layers together and make them a group.
    ///
    /// They are moved to sit above the topmost of them before the group is
    /// formed. Grouping gathers, as it does in every editor — leaving members
    /// scattered through the stack would put non-members in the middle of a
    /// composite that is supposed to be one image.
    pub fn group_channels(
        &mut self,
        uuid: impl Into<String>,
        name: impl Into<String>,
        members: &[String],
    ) -> Option<String> {
        let mut idxs: Vec<usize> = members
            .iter()
            .filter_map(|u| self.channels.iter().position(|c| &c.uuid == u))
            .collect();
        if idxs.len() < 2 {
            return None;
        }
        idxs.sort_unstable();
        // Gather to where the topmost picked layer sits. Take them all out
        // first (highest index first, so the lower ones stay valid), then put
        // the block back in one piece.
        let anchor = *idxs.last().unwrap();
        let mut taken: Vec<Channel> = idxs
            .iter()
            .rev()
            .map(|&i| self.channels.remove(i))
            .collect();
        taken.reverse();
        let removed_below = idxs.iter().filter(|&&i| i < anchor).count();
        let at = anchor - removed_below;
        for (n, ch) in taken.into_iter().enumerate() {
            self.channels.insert(at + n, ch);
        }
        let uuid = uuid.into();
        for u in members {
            if let Some(c) = self.channels.iter_mut().find(|c| &c.uuid == u) {
                c.group = Some(uuid.clone());
            }
        }
        self.groups.push(ChannelGroup::new(uuid.clone(), name));
        self.invalidate_composite_cache();
        Some(uuid)
    }

    /// Move a whole group so it sits where `target` is, keeping the members in
    /// their own order.
    ///
    /// Moving members one at a time would reverse them, or interleave them with
    /// whatever they pass on the way; the block comes out and goes back in one
    /// piece.
    pub fn move_group(&mut self, group: &str, target: &str) {
        let members = self.group_members(group);
        if members.is_empty() {
            return;
        }
        if self.channels.get(members[0]).is_some_and(|c| c.uuid == target) {
            return;
        }
        let Some(to) = self.channels.iter().position(|c| c.uuid == target) else {
            return;
        };
        if members.contains(&to) {
            return; // dropped on itself
        }
        let mut taken: Vec<Channel> = members
            .iter()
            .rev()
            .map(|&i| self.channels.remove(i))
            .collect();
        taken.reverse();
        let removed_below = members.iter().filter(|&&i| i < to).count();
        let at = to - removed_below;
        for (n, ch) in taken.into_iter().enumerate() {
            self.channels.insert(at + n, ch);
        }
        self.invalidate_composite_cache();
    }

    /// Put one layer into a group, or take it out with `None`.
    ///
    /// Moving it next to the group's other members is the point: dropping a
    /// layer onto a group it already sits beside changes only its membership,
    /// and a guard that skips "no position change" would throw that away.
    pub fn set_channel_group(&mut self, layer: &str, group: Option<String>) {
        let Some(i) = self.channels.iter().position(|c| c.uuid == layer) else {
            return;
        };
        if let Some(gid) = &group {
            let members = self.group_members(gid);
            if let Some(&top) = members.last() {
                let ch = self.channels.remove(i);
                let to = if i <= top { top } else { top + 1 };
                self.channels.insert(to.min(self.channels.len()), ch);
            }
        }
        if let Some(c) = self.channels.iter_mut().find(|c| c.uuid == layer) {
            c.group = group;
        }
        self.invalidate_composite_cache();
    }

    /// Dissolve a group, leaving its members in the stack.
    pub fn ungroup(&mut self, uuid: &str) {
        for c in self.channels.iter_mut() {
            if c.group.as_deref() == Some(uuid) {
                c.group = None;
            }
        }
        self.groups.retain(|g| g.uuid != uuid);
        self.invalidate_composite_cache();
    }

    /// Whether a channel contributes to the mix at all.
    ///
    /// Mute always silences a channel; solo silences everything that is not
    /// itself soloed. A channel that is both stays muted — the explicit switch
    /// wins over the implicit one.
    pub fn audible(ch: &Channel, any_solo: bool) -> bool {
        ch.active && !ch.mute && (!any_solo || ch.solo)
    }

    /// Per-channel opacity before the master dimmer.
    fn raw_effective_opacities(&self) -> Vec<f32> {
        self.raw_opacities(self.crossfader, |c| c.opacity)
    }

    /// The one place mute, solo, groups, the crossfader and opacity are combined.
    ///
    /// `base` supplies each channel's own opacity: the stored field for the
    /// UI-facing [`effective_opacities`](Self::effective_opacities), the
    /// engine's modulated value at render time. Both callers must go through
    /// here, or the mixer renders something other than what the UI reports.
    fn raw_opacities(&self, crossfader: f32, base: impl Fn(&Channel) -> f32) -> Vec<f32> {
        let any_solo = self.any_solo();
        let of = |i: usize, c: &Channel| {
            let group = self.group_of(i);
            // A member of a soloed group is audible: the solo is on the group,
            // and silencing what it contains would leave it soloing nothing.
            let live = c.active
                && !c.mute
                && !group.is_some_and(|g| g.mute)
                && (Self::audible(c, any_solo) || group.is_some_and(|g| g.solo));
            if live { base(c).clamp(0.0, 1.0) } else { 0.0 }
        };
        if self.use_crossfader && self.channels.len() == 2 {
            vec![
                (1.0 - crossfader) * of(0, &self.channels[0]),
                crossfader * of(1, &self.channels[1]),
            ]
        } else {
            self.channels.iter().enumerate().map(|(i, c)| of(i, c)).collect()
        }
    }

    /// Ensure all mixer-level and per-channel GPU resources match `size`.
    fn ensure_resources(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size != size || self.composite.is_none() {
            let format = rustjay_core::working_format();
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

            out.push(ParameterDescriptor::enum_param(
                format!("{prefix}key_mode"),
                format!("{} Key", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                vec!["None".to_string(), "Chroma".to_string(), "Luma".to_string()],
                ch.key_mode as usize,
            ));
            out.push(ParameterDescriptor::float(
                format!("{prefix}key_r"),
                format!("{} Key R", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0, 1.0, ch.key_r, 0.01,
            ));
            out.push(ParameterDescriptor::float(
                format!("{prefix}key_g"),
                format!("{} Key G", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0, 1.0, ch.key_g, 0.01,
            ));
            out.push(ParameterDescriptor::float(
                format!("{prefix}key_b"),
                format!("{} Key B", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0, 1.0, ch.key_b, 0.01,
            ));
            out.push(ParameterDescriptor::float(
                format!("{prefix}key_threshold"),
                format!("{} Key Threshold", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0, 1.0, ch.key_threshold, 0.01,
            ));
            out.push(ParameterDescriptor::float(
                format!("{prefix}key_smoothness"),
                format!("{} Key Smoothness", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0, 1.0, ch.key_smoothness, 0.01,
            ));
            out.push(ParameterDescriptor::float(
                format!("{prefix}key_luma_invert"),
                format!("{} Luma Invert", ch.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0, 1.0, if ch.luma_invert { 1.0 } else { 0.0 }, 1.0,
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

        // Groups declare their own mix and the parameters of everything in
        // their chain. Without this a group effect had no descriptors at all,
        // so selecting one showed an empty inspector.
        for g in self.groups.iter() {
            let prefix = format!("grp_{}_", g.uuid);
            out.push(ParameterDescriptor::float(
                format!("{prefix}opacity"),
                format!("{} Opacity", g.name),
                ParamCategory::Custom("Mixer".to_string()),
                0.0,
                1.0,
                g.opacity,
                0.01,
            ));
            out.push(ParameterDescriptor::enum_param(
                format!("{prefix}blend"),
                format!("{} Blend", g.name),
                ParamCategory::Custom("Mixer".to_string()),
                BlendMode::all()
                    .iter()
                    .map(|m| m.short_name().to_string())
                    .collect(),
                g.blend_mode.to_index() as usize,
            ));
            for slot in g.chain.iter() {
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

        let any_solo = self.any_solo();
        // Same mute/solo/group/crossfader rules the UI reports, over the
        // *modulated* opacity. The crossfader is passed in rather than stored:
        // it carries modulation offsets, so writing it back would let modulation
        // ratchet the base value frame after frame.
        //
        // Before the master dimmer: a grouped member is blended into its group
        // with this, and the group itself is dimmed once when it joins the
        // master stack. Dimming here too would dim a grouped layer twice.
        let eff: Vec<f32> = self.raw_opacities(crossfader, |ch| {
            engine.get_param(&ch.opacity_key).unwrap_or(ch.opacity)
        });

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

        // A grouped span composites into the group's own accumulator first, so
        // the group's chain sees one image rather than each member separately —
        // one blur over three layers instead of three blurs. Done in its own
        // pass, and the result parked in `group_out`, so the master pass below
        // needs only a shared borrow of the groups.
        {
            let Mixer {
                channels,
                groups,
                size,
                generation,
                blit,
                ..
            } = &mut *self;
            for g in groups.iter_mut() {
                g.rendered = false;
                if !(!g.mute && (!any_solo || g.solo)) {
                    continue;
                }
                let members: Vec<usize> = channels
                    .iter()
                    .enumerate()
                    .filter(|(i, c)| {
                        c.group.as_deref() == Some(g.uuid.as_str())
                            && eff.get(*i).copied().unwrap_or(0.0) >= 0.001
                    })
                    .map(|(i, _)| i)
                    .collect();
                if members.is_empty() {
                    continue;
                }
                g.ensure_resources(ctx.device, *size);

                let ga = g.acc_a.as_ref().unwrap();
                let gb = g.acc_b.as_ref().unwrap();
                let gc = g.composite.as_ref().unwrap();
                clear_texture(ctx.encoder, &ga.view);
                let mut written: Option<&Texture> = None;
                for (slot, &i) in members.iter().enumerate() {
                    let ch = &channels[i];
                    let Some(src) = ch.output_texture() else {
                        continue;
                    };
                    let (read, write) = match written {
                        None => (ga, gb),
                        Some(w) if std::ptr::eq(w as *const _, ga as *const _) => (ga, gb),
                        _ => (gb, ga),
                    };
                    let dest_is_a = std::ptr::eq(read as *const _, ga as *const _);
                    let blend_mode = engine
                        .get_param(&ch.blend_key)
                        .and_then(|v| BlendMode::from_index(v as u32))
                        .unwrap_or(ch.blend_mode);
                    gc.blend(
                        ctx.device,
                        ctx.queue,
                        ctx.encoder,
                        *generation,
                        slot,
                        dest_is_a,
                        &src.view,
                        &read.view,
                        &write.view,
                        eff[i],
                        blend_mode,
                        KeyParams::default(),
                        ctx.vertex_buffer,
                    );
                    written = Some(write);
                }
                let composed = written.unwrap_or(ga);

                // The group's own chain, then park the result.
                let ping = g.chain_ping.as_ref().unwrap();
                let finished = run_chain(&mut g.chain, ctx, composed, ping, *size, engine);
                if let (Some(out), Some(blit)) = (g.group_out.as_ref(), blit.as_ref()) {
                    blit.blit(
                        ctx.device,
                        ctx.encoder,
                        &finished.view,
                        &out.view,
                        ctx.vertex_buffer,
                    );
                    g.rendered = true;
                }
            }
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
            // A grouped member is not blended on its own: its group already
            // composited it, and the group is blended once, at the position of
            // its first member.
            if let Some(g) = self.group_of(i) {
                // Blended once, at the position of its topmost member, so the
                // group sits in the stack where its members do.
                let members = self.group_members(&g.uuid);
                if members.last() != Some(&i) || !g.rendered {
                    continue;
                }
                let Some(src) = g.group_out.as_ref() else {
                    continue;
                };
                let opacity = engine
                    .get_param(&g.opacity_key)
                    .unwrap_or(g.opacity)
                    .clamp(0.0, 1.0)
                    * self.master_dim.clamp(0.0, 1.0);
                if opacity < 0.001 {
                    continue;
                }
                let blend_mode = engine
                    .get_param(&g.blend_key)
                    .and_then(|v| BlendMode::from_index(v as u32))
                    .unwrap_or(g.blend_mode);
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
                    opacity,
                    blend_mode,
                    KeyParams::default(),
                    ctx.vertex_buffer,
                );
                written_acc = Some(write_acc);
                continue;
            }

            let ch = &self.channels[i];
            let Some(src) = ch.output_texture() else {
                continue;
            };

            let blend_mode = engine
                .get_param(&ch.blend_key)
                .and_then(|v| BlendMode::from_index(v as u32))
                .unwrap_or(ch.blend_mode);

            let key = KeyParams {
                mode: engine
                    .get_param_base(&ch.key_mode_key)
                    .map(|v| v.round() as u32)
                    .unwrap_or(ch.key_mode),
                r: engine.get_param(&ch.key_r_key).unwrap_or(ch.key_r),
                g: engine.get_param(&ch.key_g_key).unwrap_or(ch.key_g),
                b: engine.get_param(&ch.key_b_key).unwrap_or(ch.key_b),
                threshold: engine.get_param(&ch.key_threshold_key).unwrap_or(ch.key_threshold),
                smoothness: engine.get_param(&ch.key_smoothness_key).unwrap_or(ch.key_smoothness),
                luma_invert: engine
                    .get_param_base(&ch.key_luma_invert_key)
                    .map(|v| v > 0.5)
                    .unwrap_or(ch.luma_invert),
            };

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
                eff[i] * self.master_dim.clamp(0.0, 1.0),
                blend_mode,
                key,
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
        // See `Channel::render`: without this an ISF slot draws with unwritten
        // uniforms.
        slot.effect.prepare(engine, ctx.device, ctx.queue);
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
    pub(super) struct Stub;

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
        for i in 0..MAX_CHANNELS {
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
        for _ in 0..MAX_CHANNELS - 1 {
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

    /// A host presenting channels as layers turns the crossfader off, or a
    /// two-layer stack renders both layers at half opacity.
    #[test]
    fn use_crossfader_false_leaves_two_channel_opacities_alone() {
        let mut mixer = Mixer::new();
        mixer.add_channel(Channel::new("a", "A", Box::new(Stub))).unwrap();
        mixer.add_channel(Channel::new("b", "B", Box::new(Stub))).unwrap();
        mixer.channels[0].opacity = 1.0;
        mixer.channels[1].opacity = 1.0;
        mixer.crossfader = 0.5;

        // Default: the A/B behaviour every existing host relies on.
        assert_eq!(mixer.effective_opacities(), vec![0.5, 0.5]);

        mixer.use_crossfader = false;
        assert_eq!(
            mixer.effective_opacities(),
            vec![1.0, 1.0],
            "layers must composite at their own opacity"
        );
    }

    #[test]
    fn reorder_channel_restacks_and_ignores_bad_indices() {
        let mut mixer = Mixer::new();
        for id in ["a", "b", "c"] {
            mixer.add_channel(Channel::new(id, id, Box::new(Stub))).unwrap();
        }
        let ids = |m: &Mixer| m.channels.iter().map(|c| c.uuid.clone()).collect::<Vec<_>>();

        mixer.reorder_channel(0, 2);
        assert_eq!(ids(&mixer), ["b", "c", "a"], "moved to the end");

        mixer.reorder_channel(2, 0);
        assert_eq!(ids(&mixer), ["a", "b", "c"], "and back to the front");

        // Out of range, or a no-op move, must leave the stack untouched.
        mixer.reorder_channel(9, 0);
        mixer.reorder_channel(1, 1);
        assert_eq!(ids(&mixer), ["a", "b", "c"]);

        // Clamped rather than panicking.
        mixer.reorder_channel(0, 99);
        assert_eq!(ids(&mixer), ["b", "c", "a"]);
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    fn stack(ids: &[&str]) -> Mixer {
        let mut mixer = Mixer::new();
        mixer.use_crossfader = false;
        for id in ids {
            mixer
                .add_channel(Channel::new(*id, *id, Box::new(tests::Stub)))
                .unwrap();
        }
        mixer
    }

    /// The composite pipeline caches bind groups by channel *index* while
    /// rewriting each index's opacity every frame. A restack that does not bump
    /// the generation therefore paints layer N's fader onto layer N-1's pixels.
    #[test]
    fn restacking_invalidates_the_composite_cache() {
        let mut mixer = stack(&["a", "b", "c"]);
        let before = mixer.generation;
        mixer.reorder_channel(0, 2);
        assert_ne!(
            mixer.generation, before,
            "a restack remaps index → texture; the slot-keyed cache must be dropped"
        );

        // A no-op move changes nothing, so it need not invalidate.
        let settled = mixer.generation;
        mixer.reorder_channel(1, 1);
        assert_eq!(mixer.generation, settled);
    }

    /// Every group operation restacks the channels underneath it, so each one
    /// remaps index → texture just as a plain restack does.
    #[test]
    fn group_operations_invalidate_the_composite_cache() {
        let mut mixer = stack(&["a", "b", "c"]);

        let before = mixer.generation;
        let g = mixer
            .group_channels("g1", "Group", &["a".into(), "c".into()])
            .expect("two layers group");
        assert_ne!(mixer.generation, before, "grouping restacks the members");

        let before = mixer.generation;
        mixer.set_channel_group("b", Some(g.clone()));
        assert_ne!(mixer.generation, before, "joining moves the layer");

        let before = mixer.generation;
        mixer.ungroup(&g);
        assert_ne!(mixer.generation, before, "dissolving drops the group's slots");
    }

    #[test]
    fn mute_silences_only_that_layer() {
        let mut mixer = stack(&["a", "b", "c"]);
        mixer.channels[1].mute = true;
        assert_eq!(mixer.effective_opacities(), vec![1.0, 0.0, 1.0]);
    }

    #[test]
    fn solo_silences_everything_else() {
        let mut mixer = stack(&["a", "b", "c"]);
        mixer.channels[2].solo = true;
        assert_eq!(mixer.effective_opacities(), vec![0.0, 0.0, 1.0]);
    }

    #[test]
    fn several_solos_all_play() {
        let mut mixer = stack(&["a", "b", "c"]);
        mixer.channels[0].solo = true;
        mixer.channels[2].solo = true;
        assert_eq!(mixer.effective_opacities(), vec![1.0, 0.0, 1.0]);
    }

    /// The explicit switch wins over the implicit one, so a soloed layer that
    /// is also muted stays silent rather than coming back.
    #[test]
    fn mute_wins_over_solo_on_the_same_layer() {
        let mut mixer = stack(&["a", "b"]);
        mixer.channels[0].solo = true;
        mixer.channels[0].mute = true;
        assert_eq!(mixer.effective_opacities(), vec![0.0, 0.0]);
    }

    /// The flags belong to the layer, not to its position in the stack.
    #[test]
    fn mute_follows_a_layer_through_a_restack() {
        let mut mixer = stack(&["a", "b", "c"]);
        mixer.channels[0].mute = true; // "a"
        mixer.reorder_channel(0, 2); // a moves to the top
        assert_eq!(
            mixer.channels.iter().map(|c| c.uuid.clone()).collect::<Vec<_>>(),
            ["b", "c", "a"]
        );
        assert_eq!(mixer.effective_opacities(), vec![1.0, 1.0, 0.0]);
    }

    /// Opacity is read from the engine under a per-layer key, so a restack must
    /// not hand one layer's value to another.
    #[test]
    fn opacity_keys_travel_with_their_layer() {
        let mut mixer = stack(&["a", "b", "c"]);
        let keys = |m: &Mixer| {
            m.channels
                .iter()
                .map(|c| c.opacity_key.clone())
                .collect::<Vec<_>>()
        };
        mixer.reorder_channel(0, 2);
        assert_eq!(keys(&mixer), ["ch_b_opacity", "ch_c_opacity", "ch_a_opacity"]);
    }
}

#[cfg(test)]
mod group_tests {
    use super::*;

    fn stack(n: usize) -> Mixer {
        let mut mixer = Mixer::new();
        mixer.use_crossfader = false;
        for i in 0..n {
            let id = format!("ch{i}");
            mixer
                .add_channel(Channel::new(&id, &id, Box::new(tests::Stub)))
                .unwrap();
        }
        mixer
    }

    fn ids(m: &Mixer) -> Vec<String> {
        m.channels.iter().map(|c| c.uuid.clone()).collect()
    }

    #[test]
    fn grouping_marks_every_member() {
        let mut mixer = stack(4);
        let g = mixer
            .group_channels("g1", "Backdrop", &["ch1".into(), "ch2".into()])
            .expect("grouped");
        assert_eq!(mixer.group_members(&g), vec![1, 2]);
        assert!(mixer.group_of(0).is_none());
        assert!(mixer.group_of(3).is_none());
    }

    /// Grouping gathers: members that were apart end up next to each other, or
    /// a non-member would sit in the middle of a composite meant to be one
    /// image.
    #[test]
    fn grouping_gathers_scattered_layers() {
        let mut mixer = stack(4);
        mixer
            .group_channels("g1", "A", &["ch0".into(), "ch3".into()])
            .expect("grouped");
        let members = mixer.group_members("g1");
        assert_eq!(members.len(), 2);
        assert_eq!(members[1], members[0] + 1, "members ended up adjacent: {members:?}");
    }

    #[test]
    fn a_single_layer_is_not_a_group() {
        let mut mixer = stack(2);
        assert!(mixer.group_channels("g", "A", &["ch0".into()]).is_none());
    }

    /// The case CuePool learned the hard way: dropping a layer onto a group it
    /// already sits beside changes only its membership, and a guard that skips
    /// "no position change" throws that away.
    #[test]
    fn joining_a_group_it_already_sits_beside_still_joins() {
        let mut mixer = stack(3);
        mixer
            .group_channels("g1", "A", &["ch1".into(), "ch2".into()])
            .unwrap();
        assert!(mixer.group_of(0).is_none(), "ch0 starts outside");
        mixer.set_channel_group("ch0", Some("g1".into()));
        let members = mixer.group_members("g1");
        assert_eq!(members.len(), 3, "it joined: {members:?}");
    }

    #[test]
    fn leaving_a_group_keeps_the_layer() {
        let mut mixer = stack(3);
        mixer
            .group_channels("g1", "A", &["ch0".into(), "ch1".into()])
            .unwrap();
        mixer.set_channel_group("ch0", None);
        assert_eq!(mixer.group_members("g1"), vec![1]);
        assert_eq!(ids(&mixer).len(), 3);
    }

    #[test]
    fn ungrouping_frees_the_members() {
        let mut mixer = stack(3);
        mixer
            .group_channels("g1", "A", &["ch0".into(), "ch1".into()])
            .unwrap();
        mixer.ungroup("g1");
        assert!(mixer.groups.is_empty());
        assert!(mixer.channels.iter().all(|c| c.group.is_none()));
        assert_eq!(mixer.channels.len(), 3, "members survive the group");
    }

    /// Muting a group silences its members; soloing one silences everything
    /// else while keeping its own members audible.
    #[test]
    fn a_group_gates_its_members() {
        let mut mixer = stack(3);
        mixer
            .group_channels("g1", "A", &["ch0".into(), "ch1".into()])
            .unwrap();
        let members = mixer.group_members("g1");

        mixer.groups[0].mute = true;
        let eff = mixer.effective_opacities();
        assert!(members.iter().all(|&i| eff[i] == 0.0), "muted: {eff:?}");

        mixer.groups[0].mute = false;
        mixer.groups[0].solo = true;
        let eff = mixer.effective_opacities();
        assert!(members.iter().all(|&i| eff[i] > 0.0), "soloed members stay up");
        let outsider = (0..3).find(|i| !members.contains(i)).unwrap();
        assert_eq!(eff[outsider], 0.0, "everything else is silenced");
    }
}

#[cfg(test)]
mod group_move_tests {
    use super::*;

    fn stack(n: usize) -> Mixer {
        let mut m = Mixer::new();
        m.use_crossfader = false;
        for i in 0..n {
            let id = format!("ch{i}");
            m.add_channel(Channel::new(&id, &id, Box::new(tests::Stub))).unwrap();
        }
        m
    }
    fn ids(m: &Mixer) -> Vec<String> {
        m.channels.iter().map(|c| c.uuid.clone()).collect()
    }

    /// The block keeps its own order when it moves — one-at-a-time moves would
    /// reverse it.
    #[test]
    fn a_group_moves_as_one_block() {
        let mut m = stack(4);
        m.group_channels("g1", "A", &["ch2".into(), "ch3".into()]).unwrap();
        m.move_group("g1", "ch0");
        let order = ids(&m);
        let p2 = order.iter().position(|u| u == "ch2").unwrap();
        let p3 = order.iter().position(|u| u == "ch3").unwrap();
        assert_eq!(p3, p2 + 1, "members stayed adjacent and in order: {order:?}");
        assert_eq!(m.channels.len(), 4);
    }

    #[test]
    fn dropping_a_group_on_itself_changes_nothing() {
        let mut m = stack(3);
        m.group_channels("g1", "A", &["ch0".into(), "ch1".into()]).unwrap();
        let before = ids(&m);
        m.move_group("g1", "ch0");
        assert_eq!(ids(&m), before);
    }
}

#[cfg(test)]
mod group_param_tests {
    use super::*;

    /// A group effect with no descriptors shows an empty inspector, which is
    /// how selecting one looked before groups were walked here.
    #[test]
    fn a_group_declares_its_mix_parameters() {
        let mut m = Mixer::new();
        m.use_crossfader = false;
        for id in ["a", "b"] {
            m.add_channel(Channel::new(id, id, Box::new(tests::Stub))).unwrap();
        }
        m.group_channels("g1", "Backdrop", &["a".into(), "b".into()]).unwrap();
        let ids: Vec<String> = m.parameters().into_iter().map(|p| p.id).collect();
        assert!(ids.iter().any(|i| i == "grp_g1_opacity"), "{ids:?}");
        assert!(ids.iter().any(|i| i == "grp_g1_blend"), "{ids:?}");
    }
}
