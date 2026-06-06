//! Sputnik — Indexed mesh + vertex-shader displacement.
//!
//! A Rutt-Etra style effect where video luminance displaces a dense grid of
//! vertices. Demonstrates `MeshDescriptor`, `vertex_reads_texture`, and
//! audio-reactive 3D mesh visuals with a per-axis animated LFO system
//! matching the original sputnikMesh (openFrameworks) feature set.

use rustjay_engine::prelude::*;

struct SputnikEffect;

/// GPU uniform block — 208 bytes, 16-byte aligned throughout.
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
    x_offset: f32,
    y_offset: f32,

    // offset  16 — 32 bytes (two vec4s)
    audio_bands_a: [f32; 4],
    audio_bands_b: [f32; 4],

    // offset  48 — X LFO (16 bytes)
    x_lfo_arg: f32,
    x_lfo_amp: f32,
    x_lfo_freq: f32,
    x_lfo_shape: u32,

    // offset  64 — Y LFO (16 bytes)
    y_lfo_arg: f32,
    y_lfo_amp: f32,
    y_lfo_freq: f32,
    y_lfo_shape: u32,

    // offset  80 — Z LFO (16 bytes; Z scales XY position)
    z_lfo_arg: f32,
    z_lfo_amp: f32,
    z_lfo_freq: f32,
    z_lfo_shape: u32,

    // offset  96 — modulation flags (16 bytes)
    x_phasemod: u32,
    x_ringmod: u32,
    y_phasemod: u32,
    y_ringmod: u32,

    // offset 112 — more flags + input texture dimensions (16 bytes)
    z_phasemod: u32,
    z_ringmod: u32,
    tex_width: f32,
    tex_height: f32,

    // offset 128 — offset and audio reactivity (16 bytes)
    z_offset: f32,
    audio_reactivity: f32,
    _pad0: [u32; 2],

    // offset 144 — MVP matrix (64 bytes, 16-byte aligned)
    mvp: glam::Mat4,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SputnikState {
    // Mesh
    mesh_cols: u32,
    mesh_rows: u32,
    topology: u32,

    // Video displacement
    displacement_scale: f32,
    bright_invert: bool,

    // Offsets applied before LFO displacement
    x_offset: f32,
    y_offset: f32,
    z_offset: f32,

    // LFO X — horizontal displacement
    x_lfo_rate: f32,
    x_lfo_amp: f32,
    x_lfo_freq: f32,
    x_lfo_shape: u32,
    x_phasemod: bool,
    x_ringmod: bool,
    x_tempo_sync: bool,
    x_beat_division: usize,

    // LFO Y — vertical displacement
    y_lfo_rate: f32,
    y_lfo_amp: f32,
    y_lfo_freq: f32,
    y_lfo_shape: u32,
    y_phasemod: bool,
    y_ringmod: bool,
    y_tempo_sync: bool,
    y_beat_division: usize,

    // LFO Z — scales XY position (zoom-pulse effect)
    z_lfo_rate: f32,
    z_lfo_amp: f32,
    z_lfo_freq: f32,
    z_lfo_shape: u32,
    z_phasemod: bool,
    z_ringmod: bool,
    z_tempo_sync: bool,
    z_beat_division: usize,

    // Camera
    camera_distance: f32,
    camera_tilt: f32,

    // Audio
    audio_reactivity: f32,

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
            topology: 0,
            displacement_scale: 0.3,
            bright_invert: false,
            x_offset: 0.0,
            y_offset: 0.0,
            z_offset: 0.0,
            x_lfo_rate: 1.0,
            x_lfo_amp: 0.1,
            x_lfo_freq: 2.0,
            x_lfo_shape: 0,
            x_phasemod: false,
            x_ringmod: false,
            x_tempo_sync: false,
            x_beat_division: 2, // 1/4 note
            y_lfo_rate: 0.7,
            y_lfo_amp: 0.05,
            y_lfo_freq: 3.0,
            y_lfo_shape: 0,
            y_phasemod: false,
            y_ringmod: false,
            y_tempo_sync: false,
            y_beat_division: 2, // 1/4 note
            z_lfo_rate: 0.3,
            z_lfo_amp: 0.0,
            z_lfo_freq: 1.0,
            z_lfo_shape: 0,
            z_phasemod: false,
            z_ringmod: false,
            z_tempo_sync: false,
            z_beat_division: 2, // 1/4 note
            camera_distance: 3.0,
            camera_tilt: 0.0,
            audio_reactivity: 0.0,
            x_lfo_arg: 0.0,
            y_lfo_arg: 0.0,
            z_lfo_arg: 0.0,
        }
    }
}

