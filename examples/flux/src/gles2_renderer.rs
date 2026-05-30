//! Native GLES 2.0 renderer for Flux — used on Pi 2 VC4 hardware.
//!
//! Three fullscreen passes with individual uniforms (no UBOs):
//!   1. Flow  — Lucas-Kanade optical flow from current + previous webcam frames
//!   2. Warp  — displace webcam UV by flow field, accumulate feedback
//!   3. Blit  — copy accumulated buffer to the EGL surface
//!
//! The renderer owns its own webcam capture thread; no wgpu InputManager is used.

use anyhow::{anyhow, Result};
use glow::{Context as Gl, HasContext};
use rustjay_engine::{EngineState, gles2::Gles2Effect};
use rustjay_io::{WebcamCapture, WebcamFrame};
use std::sync::mpsc::Receiver;

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Compile + link a vertex/fragment pair. Panics on compile error (logged first).
unsafe fn compile_program(gl: &Gl, vert_src: &str, frag_src: &str) -> glow::Program {
    let vert = gl.create_shader(glow::VERTEX_SHADER).unwrap();
    gl.shader_source(vert, vert_src);
    gl.compile_shader(vert);
    if !gl.get_shader_compile_status(vert) {
        panic!("Vertex shader error: {}", gl.get_shader_info_log(vert));
    }

    let frag = gl.create_shader(glow::FRAGMENT_SHADER).unwrap();
    gl.shader_source(frag, frag_src);
    gl.compile_shader(frag);
    if !gl.get_shader_compile_status(frag) {
        panic!("Fragment shader error: {}", gl.get_shader_info_log(frag));
    }

    let prog = gl.create_program().unwrap();
    gl.attach_shader(prog, vert);
    gl.attach_shader(prog, frag);
    gl.link_program(prog);
    if !gl.get_program_link_status(prog) {
        panic!("Program link error: {}", gl.get_program_info_log(prog));
    }
    gl.delete_shader(vert);
    gl.delete_shader(frag);
    prog
}

/// Create a plain RGBA8 2D texture.
unsafe fn make_texture(gl: &Gl, width: u32, height: u32, data: Option<&[u8]>) -> glow::Texture {
    let tex = gl.create_texture().unwrap();
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
    gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
    gl.tex_image_2d(
        glow::TEXTURE_2D, 0,
        glow::RGBA as i32,
        width as i32, height as i32, 0,
        glow::RGBA, glow::UNSIGNED_BYTE,
        glow::PixelUnpackData::Slice(Some(data.unwrap_or(&vec![0u8; (width * height * 4) as usize]))),
    );
    gl.bind_texture(glow::TEXTURE_2D, None);
    tex
}

/// Create an FBO backed by a texture. Returns (fbo, texture).
unsafe fn make_fbo(gl: &Gl, width: u32, height: u32) -> (glow::Framebuffer, glow::Texture) {
    let tex = make_texture(gl, width, height, None);
    let fbo = gl.create_framebuffer().unwrap();
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D, Some(tex), 0,
    );
    assert_eq!(
        gl.check_framebuffer_status(glow::FRAMEBUFFER),
        glow::FRAMEBUFFER_COMPLETE,
        "FBO incomplete"
    );
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    (fbo, tex)
}

// ── GPU resources ─────────────────────────────────────────────────────────────

struct GlState {
    // Programs
    flow_prog: glow::Program,
    warp_prog: glow::Program,
    blit_prog: glow::Program,

    // Vertex buffer (fullscreen quad, 6 verts × [pos2, uv2])
    vbo: glow::Buffer,

    // Textures / FBOs
    webcam_tex:  glow::Texture,
    prev_tex:    glow::Texture,

    flow_fbo:    [glow::Framebuffer; 2],
    flow_tex:    [glow::Texture;     2],

    accum_fbo:   [glow::Framebuffer; 2],
    accum_tex:   [glow::Texture;     2],

    flow_read:   usize,
    accum_read:  usize,

