// Block B — Blur + trail decay

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
    let texel = 1.0 / vec2<f32>(textureDimensions(input_tex));

    var color = textureSample(input_tex, input_sampler, uv);

    // Simple box blur
    if u.blur_amount > 0.0 {
        let offset = texel * u.blur_amount * 3.0;
        var accum = color;
        accum = accum + textureSample(input_tex, input_sampler, uv + vec2<f32>( offset.x, 0.0));
        accum = accum + textureSample(input_tex, input_sampler, uv + vec2<f32>(-offset.x, 0.0));
        accum = accum + textureSample(input_tex, input_sampler, uv + vec2<f32>(0.0,  offset.y));
        accum = accum + textureSample(input_tex, input_sampler, uv + vec2<f32>(0.0, -offset.y));
        let blurred = accum / 5.0;
        color = mix(color, blurred, u.blur_amount);
    }

    // Trail decay (darken over time)
    color = color * u.trail_decay;

    return color;
}
