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
    // Global canvas opacity (Stop-cue picture fade).
    opacity: f32,
    // Black-level uplift (linear light) added where no edge-blend ramp is
    // active, to match the doubled black floor of the overlap zone.
    black_uplift: f32,
    // Calibration flat field replacing the canvas content: 0=off, 1=black,
    // 2=white. Edge blend and uplift still apply on top.
    test_pattern: f32,
}

@group(0) @binding(0)
var canvas_texture: texture_2d<f32>;

@group(0) @binding(1)
var canvas_sampler: sampler;

@group(0) @binding(2)
var<uniform> uniforms: Uniforms;

// Text layer, same size/UV space as the canvas; transparent where no text.
@group(0) @binding(3)
var overlay_texture: texture_2d<f32>;

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

    // Composite the text overlay over the canvas (edge blend below dims both).
    let overlay = textureSample(overlay_texture, canvas_sampler, uv);
    color = vec4<f32>(mix(color.rgb, overlay.rgb, overlay.a), color.a);

    // Calibration flat field (black for uplift calibration, white for blend
    // width/gamma calibration), replacing canvas + overlay content.
    if uniforms.test_pattern > 0.5 {
        let flat = select(vec3<f32>(0.0), vec3<f32>(1.0), uniforms.test_pattern > 1.5);
        color = vec4<f32>(flat, color.a);
    }

    // Distance from each output edge in pixels.
    let left_dist = in.texcoord.x * uniforms.output_size.x;
    let right_dist = (1.0 - in.texcoord.x) * uniforms.output_size.x;
    let top_dist = in.texcoord.y * uniforms.output_size.y;
    let bottom_dist = (1.0 - in.texcoord.y) * uniforms.output_size.y;

    let left = edge_alpha(uniforms.edge_left.x, left_dist, uniforms.edge_left.y, uniforms.edge_left.z);
    let right = edge_alpha(uniforms.edge_right.x, right_dist, uniforms.edge_right.y, uniforms.edge_right.z);
    let top = edge_alpha(uniforms.edge_top.x, top_dist, uniforms.edge_top.y, uniforms.edge_top.z);
    let bottom = edge_alpha(uniforms.edge_bottom.x, bottom_dist, uniforms.edge_bottom.y, uniforms.edge_bottom.z);

    // Modulate RGB by the product of enabled edge ramps and global opacity.
    let edge = left * right * top * bottom;
    let blend = edge * uniforms.opacity;
    color.r *= blend;
    color.g *= blend;
    color.b *= blend;

    // Black-level uplift: raise the black floor where this output is not edge
    // blended (solo area) to match the doubled black floor of the overlap zone,
    // where both projectors' lamp leakage adds up. Hard step at the ramp
    // boundary: leakage is not attenuated by the ramp, so neither is the uplift.
    // Not scaled by opacity — lamp leakage doesn't fade with the picture.
    if edge >= 1.0 {
        color = vec4<f32>(color.rgb + vec3<f32>(uniforms.black_uplift), color.a);
    }

    return color;
}
