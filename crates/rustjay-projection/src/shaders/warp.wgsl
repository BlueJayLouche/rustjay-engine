struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct WarpParams {
    // 3x3 homography stored as 3 vec4s (xyz used, w padding)
    h_row0: vec4<f32>,
    h_row1: vec4<f32>,
    h_row2: vec4<f32>,
    // 1.0 = use homography (corner-pin), 0.0 = passthrough (mesh)
    use_homography: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> params: WarpParams;

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;

    if params.use_homography > 0.5 {
        // Corner-pin: position is unit-square [0,1], apply homography
        let p = vec3<f32>(in.position, 1.0);
        let hx = dot(params.h_row0.xyz, p);
        let hy = dot(params.h_row1.xyz, p);
        let hw = dot(params.h_row2.xyz, p);
        // Map [0,1] output of homography to clip space
        out.position = vec4<f32>(hx * 2.0 - hw, hw - hy * 2.0, 0.0, hw);
        out.uv = in.position;
    } else {
        // Mesh mode: position is already output [0,1], uv is source UV
        out.position = vec4<f32>(in.position.x * 2.0 - 1.0, 1.0 - in.position.y * 2.0, 0.0, 1.0);
        out.uv = in.uv;
    }

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(source_tex, source_sampler, in.uv);
}