impl EffectPlugin for SputnikEffect {
    type State = SputnikState;
    type Uniforms = SputnikUniforms;

    fn app_name(&self) -> &str {
        "sputnik"
    }

    fn default_state(&self) -> SputnikState {
        SputnikState::default()
    }

    fn hidden_tabs(&self) -> Vec<GuiTab> {
        vec![GuiTab::Motion]
    }

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
        Some(MeshDescriptor {
            cols: state.mesh_cols,
            rows: state.mesh_rows,
            topology,
        })
    }

    fn parameters(&self) -> Vec<ParameterDescriptor> {
        let cat = ParamCategory::Custom("Sputnik".into());
        vec![
            ParameterDescriptor::float(
                "displacement_scale",
                "Displacement",
                cat.clone(),
                0.0,
                2.0,
                0.3,
                0.01,
            ),
            ParameterDescriptor::float("x_offset", "X Offset", cat.clone(), -2.0, 2.0, 0.0, 0.01),
            ParameterDescriptor::float("y_offset", "Y Offset", cat.clone(), -2.0, 2.0, 0.0, 0.01),
            ParameterDescriptor::float("z_offset", "Z Offset", cat.clone(), 0.0, 1.0, 0.0, 0.01),
            ParameterDescriptor::float(
                "x_lfo_rate",
                "X LFO Rate",
                cat.clone(),
                0.0,
                10.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float("x_lfo_amp", "X LFO Amp", cat.clone(), 0.0, 1.0, 0.1, 0.01),
            ParameterDescriptor::float(
                "x_lfo_freq",
                "X LFO Freq",
                cat.clone(),
                0.0,
                20.0,
                2.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "y_lfo_rate",
                "Y LFO Rate",
                cat.clone(),
                0.0,
                10.0,
                0.7,
                0.01,
            ),
            ParameterDescriptor::float("y_lfo_amp", "Y LFO Amp", cat.clone(), 0.0, 1.0, 0.05, 0.01),
            ParameterDescriptor::float(
                "y_lfo_freq",
                "Y LFO Freq",
                cat.clone(),
                0.0,
                20.0,
                3.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "z_lfo_rate",
                "Z LFO Rate",
                cat.clone(),
                0.0,
                10.0,
                0.3,
                0.01,
            ),
            ParameterDescriptor::float("z_lfo_amp", "Z LFO Amp", cat.clone(), 0.0, 1.0, 0.0, 0.01),
            ParameterDescriptor::float(
                "z_lfo_freq",
                "Z LFO Freq",
                cat.clone(),
                0.0,
                20.0,
                1.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "camera_distance",
                "Camera Dist",
                cat.clone(),
                0.5,
                10.0,
                3.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "camera_tilt",
                "Camera Tilt",
                cat.clone(),
                -1.0,
                1.0,
                0.0,
                0.01,
            ),
            ParameterDescriptor::float(
                "audio_reactivity",
                "Audio Reactivity",
                cat.clone(),
                0.0,
                2.0,
                0.0,
                0.01,
            ),
        ]
    }

    fn vertex_reads_texture(&self) -> bool {
        true
    }

    /// Advance LFO phase accumulators using real elapsed time.
    fn prepare(
        &mut self,
        state: &mut SputnikState,
        engine: &EngineState,
        _device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) {
        let dt = engine.performance.frame_time_ms / 1000.0;
        let bpm = engine.effective_bpm();
        let xr = if state.x_tempo_sync {
            beat_division_to_hz(state.x_beat_division, bpm)
        } else {
            engine.get_param("x_lfo_rate").unwrap_or(state.x_lfo_rate)
        };
        let yr = if state.y_tempo_sync {
            beat_division_to_hz(state.y_beat_division, bpm)
        } else {
            engine.get_param("y_lfo_rate").unwrap_or(state.y_lfo_rate)
        };
        let zr = if state.z_tempo_sync {
            beat_division_to_hz(state.z_beat_division, bpm)
        } else {
            engine.get_param("z_lfo_rate").unwrap_or(state.z_lfo_rate)
        };
        state.x_lfo_arg += xr * dt;
        state.y_lfo_arg += yr * dt;
        state.z_lfo_arg += zr * dt;
    }

    fn build_uniforms(&self, s: &SputnikState, engine: &EngineState) -> SputnikUniforms {
        let aspect = if engine.resolution.internal_height > 0 {
            engine.resolution.internal_width as f32 / engine.resolution.internal_height as f32
        } else {
            16.0 / 9.0
        };

        let displacement_scale = engine
            .get_param("displacement_scale")
            .unwrap_or(s.displacement_scale);
        let x_offset = engine.get_param("x_offset").unwrap_or(s.x_offset);
        let y_offset = engine.get_param("y_offset").unwrap_or(s.y_offset);
        let z_offset = engine.get_param("z_offset").unwrap_or(s.z_offset);
        let x_lfo_amp = engine.get_param("x_lfo_amp").unwrap_or(s.x_lfo_amp);
        let x_lfo_freq = engine.get_param("x_lfo_freq").unwrap_or(s.x_lfo_freq);
        let y_lfo_amp = engine.get_param("y_lfo_amp").unwrap_or(s.y_lfo_amp);
        let y_lfo_freq = engine.get_param("y_lfo_freq").unwrap_or(s.y_lfo_freq);
        let z_lfo_amp = engine.get_param("z_lfo_amp").unwrap_or(s.z_lfo_amp);
        let z_lfo_freq = engine.get_param("z_lfo_freq").unwrap_or(s.z_lfo_freq);
        let camera_distance = engine
            .get_param("camera_distance")
            .unwrap_or(s.camera_distance);
        let camera_tilt = engine.get_param("camera_tilt").unwrap_or(s.camera_tilt);
        let audio_reactivity = engine
            .get_param("audio_reactivity")
            .unwrap_or(s.audio_reactivity);

        let mut bands_a = [0.0f32; 4];
        let mut bands_b = [0.0f32; 4];
        for i in 0..4 {
            bands_a[i] = engine.audio.fft[i] * audio_reactivity;
            bands_b[i] = engine.audio.fft[i + 4] * audio_reactivity;
        }

        let projection = glam::Mat4::perspective_rh(60.0f32.to_radians(), aspect, 0.1, 100.0);
        let dist = camera_distance.max(0.1);
        let eye = glam::Vec3::new(0.0, camera_tilt.sin() * dist, camera_tilt.cos() * dist);
        let view = glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, glam::Vec3::Y);
        let mvp = projection * view;

        let tex_width = engine.resolution.input_width.max(1) as f32;
        let tex_height = engine.resolution.input_height.max(1) as f32;

        SputnikUniforms {
            displacement_scale,
            bright_invert: s.bright_invert as u32,
            x_offset,
            y_offset,
            audio_bands_a: bands_a,
            audio_bands_b: bands_b,
            x_lfo_arg: s.x_lfo_arg,
            x_lfo_amp,
            x_lfo_freq,
            x_lfo_shape: s.x_lfo_shape,
            y_lfo_arg: s.y_lfo_arg,
            y_lfo_amp,
            y_lfo_freq,
            y_lfo_shape: s.y_lfo_shape,
            z_lfo_arg: s.z_lfo_arg,
            z_lfo_amp,
            z_lfo_freq,
            z_lfo_shape: s.z_lfo_shape,
            x_phasemod: s.x_phasemod as u32,
            x_ringmod: s.x_ringmod as u32,
            y_phasemod: s.y_phasemod as u32,
            y_ringmod: s.y_ringmod as u32,
            z_phasemod: s.z_phasemod as u32,
            z_ringmod: s.z_ringmod as u32,
            tex_width,
            tex_height,
            z_offset,
            audio_reactivity,
            _pad0: [0; 2],
            mvp,
        }
    }
}

