// Passthrough stub — the engine compiles this but IsfEffect::render() takes over.
@group(0) @binding(0) var t_input: texture_2d<f32>;
@group(0) @binding(1) var s_input: sampler;

struct VertIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv:  vec2<f32>,
}

struct VertOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(in: VertIn) -> VertOut {
    var out: VertOut;
    out.clip = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv   = in.uv;
    return out;
}

@fragment
fn fs_main(in: VertOut) -> @location(0) vec4<f32> {
    return textureSample(t_input, s_input, in.uv);
}
