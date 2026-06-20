//! A deck-style ISF effect chain, modelled on `examples/vjarda`'s deck: an
//! ordered list of `IsfEffect`s, ping-ponged source → fx0 → fx1 → … → output.
//!
//! Each slot's parameters are exposed under a stable prefix (`s<n>_`) so they're
//! independently controllable by the GUI / MIDI / OSC / modulation — the engine's
//! `param_lookup_prefix` resolver maps a node's unprefixed reads to its slot.
//!
//! The render-side `nodes` are authoritative; the UI tab drives them through a
//! shared command queue (nodes need a `wgpu::Device`, only available in `render`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rustjay_core::{EffectInput, EffectInstance, RenderCtx, RenderTarget};
use rustjay_engine::prelude::*;
use rustjay_isf::IsfEffect;
use rustjay_render::{EffectNode, Texture};

/// Per-slot info published for the UI (which doesn't see the GPU nodes).
#[derive(Clone)]
pub struct SlotInfo {
    pub prefix: String,
    pub name: String,
}

/// Commands the UI tab posts; drained on the render thread.
pub enum ChainCmd {
    Add(PathBuf),
    Remove(String),         // by prefix
    Move(String, i32),      // by prefix, ±1
    Replace(Vec<PathBuf>),  // wholesale (profile load)
}

/// Shared handles the UI tab holds clones of.
#[derive(Clone)]
pub struct ChainHandle {
    pub cmds: Arc<Mutex<Vec<ChainCmd>>>,
    pub roster: Arc<Mutex<Vec<SlotInfo>>>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct ChainState;

struct Slot {
    prefix: String,
    name: String,
    node: EffectNode<IsfEffect>,
}

pub struct ChainEffect {
    nodes: Vec<Slot>,
    next_id: u32,
    params_dirty: bool,
    ping_a: Option<Texture>,
    ping_b: Option<Texture>,
    size: [u32; 2],
    handle: ChainHandle,
}

impl ChainEffect {
    /// Build a chain seeded with `initial` shaders (added on first render).
    pub fn new(initial: Vec<PathBuf>) -> Self {
        let cmds: Vec<ChainCmd> = initial.into_iter().map(ChainCmd::Add).collect();
        Self {
            nodes: Vec::new(),
            next_id: 0,
            params_dirty: false,
            ping_a: None,
            ping_b: None,
            size: [0, 0],
            handle: ChainHandle {
                cmds: Arc::new(Mutex::new(cmds)),
                roster: Arc::new(Mutex::new(Vec::new())),
            },
        }
    }

    pub fn handle(&self) -> ChainHandle {
        self.handle.clone()
    }

    fn publish_roster(&self) {
        if let Ok(mut r) = self.handle.roster.lock() {
            *r = self
                .nodes
                .iter()
                .map(|s| SlotInfo {
                    prefix: s.prefix.clone(),
                    name: s.name.clone(),
                })
                .collect();
        }
    }

    fn add_shader(&mut self, path: &std::path::Path, device: &wgpu::Device, queue: &wgpu::Queue, engine: &EngineState) {
        let effect = match IsfEffect::from_path(path) {
            Ok(e) => e,
            Err(e) => {
                log::error!("Chain add failed for {}: {e}", path.display());
                return;
            }
        };
        let name = effect.shader_name.clone();
        let prefix = format!("s{}_", self.next_id);
        self.next_id += 1;
        let mut node = EffectNode::new(effect, name.clone(), device, queue, engine);
        node.set_param_prefix(&prefix);
        self.nodes.push(Slot { prefix, name, node });
    }

