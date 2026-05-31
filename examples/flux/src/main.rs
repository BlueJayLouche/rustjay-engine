//! Flux — optical-flow warp with motion feedback trails.
//!
//! Per-pixel Lucas-Kanade flow is estimated in a half-resolution pass from
//! consecutive webcam frames.  The flow field displaces the current webcam
//! UV (warp) and nudges the accumulated feedback buffer (drift), producing
//! motion-following trails and smear effects.  An optional HSV overlay
//! colour-maps flow direction (hue) and speed (brightness).
//!
//! Render sequence each frame:
//!   1. Flow pass — input + prev_frame + prev_flow → flow_tex (encoded)
//!   2. Warp pass — input + flow_tex + accum[read] → accum[write]
//!   3. Blit pass — accum[write] → render_target_view
//!   4. GPU copy  — current input → prev_frame (for next frame's It)
//!   5. Ping-pong flow / accum indices

use rustjay_engine::prelude::*;
use wgpu::util::DeviceExt;

#[cfg(feature = "gles2")]
mod gles2_renderer;

// ---------------------------------------------------------------------------
// Uniforms — must match both WGSL structs exactly (16-byte aligned vec4s)
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct FluxUniforms {
    // vec4  0
    flow_lambda:    f32,
    flow_smooth:    f32,
    flow_scale:     f32,
    _pad0:          f32,
    // vec4  1
    warp_strength:  f32,
    drift_strength: f32,
    feedback_decay: f32,
    webcam_mix:     f32,
    // vec4  2
    flow_viz:       f32,
    flow_viz_scale: f32,
    _pad1:          f32,
    _pad2:          f32,
    // vec4  3
    audio_level:    f32,
    bass:           f32,
    mid:            f32,
    treble:         f32,
}

// ---------------------------------------------------------------------------
// App state (serialised to disk)
// ---------------------------------------------------------------------------

#[derive(serde::Serialize, serde::Deserialize)]
struct FluxState {
    /// Regularisation constant — higher = smoother but less sensitive (0.001–0.1)
    flow_lambda:    f32,
    /// Temporal IIR blend (0 = no smoothing, 0.95 = very smooth)
    flow_smooth:    f32,
    /// Overall flow magnitude multiplier
    flow_scale:     f32,
    /// How far the webcam UV is displaced by the flow (0–2)
    warp_strength:  f32,
    /// How much the feedback buffer drifts with the flow (0–0.5)
    drift_strength: f32,
    /// Per-frame feedback multiplier (0.8–0.999)
    feedback_decay: f32,
    /// How much of the raw warped webcam bleeds in each frame (0–1)
    webcam_mix:     f32,
    /// Amount of flow vector overlay (0 = off, 1 = full)
    flow_viz:       f32,
    /// Scale for mapping flow magnitude to overlay brightness
    flow_viz_scale: f32,
    /// Enable audio modulation of warp + decay
    audio_reactive: bool,
    /// Video standard: 0=PAL, 1=NTSC
    video_standard: u32,
}

impl Default for FluxState {
    fn default() -> Self {
        Self {
            flow_lambda:    0.005,
            flow_smooth:    0.7,
            flow_scale:     1.5,
            warp_strength:  0.6,
            drift_strength: 0.15,
            feedback_decay: 0.93,
            webcam_mix:     0.25,
            flow_viz:       0.0,
            flow_viz_scale: 5.0,
            audio_reactive: true,
            video_standard: 0, // PAL default
        }
    }
}

// ---------------------------------------------------------------------------
// GPU texture pair used for ping-pong
// ---------------------------------------------------------------------------

struct PingPong {
    textures: [Texture; 2],
    read:     usize,
}

impl PingPong {
    fn new(device: &wgpu::Device, w: u32, h: u32, label: &str) -> Self {
        Self {
            textures: [
                Texture::create_render_target(device, w, h, &format!("{label}_A")),
                Texture::create_render_target(device, w, h, &format!("{label}_B")),
            ],
            read: 0,
        }
    }

    fn read(&self)  -> &Texture { &self.textures[self.read] }
    fn write(&self) -> &Texture { &self.textures[1 - self.read] }
    fn swap(&mut self)          { self.read = 1 - self.read; }

