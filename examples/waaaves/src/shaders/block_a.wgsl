// Block A — Feedback mix + warp distortion

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

struct WaaavesUniforms {
    feedback_amount: f32,
    warp_amount: f32,
    blur_amount: f32,
    hue_shift: f32,
    saturation: f32,
    brightness: f32,
    trail_decay: f32,
    mix_original: f32,
};

@group(0) @binding(0)
var input_tex: texture_2d<f32>;
@group(0) @binding(1)
var input_sampler: sampler;
@group(0) @binding(2)
var feedback_tex: texture_2d<f32>;
@group(0) @binding(3)
var feedback_sampler: sampler;

@group(1) @binding(0)
var<uniform> u: WaaavesUniforms;

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

    // Sample input and feedback
    let input_color = textureSample(input_tex, input_sampler, uv);
    let feedback_color = textureSample(feedback_tex, feedback_sampler, uv);

    // Mix feedback with input
    var color = mix(input_color, feedback_color, u.feedback_amount);

    // Warp distortion
    if u.warp_amount > 0.0 {
        let center = uv - 0.5;
        let dist = length(center);
        let angle = atan2(center.y, center.x);
        let warp = sin(dist * 10.0) * u.warp_amount * 0.05;
        let warped_uv = uv + vec2<f32>(
            cos(angle) * warp,
            sin(angle) * warp,
        );
        let warped_color = textureSample(input_tex, input_sampler, warped_uv);
        color = mix(color, warped_color, u.warp_amount);
    }

    return color;
}
