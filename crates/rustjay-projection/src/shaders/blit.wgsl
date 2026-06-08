struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

struct BlitParams {
    uv_scale: vec2<f32>,
    uv_offset: vec2<f32>,
    uv_crop: vec4<f32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: BlitParams;

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.texcoord * params.uv_scale + params.uv_offset;
    let crop_min = params.uv_crop.xy;
    let crop_max = params.uv_crop.zw;
    let cropped = uv * (crop_max - crop_min) + crop_min;
    return textureSample(source_tex, source_sampler, cropped);
}
