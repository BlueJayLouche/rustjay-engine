//! Slice + edge-blend renderer for projector outputs.

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) texcoord: vec2<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(position, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

struct Uniforms {
    // UV range on the canvas texture to sample.
    source_uv_min: vec2<f32>,
    source_uv_max: vec2<f32>,
    // Output resolution in pixels, for edge-blend distance math.
    output_size: vec2<f32>,
    // edge: x=enabled(0/1), y=width_px, z=gamma
    edge_left: vec3<f32>,
    edge_right: vec3<f32>,
    edge_top: vec3<f32>,
    edge_bottom: vec3<f32>,
}

@group(0) @binding(0)
var canvas_texture: texture_2d<f32>;

@group(0) @binding(1)
var canvas_sampler: sampler;

@group(0) @binding(2)
var<uniform> uniforms: Uniforms;

fn edge_alpha(enabled: f32, dist_px: f32, width_px: f32, gamma: f32) -> f32 {
    if enabled < 0.5 || width_px <= 0.0 {
        return 1.0;
    }
    let t = clamp(dist_px / width_px, 0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t); // smoothstep
    return pow(s, gamma);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample the canvas with pixel-center alignment baked into source_uv_min/max.
    let uv = mix(uniforms.source_uv_min, uniforms.source_uv_max, in.texcoord);
    var color = textureSample(canvas_texture, canvas_sampler, uv);

    // Distance from each output edge in pixels.
    let left_dist = in.texcoord.x * uniforms.output_size.x;
    let right_dist = (1.0 - in.texcoord.x) * uniforms.output_size.x;
    let top_dist = in.texcoord.y * uniforms.output_size.y;
    let bottom_dist = (1.0 - in.texcoord.y) * uniforms.output_size.y;

    let left = edge_alpha(uniforms.edge_left.x, left_dist, uniforms.edge_left.y, uniforms.edge_left.z);
    let right = edge_alpha(uniforms.edge_right.x, right_dist, uniforms.edge_right.y, uniforms.edge_right.z);
    let top = edge_alpha(uniforms.edge_top.x, top_dist, uniforms.edge_top.y, uniforms.edge_top.z);
    let bottom = edge_alpha(uniforms.edge_bottom.x, bottom_dist, uniforms.edge_bottom.y, uniforms.edge_bottom.z);

    // Modulate RGB by the product of enabled edge ramps.
    let blend = left * right * top * bottom;
    color.r *= blend;
    color.g *= blend;
    color.b *= blend;

    return color;
}
