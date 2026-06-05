struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((vertex_index & 1u) << 2u);
    let y = f32((vertex_index & 2u) << 1u);
    out.position = vec4<f32>(x - 1.0, 1.0 - y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5, y * 0.5);
    return out;
}

struct EdgeBlendParams {
    left_enabled: f32,
    left_width: f32,
    left_gamma: f32,
    _pad0: f32,
    right_enabled: f32,
    right_width: f32,
    right_gamma: f32,
    _pad1: f32,
    top_enabled: f32,
    top_width: f32,
    top_gamma: f32,
    _pad2: f32,
    bottom_enabled: f32,
    bottom_width: f32,
    bottom_gamma: f32,
    _pad3: f32,
}

@group(0) @binding(0) var texture_sampler: sampler;
@group(0) @binding(1) var source_texture: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: EdgeBlendParams;

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(source_texture, texture_sampler, in.uv);
    var alpha = 1.0;

    if (params.left_enabled > 0.5) {
        let t = clamp(in.uv.x / params.left_width, 0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        alpha *= pow(s, params.left_gamma);
    }
    if (params.right_enabled > 0.5) {
        let t = clamp((1.0 - in.uv.x) / params.right_width, 0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        alpha *= pow(s, params.right_gamma);
    }
    if (params.top_enabled > 0.5) {
        let t = clamp(in.uv.y / params.top_width, 0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        alpha *= pow(s, params.top_gamma);
    }
    if (params.bottom_enabled > 0.5) {
        let t = clamp((1.0 - in.uv.y) / params.bottom_width, 0.0, 1.0);
        let s = t * t * (3.0 - 2.0 * t);
        alpha *= pow(s, params.bottom_gamma);
    }

    return vec4<f32>(color.rgb * alpha, color.a);
}
