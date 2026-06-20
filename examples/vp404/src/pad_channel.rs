//! Per-pad HAP-to-RGBA converter.
//!
//! Implements [`EffectInstance`] so each pad can be dropped into a
//! `rustjay_mixer::Channel`. The channel's opacity/blend become the pad's
//! mix controls; this effect only exposes the pad's playback `speed` as an
//! engine parameter (read by the plugin in `prepare()` and applied to the
//! [`Pad`] before advancement).

use std::sync::{Arc, Mutex};

use rustjay_core::{
    lfo::BEAT_DIVISION_NAMES, EffectInput, EffectInstance, EngineState, ParamCategory,
    ParameterDescriptor, RenderCtx, RenderTarget,
};

use crate::bank::Bank;
use crate::pad::PlaybackMode;
use crate::sample::ColorSpace;

/// GPU uniform data for `hap_convert.wgsl`.
///
/// Keep byte-for-byte identical to the WGSL struct (8 x f32).
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvertParams {
    opacity: f32,
    do_ycocg: f32,
    has_alpha_plane: f32,
    _pad: f32,
    uv_scale: [f32; 2],
    uv_offset: [f32; 2],
}

/// Immutable GPU resources for the convert pass.
///
/// Built once and shared across every [`PadChannel`] — the pipeline, bind-group
/// layout, sampler and dummy alpha texture are identical for all pads.
pub(crate) struct ConvertGpuShared {
    pipeline: wgpu::RenderPipeline,
    bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// binding 3 must always be bound even when `has_alpha_plane == 0`.
    dummy_alpha: wgpu::TextureView,
}

impl ConvertGpuShared {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vp404 hap_convert"),
            source: wgpu::ShaderSource::Wgsl(include_str!("hap_convert.wgsl").into()),
        });

        let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("vp404 convert bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex_entry(1),
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
                tex_entry(3),
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("vp404 convert layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("vp404 convert pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: rustjay_core::working_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("vp404 sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let dummy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("vp404 dummy alpha"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let dummy_alpha = dummy.create_view(&Default::default());

        Self {
            pipeline,
            bgl,
            sampler,
            dummy_alpha,
        }
    }
}

/// Renders one pad's current HAP frame through the YCoCg/alpha convert pass
/// into the channel texture owned by `rustjay-mixer`.
pub struct PadChannel {
    bank: Arc<Mutex<Bank>>,
    pad_index: usize,
    shared: Option<Arc<ConvertGpuShared>>,
    /// Per-pad params buffer. Each pad writes different `do_ycocg` / `uv_scale`
    /// right before its draw, and `queue.write_buffer` for one submit is applied
    /// before any draw runs — so a shared buffer would make every pad render
    /// with the last pad's params. This MUST stay per-pad.
    params: Option<wgpu::Buffer>,
}

impl PadChannel {
    pub fn new(bank: Arc<Mutex<Bank>>, pad_index: usize) -> Self {
        Self {
            bank,
            pad_index,
            shared: None,
            params: None,
        }
    }

    pub fn set_shared_gpu(&mut self, shared: Arc<ConvertGpuShared>) {
        self.shared = Some(shared);
    }
}

impl EffectInstance for PadChannel {
    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }

    fn label(&self) -> &str {
        "pad-channel"
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float(
                "speed",
                "Speed",
                ParamCategory::Custom("Pad".to_string()),
                -5.0,
                5.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::enum_param(
                "mode",
                "Mode",
                ParamCategory::Custom("Pad".to_string()),
                PlaybackMode::labels()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                PlaybackMode::Free.to_index(),
            ),
            ParameterDescriptor::enum_param(
                "division",
                "Beat Division",
                ParamCategory::Custom("Pad".to_string()),
                BEAT_DIVISION_NAMES.iter().map(|s| s.to_string()).collect(),
                2, // 1/4 note
            ),
        ]
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        let shared = self
            .shared
            .as_ref()
            .expect("PadChannel shared GPU not initialized");
        let params = self.params.get_or_insert_with(|| {
            ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("vp404 convert params"),
                size: std::mem::size_of::<ConvertParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let mut bank = self.bank.lock().unwrap_or_else(|e| e.into_inner());
        let pad = bank.pads.get_mut(self.pad_index);

        // The mixer elides zero-opacity channels, so this is only called when
        // the pad is actually playing. Still guard defensively.
        let playing = pad.as_ref().is_some_and(|p| p.is_playing && p.has_sample());
        if !playing {
            return;
        }

        let pad = pad.unwrap();
        let frame = pad.current_frame as u32;
        let Some(sample) = pad.sample.as_mut() else {
            return;
        };
        // Read metadata before decoding because `frame_texture` borrows `sample`
        // mutably for its cache.
        let do_ycocg = matches!(sample.color_space(), ColorSpace::YcoCg);
        let uv_scale = sample.uv_scale();
        let Some(tex) = sample.frame_texture(ctx.device, ctx.queue, frame) else {
            return;
        };

        let uniforms = ConvertParams {
            opacity: 1.0,
            do_ycocg: if do_ycocg { 1.0 } else { 0.0 },
            has_alpha_plane: 0.0,
            _pad: 0.0,
            uv_scale,
            uv_offset: [0.0, 0.0],
        };
        ctx.queue
            .write_buffer(params, 0, bytemuck::bytes_of(&uniforms));

        let bind_group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("vp404 convert bg"),
            layout: &shared.bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(&shared.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&tex.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shared.dummy_alpha),
                },
            ],
        });

        let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("vp404 hap convert"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target.view,
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
        pass.set_pipeline(&shared.pipeline);
        pass.set_bind_group(0, &bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}
