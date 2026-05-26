# Single-Pass Effects

The default rendering mode is a single fullscreen pass: the engine draws two triangles covering the screen, runs your fragment shader once per pixel, and writes to the output window.

This covers a large class of effects:
- Colour grading (HSB, LUT, tinting)
- Kaleidoscope, mirror, tile
- Blur, sharpen, edge detection
- Displacement, wave distortion
- Noise overlays, grain, glitch
- Generative patterns (no video input)

## How it works

Each frame the engine:
1. Calls `build_uniforms()` and uploads the result to `@group(1) @binding(0)`
2. Binds the current video frame at `@group(0) @binding(0/1)`
3. Draws the fullscreen quad with your shader
4. Presents the result

You control the output entirely through your fragment shader and your uniform values.

## A complete single-pass example

`examples/template` is the canonical reference: HSB colour adjustment with audio reactivity, LFO targets, and MIDI/OSC/web control in ~80 lines.

```sh
cargo run -p template
```

Key points from its implementation:

```rust
// Declare parameters → auto UI, LFO targets, MIDI learn
fn parameters(&self) -> Vec<ParameterDescriptor> {
    vec![
        ParameterDescriptor::float("hue_shift",  "Hue Shift",  ParamCategory::Color, -180.0, 180.0, 0.0, 1.0),
        ParameterDescriptor::float("saturation", "Saturation", ParamCategory::Color, 0.0, 2.0, 1.0, 0.01),
        ParameterDescriptor::float("brightness", "Brightness", ParamCategory::Color, 0.0, 2.0, 1.0, 0.01),
    ]
}

// get_param() returns base + LFO + audio routing contributions
fn build_uniforms(&self, s: &HsbState, engine: &EngineState) -> HsbUniforms {
    HsbUniforms { values: [
        engine.get_param("hue_shift").unwrap_or(s.hue_shift),
        engine.get_param("saturation").unwrap_or(s.saturation),
        engine.get_param("brightness").unwrap_or(s.brightness),
        0.0, // padding
    ]}
}
```

The WGSL shader does the actual colour work:

```wgsl
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var col = textureSample(input_tex, input_sampler, in.texcoord);
    // apply hue rotation, saturation scale, brightness scale ...
    return col;
}
```

## When single-pass isn't enough

- **Frame history** — you need to read from a previous frame, or accumulate across multiple frames → [Frame History & Custom Pipelines](frame-history.md)
- **Multiple stages** — blur then mix, or feedback loop → [Multi-Pass with RenderGraph](render-graph.md)
- **Vertex displacement** — displace a mesh in 3D space from a texture → [Mesh Displacement](mesh-displacement.md)
