//! Plugin-aware renderer that compiles app-provided shaders and manages
//! the per-effect pipeline, uniform buffer, and bind groups.

use rustjay_core::{EffectPlugin, EngineState, PassInput, Vertex};
use crate::texture::{InputTexture, PreviousFrameTexture, Texture};

pub struct PluginRenderer<P: EffectPlugin> {
    pub plugin: P,
    pub pipeline: wgpu::RenderPipeline,
    pub pipeline_layout: wgpu::PipelineLayout,
    pub texture_bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
    pub uniform_bind_group: wgpu::BindGroup,
    cached_texture_bind_group: Option<wgpu::BindGroup>,
    cached_texture_gen: u64,
    dummy_feedback: Texture,

    // Multi-pass state
    graph_pipelines: Vec<wgpu::RenderPipeline>,
    graph_shaders: Vec<wgpu::ShaderModule>,
    pub intermediate_textures: Vec<Texture>,
}

impl<P: EffectPlugin> PluginRenderer<P> {
    pub fn new(plugin: P, device: &wgpu::Device, queue: &wgpu::Queue, _engine_state: &EngineState) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Plugin Shader"),
            source: wgpu::ShaderSource::Wgsl(plugin.shader_source().into()),
        });

        // Unified texture bind group layout: always 4 entries so the same
        // layout works for single-pass plugins and multi-pass graph passes.
        // Single-pass shaders simply omit @binding(2) and @binding(3).
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

        let dummy_feedback = Texture::from_bgra(
            device, queue, 1, 1, "Dummy Feedback",
            &[0, 0, 0, 255],
        );

        let mut renderer = Self {
            plugin,
            pipeline,
            pipeline_layout,
            texture_bind_group_layout,
            uniform_bind_group_layout,
            uniform_buffer,
            uniform_bind_group,
            cached_texture_bind_group: None,
            cached_texture_gen: u64::MAX,
            dummy_feedback,
            graph_pipelines: Vec::new(),
            graph_shaders: Vec::new(),
            intermediate_textures: Vec::new(),
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
        feedback_texture: Option<&PreviousFrameTexture>,
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

        // Multi-pass graph path
        if let Some(graph) = self.plugin.render_graph() {
            if !graph.passes.is_empty() {
                self.render_graph(
                    encoder, device, queue,
                    input_texture, feedback_texture, render_target,
                    app_state, engine_state, vertex_buffer, &graph,
                );
                return;
            }
        }

        // Single-pass path
        let uniforms = self.plugin.build_uniforms(app_state, engine_state);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

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
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(&self.dummy_feedback.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.dummy_feedback.sampler),
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

    fn render_graph(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_texture: &InputTexture,
        feedback_texture: Option<&PreviousFrameTexture>,
        render_target: &Texture,
        app_state: &mut P::State,
        engine_state: &EngineState,
        vertex_buffer: &wgpu::Buffer,
        graph: &rustjay_core::RenderGraph,
    ) {
        let target_width = render_target.width;
        let target_height = render_target.height;

        // Ensure intermediate textures
        let needed_intermediates = graph.passes.len().saturating_sub(1);
        if self.intermediate_textures.len() != needed_intermediates {
            self.intermediate_textures.clear();
            for i in 0..needed_intermediates {
                self.intermediate_textures.push(Texture::create_render_target(
                    device, target_width, target_height,
                    &format!("Graph Intermediate {}", i),
                ));
            }
        }

        // Ensure graph pipelines and shaders
        if self.graph_pipelines.len() != graph.passes.len() {
            self.graph_pipelines.clear();
            self.graph_shaders.clear();
            for pass in &graph.passes {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(pass.label),
                    source: wgpu::ShaderSource::Wgsl(pass.shader.into()),
                });
                let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(pass.label),
                    layout: Some(&self.pipeline_layout),
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
                self.graph_shaders.push(shader);
                self.graph_pipelines.push(pipeline);
            }
        }

        let intermediate = &self.intermediate_textures;
        let pipelines = &self.graph_pipelines;
        let dummy = &self.dummy_feedback;

        for (i, pass) in graph.passes.iter().enumerate() {
            let is_last = i == graph.passes.len() - 1;
            let output_view: &wgpu::TextureView = if is_last {
                &render_target.view
            } else {
                &intermediate[i].view
            };

            // Resolve input texture & sampler for this pass
            let (input_view, input_sampler): (Option<&wgpu::TextureView>, Option<&wgpu::Sampler>) = match pass.input {
                PassInput::EngineInput => (
                    input_texture.binding_view(),
                    input_texture.binding_sampler(),
                ),
                PassInput::PreviousPass if i > 0 => (
                    Some(&intermediate[i - 1].view),
                    Some(&intermediate[i - 1].sampler),
                ),
                PassInput::Feedback => (
                    feedback_texture.map(|f| &f.texture.view),
                    feedback_texture.map(|f| &f.texture.sampler),
                ),
                _ => (None, None),
            };

            // Update uniforms for this specific pass
            let uniforms = self.plugin.build_pass_uniforms(i, app_state, engine_state);
            queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniforms));

            // Build bind group
            let (iv, is) = match (input_view, input_sampler) {
                (Some(v), Some(s)) => (v, s),
                _ => (&dummy.view, &dummy.sampler),
            };

            let (fbv, fbs) = if graph.feedback {
                match feedback_texture {
                    Some(f) => (&f.texture.view, &f.texture.sampler),
                    None => (&dummy.view, &dummy.sampler),
                }
            } else {
                (&dummy.view, &dummy.sampler)
            };

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("Pass {} Bind Group", i)),
                layout: &self.texture_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(iv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(is),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(fbv),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(fbs),
                    },
                ],
            });

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(pass.label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output_view,
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

            render_pass.set_pipeline(&pipelines[i]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_bind_group(0, &bind_group, &[]);
            render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);
            render_pass.draw(0..6, 0..1);
        }
    }
}
