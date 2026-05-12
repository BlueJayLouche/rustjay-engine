// Main shader for RustJay Template
// Features: BGRA input, HSB color manipulation, fullscreen output

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

struct HsbParams {
    // hue_shift, saturation, brightness, _padding
    values: vec4<f32>,
};

@group(0) @binding(0)
var input_tex: texture_2d<f32>;
@group(0) @binding(1)
var input_sampler: sampler;

// HSB color adjustment parameters
@group(1) @binding(0)
var<uniform> hsb_params: HsbParams;

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

/// Convert RGB to HSV
/// Returns: vec3(hue [0-1], saturation [0-1], value [0-1])
fn rgb_to_hsv(rgb: vec3<f32>) -> vec3<f32> {
    let r = rgb.r;
    let g = rgb.g;
    let b = rgb.b;

    let max_val = max(max(r, g), b);
    let min_val = min(min(r, g), b);
    let delta = max_val - min_val;

    // Value
    let v = max_val;

    // Saturation
    var s = 0.0;
    if max_val > 0.0 {
        s = delta / max_val;
    }

    // Hue
    var h = 0.0;
    if delta > 0.0 {
        if max_val == r {
            h = ((g - b) / delta) / 6.0;
            if g < b {
                h = h + 1.0;
            }
        } else if max_val == g {
            h = ((b - r) / delta + 2.0) / 6.0;
        } else {
            h = ((r - g) / delta + 4.0) / 6.0;
        }
    }

    return vec3<f32>(h, s, v);
}

/// Convert HSV to RGB
fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = hsv.x * 6.0; // Scale to 0-6
    let s = hsv.y;
    let v = hsv.z;

    let i = floor(h);
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);

    var rgb: vec3<f32>;
    switch u32(i) % 6u {
        case 0u: { rgb = vec3<f32>(v, t, p); }
        case 1u: { rgb = vec3<f32>(q, v, p); }
        case 2u: { rgb = vec3<f32>(p, v, t); }
        case 3u: { rgb = vec3<f32>(p, q, v); }
        case 4u: { rgb = vec3<f32>(t, p, v); }
        case 5u: { rgb = vec3<f32>(v, p, q); }
        default: { rgb = vec3<f32>(v, p, q); }
    }

    return rgb;
}

/// Apply HSB adjustments to a color
/// hsb_params.values = (hue_shift_degrees, saturation_mult, brightness_mult, _)
fn apply_hsb(rgb: vec3<f32>, params: HsbParams) -> vec3<f32> {
    // Convert to HSV
    var hsv = rgb_to_hsv(rgb);

    // Apply hue shift (convert degrees to 0-1 range)
    let hue_shift = params.values.x / 360.0;
    hsv.x = fract(hsv.x + hue_shift);

    // Apply saturation multiplier
    hsv.y = clamp(hsv.y * params.values.y, 0.0, 1.0);

    // Apply brightness/value multiplier
    hsv.z = clamp(hsv.z * params.values.z, 0.0, 1.0);

    // Convert back to RGB
    return hsv_to_rgb(hsv);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Sample input texture
    var color = textureSample(input_tex, input_sampler, in.texcoord);

    // `textureSample` returns logical RGBA even for BGRA-backed textures.
    let adjusted_rgb = apply_hsb(color.rgb, hsb_params);
    color = vec4<f32>(adjusted_rgb, color.a);

    return color;
}
