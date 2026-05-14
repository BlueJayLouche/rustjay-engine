//! Plugin-aware renderer that compiles app-provided shaders and manages
//! the per-effect pipeline, uniform buffer, and bind groups.

use rustjay_core::{EffectPlugin, EngineState, MeshDescriptor, MeshTopology, PassInput, Vertex};
use crate::texture::{InputTexture, PreviousFrameTexture, Texture};
use wgpu::util::DeviceExt;

pub(crate) struct PluginRenderer<P: EffectPlugin> {
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

    /// Cached result of `plugin.render_graph()` — avoids a Vec allocation every frame.
    pub cached_graph: Option<rustjay_core::RenderGraph>,

    // Multi-pass state
    graph_pipelines: Vec<wgpu::RenderPipeline>,
    graph_shaders: Vec<wgpu::ShaderModule>,
    /// Stores the shader source pointer for each compiled pass so we can detect
    /// shader changes (not just count changes) when deciding whether to rebuild.
    graph_shader_sources: Vec<&'static str>,
    /// Per-pass uniform buffer — one per pass so build_pass_uniforms works correctly.
    graph_uniform_buffers: Vec<wgpu::Buffer>,
    /// Per-pass uniform bind group referencing the corresponding buffer.
    graph_uniform_bind_groups: Vec<wgpu::BindGroup>,
    pub intermediate_textures: Vec<Texture>,

    // Mesh state
    mesh_vertex_buffer: Option<wgpu::Buffer>,
    mesh_index_buffer: Option<wgpu::Buffer>,
    mesh_index_count: u32,
    mesh_vertex_count: u32,
    cached_mesh: Option<MeshDescriptor>,

    // Compute mesh state
    compute_pipeline: Option<wgpu::ComputePipeline>,
    compute_bind_group: Option<wgpu::BindGroup>,
    compute_workgroups: (u32, u32, u32),
}

/// Generate mesh vertex and index data from a descriptor.
///
/// Returns `(vertices, indices)`. For `MeshTopology::Points`, `indices` is empty.
fn generate_mesh_data(desc: MeshDescriptor) -> (Vec<Vertex>, Vec<u32>) {
    let cols = desc.cols.max(1);
    let rows = desc.rows.max(1);
    let vertex_count = ((cols + 1) * (rows + 1)) as usize;

    let mut vertices = Vec::with_capacity(vertex_count);
    for row in 0..=rows {
        let v = row as f32 / rows as f32;
        for col in 0..=cols {
            let u = col as f32 / cols as f32;
            let x = u * 2.0 - 1.0;
            let y = 1.0 - v * 2.0;
            vertices.push(Vertex {
                position: [x, y],
                texcoord: [u, v],
            });
        }
    }

    let mut indices = Vec::new();
    match desc.topology {
        MeshTopology::Scanlines => {
            // (rows + 1) horizontal lines, each with cols segments.
            indices.reserve(((rows + 1) * cols * 2) as usize);
            for row in 0..=rows {
                for col in 0..cols {
                    let base = row * (cols + 1) + col;
                    indices.push(base);
                    indices.push(base + 1);
                }
            }
        }
        MeshTopology::Triangles | MeshTopology::Wireframe => {
            // rows * cols cells, 2 triangles each, 6 indices.
            // Wireframe uses the same index buffer but PolygonMode::Line.
            indices.reserve((rows * cols * 6) as usize);
            for row in 0..rows {
                for col in 0..cols {
                    let tl = row * (cols + 1) + col;
                    let tr = tl + 1;
                    let bl = (row + 1) * (cols + 1) + col;
                    let br = bl + 1;
                    // CCW winding
                    indices.push(tl);
                    indices.push(bl);
                    indices.push(tr);
                    indices.push(tr);
                    indices.push(bl);
                    indices.push(br);
                }
            }
        }
        MeshTopology::Points => {
            // No index buffer needed — vertices are drawn directly as PointList.
        }
    }

    (vertices, indices)
}

/// Create GPU buffers from mesh data.
///
/// Returns `(vertex_buffer, index_buffer, index_count)`.
fn create_mesh_buffers(
    device: &wgpu::Device,
    vertices: &[Vertex],
    indices: &[u32],
    vertex_usage: wgpu::BufferUsages,
) -> (wgpu::Buffer, Option<wgpu::Buffer>, u32) {
    let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("Mesh Vertex Buffer"),
        contents: bytemuck::cast_slice(vertices),
        usage: vertex_usage,
    });

    let index_count = indices.len() as u32;

    let index_buffer = if indices.is_empty() {
        None
    } else {
        Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Mesh Index Buffer"),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        }))
    };

    (vertex_buffer, index_buffer, index_count)
}