    // Display resolution (DRM scan-out size)
    width:  u32,
    height: u32,
    // Internal render resolution (accum/warp FBOs — may be smaller than display for perf)
    render_w: u32,
    render_h: u32,
    // Webcam/flow texture resolution (camera capture size, typically 640×480)
    cam_w:  u32,
    cam_h:  u32,
}

// ── FluxGles2 ─────────────────────────────────────────────────────────────────

/// GLES 2.0 Flux effect.  Owns webcam capture independently of the engine's
/// InputManager (which requires a wgpu device that doesn't exist in this path).
pub struct FluxGles2 {
    gl_state:  Option<GlState>,
    webcam:    Option<WebcamCapture>,
    receiver:  Option<Receiver<WebcamFrame>>,
    last_frame: Option<WebcamFrame>,
    /// Fixed render resolution override. Takes precedence over render_scale.
    render_w:    Option<u32>,
    render_h:    Option<u32>,
    /// Scale factor applied to the display resolution (preserves aspect ratio).
    render_scale: Option<f32>,
}

impl Default for FluxGles2 {
    fn default() -> Self {
        Self { gl_state: None, webcam: None, receiver: None, last_frame: None,
               render_w: None, render_h: None, render_scale: None }
    }
}

impl FluxGles2 {
    pub fn with_render_size(render_w: u32, render_h: u32) -> Self {
        Self { render_w: Some(render_w), render_h: Some(render_h), ..Self::default() }
    }
    pub fn with_render_scale(scale: f32) -> Self {
        Self { render_scale: Some(scale), ..Self::default() }
    }
}

impl FluxGles2 {
    fn open_webcam(&mut self, device_index: usize) {
        match WebcamCapture::new(device_index, 640, 480, 30) {
            Ok(mut cap) => match cap.start() {
                Ok(rx) => {
                    self.receiver = Some(rx);
                    self.webcam   = Some(cap);
                    log::info!("GLES2 flux: opened webcam {device_index} at 640×480@30");
                }
                Err(e) => log::error!("GLES2 flux: webcam {device_index} start failed: {e}"),
            },
            Err(e) => log::error!("GLES2 flux: webcam {device_index} open failed: {e}"),
        }
    }

    fn poll_webcam_frame(&mut self) {
        if let Some(ref rx) = self.receiver {
            while let Ok(frame) = rx.try_recv() {
                self.last_frame = Some(frame);
            }
        }
    }

    /// Upload the latest webcam frame into `webcam_tex`.
    /// Frames arrive as BGRA; we swap to RGBA for correct colours.
    unsafe fn upload_webcam(&self, gl: &Gl, gs: &GlState) {
        let Some(ref frame) = self.last_frame else { return };
        if frame.data.is_empty() { return; }

        // Swap B↔R (BGRA → RGBA) and flip horizontally (un-mirror webcam selfie mode).
        let w = frame.width as usize;
        let mut rgba = frame.data.clone();
        for row in rgba.chunks_exact_mut(w * 4) {
            // B↔R swap every pixel
            for px in row.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            // Reverse pixel order within the row
            for i in 0..w / 2 {
                let j = w - 1 - i;
                row.swap(i * 4,     j * 4);
                row.swap(i * 4 + 1, j * 4 + 1);
                row.swap(i * 4 + 2, j * 4 + 2);
                row.swap(i * 4 + 3, j * 4 + 3);
            }
        }

        gl.bind_texture(glow::TEXTURE_2D, Some(gs.webcam_tex));
        gl.tex_sub_image_2d(
            glow::TEXTURE_2D, 0,
            0, 0, frame.width as i32, frame.height as i32,
            glow::RGBA, glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&rgba)),
        );
        gl.bind_texture(glow::TEXTURE_2D, None);
    }
}