    fn resize(&mut self, device: &wgpu::Device, w: u32, h: u32, label: &str) {
        if self.textures[0].width != w || self.textures[0].height != h {
            *self = Self::new(device, w, h, label);
        }
    }
}

// ---------------------------------------------------------------------------
// Effect plugin
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FluxEffect {
    // pipelines
    flow_pipeline: Option<wgpu::RenderPipeline>,
    warp_pipeline: Option<wgpu::RenderPipeline>,
    blit_pipeline: Option<wgpu::RenderPipeline>,

    // bind group layouts
    flow_bgl:    Option<wgpu::BindGroupLayout>, // curr + prev + prev_flow + sampler
    warp_bgl:    Option<wgpu::BindGroupLayout>, // input + flow + accum + sampler
    blit_bgl:    Option<wgpu::BindGroupLayout>, // src + sampler
    uniform_bgl: Option<wgpu::BindGroupLayout>,

    // per-frame GPU resources (created/resized on first frame)
    prev_frame: Option<Texture>,  // previous webcam frame
    flow:       Option<PingPong>, // flow ping-pong (curr + smoothed)
    accum:      Option<PingPong>, // accumulated output ping-pong

    // shared resources
    vertex_buffer:  Option<wgpu::Buffer>,
    uniform_buffer: Option<wgpu::Buffer>,
    uniform_bg:     Option<wgpu::BindGroup>,
}

// Stub shader — engine needs something to compile; render() returns true.
const STUB_SHADER: &str = r#"
@vertex fn vs_main(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(p, 0.0, 1.0);
}
@fragment fn fs_main() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }
"#;

impl FluxEffect {
    fn build_uniform_bgl(device: &wgpu::Device) -> wgpu::BindGroupLayout {
        device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Flux Uniform BGL"),
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

    fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
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
    }

    fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
        wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        }
    }

    fn make_pipeline(
        device: &wgpu::Device,
        shader_src: &str,
        label: &str,
        bgls: &[Option<&wgpu::BindGroupLayout>],
    ) -> wgpu::RenderPipeline {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(label),
            source: wgpu::ShaderSource::Wgsl(shader_src.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(label),
            bind_group_layouts: bgls,
            immediate_size: 0,
        });
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        })
    }

    fn fullscreen_pass<'a>(
        encoder: &'a mut wgpu::CommandEncoder,
        target: &'a wgpu::TextureView,
        label: &str,
    ) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(label),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load:  wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes:         None,
            occlusion_query_set:      None,
            multiview_mask:           None,
        })
    }

    /// Ensure all GPU textures match the current input size.
    fn resize_if_needed(&mut self, device: &wgpu::Device, w: u32, h: u32) {
        let needs = self.prev_frame.as_ref().map_or(true, |t| t.width != w || t.height != h);
        if needs {
            self.prev_frame = Some(Texture::create_render_target(device, w, h, "Flux Prev Frame"));
        }
        if let Some(f) = &mut self.flow {
            f.resize(device, w, h, "Flux Flow");
        } else {
            self.flow = Some(PingPong::new(device, w, h, "Flux Flow"));
        }
        if let Some(a) = &mut self.accum {
            a.resize(device, w, h, "Flux Accum");
        } else {
            self.accum = Some(PingPong::new(device, w, h, "Flux Accum"));
        }
    }
}

impl EffectPlugin for FluxEffect {
    type State    = FluxState;
    type Uniforms = FluxUniforms;

