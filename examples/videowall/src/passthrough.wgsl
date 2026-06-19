// Passthrough — render the live source into the main target. Each projector's
// MatrixStage then composites regions of this frame onto its wall.

@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var s_input: sampler;

// Engine binds the plugin's Uniforms here; unused but declared so the default
// pipeline layout matches.
struct Params { values: vec4<f32> }
@group(1) @binding(0) var<uniform> params: Params;

struct VertOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VertOut {
    var out: VertOut;
    out.clip = vec4<f32>(pos, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@fragment
fn fs_main(in: VertOut) -> @location(0) vec4<f32> {
    return textureSample(t_input, s_input, in.uv);
}