impl Gles2Effect for FluxGles2 {
    fn init_gl(&mut self, gl: &Gl, width: u32, height: u32, state: &EngineState) -> Result<()> {
        let vert = include_str!("shaders/flux_vert_es1.glsl");

        let flow_prog = unsafe { compile_program(gl, vert, include_str!("shaders/flux_flow_es1.glsl")) };
        let warp_prog = unsafe { compile_program(gl, vert, include_str!("shaders/flux_warp_es1.glsl")) };
        let blit_prog = unsafe { compile_program(gl, vert, include_str!("shaders/flux_blit_es1.glsl")) };

        // Fullscreen quad: 2 triangles, NDC coords + UV
        #[rustfmt::skip]
        let verts: [f32; 24] = [
            -1.0, -1.0,  0.0, 1.0,
             1.0, -1.0,  1.0, 1.0,
             1.0,  1.0,  1.0, 0.0,
            -1.0, -1.0,  0.0, 1.0,
             1.0,  1.0,  1.0, 0.0,
            -1.0,  1.0,  0.0, 0.0,
        ];
        let vbo = unsafe {
            let buf = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buf));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck::cast_slice(&verts),
                glow::STATIC_DRAW,
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            buf
        };

        // Webcam and optical-flow textures live at capture resolution (640×480).
        // Accum (feedback) lives at render resolution (may be smaller than display).
        // The blit pass upscales from render resolution to display resolution.
        let cam_w = 640u32;
        let cam_h = 480u32;

        let render_w = self.render_w.unwrap_or_else(|| {
            self.render_scale
                .map(|s| ((width as f32 * s).round() as u32).max(1))
                .unwrap_or(width)
        });
        let render_h = self.render_h.unwrap_or_else(|| {
            self.render_scale
                .map(|s| ((height as f32 * s).round() as u32).max(1))
                .unwrap_or(height)
        });
        log::info!("GLES2 flux: display {}×{}, render {}×{}", width, height, render_w, render_h);

        let webcam_tex = unsafe { make_texture(gl, cam_w, cam_h, None) };
        let prev_tex   = unsafe { make_texture(gl, cam_w, cam_h, None) };

        let (flow_fbo0, flow_tex0) = unsafe { make_fbo(gl, cam_w, cam_h) };
        let (flow_fbo1, flow_tex1) = unsafe { make_fbo(gl, cam_w, cam_h) };
        let (accum_fbo0, accum_tex0) = unsafe { make_fbo(gl, render_w, render_h) };
        let (accum_fbo1, accum_tex1) = unsafe { make_fbo(gl, render_w, render_h) };

        self.gl_state = Some(GlState {
            flow_prog, warp_prog, blit_prog,
            vbo,
            webcam_tex, prev_tex,
            flow_fbo:  [flow_fbo0,  flow_fbo1],
            flow_tex:  [flow_tex0,  flow_tex1],
            accum_fbo: [accum_fbo0, accum_fbo1],
            accum_tex: [accum_tex0, accum_tex1],
            flow_read: 0, accum_read: 0,
            width, height, render_w, render_h, cam_w, cam_h,
        });

        // Open webcam using the configured device index
        let dev = state.startup_webcam_device.unwrap_or(0);
        self.open_webcam(dev);

        Ok(())
    }

    fn render_frame(&mut self, gl: &Gl, state: &EngineState) -> Result<bool> {
        self.poll_webcam_frame();

        // Upload the latest webcam frame before taking the mutable borrow.
        {
            let gs = self.gl_state.as_ref().ok_or_else(|| anyhow!("GL not initialised"))?;
            unsafe { self.upload_webcam(gl, gs); }
        }
        let gs = self.gl_state.as_mut().ok_or_else(|| anyhow!("GL not initialised"))?;

        // Read params from engine state (registered by FluxEffect::parameters())
        let flow_lambda    = state.get_param("flow_lambda").unwrap_or(0.005);
        let flow_smooth    = state.get_param("flow_smooth").unwrap_or(0.7);
        let flow_scale     = state.get_param("flow_scale").unwrap_or(1.5);
        let warp_strength  = state.get_param("warp_strength").unwrap_or(0.6);
        let drift_strength = state.get_param("drift_strength").unwrap_or(0.15);
        let feedback_decay = state.get_param("feedback_decay").unwrap_or(0.93);
        let webcam_mix     = state.get_param("webcam_mix").unwrap_or(0.25);
        let flow_viz       = state.get_param("flow_viz").unwrap_or(0.0);
        let flow_viz_scale = state.get_param("flow_viz_scale").unwrap_or(5.0);
        let audio_reactive = state.get_param("audio_reactive").unwrap_or(1.0) > 0.5;

        let (audio_level, bass, _mid, treble) = if audio_reactive {
            let fft = &state.audio.fft;
            let level = fft.iter().copied().fold(0.0_f32, f32::max);
            (level, fft.get(0).copied().unwrap_or(0.0),
                    fft.get(2).copied().unwrap_or(0.0),
                    fft.get(7).copied().unwrap_or(0.0))
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };

        let w  = gs.width    as i32;
        let h  = gs.height   as i32;
        let rw = gs.render_w as i32;
        let rh = gs.render_h as i32;
        let cw = gs.cam_w    as i32;
        let ch = gs.cam_h    as i32;

        unsafe {

            // Bind VBO and set attribs (shared by all passes)
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(gs.vbo));
            let stride = (4 * std::mem::size_of::<f32>()) as i32;

            // ── Pass 1: Flow (at webcam resolution) ───────────────────────────
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(gs.flow_fbo[1 - gs.flow_read]));
            gl.viewport(0, 0, cw, ch);
            gl.use_program(Some(gs.flow_prog));

            let a_pos = gl.get_attrib_location(gs.flow_prog, "a_pos").unwrap();
            let a_uv  = gl.get_attrib_location(gs.flow_prog, "a_uv").unwrap();
            gl.enable_vertex_attrib_array(a_pos);
            gl.enable_vertex_attrib_array(a_uv);
            gl.vertex_attrib_pointer_f32(a_pos, 2, glow::FLOAT, false, stride, 0);
            gl.vertex_attrib_pointer_f32(a_uv,  2, glow::FLOAT, false, stride, 8);

            bind_texture(gl, gs.flow_prog, "u_curr",      0, gs.webcam_tex);
            bind_texture(gl, gs.flow_prog, "u_prev",      1, gs.prev_tex);
            bind_texture(gl, gs.flow_prog, "u_prev_flow", 2, gs.flow_tex[gs.flow_read]);

            set_uniform1f(gl, gs.flow_prog, "u_flow_lambda",  flow_lambda);
            set_uniform1f(gl, gs.flow_prog, "u_flow_smooth",  flow_smooth);
            set_uniform1f(gl, gs.flow_prog, "u_flow_scale",   flow_scale);
            set_uniform1f(gl, gs.flow_prog, "u_audio_level",  audio_level);
            // u_resolution must match the webcam texture so gradient dx/dy are correct
            set_uniform2f(gl, gs.flow_prog, "u_resolution",   cw as f32, ch as f32);

            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            // ── Copy webcam → prev_frame ──────────────────────────────────────
            // (blit webcam_tex into prev_tex for next frame's temporal gradient)
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            // Re-use blit prog to copy webcam → a scratch FBO of prev_tex
            // Simplest approach: copy via CPU is too slow; use FBO attachment swap.
            // We swap using a 1-pixel blit trick: bind prev_tex to accum slot temporarily.
            // Actually the cleanest way: copy texture via FBO + blit pass.
            copy_tex_via_fbo(gl, gs.webcam_tex, gs.prev_tex, cw, ch);

            // ── Pass 2: Warp ──────────────────────────────────────────────────
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(gs.accum_fbo[1 - gs.accum_read]));
            gl.viewport(0, 0, rw, rh);
            gl.use_program(Some(gs.warp_prog));

            let a_pos = gl.get_attrib_location(gs.warp_prog, "a_pos").unwrap();
            let a_uv  = gl.get_attrib_location(gs.warp_prog, "a_uv").unwrap();
            gl.enable_vertex_attrib_array(a_pos);
            gl.enable_vertex_attrib_array(a_uv);
            gl.vertex_attrib_pointer_f32(a_pos, 2, glow::FLOAT, false, stride, 0);
            gl.vertex_attrib_pointer_f32(a_uv,  2, glow::FLOAT, false, stride, 8);

            bind_texture(gl, gs.warp_prog, "u_input", 0, gs.webcam_tex);
            bind_texture(gl, gs.warp_prog, "u_flow",  1, gs.flow_tex[1 - gs.flow_read]);
            bind_texture(gl, gs.warp_prog, "u_accum", 2, gs.accum_tex[gs.accum_read]);

            set_uniform1f(gl, gs.warp_prog, "u_warp_strength",  warp_strength);
            set_uniform1f(gl, gs.warp_prog, "u_drift_strength", drift_strength);
            set_uniform1f(gl, gs.warp_prog, "u_feedback_decay", feedback_decay);
            set_uniform1f(gl, gs.warp_prog, "u_webcam_mix",     webcam_mix);
            set_uniform1f(gl, gs.warp_prog, "u_flow_viz",       flow_viz);
            set_uniform1f(gl, gs.warp_prog, "u_flow_viz_scale", flow_viz_scale);
            set_uniform1f(gl, gs.warp_prog, "u_bass",           bass);
            set_uniform1f(gl, gs.warp_prog, "u_treble",         treble);

            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            // ── Pass 3: Blit to screen ────────────────────────────────────────
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.viewport(0, 0, w, h);
            gl.use_program(Some(gs.blit_prog));

            let a_pos = gl.get_attrib_location(gs.blit_prog, "a_pos").unwrap();
            let a_uv  = gl.get_attrib_location(gs.blit_prog, "a_uv").unwrap();
            gl.enable_vertex_attrib_array(a_pos);
            gl.enable_vertex_attrib_array(a_uv);
            gl.vertex_attrib_pointer_f32(a_pos, 2, glow::FLOAT, false, stride, 0);
            gl.vertex_attrib_pointer_f32(a_uv,  2, glow::FLOAT, false, stride, 8);

            bind_texture(gl, gs.blit_prog, "u_src", 0, gs.accum_tex[1 - gs.accum_read]);

            gl.draw_arrays(glow::TRIANGLES, 0, 6);

            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            gl.use_program(None);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);

            // Advance ping-pong
            gs.flow_read  = 1 - gs.flow_read;
            gs.accum_read = 1 - gs.accum_read;
        }

        Ok(true)
    }
}