    fn app_name(&self)        -> &str        { "flux" }
    fn shader_source(&self)  -> &'static str { STUB_SHADER }
    fn default_state(&self)  -> FluxState    { FluxState::default() }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor::float("flow_scale",     "Flow Scale",     ParamCategory::Motion, 0.1, 5.0, 1.5, 0.05),
            ParameterDescriptor::float("flow_smooth",    "Flow Smooth",    ParamCategory::Motion, 0.0, 0.95, 0.7, 0.01),
            ParameterDescriptor::float("flow_lambda",    "Reg. Lambda",    ParamCategory::Motion, 0.001, 0.1, 0.005, 0.001),
            ParameterDescriptor::float("warp_strength",  "Warp",           ParamCategory::Motion, 0.0, 2.0, 0.6, 0.01),
            ParameterDescriptor::float("drift_strength", "Drift",          ParamCategory::Motion, 0.0, 0.5, 0.15, 0.005),
            ParameterDescriptor::float("feedback_decay", "Feedback",       ParamCategory::Motion, 0.8, 0.999, 0.93, 0.001),
            ParameterDescriptor::float("webcam_mix",     "Webcam Mix",     ParamCategory::Motion, 0.0, 1.0, 0.25, 0.01),
            ParameterDescriptor::float("flow_viz",       "Flow Viz",       ParamCategory::Color,  0.0, 1.0, 0.0, 0.01),
            ParameterDescriptor::float("flow_viz_scale", "Viz Scale",      ParamCategory::Color,  0.5, 20.0, 5.0, 0.1),
            ParameterDescriptor::bool("audio_reactive",  "Audio Reactive", ParamCategory::Motion, true),
            ParameterDescriptor::enum_param("video_standard", "Video Standard", ParamCategory::Settings, vec!["PAL".into(), "NTSC".into()], 0),
        ]
    }

    fn hidden_tabs(&self) -> Vec<GuiTab> { vec![] }

    // -----------------------------------------------------------------------
    // init — create pipelines and shared buffers
    // -----------------------------------------------------------------------
    fn init(&mut self, device: &wgpu::Device, _queue: &wgpu::Queue) {
        let uniform_bgl = Self::build_uniform_bgl(device);

        // Flow BGL: curr(0) + prev(1) + prev_flow(2) + sampler(3)
        let flow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Flux Flow BGL"),
            entries: &[
                Self::tex_entry(0),
                Self::tex_entry(1),
                Self::tex_entry(2),
                Self::sampler_entry(3),
            ],
        });

        // Warp BGL: input(0) + flow(1) + accum(2) + sampler(3)
        let warp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Flux Warp BGL"),
            entries: &[
                Self::tex_entry(0),
                Self::tex_entry(1),
                Self::tex_entry(2),
                Self::sampler_entry(3),
            ],
        });

        // Blit BGL: src(0) + sampler(1)
        let blit_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Flux Blit BGL"),
            entries: &[Self::tex_entry(0), Self::sampler_entry(1)],
        });

        let flow_pipeline = Self::make_pipeline(
            device,
            include_str!("shaders/flux_flow.wgsl"),
            "Flux Flow Pipeline",
            &[Some(&flow_bgl), Some(&uniform_bgl)],
        );
        let warp_pipeline = Self::make_pipeline(
            device,
            include_str!("shaders/flux_warp.wgsl"),
            "Flux Warp Pipeline",
            &[Some(&warp_bgl), Some(&uniform_bgl)],
        );
        let blit_pipeline = Self::make_pipeline(
            device,
            include_str!("shaders/flux_blit.wgsl"),
            "Flux Blit Pipeline",
            &[Some(&blit_bgl)],
        );

        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label:    Some("Flux Vertex Buffer"),
            contents: bytemuck::cast_slice(&Vertex::quad_vertices()),
            usage:    wgpu::BufferUsages::VERTEX,
        });

        let ub = device.create_buffer(&wgpu::BufferDescriptor {
            label:              Some("Flux Uniform Buffer"),
            size:               std::mem::size_of::<FluxUniforms>() as u64,
            usage:              wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let ubg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("Flux Uniform BG"),
            layout:  &uniform_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() }],
        });

        self.flow_pipeline = Some(flow_pipeline);
        self.warp_pipeline = Some(warp_pipeline);
        self.blit_pipeline = Some(blit_pipeline);
        self.flow_bgl      = Some(flow_bgl);
        self.warp_bgl      = Some(warp_bgl);
        self.blit_bgl      = Some(blit_bgl);
        self.vertex_buffer = Some(vb);
        self.uniform_buffer = Some(ub);
        self.uniform_bg     = Some(ubg);
    }

    // -----------------------------------------------------------------------
    // render — 3 passes + 1 GPU copy each frame
    // -----------------------------------------------------------------------
    fn render(
        &mut self,
        encoder:           &mut wgpu::CommandEncoder,
        device:            &wgpu::Device,
        queue:             &wgpu::Queue,
        input_view:         Option<&wgpu::TextureView>,
        input_sampler:      Option<&wgpu::Sampler>,
        render_target_view: &wgpu::TextureView,
        app_state:         &mut Self::State,
        engine_state:      &EngineState,
        _vertex_buffer:    &wgpu::Buffer,
        input_texture:      Option<&wgpu::Texture>,
    ) -> bool {
        let Some(input_tex) = input_texture  else { return false };
        let Some(input_view) = input_view    else { return false };
        let Some(input_samp) = input_sampler else { return false };

        // Resize textures before borrowing pipelines/buffers from self
        let w = input_tex.width();
        let h = input_tex.height();
        self.resize_if_needed(device, w, h);

        let Some(vb)      = &self.vertex_buffer  else { return false };
        let Some(ub)      = &self.uniform_buffer else { return false };
        let Some(ubg)     = &self.uniform_bg     else { return false };
        let Some(flow_pl) = &self.flow_pipeline  else { return false };
        let Some(warp_pl) = &self.warp_pipeline  else { return false };
        let Some(blit_pl) = &self.blit_pipeline  else { return false };
        let Some(flow_bgl) = &self.flow_bgl      else { return false };
        let Some(warp_bgl) = &self.warp_bgl      else { return false };
        let Some(blit_bgl) = &self.blit_bgl      else { return false };
        let prev_frame = self.prev_frame.as_ref().unwrap();
        let flow       = self.flow.as_ref().unwrap();
        let accum      = self.accum.as_ref().unwrap();

        // Upload uniforms (build before holding any further borrows on self)
        let uniforms = FluxEffect::build_uniforms_static(app_state, engine_state);
        queue.write_buffer(ub, 0, bytemuck::bytes_of(&uniforms));

        // ------------------------------------------------------------------
        // 1. Flow pass → flow.write()
        // ------------------------------------------------------------------
        let flow_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("Flux Flow BG"),
            layout:  flow_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&prev_frame.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&flow.read().view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(input_samp) },
            ],
        });
        {
            let mut pass = Self::fullscreen_pass(encoder, &flow.write().view, "Flux Flow Pass");
            pass.set_pipeline(flow_pl);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &flow_bg, &[]);
            pass.set_bind_group(1, ubg, &[]);
            pass.draw(0..6, 0..1);
        }

        // ------------------------------------------------------------------
        // 3. Warp pass → accum.write()
        // ------------------------------------------------------------------
        let warp_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("Flux Warp BG"),
            layout:  warp_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&flow.write().view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&accum.read().view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(input_samp) },
            ],
        });
        {
            let mut pass = Self::fullscreen_pass(encoder, &accum.write().view, "Flux Warp Pass");
            pass.set_pipeline(warp_pl);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &warp_bg, &[]);
            pass.set_bind_group(1, ubg, &[]);
            pass.draw(0..6, 0..1);
        }

        // ------------------------------------------------------------------
        // 4. Blit pass → render_target_view (screen)
        // ------------------------------------------------------------------
        let blit_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label:   Some("Flux Blit BG"),
            layout:  blit_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&accum.write().view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(input_samp) },
            ],
        });
        {
            let mut pass = Self::fullscreen_pass(encoder, render_target_view, "Flux Blit Pass");
            pass.set_pipeline(blit_pl);
            pass.set_vertex_buffer(0, vb.slice(..));
            pass.set_bind_group(0, &blit_bg, &[]);
            pass.draw(0..6, 0..1);
        }

        // ------------------------------------------------------------------
        // 4. GPU copy: current input → prev_frame (for next frame's It)
        // ------------------------------------------------------------------
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture:   input_tex,
                mip_level: 0,
                origin:    wgpu::Origin3d::ZERO,
                aspect:    wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture:   &prev_frame.texture,
                mip_level: 0,
                origin:    wgpu::Origin3d::ZERO,
                aspect:    wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );

        // ------------------------------------------------------------------
        // 5. Advance ping-pong indices for next frame
        // ------------------------------------------------------------------
        // SAFETY: render() has exclusive &mut self; no aliasing.
        self.flow.as_mut().unwrap().swap();
        self.accum.as_mut().unwrap().swap();

        true
    }

    fn build_uniforms(&self, s: &FluxState, e: &EngineState) -> FluxUniforms {
        Self::build_uniforms_static(s, e)
    }

}

