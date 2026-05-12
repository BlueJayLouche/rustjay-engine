//! Plugin-aware renderer that compiles app-provided shaders and manages
//! the per-effect pipeline, uniform buffer, and bind groups.

use rustjay_core::{EffectPlugin, EngineState, Vertex};
use crate::texture::{InputTexture, Texture};

pub struct PluginRenderer<P: EffectPlugin> {
    pub plugin: P,
    pub pipeline: wgpu::RenderPipeline,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    cached_texture_bind_group: Option<wgpu::BindGroup>,
    cached_texture_gen: u64,
}

impl<P: EffectPlugin> PluginRenderer<P> {
    pub fn new(plugin: P, device: &wgpu::Device, queue: &wgpu::Queue, _engine_state: &EngineState) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Plugin Shader"),
            source: wgpu::ShaderSource::Wgsl(plugin.shader_source().into()),
        });

        let texture_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Uniform Bind Group Layout"),
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
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Pipeline Layout"),
            bind_group_layouts: &[Some(&texture_bind_group_layout), Some(&uniform_bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
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
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
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
        });

        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Uniform Buffer"),
            size: std::mem::size_of::<P::Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Uniform Bind Group"),
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });

        let mut renderer = Self {
            plugin,
            pipeline,
            texture_bind_group_layout,
            uniform_bind_group_layout,
            uniform_buffer,
            uniform_bind_group,
            cached_texture_bind_group: None,
            cached_texture_gen: u64::MAX,
        };
        renderer.plugin.init(device, queue);
        renderer
    }

    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_texture: &InputTexture,
        render_target: &Texture,
        app_state: &mut P::State,
        engine_state: &EngineState,
        vertex_buffer: &wgpu::Buffer,
    ) {
        // Give the plugin a chance to do its own render pass
        if self.plugin.render(
            encoder,
            device,
            queue,
            input_texture.binding_view(),
            input_texture.binding_sampler(),
            &render_target.view,
            app_state,
            engine_state,
            vertex_buffer,
        ) {
            return;
        }

        // Update uniforms
        let uniforms = self.plugin.build_uniforms(app_state, engine_state);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

        // Update texture bind group if input changed
        let current_gen = input_texture.texture_generation;
        if self.cached_texture_gen != current_gen {
            if let (Some(input_view), Some(input_sampler)) = (
                input_texture.binding_view(),
                input_texture.binding_sampler(),
            ) {
                self.cached_texture_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Texture Bind Group"),
                    layout: &self.texture_bind_group_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(input_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(input_sampler),
                        },
                    ],
                }));
                self.cached_texture_gen = current_gen;
            }
        }

        let Some(ref texture_bind_group) = self.cached_texture_bind_group else {
            return;
        };

        let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Main Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &render_target.view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_bind_group(0, texture_bind_group, &[]);
        render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);
        render_pass.draw(0..6, 0..1);
    }
}
