//! Edge blend post-process pipeline — smoothstep alpha ramps on output edges
//! for seamless multi-projector overlap blending.

use crate::stage::ProjectionStage;
use rustjay_core::RenderCtx;
use wgpu::util::DeviceExt;

/// Per-edge blend configuration.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct EdgeBlendEdge {
    /// Whether this edge has blending enabled.
    pub enabled: bool,
    /// Blend zone width as fraction of output dimension (0.0–0.5).
    pub width: f32,
    /// Gamma curve exponent for the blend ramp (typically 1.0–3.0).
    pub gamma: f32,
}

impl Default for EdgeBlendEdge {
    fn default() -> Self {
        Self {
            enabled: false,
            width: 0.1,
            gamma: 2.2,
        }
    }
}

/// Edge blending configuration for an output — four independent edges.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct EdgeBlendConfig {
    /// Left edge blend.
    pub left: EdgeBlendEdge,
    /// Right edge blend.
    pub right: EdgeBlendEdge,
    /// Top edge blend.
    pub top: EdgeBlendEdge,
    /// Bottom edge blend.
    pub bottom: EdgeBlendEdge,
}

impl EdgeBlendConfig {
    /// Returns true if any edge has blending enabled.
    pub fn any_enabled(&self) -> bool {
        self.left.enabled || self.right.enabled || self.top.enabled || self.bottom.enabled
    }
}

/// GPU-side uniform for the edge blend shader. 16 floats = 64 bytes.
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct EdgeBlendParams {
    left_enabled: f32,
    left_width: f32,
    left_gamma: f32,
    _pad0: f32,
    right_enabled: f32,
    right_width: f32,
    right_gamma: f32,
    _pad1: f32,
    top_enabled: f32,
    top_width: f32,
    top_gamma: f32,
    _pad2: f32,
    bottom_enabled: f32,
    bottom_width: f32,
    bottom_gamma: f32,
    _pad3: f32,
}

impl From<&EdgeBlendConfig> for EdgeBlendParams {
    fn from(cfg: &EdgeBlendConfig) -> Self {
        Self {
            left_enabled: if cfg.left.enabled { 1.0 } else { 0.0 },
            left_width: cfg.left.width.max(0.001),
            left_gamma: cfg.left.gamma,
            _pad0: 0.0,
            right_enabled: if cfg.right.enabled { 1.0 } else { 0.0 },
            right_width: cfg.right.width.max(0.001),
            right_gamma: cfg.right.gamma,
            _pad1: 0.0,
            top_enabled: if cfg.top.enabled { 1.0 } else { 0.0 },
            top_width: cfg.top.width.max(0.001),
            top_gamma: cfg.top.gamma,
            _pad2: 0.0,
            bottom_enabled: if cfg.bottom.enabled { 1.0 } else { 0.0 },
            bottom_width: cfg.bottom.width.max(0.001),
            bottom_gamma: cfg.bottom.gamma,
            _pad3: 0.0,
        }
    }
}

/// Full-screen post-process pipeline that applies edge blending.
pub struct EdgeBlendStage {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    params_buffer: wgpu::Buffer,
    /// Current edge-blend configuration.
    pub config: EdgeBlendConfig,
    cached_bind_group: Option<wgpu::BindGroup>,
    cached_input_ptr: Option<usize>,
}

impl EdgeBlendStage {
    /// Create a new edge-blend stage.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Edge Blend BGL"),
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
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

        let params_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Edge Blend Params"),
            contents: bytemuck::cast_slice(&[EdgeBlendParams::from(&EdgeBlendConfig::default())]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Edge Blend Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Edge Blend Shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("shaders/edge_blend.wgsl").into(),
            ),
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Edge Blend Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Edge Blend Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Self {
            pipeline,
            bind_group_layout,
            sampler,
            params_buffer,
            config: EdgeBlendConfig::default(),
            cached_bind_group: None,
            cached_input_ptr: None,
        }
    }
}

