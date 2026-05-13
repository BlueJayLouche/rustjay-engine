//! Sputnik — Indexed mesh + vertex-shader displacement.
//!
//! A Rutt-Etra style effect where video luminance displaces a dense grid of
//! vertices. Demonstrates `MeshDescriptor`, `vertex_reads_texture`, and
//! audio-reactive 3D mesh visuals.

use rustjay_engine::prelude::*;

struct SputnikEffect;

#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct SputnikUniforms {
    displacement_scale: f32,
    rotation: f32,
    zoom: f32,
    aspect_ratio: f32,
    audio_bands: [f32; 8],
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SputnikState {
    mesh_cols: u32,
    mesh_rows: u32,
    topology: u32,
    displacement_scale: f32,
    rotation: f32,
    zoom: f32,
    audio_band_weights: [f32; 8],
}

impl EffectPlugin for SputnikEffect {
    type State = SputnikState;
    type Uniforms = SputnikUniforms;

    fn app_name(&self) -> &str {
        "sputnik"
    }

    fn default_state(&self) -> SputnikState {
        SputnikState {
            mesh_cols: 320,
            mesh_rows: 180,
            topology: 0,
            displacement_scale: 0.3,
            rotation: 0.0,
            zoom: 1.0,
            audio_band_weights: [0.0; 8],
        }
    }

    fn shader_source(&self) -> &'static str {
        include_str!("shaders/sputnik.wgsl")
    }

    fn mesh_descriptor(&self, state: &SputnikState) -> Option<MeshDescriptor> {
        let topology = if state.topology == 0 {
            MeshTopology::Scanlines
        } else {
            MeshTopology::Triangles
        };
        Some(MeshDescriptor {
            cols: state.mesh_cols,
            rows: state.mesh_rows,
            topology,
        })
    }

    fn vertex_reads_texture(&self) -> bool {
        true
    }

    fn build_uniforms(&self, s: &SputnikState, engine: &EngineState) -> SputnikUniforms {
        let aspect = if engine.resolution.internal_height > 0 {
            engine.resolution.internal_width as f32 / engine.resolution.internal_height as f32
        } else {
            16.0 / 9.0
        };

        let mut bands = [0.0f32; 8];
        for i in 0..8 {
            bands[i] = engine.audio.fft[i] * s.audio_band_weights[i];
        }

        SputnikUniforms {
            displacement_scale: s.displacement_scale,
            rotation: s.rotation,
            zoom: s.zoom,
            aspect_ratio: aspect,
            audio_bands: bands,
        }
    }
}

struct SputnikTab;

impl AnyGuiTab for SputnikTab {
    fn name(&self) -> &str {
        "Sputnik"
    }

    fn draw(
        &mut self,
        ui: &imgui::Ui,
        app_state: &mut dyn std::any::Any,
        _engine: &mut EngineState,
    ) {
        let state = app_state
            .downcast_mut::<SputnikState>()
            .expect("SputnikTab expects SputnikState");

        ui.text("Mesh Displacement");
        ui.separator();

        // Topology radio buttons
        ui.text("Topology");
        let scanlines = state.topology == 0;
        let triangles = state.topology == 1;
        if ui.radio_button_bool("Scanlines", scanlines) {
            state.topology = 0;
        }
        if ui.radio_button_bool("Triangles", triangles) {
            state.topology = 1;
        }

        ui.separator();

        // Mesh resolution
        ui.text("Mesh Resolution (triggers rebuild on change)");
        let mut cols = state.mesh_cols as i32;
        let mut rows = state.mesh_rows as i32;
        ui.input_int("Columns", &mut cols).build();
        ui.input_int("Rows", &mut rows).build();
        state.mesh_cols = cols.max(1) as u32;
        state.mesh_rows = rows.max(1) as u32;

        ui.separator();

        ui.slider_config("Displacement", 0.0, 2.0)
            .build(&mut state.displacement_scale);
        ui.slider_config("Rotation", -3.14, 3.14)
            .build(&mut state.rotation);
        ui.slider_config("Zoom", 0.1, 3.0)
            .build(&mut state.zoom);

        ui.separator();
        ui.text("Audio Band Weights");
        for i in 0..8 {
            let label = format!("Band {}", i + 1);
            ui.slider_config(&label, 0.0, 2.0)
                .build(&mut state.audio_band_weights[i]);
        }
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

    rustjay_engine::run_with_tabs(SputnikEffect, vec![Box::new(SputnikTab)])
}
