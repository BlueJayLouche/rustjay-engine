//! Pipeline and bind-group layout creation for the three waaaves passes.
//!
//! ## Pass layout (design.md §9)
//!
//! | Pass | Shader | Group 0 | Group 1 | Group 2 |
//! |------|--------|---------|---------|---------|
//! | 0 (Block A) | block_a | 4 tex+sampler (ch1, ch2) | uniform | 4 tex+sampler (fb1, temporal) |
//! | 1 (Block B) | block_b | 2 tex+sampler (intermediate_a) | uniform | 4 tex+sampler (fb2, temporal) |
//! | 2 (Block C) | block_c | 4 tex+sampler (inter_a, inter_b) | uniform | — |
//!
//! Bind group layouts:
//! * `bgl_a` — group 0, 4 texture+sampler pairs (pass A & C)
//! * `bgl_b` — group 2, 4 texture+sampler pairs (pass A & B)
//! * `bgl_c` — group 0, 2 texture+sampler pairs (pass B)
//! * `bgl_uniform` — group 1, 1 uniform buffer (all passes)

use rustjay_engine::prelude::{working_format, Vertex};

const BLOCK_A_SHADER: &str = include_str!("../shaders/block_a.wgsl");
const BLOCK_B_SHADER: &str = include_str!("../shaders/block_b.wgsl");
const BLOCK_C_SHADER: &str = include_str!("../shaders/block_c.wgsl");

// ------------------------------------------------------------------
// Sampler & dummy texture
// ------------------------------------------------------------------

/// Shared linear-clamp sampler.
pub fn create_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("waaaves sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    })
}

/// 1×1 black `Bgra8Unorm` texture for unbound sampler slots.
pub fn create_dummy_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("waaaves dummy"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0, 0, 0, 255], // BGRA black
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );

    (texture, view)
}

// ------------------------------------------------------------------
// Bind group layouts
// ------------------------------------------------------------------

fn tex_sampler_pair(binding_base: u32) -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: binding_base,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: binding_base + 1,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
    ]
}

/// Group 0 — 4 texture + sampler pairs (pass A & C).
pub fn create_bgl_a(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl_a (group0 4tex)"),
        entries: &[
            tex_sampler_pair(0), // ch1 / inter_a
            tex_sampler_pair(2), // ch2 / inter_b
        ]
        .concat(),
    })
}

/// Group 2 — 4 texture + sampler pairs (pass A & B feedback/temporal).
pub fn create_bgl_b(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl_b (group2 4tex)"),
        entries: &[
            tex_sampler_pair(0), // fb read
            tex_sampler_pair(2), // temporal
        ]
        .concat(),
    })
}

/// Group 0 — 2 texture + sampler pairs (pass B).
pub fn create_bgl_c(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl_c (group0 2tex)"),
        entries: &tex_sampler_pair(0),
    })
}

/// Group 1 — single uniform buffer (shared across all passes).
pub fn create_bgl_uniform(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("bgl_uniform"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

// ------------------------------------------------------------------
// Render pipelines
// ------------------------------------------------------------------

fn create_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::PipelineLayout,
    shader_source: &str,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[Vertex::desc()],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: working_format(),
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// Pass 0 — Block A (feedback mix + warp).
pub fn create_pipeline_a(
    device: &wgpu::Device,
    bgl_a: &wgpu::BindGroupLayout,
    bgl_uniform: &wgpu::BindGroupLayout,
    bgl_b: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipeline_a layout"),
        bind_group_layouts: &[Some(bgl_a), Some(bgl_uniform), Some(bgl_b)],
        immediate_size: 0,
    });
    create_pipeline(device, "pipeline_a", &layout, BLOCK_A_SHADER)
}

/// Pass 1 — Block B (blur + trail).
pub fn create_pipeline_b(
    device: &wgpu::Device,
    bgl_c: &wgpu::BindGroupLayout,
    bgl_uniform: &wgpu::BindGroupLayout,
    bgl_b: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipeline_b layout"),
        bind_group_layouts: &[Some(bgl_c), Some(bgl_uniform), Some(bgl_b)],
        immediate_size: 0,
    });
    create_pipeline(device, "pipeline_b", &layout, BLOCK_B_SHADER)
}

/// Pass 2 — Block C (color grading / HSB).
pub fn create_pipeline_c(
    device: &wgpu::Device,
    bgl_a: &wgpu::BindGroupLayout,
    bgl_uniform: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("pipeline_c layout"),
        bind_group_layouts: &[Some(bgl_a), Some(bgl_uniform)],
        immediate_size: 0,
    });
    create_pipeline(device, "pipeline_c", &layout, BLOCK_C_SHADER)
}

// ------------------------------------------------------------------
// Uniform buffer & bind group
// ------------------------------------------------------------------

/// Create the uniform buffer (`UNIFORM | COPY_DST`).
pub fn create_uniform_buffer(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("waaaves uniforms"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Create the bind group that binds `uniform_buf` to `bgl_uniform`.
pub fn create_uniform_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("waaaves uniform bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: None,
            }),
        }],
    })
}