// ── GUI ────────────────────────────────────────────────────────────────────

struct SputnikTab;

const SHAPE_LABELS: [&str; 4] = ["Sine", "Square", "Sawtooth", "Noise"];

/// Draw a float parameter slider reading from / writing to engine state.
fn param_slider(
    ui: &imgui::Ui,
    engine: &mut EngineState,
    id: &str,
    label: &str,
    min: f32,
    max: f32,
) {
    let mut val = engine.get_param_base(id).unwrap_or(0.0);
    if ui.slider_config(label, min, max).build(&mut val) {
        engine.set_param_base(id, val);
    }
}

/// Compact LFO axis row: rate, amp, freq sliders.
fn lfo_axis_sliders(ui: &imgui::Ui, engine: &mut EngineState, label: &str, prefix: &str) {
    ui.text(label);
    param_slider(ui, engine, &format!("{prefix}_lfo_rate"), "Rate", 0.0, 10.0);
    param_slider(ui, engine, &format!("{prefix}_lfo_amp"), "Amp", 0.0, 1.0);
    param_slider(ui, engine, &format!("{prefix}_lfo_freq"), "Freq", 0.0, 20.0);
}

impl AnyGuiTab for SputnikTab {
    fn name(&self) -> &str {
        "Sputnik"
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        engine: &mut EngineState,
    ) {
        let s = app_state
            .downcast_mut::<SputnikState>()
            .expect("SputnikTab expects SputnikState");

        // ── Topology ──────────────────────────────────────────────────────
        ui.text("Topology");
        for (i, label) in ["Scanlines", "Triangles", "Wireframe", "Points"]
            .iter()
            .enumerate()
        {
            if ui.radio_button_bool(label, s.topology == i as u32) {
                s.topology = i as u32;
            }
            if i < 3 {
                ui.same_line();
            }
        }

        ui.separator();

        // ── Mesh resolution ───────────────────────────────────────────────
        ui.text("Mesh Resolution");
        let mut cols = s.mesh_cols as i32;
        let mut rows = s.mesh_rows as i32;
        ui.input_int("Columns", &mut cols).build();
        ui.input_int("Rows", &mut rows).build();
        s.mesh_cols = cols.max(1) as u32;
        s.mesh_rows = rows.max(1) as u32;

        ui.separator();

        // ── Offsets ───────────────────────────────────────────────────────
        ui.text("Offsets");
        param_slider(ui, engine, "x_offset", "X Offset", -2.0, 2.0);
        param_slider(ui, engine, "y_offset", "Y Offset", -2.0, 2.0);
        param_slider(ui, engine, "z_offset", "Z Offset", 0.0, 1.0);

        ui.separator();

        // ── Video displacement ────────────────────────────────────────────
        ui.text("Video Displacement");
        param_slider(ui, engine, "displacement_scale", "Displacement", 0.0, 2.0);
        ui.checkbox("Invert brightness", &mut s.bright_invert);

        ui.separator();

        // ── LFO Parameters ────────────────────────────────────────────────
        ui.text("LFO Parameters");
        lfo_axis_sliders(ui, engine, "X (horizontal)", "x");
        lfo_axis_sliders(ui, engine, "Y (vertical)", "y");
        lfo_axis_sliders(ui, engine, "Z (zoom pulse)", "z");

        ui.separator();

        // ── LFO Shape ─────────────────────────────────────────────────────
        ui.text("LFO Shape");
        for (label, shape, phasemod, ringmod) in [
            ("X", &mut s.x_lfo_shape, &mut s.x_phasemod, &mut s.x_ringmod),
            ("Y", &mut s.y_lfo_shape, &mut s.y_phasemod, &mut s.y_ringmod),
            ("Z", &mut s.z_lfo_shape, &mut s.z_phasemod, &mut s.z_ringmod),
        ] {
            ui.text(label);
            ui.same_line();
            for (i, &name) in SHAPE_LABELS.iter().enumerate() {
                if ui.radio_button_bool(format!("{name}##{label}shape{i}"), *shape == i as u32) {
                    *shape = i as u32;
                }
                if i < 3 {
                    ui.same_line();
                }
            }
            ui.checkbox(format!("Phase mod##{label}"), phasemod);
            ui.same_line();
            ui.checkbox(format!("Ring mod##{label}"), ringmod);
        }

        ui.separator();

        // ── Camera ────────────────────────────────────────────────────────
        ui.text("Camera");
        param_slider(ui, engine, "camera_distance", "Distance", 0.5, 10.0);
        param_slider(ui, engine, "camera_tilt", "Tilt", -1.0, 1.0);

        ui.separator();

        // ── Audio ─────────────────────────────────────────────────────────
        ui.text("Audio");
        param_slider(ui, engine, "audio_reactivity", "Reactivity", 0.0, 2.0);
    }
}

fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_default_env()
        .filter_module("wgpu_hal::metal", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("winit", log::LevelFilter::Warn)
        .filter_module("tracing::span", log::LevelFilter::Warn)
        .init();

    log::info!("Starting RustJay Sputnik v{}", env!("CARGO_PKG_VERSION"));
    let nogui = std::env::args().any(|a| a == "--nogui");
    if nogui {
        rustjay_engine::run_headless_with_tabs(SputnikEffect, vec![Box::new(SputnikTab)])
    } else {
        rustjay_engine::run_with_tabs(SputnikEffect, vec![Box::new(SputnikTab)])
    }
}
