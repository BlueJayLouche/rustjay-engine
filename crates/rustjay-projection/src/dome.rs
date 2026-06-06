//! Dome projection stage — cubemap capture + equidistant azimuthal fisheye.

use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use wgpu::util::DeviceExt;

/// Domemaster resolution presets.
/// Domemaster resolution presets.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum DomemasterResolution {
    /// 1024×1024
    R1K,
    /// 2048×2048 (default)
    #[default]
    R2K,
    /// 4096×4096
    R4K,
}

impl DomemasterResolution {
    /// Pixel dimensions of the square output.
    pub fn pixels(self) -> u32 {
        match self {
            Self::R1K => 1024,
            Self::R2K => 2048,
            Self::R4K => 4096,
        }
    }
}

/// Configuration for the domemaster renderer.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DomemasterConfig {
    /// Output resolution (square)
    pub resolution: DomemasterResolution,
    /// Field of view in degrees (180 = full hemisphere)
    pub fov_degrees: f32,
    /// Content tilt in degrees (0 = zenith centered)
    pub tilt_degrees: f32,
}

impl Default for DomemasterConfig {
    fn default() -> Self {
        Self {
            resolution: DomemasterResolution::R2K,
            fov_degrees: 180.0,
            tilt_degrees: 0.0,
        }
    }
}

/// GPU uniform for the domemaster shader.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DomemasterParams {
    fov: f32,
    tilt: f32,
    content_az: f32,
    content_el: f32,
    content_roll: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

const NUM_FACES: usize = 5;
const FACE_FRONT: usize = 0;
const FACE_RIGHT: usize = 1;
const FACE_BACK: usize = 2;
const FACE_LEFT: usize = 3;
const FACE_TOP: usize = 4;

/// Dome projection stage: captures input into cubemap faces and projects to fisheye.
pub struct DomeStage {
    face_textures: Vec<wgpu::Texture>,
    face_views: Vec<wgpu::TextureView>,
    projection_pipeline: wgpu::RenderPipeline,
    projection_bind_group: wgpu::BindGroup,
    params_buffer: wgpu::Buffer,
    /// Current domemaster configuration.
    pub config: DomemasterConfig,
    face_size: u32,
    /// Content rotation in radians: [azimuth, elevation, roll].
    pub content_rotation: [f32; 3],
}

impl DomeStage {
    /// Create a new dome stage with the given configuration and target format.
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        config: DomemasterConfig,
    ) -> Self {
        let output_size = config.resolution.pixels();
        let face_size = output_size / 2;

        let create_texture = |label: &str, size: u32| -> (wgpu::Texture, wgpu::TextureView) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            (tex, view)
        };

        let face_labels = [
            "Dome Face Front",
            "Dome Face Right",
            "Dome Face Back",
            "Dome Face Left",
            "Dome Face Top",
        ];
        let mut face_textures = Vec::with_capacity(NUM_FACES);
        let mut face_views = Vec::with_capacity(NUM_FACES);
        for label in &face_labels {
            let (tex, view) = create_texture(label, face_size);
            face_textures.push(tex);
            face_views.push(view);
        }

        let tex_entry = |binding: u32| -> wgpu::BindGroupLayoutEntry {
            wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }
        };

        let projection_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Domemaster Projection BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    tex_entry(1),
                    tex_entry(2),
                    tex_entry(3),
                    tex_entry(4),
                    tex_entry(5),
                    wgpu::BindGroupLayoutEntry {
                        binding: 6,
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

        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Domemaster Vertex Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/domemaster.wgsl").into()),
        });
        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Domemaster Fragment Shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/domemaster.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Domemaster Projection Pipeline Layout"),
            bind_group_layouts: &[Some(&projection_bind_group_layout)],
            ..Default::default()
        });

        let projection_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Domemaster Projection Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Domemaster Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Domemaster Params Buffer"),
            contents: bytemuck::cast_slice(&[DomemasterParams {
                fov: config.fov_degrees.to_radians(),
                tilt: config.tilt_degrees.to_radians(),
                content_az: 0.0,
                content_el: 0.0,
                content_roll: 0.0,
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            }]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let projection_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Domemaster Projection Bind Group"),
            layout: &projection_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&face_views[FACE_FRONT]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&face_views[FACE_RIGHT]),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&face_views[FACE_BACK]),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&face_views[FACE_LEFT]),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&face_views[FACE_TOP]),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: params_buffer.as_entire_binding(),
                },
            ],
        });

        Self {
            face_textures,
            face_views,
            projection_pipeline,
            projection_bind_group,
            params_buffer,
            config,
            face_size,
            content_rotation: [0.0; 3],
        }
    }

    /// Write current config + content rotation to the GPU uniform buffer.
    pub fn update_params(&self, queue: &wgpu::Queue) {
        queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::cast_slice(&[DomemasterParams {
                fov: self.config.fov_degrees.to_radians(),
                tilt: self.config.tilt_degrees.to_radians(),
                content_az: self.content_rotation[0],
                content_el: self.content_rotation[1],
                content_roll: self.content_rotation[2],
                _pad0: 0.0,
                _pad1: 0.0,
                _pad2: 0.0,
            }]),
        );
    }

    /// Set content rotation (azimuth, elevation, roll) in radians.
    pub fn set_content_rotation(&mut self, az: f32, el: f32, roll: f32) {
        self.content_rotation = [az, el, roll];
    }

    /// Current output resolution in pixels (square).
    pub fn output_size(&self) -> u32 {
        self.config.resolution.pixels()
    }
}