fn wgpu_topology(topology: MeshTopology) -> wgpu::PrimitiveTopology {
    match topology {
        MeshTopology::Scanlines => wgpu::PrimitiveTopology::LineList,
        MeshTopology::Triangles | MeshTopology::Wireframe => wgpu::PrimitiveTopology::TriangleList,
        MeshTopology::Points => wgpu::PrimitiveTopology::PointList,
    }
}

fn wgpu_polygon_mode(topology: MeshTopology) -> wgpu::PolygonMode {
    match topology {
        MeshTopology::Wireframe => wgpu::PolygonMode::Line,
        _ => wgpu::PolygonMode::Fill,
    }
}

/// Build compute pipeline and bind group for GPU mesh deformation.
///
/// Dispatches 1D workgroups of size 256. The compute shader should use
/// `@workgroup_size(256, 1, 1)` and index vertices with `id.x`.
fn build_compute_resources(
    device: &wgpu::Device,
    compute_shader: &str,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
    vertex_buffer: &wgpu::Buffer,
    vertex_count: u32,
) -> (wgpu::ComputePipeline, wgpu::BindGroup, (u32, u32, u32)) {
    let cs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Compute Shader"),
        source: wgpu::ShaderSource::Wgsl(compute_shader.into()),
    });

    let storage_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Compute Storage Bind Group Layout"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });

    let compute_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Compute Pipeline Layout"),
        bind_group_layouts: &[
            Some(uniform_bind_group_layout),
            Some(&storage_bind_group_layout),
        ],
        ..Default::default()
    });

    let compute_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Compute Pipeline"),
        layout: Some(&compute_pipeline_layout),
        module: &cs_module,
        entry_point: Some("cs_main"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let storage_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Compute Storage Bind Group"),
        layout: &storage_bind_group_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: vertex_buffer.as_entire_binding(),
        }],
    });

    // 1D dispatch: workgroup size 256.
    let workgroups = ((vertex_count + 255) / 256).max(1);

    (compute_pipeline, storage_bind_group, (workgroups, 1, 1))
}

