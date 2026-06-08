struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct RotationParams {
    rotation: u32,
}

@group(0) @binding(0)
var source_tex: texture_2d<f32>;

@group(0) @binding(1)
var source_sampler: sampler;

@group(0) @binding(2)
var<uniform> params: RotationParams;

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = texcoord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = in.uv;

    // Apply rotation
    switch (params.rotation) {
        case 1u: {
            // 90° CW: (u,v) → (v, 1-u)
            uv = vec2<f32>(uv.y, 1.0 - uv.x);
        }
        case 2u: {
            // 180°: (u,v) → (1-u, 1-v)
            uv = vec2<f32>(1.0 - uv.x, 1.0 - uv.y);
        }
        case 3u: {
            // 270° CW: (u,v) → (1-v, u)
            uv = vec2<f32>(1.0 - uv.y, uv.x);
        }
        default: {
            // 0°: no rotation
        }
    }

    return textureSample(source_tex, source_sampler, uv);
}
