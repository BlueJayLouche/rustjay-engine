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
use wgpu::util::DeviceExt;

/// Renders HAP video frames to the target.
/// The blit: crop the block padding, and decode YCoCg when the clip is Hap Q.
const SHADER: &str = r#"
                struct VertexOutput {
                    @builtin(position) position: vec4<f32>,
                    @location(0) texcoord: vec2<f32>,
                };

                @group(0) @binding(0) var tex: texture_2d<f32>;
                @group(0) @binding(1) var sam: sampler;
                struct Convert {
                    // 1.0 for HapY (Hap Q), whose DXT5 carries scaled YCoCg.
                    do_ycocg: f32,
                    _pad: f32,
                    // The real image inside the block-padded texture.
                    uv_scale: vec2<f32>,
                };
                @group(0) @binding(2) var<uniform> conv: Convert;

                // Hap Q packs Co/Cg with a per-pixel scale in blue and luma in
                // alpha. Sampled raw it reads as the green/blue mess that gave
                // this away.
                fn ycocg_to_rgb(c: vec4<f32>) -> vec3<f32> {
                    let scale = (c.b * (255.0 / 8.0)) + 1.0;
                    let co = (c.r - (0.5 * 256.0 / 255.0)) / scale;
                    let cg = (c.g - (0.5 * 256.0 / 255.0)) / scale;
                    let y = c.a;
                    return vec3<f32>(y + co - cg, y + cg, y - co - cg);
                }

                @vertex
                fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
                    var out: VertexOutput;
                    out.position = vec4<f32>(position, 0.0, 1.0);
                    out.texcoord = texcoord;
                    return out;
                }

                @fragment
                fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
                    let c = textureSample(tex, sam, in.texcoord * conv.uv_scale);
                    if (conv.do_ycocg > 0.5) {
                        return vec4<f32>(ycocg_to_rgb(c), 1.0);
                    }
                    return c;
                }
                "#;

/// Convert-pass constants: which colour space the frames are in, and where the
/// real image sits inside the block-padded texture.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct Convert {
    do_ycocg: f32,
    _pad: f32,
    uv_scale: [f32; 2],
}

pub struct HapSource {
    player: hap_wgpu::HapPlayer,
    /// Colour space and padding crop, written once at open.
    convert: wgpu::Buffer,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: Option<wgpu::BindGroup>,
    sampler: wgpu::Sampler,
    current_frame: Option<Arc<hap_wgpu::HapTexture>>,
    param_prefix: String,
    speed_key: String,
    playing_key: String,
    loop_key: String,
    sync_key: String,
    mode_key: String,
    div_key: String,
    position_key: String,
    in_point_key: String,
    out_point_key: String,
    /// Our own copy of the playhead, in frames.
    ///
    /// `hap-wgpu` has no playhead getter and `seek_to_frame` clears its frame
    /// cache, so trimming by seeking every frame would re-decode every frame.
    /// Instead the advance is mirrored here and a seek only happens at the
    /// out point, where both clocks are reset together.
    frame: f32,
    last_tick: Option<std::time::Instant>,
    last_speed: f32,
    last_playing: bool,
    last_loop: i32,
    /// Whether the clip is on screen — not playing means black, in every mode.
    visible: bool,
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
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
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
                buffers: &[Some(rustjay_core::Vertex::desc())],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: rustjay_core::working_format(),
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

        // Hap Q (`HapY`) stores scaled YCoCg in a DXT5 block; everything else
        // is already RGB(A). The frames are padded up to whole 4-pixel blocks,
        // so the real image is a fraction of the texture.
        let (w, h) = player.dimensions();
        let (pw, ph) = player.padded_dimensions();
        let convert = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("HapSource Convert"),
            contents: bytemuck::bytes_of(&Convert {
                do_ycocg: f32::from(u8::from(player.codec_type() == "HapY")),
                _pad: 0.0,
                uv_scale: [
                    w as f32 / pw.max(1) as f32,
                    h as f32 / ph.max(1) as f32,
                ],
            }),
            usage: wgpu::BufferUsages::UNIFORM,
        });

        Ok(Self {
            convert,
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
            sync_key: String::new(),
            mode_key: String::new(),
            div_key: String::new(),
            position_key: String::new(),
            in_point_key: String::new(),
            out_point_key: String::new(),
            frame: 0.0,
            last_tick: None,
            last_speed: 1.0,
            last_playing: true,
            last_loop: 1,
            visible: true,
            last_position: 0.0,
            needs_sync: true,
        })
    }

    fn recompute_keys(&mut self) {
        let p = &self.param_prefix;
        self.speed_key = format!("{p}speed");
        self.playing_key = format!("{p}playing");
        self.loop_key = format!("{p}loop");
        self.sync_key = format!("{p}sync");
        self.mode_key = format!("{p}mode");
        self.div_key = format!("{p}div");
        self.position_key = format!("{p}position");
        self.in_point_key = format!("{p}in_point");
        self.out_point_key = format!("{p}out_point");
    }
}


