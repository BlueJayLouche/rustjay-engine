//! The composite pipeline: blends one channel onto the running accumulation.
//!
//! Reads `source` (the channel) and `dest` (composite-so-far), writes the blend
//! to a third target via `BlendState::REPLACE`. The mixer ping-pongs two
//! accumulation textures because a shader cannot sample its own render target.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::num::NonZeroU64;

use crate::blend::BlendMode;
use crate::preset::MAX_CHANNELS;
use rustjay_core::Vertex;

/// GPU uniform for one composite invocation. 32 bytes; mirrors `CompositeParams`
/// in `composite.wgsl`.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct CompositeParams {
    opacity: f32,
    blend_mode: u32,
    uv_scale: [f32; 2],
    uv_offset: [f32; 2],
    _pad: [f32; 2],
}

/// Shader-based compositor supporting every [`BlendMode`] via a uniform index.
///
/// **Allocation model (T19 / REQ-11.1):** the pipeline owns a single
/// dynamic-offset uniform buffer (one [`CompositeParams`] slot per channel) and a
/// bind-group cache keyed by `(slot, dest_is_acc_a)`. [`blend`](Self::blend)
/// itself allocates no GPU memory in steady state — it writes the slot's params
/// with `queue.write_buffer` (no GPU alloc) and reuses the cached bind group.
/// (The enclosing `Mixer::render_to` still allocates small per-frame `Vec`s;
/// see PERF-4.) The cache is invalidated wholesale when the caller's `generation`
/// changes (resize or channel add/remove), so stale texture views are never sampled.
pub struct CompositePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Dynamic-offset uniform buffer: `MAX_CHANNELS` slots of `slot_stride` bytes.
    uniform_buffer: wgpu::Buffer,
    /// Per-slot stride, aligned to `min_uniform_buffer_offset_alignment`.
    slot_stride: u32,
    /// Bind groups keyed by `(slot, dest_is_acc_a)`; valid for `cache_generation`.
    cache: RefCell<HashMap<(usize, bool), wgpu::BindGroup>>,
    /// The generation the cached bind groups were built for.
    cache_generation: Cell<u64>,
}

impl CompositePipeline {
    /// Build the pipeline for the given accumulation texture format (typically
    /// `Rgba8Unorm` / `Bgra8Unorm`).
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mixer Composite Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("composite.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Mixer Composite BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: NonZeroU64::new(
                            std::mem::size_of::<CompositeParams>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Mixer Composite Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Mixer Composite Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        // Dynamic-offset uniform buffer: one slot per channel, each aligned to
        // the device's minimum offset alignment (REQ-11.1).
        let align = device.limits().min_uniform_buffer_offset_alignment;
        let slot_size = std::mem::size_of::<CompositeParams>() as u32;
        let slot_stride = slot_size.div_ceil(align) * align;
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mixer Composite Params (dynamic)"),
            size: (slot_stride as u64) * (MAX_CHANNELS as u64),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            uniform_buffer,
            slot_stride,
            cache: RefCell::new(HashMap::new()),
            cache_generation: Cell::new(u64::MAX),
        }
    }

    /// Blend `source` over `dest`, writing the result into `out`.
    ///
    /// `out` must be a different texture than `dest` (and than `source`). The
    /// mixer alternates two accumulation textures so this holds.
    ///
    /// `slot` is the channel index (0..[`MAX_CHANNELS`]); it selects this
    /// channel's uniform slot *and* keys the bind-group cache together with
    /// `dest_is_a` (whether `dest` is the `acc_a` accumulation texture). The
    /// caller passes a `generation` that bumps whenever any input texture is
    /// reallocated (resize) or the channel set changes; on a new generation the
    /// whole cache is dropped so no stale view is ever sampled (REQ-11.1).
    ///
    /// Steady-state cost: one `queue.write_buffer` (no GPU allocation) plus a
    /// cached bind-group lookup. Nothing is allocated per frame (REQ-11.2).
    #[allow(clippy::too_many_arguments)]
    pub fn blend(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        generation: u64,
        slot: usize,
        dest_is_a: bool,
        source: &wgpu::TextureView,
        dest: &wgpu::TextureView,
        out: &wgpu::TextureView,
        opacity: f32,
        blend_mode: BlendMode,
        vertex_buffer: &wgpu::Buffer,
    ) {
        debug_assert!(slot < MAX_CHANNELS, "composite slot {slot} exceeds MAX_CHANNELS");

        // Drop cached bind groups built for a previous generation (resize /
        // channel add-remove changed the texture views they reference).
        if self.cache_generation.get() != generation {
            self.cache.borrow_mut().clear();
            self.cache_generation.set(generation);
        }

        // Write this channel's params into its dynamic-offset slot. write_buffer
        // does not allocate GPU memory after the buffer's initial sizing.
        let offset = (slot as u64) * (self.slot_stride as u64);
        let params = CompositeParams {
            opacity,
            blend_mode: blend_mode.to_index(),
            uv_scale: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
            _pad: [0.0, 0.0],
        };
        queue.write_buffer(&self.uniform_buffer, offset, bytemuck::bytes_of(&params));

        // The bind group depends only on (source, dest) views, which are stable
        // for a fixed (slot, dest_is_a) within one generation — so cache it.
        let key = (slot, dest_is_a);
        if !self.cache.borrow().contains_key(&key) {
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Mixer Composite Bind Group"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(source) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(dest) },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.uniform_buffer,
                            offset: 0,
                            size: NonZeroU64::new(std::mem::size_of::<CompositeParams>() as u64),
                        }),
                    },
                ],
            });
            self.cache.borrow_mut().insert(key, bind_group);
        }
        let cache = self.cache.borrow();
        let bind_group = cache.get(&key).expect("bind group just inserted");

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Mixer Composite Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: out,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        pass.set_bind_group(0, bind_group, &[offset as u32]);
        pass.draw(0..6, 0..1);
    }
}

#[cfg(test)]
mod tests {
    /// Validate `composite.wgsl` with naga's WGSL front-end — no GPU required.
    /// Satisfies the spec's "naga validate with zero errors" acceptance.
    #[test]
    fn composite_shader_validates() {
        let src = include_str!("composite.wgsl");
        let module = naga::front::wgsl::parse_str(src).expect("composite.wgsl parses");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator.validate(&module).expect("composite.wgsl validates");
    }
}
