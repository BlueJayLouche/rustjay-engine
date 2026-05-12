// Block C — Color grading (HSB)

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

fn rgb_to_hsv(rgb: vec3<f32>) -> vec3<f32> {
    let r = rgb.r;
    let g = rgb.g;
    let b = rgb.b;
    let max_val = max(max(r, g), b);
    let min_val = min(min(r, g), b);
    let delta = max_val - min_val;
    let v = max_val;
    var s = 0.0;
    if max_val > 0.0 {
        s = delta / max_val;
    }
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

fn hsv_to_rgb(hsv: vec3<f32>) -> vec3<f32> {
    let h = hsv.x * 6.0;
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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.texcoord;
    var color = textureSample(input_tex, input_sampler, uv);

    // HSB adjustment
    var hsv = rgb_to_hsv(color.rgb);
    hsv.x = fract(hsv.x + u.hue_shift / 360.0);
    hsv.y = clamp(hsv.y * u.saturation, 0.0, 1.0);
    hsv.z = clamp(hsv.z * u.brightness, 0.0, 1.0);
    color = vec4<f32>(hsv_to_rgb(hsv), color.a);

    return color;
}
