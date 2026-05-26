# Sputnik — Rutt-Etra Mesh Displacement

`examples/sputnik` is a [Rutt-Etra](https://en.wikipedia.org/wiki/Rutt/Etra_Video_Synthesizer)-style effect where video luminance pushes a dense grid of vertices into 3D space. The brighter a pixel, the further its mesh vertex is displaced toward the viewer. With a moving camera and animated LFOs warping the grid before the luminance pass, the result is a continuously morphing 3D wireframe portrait of the video signal.

```sh
cargo run -p sputnik
```

This is the most technically involved single-pass example. It demonstrates `MeshDescriptor`, `vertex_reads_texture`, `compute_shader`, per-axis LFO phase accumulation in `prepare()`, tempo sync, and an eight-band audio reactivity system.

## What it does

Video is not rendered as a flat rectangle. Instead, a dense grid of vertices — 320 columns × 180 rows by default (57,600 vertices) — is displaced in two stages every frame:

1. **Compute pass** — the LFO system warps the flat grid into undulating 3D shapes before any video is involved
2. **Vertex pass** — each vertex samples the video texture, converts its pixel to luminance, and displaces further along Y and Z by that brightness value

The fragment shader then samples the same texture at each vertex's UV coordinate, painting the displaced mesh with the live video. The result combines spatial distortion from the LFOs with luminance-driven depth from the image itself.

## The two-pass GPU pipeline

### Pass 1 — compute shader

The compute shader runs once per vertex (`@workgroup_size(256, 1, 1)`) before the render pass. It reads and writes a `storage` vertex buffer:

```wgsl
@group(0) @binding(0) var<uniform>             u:        SputnikUniforms;
@group(1) @binding(0) var<storage, read_write> vertices: array<Vertex>;
```

For each vertex it:

1. Reconstructs the base XY position from the vertex's UV coordinate, aspect-corrected to match the input texture
2. Evaluates three independent LFO values — X (horizontal), Y (vertical), Z (distance from UV centre)
3. Optionally applies **phase modulation** (adds a neighbour axis's raw output to the phase argument before re-evaluating the waveform — FM-style cross-axis patterns)
4. Optionally applies **ring modulation** (multiplies X or Y displacement by the Z value)
5. Applies the Z LFO as a **zoom-pulse**: scales the base XY position by `(1 - z_total)`, then adds the X/Y displacements
6. Writes the displaced `position` back to the storage buffer

The base position is always reconstructed from the UV coordinate, never accumulated — this prevents runaway drift across frames.

### Pass 2 — vertex + fragment shader

The vertex shader reads the compute-displaced position and adds a second displacement layer from the video:

```wgsl
// textureSampleLevel is required in vertex stage (no screen-space derivatives)
let color = textureSampleLevel(input_tex, input_sampler, texcoord, 0.0);
var bright = dot(color.rgb, vec3<f32>(0.299, 0.587, 0.114));
if u.bright_invert != 0u { bright = 1.0 - bright; }

// Logarithmic scaling — matches the original sputnikMesh feel
bright = 2.0 * log(1.0 + bright);

let displacement = (bright + audio_lift) * u.displacement_scale;
let pos3 = vec3<f32>(position.x, position.y + displacement, displacement * 0.5);

out.position = u.mvp * vec4<f32>(pos3, 1.0);
```

The logarithmic curve (`2 × log(1 + luma)`) gives the effect the same feel as the original openFrameworks `sputnikMesh`: shadow regions stay relatively flat while bright regions punch sharply forward.

Audio adds a second lift to the displacement, mapped across 8 frequency bands — each column of the mesh is biased by the band that corresponds to its horizontal position.

The fragment shader is trivial — it just samples the texture at the vertex UV and returns the colour.

## Declaring the mesh

```rust
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

fn compute_shader(&self) -> Option<&'static str> {
    Some(include_str!("shaders/sputnik_compute.wgsl"))
}
```

`vertex_reads_texture()` returning `true` tells the engine to bind the input texture at group 0 binding 0 during the vertex stage — by default, vertex stage texture access is not enabled.

`compute_shader()` returning `Some(...)` causes the engine to run that compute dispatch before the render pass each frame.

## Topology modes

| Mode | Appearance |
|---|---|
| **Scanlines** | Horizontal line strips — the classic Rutt-Etra look |
| **Triangles** | Filled mesh — video as a 3D surface |
| **Wireframe** | Mesh edges — structural/architectural feel |
| **Points** | One dot per vertex — particle-cloud style |

The mesh resolution (columns × rows) can be changed at runtime from the Sputnik tab. Higher values produce finer detail at the cost of vertex count.

## The three-axis LFO system

Each axis has an independent LFO that runs at frame-rate-accurate speed regardless of render framerate. Four waveforms are available: Sine, Square, Sawtooth, and Noise.

| Axis | Effect |
|---|---|
| **X** | Horizontal displacement — waves the columns left/right |
| **Y** | Vertical displacement — waves the rows up/down |
| **Z** | Scales the base XY position — zoom-pulse expanding from the centre |

Each axis has three parameters:

| Parameter | Description |
|---|---|
| **Rate** | LFO speed in Hz (or beat division when tempo-sync is on) |
| **Amp** | Displacement amplitude |
| **Freq** | Spatial frequency — how many cycles fit across the mesh |

The spatial frequency parameter is key: `lfo_freq = 0` produces a uniform wave across the whole mesh, while higher values create many small oscillations across the surface.

### Phase accumulation in `prepare()`

The LFO phase accumulators are advanced in `prepare()`, which runs once per frame before `build_uniforms()`:

```rust
fn prepare(&mut self, state: &mut SputnikState, engine: &EngineState, ...) {
    let dt  = engine.performance.frame_time_ms / 1000.0;
    let bpm = engine.effective_bpm();

    let xr = if state.x_tempo_sync {
        beat_division_to_hz(state.x_beat_division, bpm)
    } else {
        engine.get_param("x_lfo_rate").unwrap_or(state.x_lfo_rate)
    };
    // ... same for y, z ...

    state.x_lfo_arg += xr * dt;
    state.y_lfo_arg += yr * dt;
    state.z_lfo_arg += zr * dt;
}
```

`frame_time_ms / 1000.0` converts the engine's elapsed frame time to seconds. Multiplying by rate (Hz) gives the correct phase increment regardless of how fast or slowly the GPU is rendering.

The phase accumulators are marked `#[serde(skip)]` in `SputnikState` — they reset to zero when a preset is loaded (intentional: resuming from a saved snapshot with stale phase values would look wrong).

### Tempo sync

Setting `x_tempo_sync = true` replaces the freerunning `x_lfo_rate` Hz value with a rate derived from the global BPM:

```rust
beat_division_to_hz(state.x_beat_division, bpm)
```

`x_beat_division` indexes a table of musical subdivisions (whole note, half, quarter, eighth, etc.). The LFO completes one cycle every N beats, staying locked to the track tempo.

### Phase and ring modulation

**Phase modulation** adds a neighbouring axis's raw output to the LFO's phase argument before evaluating the waveform:

```wgsl
if u.x_phasemod != 0u {
    x_lfo = lfo(u.x_lfo_arg + tc.x * u.x_lfo_freq + y_raw, u.x_lfo_shape);
}
```

X is phase-modulated by Y, Y by X, Z by X. With both axes at Sine waveform this produces FM-style Lissajous patterns.

**Ring modulation** multiplies the X or Y displacement by the current Z LFO value. At low Z amplitude this creates a subtle amplitude envelope across the mesh; pushed hard it produces sharp nodal bands.

## Audio reactivity

`audio_reactivity` scales how strongly the audio spectrum lifts the mesh displacement:

```rust
for i in 0..4 {
    bands_a[i] = engine.audio.fft[i]     * audio_reactivity;
    bands_b[i] = engine.audio.fft[i + 4] * audio_reactivity;
}
```

Eight frequency bands (from `engine.audio.fft`) are passed to the vertex shader in two `vec4` uniforms. Each column of the mesh maps to one of the eight bands based on its horizontal UV coordinate — so bass frequencies affect the left side and treble frequencies affect the right side:

```wgsl
let band_idx  = clamp(u32(texcoord.x * 8.0), 0u, 7u);
let audio_lift = bands[band_idx] * u.audio_reactivity;
```

Combined with video luminance, audio lift gives you a mesh that pulses in time with the music and also reveals the structure of the video.

## Camera

The MVP matrix is built each frame in `build_uniforms()`:

```rust
let projection = glam::Mat4::perspective_rh(60.0f32.to_radians(), aspect, 0.1, 100.0);
let eye = glam::Vec3::new(
    0.0,
    camera_tilt.sin() * dist,
    camera_tilt.cos() * dist,
);
let view = glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, glam::Vec3::Y);
let mvp  = projection * view;
```

| Parameter | Range | Default | Description |
|---|---|---|---|
| **Camera Dist** | 0.5–10 | 3.0 | Distance from origin — zoom in/out |
| **Camera Tilt** | −1–1 | 0.0 | Vertical orbit around origin in radians |

Both parameters are exposed to the engine's LFO and audio routing system via `ParameterDescriptor`, so you can automate a slow orbit or sync a camera tilt to the beat.

## Parameters

All parameters live in `ParamCategory::Custom("Sputnik")` and appear in the **Sputnik** tab.

| Parameter | Range | Default | Description |
|---|---|---|---|
| `displacement_scale` | 0–2 | 0.3 | Overall luminance displacement depth |
| `x_offset` | −2–2 | 0.0 | Horizontal grid offset before LFOs |
| `y_offset` | −2–2 | 0.0 | Vertical grid offset before LFOs |
| `z_offset` | 0–1 | 0.0 | Static zoom offset (Z axis) |
| `x/y/z_lfo_rate` | 0–10 | 1.0/0.7/0.3 | LFO speed in Hz |
| `x/y/z_lfo_amp` | 0–1 | 0.1/0.05/0.0 | LFO amplitude |
| `x/y/z_lfo_freq` | 0–20 | 2.0/3.0/1.0 | Spatial frequency across the mesh |
| `camera_distance` | 0.5–10 | 3.0 | Camera distance from origin |
| `camera_tilt` | −1–1 | 0.0 | Camera vertical orbit |
| `audio_reactivity` | 0–2 | 0.0 | Audio spectrum lift scale |

The topology (Scanlines/Triangles/Wireframe/Points), mesh resolution (columns/rows), LFO shapes, phase/ring mod flags, invert brightness, and tempo-sync settings are state fields controlled from the Sputnik tab directly — they're not declared as `ParameterDescriptor` entries because they are discrete choices rather than continuous values.

## The Sputnik tab

`SputnikTab` adds a new tab named `"Sputnik"` without replacing any built-in tab:

```rust
impl AnyGuiTab for SputnikTab {
    fn name(&self) -> &str { "Sputnik" }
    // no replaces() — adds alongside the existing tabs
    fn draw(&mut self, ui: &imgui::Ui, app_state: &mut dyn Any, engine: &mut EngineState) {
        let s = app_state.downcast_mut::<SputnikState>().unwrap();
        // topology radio buttons, mesh resolution inputs, LFO sliders, ...
    }
}
```

The `Motion` tab is explicitly hidden since sputnik manages its own motion controls:

```rust
fn hidden_tabs(&self) -> Vec<GuiTab> {
    vec![GuiTab::Motion]
}
```

The tab uses `lfo_axis_sliders()` — a local helper that draws Rate, Amp, and Freq sliders for one axis in one go — to keep the LFO section compact.

## Starting point for mesh effects

To build a different mesh displacement effect from sputnik:

1. Return `Some(MeshDescriptor { cols, rows, topology })` from `mesh_descriptor()`
2. If the vertex shader needs to read the video texture, return `true` from `vertex_reads_texture()`
3. If pre-frame vertex transformation is needed, supply a compute shader via `compute_shader()`
4. Accumulate phase or other per-frame state in `prepare()` using `engine.performance.frame_time_ms`
5. The MVP pattern (perspective × look_at) works for any 3D mesh effect — adjust `eye`, `center`, and `up` vectors for your camera behaviour
