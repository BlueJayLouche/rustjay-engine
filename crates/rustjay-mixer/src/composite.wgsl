// Composite blend shader for rustjay-mixer.
//
// Samples a source layer and the destination (composite-so-far), blends per
// pixel by `params.blend_mode`, and writes to a THIRD target (the pipeline uses
// BlendState::REPLACE). You cannot sample the texture you are rendering into, so
// the mixer ping-pongs two accumulation textures.
//
// `CompositeParams` is 32 bytes (8 × f32). The `mode == Nu` branches must match
// `BlendMode::to_index` in blend.rs.

struct CompositeParams {
    opacity: f32,
    blend_mode: u32,
    uv_scale: vec2<f32>,
    uv_offset: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var texture_sampler: sampler;
@group(0) @binding(1) var source_texture:  texture_2d<f32>;
@group(0) @binding(2) var dest_texture:    texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: CompositeParams;

const EPSILON: f32 = 0.001;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.uv = texcoord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;

    // Source sampled with UV transform (scaling modes); out-of-range → transparent.
    let source_uv = uv * params.uv_scale + params.uv_offset;
    var src: vec4<f32>;
    if (source_uv.x < 0.0 || source_uv.x > 1.0 || source_uv.y < 0.0 || source_uv.y > 1.0) {
        src = vec4<f32>(0.0);
    } else {
        src = textureSample(source_texture, texture_sampler, source_uv);
    }

    // Destination is the full composite-so-far, sampled at raw UV.
    let dst = textureSample(dest_texture, texture_sampler, uv);

    let src_a = src.a * params.opacity;
    if (src_a <= 0.0) {
        return dst;
    }

    var blended: vec3<f32>;
    let mode = params.blend_mode;

    if (mode == 0u) {
        blended = src.rgb;                                              // Normal
    } else if (mode == 1u) {
        blended = clamp(src.rgb + dst.rgb, vec3<f32>(0.0), vec3<f32>(1.0)); // Add
    } else if (mode == 2u) {
        blended = clamp(dst.rgb - src.rgb, vec3<f32>(0.0), vec3<f32>(1.0)); // Subtract
    } else if (mode == 3u) {
        blended = src.rgb * dst.rgb;                                   // Multiply
    } else if (mode == 4u) {
        blended = vec3<f32>(1.0) - (vec3<f32>(1.0) - src.rgb) * (vec3<f32>(1.0) - dst.rgb); // Screen
    } else if (mode == 5u) {
        blended = vec3<f32>(                                           // Overlay
            select(1.0 - 2.0 * (1.0 - src.r) * (1.0 - dst.r), 2.0 * src.r * dst.r, dst.r < 0.5),
            select(1.0 - 2.0 * (1.0 - src.g) * (1.0 - dst.g), 2.0 * src.g * dst.g, dst.g < 0.5),
            select(1.0 - 2.0 * (1.0 - src.b) * (1.0 - dst.b), 2.0 * src.b * dst.b, dst.b < 0.5),
        );
    } else if (mode == 6u) {
        blended = (vec3<f32>(1.0) - 2.0 * src.rgb) * dst.rgb * dst.rgb + 2.0 * src.rgb * dst.rgb; // Soft Light
    } else if (mode == 7u) {
        blended = vec3<f32>(                                           // Hard Light
            select(1.0 - 2.0 * (1.0 - src.r) * (1.0 - dst.r), 2.0 * src.r * dst.r, src.r < 0.5),
            select(1.0 - 2.0 * (1.0 - src.g) * (1.0 - dst.g), 2.0 * src.g * dst.g, src.g < 0.5),
            select(1.0 - 2.0 * (1.0 - src.b) * (1.0 - dst.b), 2.0 * src.b * dst.b, src.b < 0.5),
        );
    } else if (mode == 8u) {
        blended = clamp(vec3<f32>(                                     // Color Dodge
            dst.r / max(1.0 - src.r, EPSILON),
            dst.g / max(1.0 - src.g, EPSILON),
            dst.b / max(1.0 - src.b, EPSILON),
        ), vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (mode == 9u) {
        blended = clamp(vec3<f32>(                                     // Color Burn
            1.0 - (1.0 - dst.r) / max(src.r, EPSILON),
            1.0 - (1.0 - dst.g) / max(src.g, EPSILON),
            1.0 - (1.0 - dst.b) / max(src.b, EPSILON),
        ), vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (mode == 10u) {
        blended = abs(src.rgb - dst.rgb);                              // Difference
    } else if (mode == 11u) {
        blended = src.rgb + dst.rgb - 2.0 * src.rgb * dst.rgb;         // Exclusion
    } else if (mode == 12u) {
        blended = min(src.rgb, dst.rgb);                               // Darken
    } else if (mode == 13u) {
        blended = max(src.rgb, dst.rgb);                               // Lighten
    } else if (mode == 14u) {
        blended = max(src.rgb + dst.rgb - vec3<f32>(1.0), vec3<f32>(0.0)); // Linear Burn
    } else {
        blended = src.rgb;                                             // Fallback: Normal
    }

    // Standard source-over: blend by source alpha, accumulate alpha.
    let result_rgb = mix(dst.rgb, blended, src_a);
    let result_a = src_a + dst.a * (1.0 - src_a);
    return vec4<f32>(result_rgb, result_a);
}
