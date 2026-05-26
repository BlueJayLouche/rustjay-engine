// Delta — passthrough stub for engine pipeline creation.
// The real motion-extraction shader is compiled inside DeltaEffect::init().

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
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
var<uniform> uniforms: vec4<f32>;

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
    return textureSample(input_tex, input_sampler, in.texcoord);
}