// ── Inline GL helpers ─────────────────────────────────────────────────────────

unsafe fn bind_texture(gl: &Gl, prog: glow::Program, name: &str, unit: u32, tex: glow::Texture) {
    gl.active_texture(glow::TEXTURE0 + unit);
    gl.bind_texture(glow::TEXTURE_2D, Some(tex));
    if let Some(loc) = gl.get_uniform_location(prog, name) {
        gl.uniform_1_i32(Some(&loc), unit as i32);
    }
}

unsafe fn set_uniform1f(gl: &Gl, prog: glow::Program, name: &str, v: f32) {
    if let Some(loc) = gl.get_uniform_location(prog, name) {
        gl.uniform_1_f32(Some(&loc), v);
    }
}

unsafe fn set_uniform2f(gl: &Gl, prog: glow::Program, name: &str, x: f32, y: f32) {
    if let Some(loc) = gl.get_uniform_location(prog, name) {
        gl.uniform_2_f32(Some(&loc), x, y);
    }
}

/// Copy `src` texture into `dst` texture using glCopyTexSubImage2D (pure GPU, no CPU roundtrip).
/// Both textures must be w×h RGBA8.
unsafe fn copy_tex_via_fbo(gl: &Gl, src: glow::Texture, dst: glow::Texture, w: i32, h: i32) {
    // Attach src to a read FBO; glCopyTexSubImage2D reads from the current framebuffer
    // and writes directly into the bound texture — no glReadPixels CPU stall.
    let read_fbo = gl.create_framebuffer().unwrap();
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(read_fbo));
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D, Some(src), 0,
    );
    gl.bind_texture(glow::TEXTURE_2D, Some(dst));
    gl.copy_tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, 0, 0, w, h);
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    gl.delete_framebuffer(read_fbo);
    gl.bind_texture(glow::TEXTURE_2D, None);
}