impl FluxEffect {
    fn build_uniforms_static(s: &FluxState, e: &EngineState) -> FluxUniforms {
        let audio_reactive = e.get_param("audio_reactive")
            .map(|v| v > 0.5)
            .unwrap_or(s.audio_reactive);

        let (audio_level, bass, mid, treble) = if audio_reactive {
            let fft = &e.audio.fft;
            let level = fft.iter().copied().fold(0.0_f32, f32::max);
            (level, fft[0], fft[2], fft[7])
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        FluxUniforms {
            flow_lambda:    e.get_param("flow_lambda").unwrap_or(s.flow_lambda),
            flow_smooth:    e.get_param("flow_smooth").unwrap_or(s.flow_smooth),
            flow_scale:     e.get_param("flow_scale").unwrap_or(s.flow_scale),
            _pad0:          0.0,
            warp_strength:  e.get_param("warp_strength").unwrap_or(s.warp_strength),
            drift_strength: e.get_param("drift_strength").unwrap_or(s.drift_strength),
            feedback_decay: e.get_param("feedback_decay").unwrap_or(s.feedback_decay),
            webcam_mix:     e.get_param("webcam_mix").unwrap_or(s.webcam_mix),
            flow_viz:       e.get_param("flow_viz").unwrap_or(s.flow_viz),
            flow_viz_scale: e.get_param("flow_viz_scale").unwrap_or(s.flow_viz_scale),
            _pad1:          0.0,
            _pad2:          0.0,
            audio_level,
            bass,
            mid,
            treble,
        }
    }
}

// ---------------------------------------------------------------------------
// GUI tab
// ---------------------------------------------------------------------------

struct FluxTab;

impl AnyGuiTab for FluxTab {
    fn name(&self)     -> &str             { "Flux" }
    fn replaces(&self) -> Option<GuiTab>   { Some(GuiTab::Motion) }

