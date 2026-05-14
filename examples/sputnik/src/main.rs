//! Sputnik — Indexed mesh + vertex-shader displacement.
//!
//! A Rutt-Etra style effect where video luminance displaces a dense grid of
//! vertices. Demonstrates `MeshDescriptor`, `vertex_reads_texture`, and
//! audio-reactive 3D mesh visuals with a per-axis animated LFO system
//! matching the original sputnikMesh (openFrameworks) feature set.

use rustjay_engine::prelude::*;

struct SputnikEffect;

/// GPU uniform block — 192 bytes, 16-byte aligned throughout.
///
/// Layout must exactly match `SputnikUniforms` in both WGSL shaders.
/// All offsets are listed in comments; verify against the WGSL struct if
/// fields are ever reordered.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SputnikUniforms {
    // offset   0 — 16 bytes
    displacement_scale: f32,
    bright_invert: u32,
    _pad0: [u32; 2],

    // offset  16 — 32 bytes (two vec4s)
    audio_bands_a: [f32; 4],
    audio_bands_b: [f32; 4],

    // offset  48 — X LFO (16 bytes)
    x_lfo_arg:   f32,
    x_lfo_amp:   f32,
    x_lfo_freq:  f32,
    x_lfo_shape: u32,

    // offset  64 — Y LFO (16 bytes)
    y_lfo_arg:   f32,
    y_lfo_amp:   f32,
    y_lfo_freq:  f32,
    y_lfo_shape: u32,

    // offset  80 — Z LFO (16 bytes; Z scales XY position)
    z_lfo_arg:   f32,
    z_lfo_amp:   f32,
    z_lfo_freq:  f32,
    z_lfo_shape: u32,

    // offset  96 — modulation flags (16 bytes)
    x_phasemod: u32,
    x_ringmod:  u32,
    y_phasemod: u32,
    y_ringmod:  u32,

    // offset 112 — more flags + padding to 128 (16 bytes)
    z_phasemod: u32,
    z_ringmod:  u32,
    _pad2: [u32; 2],

    // offset 128 — MVP matrix (64 bytes, 16-byte aligned)
    mvp: glam::Mat4,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SputnikState {
    // Mesh
    mesh_cols: u32,
    mesh_rows: u32,
    topology:  u32,

    // Video displacement
    displacement_scale: f32,
    bright_invert: bool,

    // LFO X — horizontal displacement
    x_lfo_rate:  f32,
    x_lfo_amp:   f32,
    x_lfo_freq:  f32,
    x_lfo_shape: u32,
    x_phasemod:  bool,
    x_ringmod:   bool,

    // LFO Y — vertical displacement
    y_lfo_rate:  f32,
    y_lfo_amp:   f32,
    y_lfo_freq:  f32,
    y_lfo_shape: u32,
    y_phasemod:  bool,
    y_ringmod:   bool,

    // LFO Z — scales XY position (zoom-pulse effect)
    z_lfo_rate:  f32,
    z_lfo_amp:   f32,
    z_lfo_freq:  f32,
    z_lfo_shape: u32,
    z_phasemod:  bool,
    z_ringmod:   bool,

    // Camera
    camera_distance: f32,
    camera_tilt:     f32,

    // Audio
    audio_band_weights: [f32; 8],

    // Per-frame LFO phase accumulators — not saved to disk.
    #[serde(skip)]
    x_lfo_arg: f32,
    #[serde(skip)]
    y_lfo_arg: f32,
    #[serde(skip)]
    z_lfo_arg: f32,
}

impl Default for SputnikState {
    fn default() -> Self {
        Self {
            mesh_cols: 320,
            mesh_rows: 180,
            topology:  0,
            displacement_scale: 0.3,
            bright_invert: false,
            x_lfo_rate: 1.0,
            x_lfo_amp:  0.1,
            x_lfo_freq: 2.0,
            x_lfo_shape: 0,
            x_phasemod: false,
            x_ringmod:  false,
            y_lfo_rate: 0.7,
            y_lfo_amp:  0.05,
            y_lfo_freq: 3.0,
            y_lfo_shape: 0,
            y_phasemod: false,
            y_ringmod:  false,
            z_lfo_rate: 0.3,
            z_lfo_amp:  0.0,
            z_lfo_freq: 1.0,
            z_lfo_shape: 0,
            z_phasemod: false,
            z_ringmod:  false,
            camera_distance: 3.0,
            camera_tilt:     0.0,
            audio_band_weights: [0.0; 8],
            x_lfo_arg: 0.0,
            y_lfo_arg: 0.0,
            z_lfo_arg: 0.0,
        }
    }
}