    fn apply_cmds(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, engine: &EngineState) {
        let cmds: Vec<ChainCmd> = match self.handle.cmds.lock() {
            Ok(mut g) if !g.is_empty() => std::mem::take(&mut *g),
            _ => return,
        };
        let mut changed = false;
        for cmd in cmds {
            match cmd {
                ChainCmd::Add(path) => {
                    self.add_shader(&path, device, queue, engine);
                    changed = true;
                }
                ChainCmd::Remove(prefix) => {
                    if self.nodes.len() > 1 {
                        self.nodes.retain(|s| s.prefix != prefix);
                        changed = true;
                    }
                }
                ChainCmd::Move(prefix, delta) => {
                    if let Some(i) = self.nodes.iter().position(|s| s.prefix == prefix) {
                        let j = i as i32 + delta;
                        if j >= 0 && (j as usize) < self.nodes.len() {
                            self.nodes.swap(i, j as usize);
                            changed = true;
                        }
                    }
                }
                ChainCmd::Replace(paths) => {
                    self.nodes.clear();
                    for path in &paths {
                        self.add_shader(path, device, queue, engine);
                    }
                    changed = true;
                }
            }
        }
        if changed {
            self.params_dirty = true;
            self.publish_roster();
        }
    }

    fn ensure_size(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        if self.size == size && self.ping_a.is_some() {
            return;
        }
        self.ping_a = Some(Texture::create_render_target(device, size[0], size[1], "chain ping a"));
        self.ping_b = Some(Texture::create_render_target(device, size[0], size[1], "chain ping b"));
        self.size = size;
    }
}

impl EffectPlugin for ChainEffect {
    type State = ChainState;
    type Uniforms = [f32; 4];

    fn app_name(&self) -> &str {
        "ShaderGlass"
    }

    fn shader_source(&self) -> &'static str {
        // Compiled but unused — render() overrides.
        include_str!("passthrough.wgsl")
    }

    fn build_uniforms(&self, _state: &Self::State, _engine: &EngineState) -> Self::Uniforms {
        [0.0; 4]
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let mut out = Vec::new();
        for slot in &self.nodes {
            for mut d in slot.node.parameters() {
                d.id = format!("{}{}", slot.prefix, d.id);
                out.push(d);
            }
        }
        out
    }

    fn parameters_dirty(&self) -> bool {
        self.params_dirty
    }

    fn clear_parameters_dirty(&mut self) {
        self.params_dirty = false;
    }

    fn render(&mut self, ctx: &mut RenderHookCtx<'_>, _state: &mut Self::State) -> bool {
        self.apply_cmds(ctx.device, ctx.queue, ctx.engine_state);

        let res = &ctx.engine_state.resolution;
        let size = [res.internal_width.max(1), res.internal_height.max(1)];
        self.ensure_size(ctx.device, size);

        let n = self.nodes.len();
        if n == 0 {
            return true; // nothing to draw this frame
        }
        let primary = match ctx.input.as_ref() {
            Some(i) => i,
            None => return true,
        };
        let ping_a = self.ping_a.as_ref().unwrap();
        let ping_b = self.ping_b.as_ref().unwrap();

        let mut prev: Option<&Texture> = None;
        for (i, slot) in self.nodes.iter_mut().enumerate() {
            let last = i == n - 1;
            let dst: Option<&Texture> = if last {
                None
            } else if i % 2 == 0 {
                Some(ping_a)
            } else {
                Some(ping_b)
            };
            let input = match prev {
                None => EffectInput {
                    view: primary.view,
                    sampler: primary.sampler,
                    generation: primary.generation,
                    texture: primary.texture,
                },
                Some(t) => EffectInput {
                    view: &t.view,
                    sampler: &t.sampler,
                    generation: t.generation,
                    texture: Some(&t.texture),
                },
            };
            let target_view = match dst {
                None => ctx.target_view,
                Some(t) => &t.view,
            };
            slot.node.prepare(ctx.engine_state, ctx.device, ctx.queue);
            let mut rctx = RenderCtx {
                device: ctx.device,
                queue: ctx.queue,
                encoder: &mut *ctx.encoder,
                vertex_buffer: ctx.vertex_buffer,
            };
            slot.node.render_to(
                &mut rctx,
                &[input],
                RenderTarget { view: target_view, size },
                ctx.engine_state,
            );
            prev = dst;
        }
        true
    }
}
