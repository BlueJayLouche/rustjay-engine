//! HAP video source — GPU-native HAP playback via `hap-wgpu`.
//!
//! Wraps `HapPlayer` to decode HAP QuickTime files and render frames as
//! BC-compressed textures. Playback parameters (speed, loop, play/pause,
//! position) are exposed through the engine param system for modulation
//! and GUI control.
//!
//! # Known limitations
//! - YCoCg DXt5 (`HapY`) renders as raw BC3 data without YCoCg→RGB conversion.
//!   HAP1 (DXT1 / RGB) and HAP5 (DXT5 / RGBA) decode correctly.
//! - Decoding happens synchronously on the render thread inside `prepare()`.
//!   High-resolution files may cause frame drops.

use rustjay_core::{
    EffectInput, EffectInstance, EngineState, ParamCategory, ParameterDescriptor, RenderCtx,
    RenderTarget,
};
use std::path::Path;
use std::sync::Arc;

/// Renders HAP video frames to the target.
pub struct HapSource {
    player: hap_wgpu::HapPlayer,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    current_frame: Option<Arc<hap_wgpu::HapTexture>>,
    param_prefix: String,
    speed_key: String,
    playing_key: String,
    loop_key: String,
    position_key: String,
    last_speed: f32,
    last_playing: bool,
    last_loop: i32,
    last_position: f32,
    /// Forces a one-time sync of all playback params on the first prepare().
    needs_sync: bool,
}

impl HapSource {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, path: &Path) -> anyhow::Result<Self> {
        let player =
            hap_wgpu::HapPlayer::open(path, Arc::new(device.clone()), Arc::new(queue.clone()))?;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("HapSource Shader"),
            source: wgpu::ShaderSource::Wgsl(
                r#"
                struct VertexOutput {
                    @builtin(position) position: vec4<f32>,
                    @location(0) texcoord: vec2<f32>,
                };

                @group(0) @binding(0) var tex: texture_2d<f32>;
                @group(0) @binding(1) var sam: sampler;

                @vertex
                fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
                    var out: VertexOutput;
                    out.position = vec4<f32>(position, 0.0, 1.0);
                    out.texcoord = texcoord;
                    return out;
                }

                @fragment
                fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                    return textureSample(tex, sam, in.texcoord);
                }
                "#
                .into(),
            ),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("HapSource BGL"),
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

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("HapSource Pipeline Layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            ..Default::default()
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("HapSource Pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[rustjay_core::Vertex::desc()],
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
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            player,
            pipeline,
            bind_group_layout,
            bind_group: None,
            sampler,
            current_frame: None,
            param_prefix: String::new(),
            speed_key: String::new(),
            playing_key: String::new(),
            loop_key: String::new(),
            position_key: String::new(),
            last_speed: 1.0,
            last_playing: true,
            last_loop: 1,
            last_position: 0.0,
            needs_sync: true,
        })
    }

    fn recompute_keys(&mut self) {
        let p = &self.param_prefix;
        self.speed_key = format!("{p}speed");
        self.playing_key = format!("{p}playing");
        self.loop_key = format!("{p}loop");
        self.position_key = format!("{p}position");
    }
}

impl EffectInstance for HapSource {
    fn set_param_prefix(&mut self, prefix: &str) {
        self.param_prefix = prefix.to_string();
        self.recompute_keys();
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        // Return bare names — the enclosing DeckCompositor and Mixer apply the
        // canonical prefix (ch_<uuid>_deck_<uuid>_).  This avoids double-prefixing
        // when set_full_prefix() has already been called on the deck.
        vec![
            ParameterDescriptor::float(
                "speed".to_string(),
                "Speed",
                ParamCategory::Custom("Playback".to_string()),
                -5.0,
                5.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::bool(
                "playing".to_string(),
                "Playing",
                ParamCategory::Custom("Playback".to_string()),
                true,
            ),
            ParameterDescriptor::enum_param(
                "loop".to_string(),
                "Loop Mode",
                ParamCategory::Custom("Playback".to_string()),
                vec![
                    "None".to_string(),
                    "Loop".to_string(),
                    "Palindrome".to_string(),
                ],
                1,
            ),
            ParameterDescriptor::float(
                "position".to_string(),
                "Position",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                1.0,
                0.0,
                0.001,
            ),
        ]
    }

    fn prepare(&mut self, engine: &EngineState, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        // Pull next frame from the player.
        self.current_frame = self.player.update();

        // Sync playback params.
        let speed = engine.get_param(&self.speed_key).unwrap_or(1.0);
        if self.needs_sync || (speed - self.last_speed).abs() > f32::EPSILON {
            self.player.set_speed(speed);
            self.last_speed = speed;
        }

        let playing = engine.get_param(&self.playing_key).unwrap_or(1.0) > 0.5;
        if self.needs_sync || playing != self.last_playing {
            if playing {
                self.player.play();
            } else {
                self.player.pause();
            }
            self.last_playing = playing;
        }

        let loop_raw = engine.get_param(&self.loop_key).unwrap_or(1.0) as i32;
        if self.needs_sync || loop_raw != self.last_loop {
            let mode = match loop_raw {
                0 => hap_wgpu::LoopMode::None,
                2 => hap_wgpu::LoopMode::Palindrome,
                _ => hap_wgpu::LoopMode::Loop,
            };
            self.player.set_loop_mode(mode);
            self.last_loop = loop_raw;
        }

        let position = engine.get_param(&self.position_key).unwrap_or(0.0);
        if self.needs_sync || (position - self.last_position).abs() > 0.001 {
            let frame_count = self.player.frame_count().max(1);
            let target_frame = (position * (frame_count - 1) as f32) as u32;
            self.player.seek_to_frame(target_frame);
            self.last_position = position;
        }

        self.needs_sync = false;
    }

    fn render_to(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _inputs: &[EffectInput<'_>],
        target: RenderTarget<'_>,
        _engine: &EngineState,
    ) {
        if let Some(ref frame) = self.current_frame {
            // Rebuild bind group when the frame changes.
            self.bind_group = Some(ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("HapSource BG"),
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&frame.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            }));
        }

        if let Some(ref bind_group) = self.bind_group {
            let mut pass = ctx.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("HapSource Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target.view,
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
            pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
            pass.set_bind_group(0, bind_group, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}