    fn draw(
        &mut self,
        ui:         &imgui::Ui,
        app_state:  &mut dyn std::any::Any,
        engine:     &mut EngineState,
    ) {
        let s = app_state.downcast_mut::<FluxState>().unwrap();

        ui.text("Optical Flow Warp");
        ui.separator();

        let _w = ui.push_item_width(220.0);

        ui.text_colored([0.4, 0.8, 1.0, 1.0], "Flow Computation");
        if ui.slider_config("Flow Scale",  0.1_f32, 5.0).build(&mut s.flow_scale) {
            engine.set_param_base("flow_scale", s.flow_scale);
        }
        if ui.slider_config("Smoothing",   0.0_f32, 0.95).build(&mut s.flow_smooth) {
            engine.set_param_base("flow_smooth", s.flow_smooth);
        }
        if ui.slider_config("Lambda",      0.001_f32, 0.1).build(&mut s.flow_lambda) {
            engine.set_param_base("flow_lambda", s.flow_lambda);
        }

        ui.separator();
        ui.text_colored([0.4, 0.8, 1.0, 1.0], "Warp & Feedback");
        if ui.slider_config("Warp",        0.0_f32, 2.0).build(&mut s.warp_strength) {
            engine.set_param_base("warp_strength", s.warp_strength);
        }
        if ui.slider_config("Drift",       0.0_f32, 0.5).build(&mut s.drift_strength) {
            engine.set_param_base("drift_strength", s.drift_strength);
        }
        if ui.slider_config("Feedback",    0.8_f32, 0.999).build(&mut s.feedback_decay) {
            engine.set_param_base("feedback_decay", s.feedback_decay);
        }
        if ui.slider_config("Webcam Mix",  0.0_f32, 1.0).build(&mut s.webcam_mix) {
            engine.set_param_base("webcam_mix", s.webcam_mix);
        }

        ui.separator();
        ui.text_colored([0.4, 0.8, 1.0, 1.0], "Visualisation");
        if ui.slider_config("Flow Viz",    0.0_f32, 1.0).build(&mut s.flow_viz) {
            engine.set_param_base("flow_viz", s.flow_viz);
        }
        if ui.slider_config("Viz Scale",   0.5_f32, 20.0).build(&mut s.flow_viz_scale) {
            engine.set_param_base("flow_viz_scale", s.flow_viz_scale);
        }

        ui.separator();
        if ui.checkbox("Audio Reactive", &mut s.audio_reactive) {
            engine.set_param_base("audio_reactive", if s.audio_reactive { 1.0 } else { 0.0 });
        }
        if s.audio_reactive {
            ui.text_colored([0.6, 0.6, 0.6, 1.0], "  Bass → warp  |  Treble → trail length");
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga",            log::LevelFilter::Warn)
        .filter_module("wgpu_core",       log::LevelFilter::Warn)
        .filter_module("winit",           log::LevelFilter::Warn)
        .filter_module("tracing::span",   log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Flux v{}", env!("CARGO_PKG_VERSION"));

    let args: Vec<String> = std::env::args().collect();
    let nogui = args.iter().any(|a| a == "--nogui");
    let gles2 = args.iter().any(|a| a == "--gles2");
    let drm   = args.iter().any(|a| a == "--drm");

    // --render-width W  --render-height H  (optional; default = display resolution)
    let render_w = args.windows(2)
        .find(|w| w[0] == "--render-width")
        .and_then(|w| w[1].parse::<u32>().ok());
    let render_h = args.windows(2)
        .find(|w| w[0] == "--render-height")
        .and_then(|w| w[1].parse::<u32>().ok());
    let render_scale = args.windows(2)
        .find(|w| w[0] == "--render-scale")
        .and_then(|w| w[1].parse::<f32>().ok());

    if nogui && gles2 && drm {
        #[cfg(feature = "drm-gles2")]
        {
            let renderer = match (render_w, render_h, render_scale) {
                (Some(rw), Some(rh), _) => {
                    log::info!("Render resolution: {}×{}", rw, rh);
                    gles2_renderer::FluxGles2::with_render_size(rw, rh)
                }
                (_, _, Some(s)) => {
                    log::info!("Render scale: {}", s);
                    gles2_renderer::FluxGles2::with_render_scale(s)
                }
                _ => gles2_renderer::FluxGles2::default(),
            };
            return rustjay_engine::run_drm_gles2_headless_with_tabs(
                FluxEffect::default(),
                renderer,
            );
        }
        #[cfg(not(feature = "drm-gles2"))]
        {
            log::error!("--drm requires the `drm-gles2` cargo feature");
            return Err(anyhow::anyhow!("drm-gles2 feature not enabled"));
        }
    }

    if nogui && gles2 {
        #[cfg(feature = "gles2")]
        {
            let renderer = match (render_w, render_h, render_scale) {
                (Some(rw), Some(rh), _) => {
                    log::info!("Render resolution: {}×{}", rw, rh);
                    gles2_renderer::FluxGles2::with_render_size(rw, rh)
                }
                (_, _, Some(s)) => {
                    log::info!("Render scale: {}", s);
                    gles2_renderer::FluxGles2::with_render_scale(s)
                }
                _ => gles2_renderer::FluxGles2::default(),
            };
            return rustjay_engine::run_gles2_headless_with_tabs(
                FluxEffect::default(),
                renderer,
            );
        }
        #[cfg(not(feature = "gles2"))]
        {
            log::error!("--gles2 flag requires the `gles2` cargo feature");
            return Err(anyhow::anyhow!("gles2 feature not enabled"));
        }
    }

    if nogui {
        rustjay_engine::run_headless_with_tabs(FluxEffect::default(), vec![Box::new(FluxTab)])
    } else {
        rustjay_engine::run_with_tabs(FluxEffect::default(), vec![Box::new(FluxTab)])
    }
}