impl ProjectionStage for EdgeBlendStage {
    fn label(&self) -> &str {
        "edge-blend"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        ctx.queue.write_buffer(
            &self.params_buffer,
            0,
            bytemuck::cast_slice(&[EdgeBlendParams::from(&self.config)]),
        );

        let input_ptr = input as *const _ as usize;
        let bind_group = if self.cached_input_ptr == Some(input_ptr) {
            self.cached_bind_group.as_ref().unwrap()
        } else {
            let bg = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Edge Blend BG"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(input),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params_buffer.as_entire_binding(),
                    },
                ],
            });
            self.cached_bind_group = Some(bg);
            self.cached_input_ptr = Some(input_ptr);
            self.cached_bind_group.as_ref().unwrap()
        };

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Edge Blend Pass"),
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.draw(0..3, 0..1);
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        self.cached_bind_group = None;
        self.cached_input_ptr = None;
    }
}

/// Compute the smoothstep blend alpha for a given normalized position.
pub fn blend_alpha(t_normalized: f32, gamma: f32) -> f32 {
    let t = t_normalized.clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    s.powf(gamma)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_blend_config_default_none_enabled() {
        let cfg = EdgeBlendConfig::default();
        assert!(!cfg.any_enabled());
    }

    #[test]
    fn edge_blend_config_any_enabled() {
        let mut cfg = EdgeBlendConfig::default();
        cfg.left.enabled = true;
        assert!(cfg.any_enabled());
    }

    #[test]
    fn blend_alpha_endpoints() {
        assert!((blend_alpha(0.0, 2.2) - 0.0).abs() < 1e-6);
        assert!((blend_alpha(1.0, 2.2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blend_alpha_midpoint_gamma_one() {
        assert!((blend_alpha(0.5, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn blend_alpha_gamma_effect() {
        let a1 = blend_alpha(0.5, 1.0);
        let a2 = blend_alpha(0.5, 2.0);
        assert!(a2 < a1);
    }

    #[test]
    fn params_from_config() {
        let mut cfg = EdgeBlendConfig::default();
        cfg.left.enabled = true;
        cfg.left.width = 0.15;
        cfg.left.gamma = 1.8;
        let p = EdgeBlendParams::from(&cfg);
        assert_eq!(p.left_enabled, 1.0);
        assert_eq!(p.right_enabled, 0.0);
        assert!((p.left_width - 0.15).abs() < 1e-6);
    }

    #[test]
    fn edge_blend_left_ramp() {
        let (device, queue) = pollster::block_on(crate::test_harness::init_wgpu());
        let (_input_tex, input_view) = crate::test_harness::create_solid_texture(
            &device, &queue, 2, 2, [255, 255, 255, 255],
        );
        let (_output_tex, output_view) = crate::test_harness::create_output_texture(&device, 2, 2);

        let mut stage = EdgeBlendStage::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        stage.config.left = EdgeBlendEdge {
            enabled: true,
            width: 0.5,
            gamma: 1.0,
        };

        crate::test_harness::run_stage(
            &device, &queue, &mut stage,
            &input_view, Some(&_input_tex), &output_view, [2, 2],
        );

        let pixels = crate::test_harness::readback_rgba8(&device, &queue, &_output_tex, 2, 2);

        // Row-major 2×2 output:
        // Pixel (0,0): uv.x = 0.25 → t = 0.25/0.5 = 0.5 → smoothstep = 0.5 → 128
        // Pixel (1,0): uv.x = 0.75 → t = 0.75/0.5 = 1.5 → clamped = 1.0 → 255
        // Pixel (0,1): uv.x = 0.25 → 128
        // Pixel (1,1): uv.x = 0.75 → 255
        assert!((pixels[0] as i32 - 128).abs() <= 2, "left column should be ~128, got {}", pixels[0]);
        assert!((pixels[4] as i32 - 255).abs() <= 2, "right column should be ~255, got {}", pixels[4]);
        assert!((pixels[8] as i32 - 128).abs() <= 2, "left column should be ~128, got {}", pixels[8]);
        assert!((pixels[12] as i32 - 255).abs() <= 2, "right column should be ~255, got {}", pixels[12]);
    }
}
