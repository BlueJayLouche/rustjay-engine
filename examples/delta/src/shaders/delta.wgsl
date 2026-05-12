// Delta — RGB spatial delay / motion extraction
// Each channel is sampled from a spatially offset position,
// creating a chromatic aberration / motion trail effect.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

struct DeltaUniforms {
    // delay per channel (scaled to UV offset in shader)
    delay_r: f32,
    delay_g: f32,
    delay_b: f32,
    mix_amount: f32,
};

@group(0) @binding(0)
var input_tex: texture_2d<f32>;
@group(0) @binding(1)
var input_sampler: sampler;

@group(1) @binding(0)
var<uniform> uniforms: DeltaUniforms;

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) texcoord: vec2<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.texcoord;

    // Scale delay params to meaningful UV offsets
    let scale = 0.02;
    let r_uv = uv + vec2<f32>(uniforms.delay_r * scale, 0.0);
    let g_uv = uv + vec2<f32>(uniforms.delay_g * scale, 0.0);
    let b_uv = uv + vec2<f32>(uniforms.delay_b * scale, 0.0);

    // Sample each channel from its offset position
    let r = textureSample(input_tex, input_sampler, r_uv).r;
    let g = textureSample(input_tex, input_sampler, g_uv).g;
    let b = textureSample(input_tex, input_sampler, b_uv).b;
    let orig = textureSample(input_tex, input_sampler, uv);

    let delayed = vec4<f32>(r, g, b, orig.a);
    return mix(orig, delayed, uniforms.mix_amount);
}
