# ISF Shader Viewer

`examples/isf-example` loads any [ISF](https://isf.video/) (Interactive Shader Format) shader at runtime, parses its input declarations, and auto-generates the parameter UI — no Rust code required per shader.

```sh
cargo run -p isf-example
```

The engine starts immediately with the last-loaded shader. On first launch it defaults to the bundled `ColorCycle.fs`. Use the **Load Shader...** button inside the control window to pick any `.fs` or `.frag` file — the shader swaps within one frame and the control tab updates its name and sliders to match.

## What is ISF?

ISF is an open standard that wraps a GLSL fragment shader in a JSON header declaring its inputs:

```glsl
/*{
    "CREDIT": "by VIDVOX",
    "ISFVSN": "2",
    "CATEGORIES": ["Glitch", "Retro"],
    "INPUTS": [
        { "NAME": "inputImage", "TYPE": "image" },
        {
            "NAME": "noiseLevel",
            "TYPE": "float",
            "MIN": 0.0, "MAX": 1.0, "DEFAULT": 0.5
        },
        {
            "NAME": "rollAmount",
            "TYPE": "float",
            "MIN": -1.0, "MAX": 1.0, "DEFAULT": 0.0
        }
    ]
}*/

void main() {
    // ... GLSL fragment body ...
}
```

The JSON header is everything between `/*{` and `}*/`. The rest is plain GLSL. ISF is supported natively by Resolume, VDMX, and many other VJ tools — there is a large community library at [editor.isf.video](https://editor.isf.video/).

## Bundled shaders

The example ships with ~50 shaders in `examples/isf-example/shaders/`. They cover a range of styles: generative patterns, video processing, glitch, and feedback effects. Good ones to start with:

| File | What it does |
|---|---|
| `Bad TV.fs` | VHS noise, roll, and scan-line glitch (requires video input) |
| `WarpTunnel.fs` | Psychedelic tunnel with speed and twist controls |
| `GlitchBlocks.fs` | Block-shift RGB glitch |
| `FractalZoom.fs` | Continuous fractal zoom |
| `AuroraWaves.fs` | Flowing aurora borealis effect |
| `NeonPulse.fs` | Neon glow rings |
| `CellularLife.fs` | Conway-style cellular automata |

## How the viewer works

### Startup

On launch the viewer:

1. Reads `~/.config/rustjay/isf-last-shader.txt` — if it exists and the file is still on disk, that shader is loaded
2. Falls back to the bundled `ColorCycle.fs` on first launch or if the saved path is gone
3. Parses the ISF JSON header with the `isf` crate
4. Builds a `Vec<ParameterDescriptor>` from the declared inputs — these drive the auto-generated UI and the engine's LFO / MIDI / OSC systems
5. Creates an `IsfEffect` that owns the parsed data and starts the engine

### Switching shaders at runtime

The **Load Shader...** button at the top of the ISF tab opens a native file picker. Picking a new file writes the path into a shared `Arc<Mutex<Option<PathBuf>>>`. On the next `prepare()` call the effect picks up the path, rebuilds the pipeline, and signals the engine to refresh the parameter list via `parameters_dirty()`. The tab label hot-reloads to the new shader's filename within one frame.

The chosen path is saved to `~/.config/rustjay/isf-last-shader.txt` so the next launch picks up where you left off.

### File hot-reload

While the app is running, edit any `.fs` file in your editor and save. `IsfEffect::prepare()` polls the file's mtime each frame and re-transpiles automatically when it changes — no button press needed. Useful for iterating on shader code live.

### Transpilation

GLSL can't run on wgpu directly — the engine transpiles it to WGSL at startup inside `IsfEffect::init()`. The transpiler (`isf_transpiler.rs`) handles:

- Stripping the ISF JSON comment header to get raw GLSL
- Both entry-point patterns: `void main()` writing to `gl_FragColor`, and the Shadertoy-compatible `void mainImage(out vec4 fragColor, in vec2 fragCoord)` form
- Mapping ISF built-ins to WGSL equivalents:

| GLSL / ISF built-in | WGSL equivalent |
|---|---|
| `TIME` | `u.TIME` (f32 seconds since launch) |
| `RENDERSIZE` | `u.RENDERSIZE` (vec2 — output resolution) |
| `gl_FragCoord` | `in.position` |
| `isf_FragNormCoord` | `in.texcoord` |
| `texture2D(tex, uv)` | `textureSample(t_input, s_input, uv)` |
| `vec2`, `vec3`, `vec4` | `vec2<f32>`, `vec3<f32>`, `vec4<f32>` |

- Rewriting GLSL type constructors (`vec2(x, y)` → `vec2<f32>(x, y)`)
- Declaring all scalar ISF inputs as a flat `array<f32, 64>` uniform buffer

### Auto-generated parameters

Each ISF input type maps to a parameter kind:

| ISF type | Widget | Notes |
|---|---|---|
| `float` | Slider | Uses `MIN`/`MAX`/`DEFAULT` from the header |
| `bool` | Checkbox | |
| `long` (int) | Integer slider | |
| `image` | _(none)_ | Bound to the engine's live video input automatically |

Parameters declared this way become full first-class engine parameters: they can be targeted by LFOs, mapped to MIDI CC, addressed over OSC, and saved in presets — without any extra code.

### Custom pipeline

Because ISF's binding layout differs from the engine's standard layout, the viewer uses a custom render pipeline (see [Frame History & Custom Pipelines](../rendering/frame-history.md) for the general pattern):

- `shader_source()` returns a minimal passthrough stub — the engine compiles it but never runs it
- `init()` compiles the real transpiled WGSL pipeline
- `render()` uploads uniforms, builds the bind groups, runs the pass, and returns `true`

## Known limitations

The transpiler handles the most common ISF patterns but some shaders won't load:

- **Function overloading** — GLSL allows multiple functions with the same name and different types; WGSL does not. Shaders that rely on this will fail to compile and show an error in the shader name tab.
- **Multi-pass ISF** (`PASSES` array) — not supported; only single-pass shaders work.
- **`color` and `point2D` inputs** — parsed but not yet wired to UI controls (Phase 1 scope).
- **`audio` inputs** — not supported.

If a shader fails, the output window shows black and the tab name displays the error message.

## Getting more shaders

The ISF community library at **[editor.isf.video](https://editor.isf.video/)** has hundreds of free shaders. Download any `.fs` file and open it with the file picker. Shaders tagged `generator` run without video input; shaders tagged `filter` or `transition` expect a live video source connected in the Input tab.

Shadertoy shaders often work too if they use the `mainImage` entry-point pattern — download the GLSL, add a minimal ISF header, and load it:

```glsl
/*{
    "ISFVSN": "2",
    "INPUTS": []
}*/

void mainImage(out vec4 fragColor, in vec2 fragCoord) {
    // paste Shadertoy code here
    // iTime → TIME, iResolution → RENDERSIZE, iChannel0 → inputImage
}
```