impl EffectPlugin for SputnikEffect {
    type State    = SputnikState;
    type Uniforms = SputnikUniforms;

    fn app_name(&self) -> &str { "sputnik" }

    fn default_state(&self) -> SputnikState { SputnikState::default() }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/sputnik.wgsl")
    }

    fn compute_shader(&self) -> Option<&'static str> {
        Some(include_str!("shaders/sputnik_compute.wgsl"))
    }

    fn mesh_descriptor(&self, state: &SputnikState) -> Option<MeshDescriptor> {
        let topology = match state.topology {
            0 => MeshTopology::Scanlines,
            1 => MeshTopology::Triangles,
            2 => MeshTopology::Wireframe,
            3 => MeshTopology::Points,
            _ => MeshTopology::Scanlines,
        };
        Some(MeshDescriptor { cols: state.mesh_cols, rows: state.mesh_rows, topology })
    }

    fn vertex_reads_texture(&self) -> bool { true }

    /// Advance LFO phase accumulators using real elapsed time.
    fn prepare(
        &mut self,
        state:  &mut SputnikState,
        engine: &EngineState,
        _device: &wgpu::Device,
        _queue:  &wgpu::Queue,
    ) {
        let dt = engine.performance.frame_time_ms / 1000.0;
        state.x_lfo_arg += state.x_lfo_rate * dt;
        state.y_lfo_arg += state.y_lfo_rate * dt;
        state.z_lfo_arg += state.z_lfo_rate * dt;
    }

    fn build_uniforms(&self, s: &SputnikState, engine: &EngineState) -> SputnikUniforms {
        let aspect = if engine.resolution.internal_height > 0 {
            engine.resolution.internal_width as f32
                / engine.resolution.internal_height as f32
        } else {
            16.0 / 9.0
        };

        let mut bands_a = [0.0f32; 4];
        let mut bands_b = [0.0f32; 4];
        for i in 0..4 {
            bands_a[i] = engine.audio.fft[i]     * s.audio_band_weights[i];
            bands_b[i] = engine.audio.fft[i + 4] * s.audio_band_weights[i + 4];
        }

        let projection = glam::Mat4::perspective_rh(
            60.0f32.to_radians(), aspect, 0.1, 100.0,
        );
        let dist = s.camera_distance.max(0.1);
        let eye = glam::Vec3::new(
            0.0,
            s.camera_tilt.sin() * dist,
            s.camera_tilt.cos() * dist,
        );
        let view  = glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, glam::Vec3::Y);
        let mvp   = projection * view;

        SputnikUniforms {
            displacement_scale: s.displacement_scale,
            bright_invert:      s.bright_invert as u32,
            _pad0:              [0; 2],
            audio_bands_a:      bands_a,
            audio_bands_b:      bands_b,
            x_lfo_arg:          s.x_lfo_arg,
            x_lfo_amp:          s.x_lfo_amp,
            x_lfo_freq:         s.x_lfo_freq,
            x_lfo_shape:        s.x_lfo_shape,
            y_lfo_arg:          s.y_lfo_arg,
            y_lfo_amp:          s.y_lfo_amp,
            y_lfo_freq:         s.y_lfo_freq,
            y_lfo_shape:        s.y_lfo_shape,
            z_lfo_arg:          s.z_lfo_arg,
            z_lfo_amp:          s.z_lfo_amp,
            z_lfo_freq:         s.z_lfo_freq,
            z_lfo_shape:        s.z_lfo_shape,
            x_phasemod:         s.x_phasemod as u32,
            x_ringmod:          s.x_ringmod  as u32,
            y_phasemod:         s.y_phasemod as u32,
            y_ringmod:          s.y_ringmod  as u32,
            z_phasemod:         s.z_phasemod as u32,
            z_ringmod:          s.z_ringmod  as u32,
            _pad2:              [0; 2],
            mvp,
        }
    }
}

// ── GUI ────────────────────────────────────────────────────────────────────

struct SputnikTab;

const SHAPE_LABELS: [&str; 4] = ["Sine", "Square", "Sawtooth", "Noise"];

