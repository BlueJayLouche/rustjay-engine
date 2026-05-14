struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

@group(0) @binding(0)
var webcam_tex: texture_2d<f32>;
@group(0) @binding(1)
var webcam_sampler: sampler;
@group(0) @binding(2)
var feedback_tex: texture_2d<f32>;
@group(0) @binding(3)
var feedback_sampler: sampler;

struct Uniforms {
    delay_r: f32,
    delay_g: f32,
    delay_b: f32,
    mix_amount: f32,
    resolution: vec2<f32>,
};

@group(1) @binding(0)
var<uniform> u: Uniforms;

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

    // Convert pixel delays to UV offsets
    let offset_r = vec2<f32>(u.delay_r / u.resolution.x, 0.0);
    let offset_g = vec2<f32>(u.delay_g / u.resolution.x, 0.0);
    let offset_b = vec2<f32>(u.delay_b / u.resolution.x, 0.0);

    // Sample previous frame (feedback) with per-channel spatial offsets
    let r = textureSample(feedback_tex, feedback_sampler, uv + offset_r).r;
    let g = textureSample(feedback_tex, feedback_sampler, uv + offset_g).g;
    let b = textureSample(feedback_tex, feedback_sampler, uv + offset_b).b;
    let delayed = vec4<f32>(r, g, b, 1.0);

    // Sample live webcam
    let live = textureSample(webcam_tex, webcam_sampler, uv);

    // Blend live vs delayed
    return mix(live, delayed, u.mix_amount);
}
