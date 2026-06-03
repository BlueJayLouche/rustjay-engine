//! The composite pipeline: blends one channel onto the running accumulation.
//!
//! Reads `source` (the channel) and `dest` (composite-so-far), writes the blend
//! to a third target via `BlendState::REPLACE`. The mixer ping-pongs two
//! accumulation textures because a shader cannot sample its own render target.

use crate::blend::BlendMode;
use rustjay_core::Vertex;
use wgpu::util::DeviceExt;

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
pub struct CompositePipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
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
                        has_dynamic_offset: false,
                        min_binding_size: None,
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

        Self { pipeline, bind_group_layout, sampler }
    }

    /// Blend `source` over `dest`, writing the result into `out`.
    ///
    /// `out` must be a different texture than `dest` (and than `source`). The
    /// mixer alternates two accumulation textures so this holds.
    ///
    /// TODO(B3/T19): this creates a uniform buffer + bind group per call. That
    /// is correct (per-channel params can't alias a shared buffer within one
    /// submission) but allocates per frame. Replace with a generation-keyed
    /// pool / dynamic-offset uniform buffer during the performance pass.
    #[allow(clippy::too_many_arguments)]
    pub fn blend(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        dest: &wgpu::TextureView,
        out: &wgpu::TextureView,
        opacity: f32,
        blend_mode: BlendMode,
        vertex_buffer: &wgpu::Buffer,
    ) {
        let params = CompositeParams {
            opacity,
            blend_mode: blend_mode.to_index(),
            uv_scale: [1.0, 1.0],
            uv_offset: [0.0, 0.0],
            _pad: [0.0, 0.0],
        };
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mixer Composite Params"),
            contents: bytemuck::bytes_of(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Mixer Composite Bind Group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(source) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(dest) },
                wgpu::BindGroupEntry { binding: 3, resource: uniform.as_entire_binding() },
            ],
        });

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
        pass.set_bind_group(0, &bind_group, &[]);
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