impl HapSource {
    /// Move both playheads — the player's and our mirror of it — together.
    fn seek(&mut self, frame: f32) {
        self.frame = frame.max(0.0);
        self.player.seek_to_frame(self.frame as u32);
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
        let mut params = vec![
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
            ParameterDescriptor::float(
                "in_point".to_string(),
                "In Point",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                1.0,
                0.0,
                0.001,
            ),
            ParameterDescriptor::float(
                "out_point".to_string(),
                "Out Point",
                ParamCategory::Custom("Playback".to_string()),
                0.0,
                1.0,
                1.0,
                0.001,
            ),
        ];
        params.extend(super::sync_parameters());
        params
    }

    fn prepare(&mut self, engine: &EngineState, _device: &wgpu::Device, _queue: &wgpu::Queue) {
        // Pull next frame from the player.
        self.current_frame = self.player.update();

        // Sync playback params.
        // Sync stretches the in/out span to the chosen beat division, so a
        // clip loops on the bar whatever its length; Speed trims from there.
        let last_frame = (self.player.frame_count().max(1) - 1) as f32;
        let in_point = engine.get_param(&self.in_point_key).unwrap_or(0.0).clamp(0.0, 1.0);
        let out_point = engine.get_param(&self.out_point_key).unwrap_or(1.0).clamp(0.0, 1.0);
        let in_frame = in_point * last_frame;
        // At least one frame of clip, however the two points are dragged.
        let out_frame = (out_point * last_frame).max(in_frame + 1.0).min(last_frame);
        let trimmed = in_point > 0.0 || out_point < 1.0;
        let span = self.player.duration() * (out_point - in_point).max(0.0) as f64;
        let mode = super::TriggerMode::from_index(
            engine.get_param(&self.mode_key).unwrap_or(0.0).round() as i32,
        );
        let speed = super::clip_speed(
            engine.get_param(&self.speed_key).unwrap_or(1.0),
            engine.get_param(&self.sync_key).unwrap_or(0.0) >= 0.5,
            engine.get_param(&self.div_key).unwrap_or(4.0) as usize,
            engine.effective_bpm(),
            span,
        );
        if self.needs_sync || (speed - self.last_speed).abs() > f32::EPSILON {
            self.player.set_speed(speed);
            self.last_speed = speed;
        }

        let playing = engine.get_param(&self.playing_key).unwrap_or(1.0) > 0.5;
        if self.needs_sync || playing != self.last_playing {
            if playing {
                if mode.rewinds_on_press() {
                    self.seek(in_frame);
                }
                self.player.play();
            } else if mode.stops_on_release() {
                self.player.pause();
                if mode.rewinds_on_release() {
                    self.seek(in_frame);
                }
            }
            self.last_playing = playing;
        }
        // A one-shot owns its own end: it plays on after the button is
        // released and goes black when it runs out. Everything else is black
        // the moment it is not playing.
        self.visible = if mode == super::TriggerMode::OneShot {
            self.player.is_playing()
        } else {
            playing
        };

        let loop_raw = mode
            .loop_override()
            .unwrap_or_else(|| engine.get_param(&self.loop_key).unwrap_or(1.0) as i32);
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
            self.seek(position * last_frame);
            self.last_position = position;
        }

        // Mirror the player's advance, then enforce the trim.
        let now = std::time::Instant::now();
        let dt = self.last_tick.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0);
        self.last_tick = Some(now);
        if self.player.is_playing() {
            self.frame += dt * self.player.fps() * speed;
        }
        if trimmed && self.player.is_playing() && (self.frame >= out_frame || self.frame < in_frame)
        {
            // Palindrome is not honoured inside a trim: the wrap goes back to
            // the in point either way.
            if loop_raw == 0 {
                self.player.pause();
                self.visible = false;
            } else {
                self.seek(in_frame);
            }
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
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.convert,
                            offset: 0,
                            size: None,
                        }),
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
            // Stopped means black: the pass has already cleared, so the
            // draw is simply skipped.
            if self.visible {
                pass.set_pipeline(&self.pipeline);
                pass.set_vertex_buffer(0, ctx.vertex_buffer.slice(..));
                pass.set_bind_group(0, bind_group, &[]);
                pass.draw(0..6, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// Same reason as the text blit's: a shader that will not parse only shows
    /// up as a panic the first time a clip of this kind is opened.
    #[test]
    fn the_blit_shader_parses() {
        wgpu::naga::front::wgsl::parse_str(super::SHADER).expect("hap blit must be valid WGSL");
    }
}