fn lfo_section(
    ui:    &imgui::Ui,
    label: &str,
    rate:  &mut f32,
    amp:   &mut f32,
    freq:  &mut f32,
    shape: &mut u32,
    phasemod: &mut bool,
    ringmod:  &mut bool,
) {
    ui.text(label);
    ui.slider_config(&format!("Rate###{label}"), 0.0, 10.0).build(rate);
    ui.slider_config(&format!("Amp###{label}"),  0.0, 1.0 ).build(amp);
    ui.slider_config(&format!("Freq###{label}"), 0.0, 20.0).build(freq);
    ui.text("Shape");
    for (i, &name) in SHAPE_LABELS.iter().enumerate() {
        if ui.radio_button_bool(&format!("{name}###{label}shape{i}"), *shape == i as u32) {
            *shape = i as u32;
        }
        if i < 3 { ui.same_line(); }
    }
    ui.checkbox(&format!("Phase mod###{label}"), phasemod);
    ui.same_line();
    ui.checkbox(&format!("Ring mod###{label}"),  ringmod);
}

impl AnyGuiTab for SputnikTab {
    fn name(&self) -> &str { "Sputnik" }

    fn draw(
        &mut self,
        ui:        &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine:   &mut EngineState,
    ) {
        let s = app_state
            .downcast_mut::<SputnikState>()
            .expect("SputnikTab expects SputnikState");

        // ── Topology ──────────────────────────────────────────────────────
        ui.text("Topology");
        for (i, label) in ["Scanlines", "Triangles", "Wireframe", "Points"].iter().enumerate() {
            if ui.radio_button_bool(label, s.topology == i as u32) {
                s.topology = i as u32;
            }
            if i < 3 { ui.same_line(); }
        }

        ui.separator();

        // ── Mesh resolution ───────────────────────────────────────────────
        ui.text("Mesh Resolution");
        let mut cols = s.mesh_cols as i32;
        let mut rows = s.mesh_rows as i32;
        ui.input_int("Columns", &mut cols).build();
        ui.input_int("Rows",    &mut rows).build();
        s.mesh_cols = cols.max(1) as u32;
        s.mesh_rows = rows.max(1) as u32;

        ui.separator();

        // ── Video displacement ────────────────────────────────────────────
        ui.text("Video Displacement");
        ui.slider_config("Displacement", 0.0, 2.0).build(&mut s.displacement_scale);
        ui.checkbox("Invert brightness", &mut s.bright_invert);

        ui.separator();

        // ── LFO system ────────────────────────────────────────────────────
        ui.text("LFO System");
        lfo_section(
            ui, "X (horizontal)",
            &mut s.x_lfo_rate, &mut s.x_lfo_amp, &mut s.x_lfo_freq, &mut s.x_lfo_shape,
            &mut s.x_phasemod, &mut s.x_ringmod,
        );
        ui.separator();
        lfo_section(
            ui, "Y (vertical)",
            &mut s.y_lfo_rate, &mut s.y_lfo_amp, &mut s.y_lfo_freq, &mut s.y_lfo_shape,
            &mut s.y_phasemod, &mut s.y_ringmod,
        );
        ui.separator();
        lfo_section(
            ui, "Z (zoom pulse)",
            &mut s.z_lfo_rate, &mut s.z_lfo_amp, &mut s.z_lfo_freq, &mut s.z_lfo_shape,
            &mut s.z_phasemod, &mut s.z_ringmod,
        );

        ui.separator();

        // ── Camera ────────────────────────────────────────────────────────
        ui.text("Camera (3D Perspective)");
        ui.slider_config("Distance", 0.5, 10.0).build(&mut s.camera_distance);
        ui.slider_config("Tilt",    -1.0,  1.0).build(&mut s.camera_tilt);

        ui.separator();

        // ── Audio band weights ─────────────────────────────────────────────
        ui.text("Audio Band Weights");
        for i in 0..8usize {
            let label = format!("Band {}", i + 1);
            ui.slider_config(&label, 0.0, 2.0).build(&mut s.audio_band_weights[i]);
        }
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga",            log::LevelFilter::Warn)
        .filter_module("wgpu_core",       log::LevelFilter::Warn)
        .filter_module("winit",           log::LevelFilter::Warn)
        .filter_module("tracing::span",   log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Sputnik v{}", env!("CARGO_PKG_VERSION"));
    rustjay_engine::run_with_tabs(SputnikEffect, vec![Box::new(SputnikTab)])
}