impl<P: EffectPlugin> PluginRenderer<P> {
    pub fn new(plugin: P, device: &wgpu::Device, queue: &wgpu::Queue, _engine_state: &EngineState) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Plugin Shader"),
            source: wgpu::ShaderSource::Wgsl(plugin.shader_source().into()),
        });

        let shader_stages = if plugin.vertex_reads_texture() {
            wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT
        } else {
            wgpu::ShaderStages::FRAGMENT
        };

        // Unified texture bind group layout: always 4 entries so the same
        // layout works for single-pass plugins and multi-pass graph passes.
        // Single-pass shaders simply omit @binding(2) and @binding(3).
        let texture_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Texture Bind Group Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: shader_stages,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: shader_stages,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: shader_stages,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: shader_stages,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let uniform_bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Uniform Bind Group Layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: shader_stages,
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

        let initial_mesh = plugin.mesh_descriptor(&plugin.default_state());
        let initial_topology = initial_mesh.map(|m| m.topology);

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
                topology: initial_topology.map(wgpu_topology).unwrap_or(wgpu::PrimitiveTopology::TriangleList),
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: initial_topology.map(wgpu_polygon_mode).unwrap_or(wgpu::PolygonMode::Fill),
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

        // Cache the graph before plugin is moved into Self.
        let cached_graph = plugin.render_graph();

        // Generate initial mesh if the plugin declares one.
        let (mesh_vertex_buffer, mesh_index_buffer, mesh_index_count, mesh_vertex_count) =
            if let Some(desc) = initial_mesh {
                let (vertices, indices) = generate_mesh_data(desc);
                let vertex_usage = if plugin.compute_shader().is_some() {
                    wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE
                } else {
                    wgpu::BufferUsages::VERTEX
                };
                let (vb, ib, count) = create_mesh_buffers(device, &vertices, &indices, vertex_usage);
                let vertex_count = ((desc.cols + 1) * (desc.rows + 1)) as u32;
                (Some(vb), ib, count, vertex_count)
            } else {
                (None, None, 0, 0)
            };

        // Build compute resources if the plugin provides a compute shader and a mesh.
        let (compute_pipeline, compute_bind_group, compute_workgroups) =
            if let (Some(cs), Some(desc)) = (plugin.compute_shader(), initial_mesh) {
                if let Some(ref vb) = mesh_vertex_buffer {
                    let vertex_count = ((desc.cols + 1) * (desc.rows + 1)) as u32;
                    let (cp, cb, wg) = build_compute_resources(
                        device, cs, &uniform_bind_group_layout, vb, vertex_count,
                    );
                    (Some(cp), Some(cb), wg)
                } else {
                    (None, None, (0, 0, 0))
                }
            } else {
                (None, None, (0, 0, 0))
            };

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
            cached_graph,
            graph_pipelines: Vec::new(),
            graph_shaders: Vec::new(),
            graph_shader_sources: Vec::new(),
            graph_uniform_buffers: Vec::new(),
            graph_uniform_bind_groups: Vec::new(),
            intermediate_textures: Vec::new(),
            mesh_vertex_buffer,
            mesh_index_buffer,
            mesh_index_count,
            mesh_vertex_count,
            cached_mesh: initial_mesh,
            compute_pipeline,
            compute_bind_group,
            compute_workgroups,
        };
        renderer.plugin.init(device, queue);
        renderer
    }

    fn rebuild_single_pass_pipeline(&mut self, device: &wgpu::Device) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Plugin Shader"),
            source: wgpu::ShaderSource::Wgsl(self.plugin.shader_source().into()),
        });

        let topology = self.cached_mesh
            .map(|m| wgpu_topology(m.topology))
            .unwrap_or(wgpu::PrimitiveTopology::TriangleList);
        let polygon_mode = self.cached_mesh
            .map(|m| wgpu_polygon_mode(m.topology))
            .unwrap_or(wgpu::PolygonMode::Fill);

        self.pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Render Pipeline"),
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
                topology,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
    }

    fn check_mesh_dirty(&mut self, device: &wgpu::Device, app_state: &P::State) {
        let current = self.plugin.mesh_descriptor(app_state);
        if self.cached_mesh == current {
            return;
        }

        let topology_changed = self.cached_mesh.map(|m| m.topology) != current.map(|m| m.topology);

        if let Some(desc) = current {
            let (vertices, indices) = generate_mesh_data(desc);
            let vertex_usage = if self.plugin.compute_shader().is_some() {
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::STORAGE
            } else {
                wgpu::BufferUsages::VERTEX
            };
            let (vb, ib, count) = create_mesh_buffers(device, &vertices, &indices, vertex_usage);
            self.mesh_vertex_buffer = Some(vb);
            self.mesh_index_buffer = ib;
            self.mesh_index_count = count;
            self.mesh_vertex_count = ((desc.cols + 1) * (desc.rows + 1)) as u32;

            // Rebuild compute resources if the plugin uses a compute shader.
            if let Some(cs) = self.plugin.compute_shader() {
                let vb = self.mesh_vertex_buffer.as_ref().unwrap();
                let vertex_count = ((desc.cols + 1) * (desc.rows + 1)) as u32;
                let (cp, cb, wg) = build_compute_resources(
                    device, cs, &self.uniform_bind_group_layout, vb, vertex_count,
                );
                self.compute_pipeline = Some(cp);
                self.compute_bind_group = Some(cb);
                self.compute_workgroups = wg;
            }
        } else {
            self.mesh_vertex_buffer = None;
            self.mesh_index_buffer = None;
            self.mesh_index_count = 0;
            self.mesh_vertex_count = 0;
            self.compute_pipeline = None;
            self.compute_bind_group = None;
            self.compute_workgroups = (0, 0, 0);
        }

        self.cached_mesh = current;

        // Pipeline bakes topology — only rebuild when it actually changes.
        if topology_changed {
            self.rebuild_single_pass_pipeline(device);
            self.graph_pipelines.clear();
            self.graph_shaders.clear();
            self.graph_shader_sources.clear();
            self.graph_uniform_buffers.clear();
            self.graph_uniform_bind_groups.clear();
        }
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
        self.check_mesh_dirty(device, app_state);

        // Run compute pass if the plugin provides a compute shader.
        if let (Some(ref pipeline), Some(ref bind_group)) = (&self.compute_pipeline, &self.compute_bind_group) {
            let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Mesh Compute Pass"),
                timestamp_writes: None,
            });
            compute_pass.set_pipeline(pipeline);
            compute_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            compute_pass.set_bind_group(1, bind_group, &[]);
            let (wx, wy, wz) = self.compute_workgroups;
            compute_pass.dispatch_workgroups(wx, wy, wz);
        }

        // Give the plugin a chance to do its own render pass
        let raw_input = input_texture.texture.as_ref().map(|t| &t.texture);
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
            raw_input,
        ) {
            return;
        }

        // Multi-pass graph path — clone is cheap: Vec of small structs with &'static str fields.
        if let Some(graph) = self.cached_graph.clone() {
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

        let vb = self.mesh_vertex_buffer.as_ref().unwrap_or(vertex_buffer);
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_vertex_buffer(0, vb.slice(..));
        render_pass.set_bind_group(0, texture_bind_group, &[]);
        render_pass.set_bind_group(1, &self.uniform_bind_group, &[]);

        if let Some(ref index_buf) = self.mesh_index_buffer {
            render_pass.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..self.mesh_index_count, 0, 0..1);
        } else if self.mesh_vertex_buffer.is_some() {
            // PointList mode — draw vertices directly.
            render_pass.draw(0..self.mesh_vertex_count, 0..1);
        } else {
            render_pass.draw(0..6, 0..1);
        }
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

        // Ensure intermediate textures (one per non-final pass).
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

        // Rebuild pipelines + per-pass uniform buffers when pass count or
        // shader content changes, or when mesh topology changes.
        let graph_topology = self.cached_mesh
            .map(|m| wgpu_topology(m.topology))
            .unwrap_or(wgpu::PrimitiveTopology::TriangleList);
        let graph_polygon_mode = self.cached_mesh
            .map(|m| wgpu_polygon_mode(m.topology))
            .unwrap_or(wgpu::PolygonMode::Fill);

        let needs_rebuild = self.graph_pipelines.len() != graph.passes.len()
            || graph.passes.iter().zip(self.graph_shader_sources.iter())
                .any(|(pass, &src)| !std::ptr::eq(pass.shader.as_bytes(), src.as_bytes()));

        if needs_rebuild {
            self.graph_pipelines.clear();
            self.graph_shaders.clear();
            self.graph_shader_sources.clear();
            self.graph_uniform_buffers.clear();
            self.graph_uniform_bind_groups.clear();

            // Validate: PassInput::PreviousPass on pass 0 is a mistake.
            if let Some(first) = graph.passes.first() {
                if first.input == PassInput::PreviousPass {
                    log::warn!(
                        "[RenderGraph] pass[0] declares PreviousPass input — \
                         no previous pass exists; will use dummy black texture"
                    );
                }
            }

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
                        topology: graph_topology,
                        strip_index_format: None,
                        front_face: wgpu::FrontFace::Ccw,
                        cull_mode: None,
                        polygon_mode: graph_polygon_mode,
                        unclipped_depth: false,
                        conservative: false,
                    },
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });

                // Per-pass uniform buffer so build_pass_uniforms works correctly.
                let buf = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("{} Uniform Buffer", pass.label)),
                    size: std::mem::size_of::<P::Uniforms>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(&format!("{} Uniform Bind Group", pass.label)),
                    layout: &self.uniform_bind_group_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buf.as_entire_binding(),
                    }],
                });

                self.graph_shader_sources.push(pass.shader);
                self.graph_shaders.push(shader);
                self.graph_pipelines.push(pipeline);
                self.graph_uniform_buffers.push(buf);
                self.graph_uniform_bind_groups.push(bg);
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

            // Resolve the input source for this pass.
            let (input_view, input_sampler): (Option<&wgpu::TextureView>, Option<&wgpu::Sampler>) =
                match pass.input {
                    PassInput::EngineInput => (
                        input_texture.binding_view(),
                        input_texture.binding_sampler(),
                    ),
                    PassInput::PreviousPass if i > 0 => (
                        Some(&intermediate[i - 1].view),
                        Some(&intermediate[i - 1].sampler),
                    ),
                    PassInput::PreviousPass => {
                        // Warned at rebuild time; silently fall back to dummy.
                        (None, None)
                    }
                    PassInput::Feedback => (
                        feedback_texture.map(|f| &f.texture.view),
                        feedback_texture.map(|f| &f.texture.sampler),
                    ),
                };

            // Write per-pass uniforms into this pass's dedicated buffer.
            let uniforms = self.plugin.build_pass_uniforms(i, app_state, engine_state);
            queue.write_buffer(&self.graph_uniform_buffers[i], 0, bytemuck::bytes_of(&uniforms));

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

            let vb = self.mesh_vertex_buffer.as_ref().unwrap_or(vertex_buffer);
            render_pass.set_pipeline(&pipelines[i]);
            render_pass.set_vertex_buffer(0, vb.slice(..));
            render_pass.set_bind_group(0, &bind_group, &[]);
            render_pass.set_bind_group(1, &self.graph_uniform_bind_groups[i], &[]);

            if let Some(ref index_buf) = self.mesh_index_buffer {
                render_pass.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.mesh_index_count, 0, 0..1);
            } else if self.mesh_vertex_buffer.is_some() {
                // PointList mode — draw vertices directly.
                render_pass.draw(0..self.mesh_vertex_count, 0..1);
            } else {
                render_pass.draw(0..6, 0..1);
            }
        }
    }
}