impl ProjectionStage for DomeStage {
    fn label(&self) -> &str {
        "dome"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _input: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        self.update_params(ctx.queue);

        // Step 1: clear all faces to black, then copy source into front face.
        for view in &self.face_views {
            let _pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Dome Face Clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
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
        }

        if let Some(src_tex) = input_texture {
            ctx.encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: src_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &self.face_textures[FACE_FRONT],
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d {
                    width: self.face_size.min(src_tex.width()),
                    height: self.face_size.min(src_tex.height()),
                    depth_or_array_layers: 1,
                },
            );
        }

        // Step 2: projection pass
        {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Domemaster Projection Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: output,
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
            pass.set_pipeline(&self.projection_pipeline);
            pass.set_bind_group(0, &self.projection_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        // Face views are fixed; projection bind group is built once at init.
        // No invalidation needed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolution_pixels() {
        assert_eq!(DomemasterResolution::R1K.pixels(), 1024);
        assert_eq!(DomemasterResolution::R2K.pixels(), 2048);
        assert_eq!(DomemasterResolution::R4K.pixels(), 4096);
    }

    #[test]
    fn config_default() {
        let cfg = DomemasterConfig::default();
        assert_eq!(cfg.resolution, DomemasterResolution::R2K);
        assert!((cfg.fov_degrees - 180.0).abs() < f32::EPSILON);
    }

    #[test]
    fn params_alignment() {
        assert_eq!(std::mem::size_of::<DomemasterParams>(), 32);
    }

    #[test]
    fn dome_snapshot() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());

        // Create a white input texture (512×512 to match R1K face size).
        let face_size = DomemasterResolution::R1K.pixels() / 2;
        let (_input_tex, input_view) = crate::test_harness::create_solid_texture(
            &device,
            &queue,
            face_size,
            face_size,
            [255, 255, 255, 255],
        );

        let config = DomemasterConfig {
            resolution: DomemasterResolution::R1K,
            fov_degrees: 180.0,
            tilt_degrees: 0.0,
        };
        let output_size = config.resolution.pixels();
        let mut stage = DomeStage::new(&device, wgpu::TextureFormat::Rgba8Unorm, config);
        let (_output_tex, output_view) =
            crate::test_harness::create_output_texture(&device, output_size, output_size);

        crate::test_harness::run_stage(
            &device,
            &queue,
            &mut stage,
            &input_view,
            Some(&_input_tex),
            &output_view,
            [output_size, output_size],
        );

        let pixels = crate::test_harness::readback_rgba8(
            &device,
            &queue,
            &_output_tex,
            output_size,
            output_size,
        );

        // With 0° tilt the zenith maps to the top face; the front face appears
        // at the top edge of the fisheye image. Check a pixel that samples the
        // front-face center (top-middle of output).
        let front_idx = (output_size / 2) * 4;
        let front = &pixels[front_idx as usize..front_idx as usize + 4];
        assert!(
            front[0] > 200,
            "dome front-face sample should be bright, got {:?}",
            front
        );

        // Center pixel samples the top face (cleared black in this simplified
        // single-face projection).
        let center_idx = ((output_size / 2) * output_size + (output_size / 2)) * 4;
        let center = &pixels[center_idx as usize..center_idx as usize + 4];
        assert!(
            center[0] < 50,
            "dome center (top face) should be black, got {:?}",
            center
        );

        // Edge pixel (top-left corner) is outside the fisheye circle → black.
        let edge = &pixels[0..4];
        assert!(edge[0] < 50, "dome edge should be black, got {:?}", edge);
    }
}
