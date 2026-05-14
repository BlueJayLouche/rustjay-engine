// Sputnik compute shader — LFO mesh displacement.
//
// Runs once per vertex before the render pass.  Reconstructs the base
// position from texcoord each frame (storage buffer persists across frames,
// so we must never accumulate) then applies the three-axis LFO system
// matching the original sputnikMesh (openFrameworks) design:
//
//   X / Y  — independent horizontal / vertical displacement
//   Z      — scales the XY position (zoom-pulse effect)
//
// Each axis supports four waveforms, optional phase modulation (the
// previous axis's raw value is added to the phase argument), and optional
// ring modulation (output multiplied by Z).

struct Vertex {
    position: vec2<f32>,
    texcoord: vec2<f32>,
}

// Must match SputnikUniforms in main.rs exactly (192 bytes).
struct SputnikUniforms {
    displacement_scale: f32,
    bright_invert:      u32,
    pad0:               u32,
    pad1:               u32,

    audio_bands_a: vec4<f32>,
    audio_bands_b: vec4<f32>,

    x_lfo_arg:   f32,
    x_lfo_amp:   f32,
    x_lfo_freq:  f32,
    x_lfo_shape: u32,

    y_lfo_arg:   f32,
    y_lfo_amp:   f32,
    y_lfo_freq:  f32,
    y_lfo_shape: u32,

    z_lfo_arg:   f32,
    z_lfo_amp:   f32,
    z_lfo_freq:  f32,
    z_lfo_shape: u32,

    x_phasemod: u32,
    x_ringmod:  u32,
    y_phasemod: u32,
    y_ringmod:  u32,

    z_phasemod: u32,
    z_ringmod:  u32,
    pad2:       u32,
    pad3:       u32,

    mvp: mat4x4<f32>,
}

@group(0) @binding(0) var<uniform>            u:        SputnikUniforms;
@group(1) @binding(0) var<storage, read_write> vertices: array<Vertex>;

// Evaluate one LFO sample.
// arg   = lfo_arg + spatial_position * lfo_freq  (changes every frame)
// shape = 0 sine | 1 square | 2 sawtooth | 3 noise
fn lfo(arg: f32, shape: u32) -> f32 {
    switch shape {
        case 0u {
            return sin(arg);
        }
        case 1u {
            return select(-1.0, 1.0, sin(arg) >= 0.0);
        }
        case 2u {
            return fract(arg / (2.0 * 3.14159265)) * 2.0 - 1.0;
        }
        default {
            // Hash-noise: spatially varying and animated because arg changes
            // each frame as the accumulator advances.
            return fract(sin(arg * 127.1 + 311.7) * 43758.5453) * 2.0 - 1.0;
        }
    }
}

@compute @workgroup_size(256, 1, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if index >= arrayLength(&vertices) {
        return;
    }

    let v  = &vertices[index];
    let tc = (*v).texcoord;

    // Reconstruct base position from texcoord — matches generate_mesh_data().
    let base_x = tc.x * 2.0 - 1.0;
    let base_y = 1.0 - tc.y * 2.0;

    // Distance from UV centre for Z spatial modulation.
    let dist = length(tc - vec2<f32>(0.5, 0.5));

    // First pass: raw LFO values (spatial position + current phase).
    let x_raw = lfo(u.x_lfo_arg + tc.x * u.x_lfo_freq, u.x_lfo_shape);
    let y_raw = lfo(u.y_lfo_arg + tc.y * u.y_lfo_freq, u.y_lfo_shape);
    let z_raw = lfo(u.z_lfo_arg + dist  * u.z_lfo_freq, u.z_lfo_shape);

    // Second pass: phase modulation (add neighbouring axis's raw value to
    // the phase before re-evaluating the waveform — creates complex FM-style
    // patterns from simple waveforms).
    var x_lfo = x_raw;
    var y_lfo = y_raw;
    var z_lfo = z_raw;
    if u.x_phasemod != 0u {
        x_lfo = lfo(u.x_lfo_arg + tc.x * u.x_lfo_freq + y_raw, u.x_lfo_shape);
    }
    if u.y_phasemod != 0u {
        y_lfo = lfo(u.y_lfo_arg + tc.y * u.y_lfo_freq + x_raw, u.y_lfo_shape);
    }
    if u.z_phasemod != 0u {
        z_lfo = lfo(u.z_lfo_arg + dist  * u.z_lfo_freq + x_raw, u.z_lfo_shape);
    }

    // Scale by per-axis amplitude.
    var x_disp  = x_lfo * u.x_lfo_amp;
    var y_disp  = y_lfo * u.y_lfo_amp;
    let z_scale = z_lfo * u.z_lfo_amp;

    // Ring modulation: multiply X/Y displacement by Z value.
    if u.x_ringmod != 0u { x_disp = x_disp * z_lfo; }
    if u.y_ringmod != 0u { y_disp = y_disp * z_lfo; }

    // Z LFO scales the base XY position — zoom-pulse effect.
    // (Mirrors original: newPosition.xy *= (1.0 - zLfo))
    var pos = vec2<f32>(base_x, base_y) * (1.0 - z_scale);
    pos.x += x_disp;
    pos.y += y_disp;

    (*v).position = pos;
}
