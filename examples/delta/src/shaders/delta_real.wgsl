// Delta — RGB delay / motion extraction (real implementation).
// Binding layout matches DeltaEffect::init(): all four textures first, then sampler.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

// Group 0: four history/input textures + one shared sampler
@group(0) @binding(0) var red_tex:   texture_2d<f32>; // red channel history frame
@group(0) @binding(1) var green_tex: texture_2d<f32>; // green channel history frame
@group(0) @binding(2) var blue_tex:  texture_2d<f32>; // blue channel history frame
@group(0) @binding(3) var input_tex: texture_2d<f32>; // current input frame
@group(0) @binding(4) var tex_sampler: sampler;

struct DeltaUniforms {
    delays:       vec4<f32>, // red, green, blue, max_history
    settings:     vec4<f32>, // intensity, blend_mode, grayscale, unused
    channel_gain: vec4<f32>, // red, green, blue, unused
    mix_options:  vec4<f32>, // input_mix, trail_fade, threshold, smoothing
};

@group(1) @binding(0) var<uniform> u: DeltaUniforms;

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

fn blend_colors(base: vec3<f32>, layer: vec3<f32>, mode: i32) -> vec3<f32> {
    switch mode {
        case 1: { return clamp(base + layer, vec3<f32>(0.0), vec3<f32>(1.0)); }
        case 2: { return base * layer; }
        case 3: { return 1.0 - (1.0 - base) * (1.0 - layer); }
        case 4: { return abs(base - layer); }
        case 5: {
            return select(
                2.0 * base * layer,
                1.0 - 2.0 * (1.0 - base) * (1.0 - layer),
                base > vec3<f32>(0.5)
            );
        }
        case 6: { return max(base, layer); }
        case 7: { return min(base, layer); }
        default: { return layer; }
    }
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.texcoord;

    let intensity  = u.settings.x;
    let blend_mode = i32(u.settings.y);
    let grayscale  = u.settings.z > 0.5;
    let input_mix  = u.mix_options.x;
    let threshold  = u.mix_options.z;

    let r_samp = textureSample(red_tex,   tex_sampler, uv);
    let g_samp = textureSample(green_tex, tex_sampler, uv);
    let b_samp = textureSample(blue_tex,  tex_sampler, uv);
    let cur    = textureSample(input_tex, tex_sampler, uv);

    var r: f32;
    var g: f32;
    var b: f32;

    if grayscale {
        let luma = vec3<f32>(0.299, 0.587, 0.114);
        r = dot(r_samp.rgb, luma) * u.channel_gain.x;
        g = dot(g_samp.rgb, luma) * u.channel_gain.y;
        b = dot(b_samp.rgb, luma) * u.channel_gain.z;
    } else {
        r = r_samp.r * u.channel_gain.x;
        g = g_samp.g * u.channel_gain.y;
        b = b_samp.b * u.channel_gain.z;
    }

    var delayed = vec3<f32>(r, g, b);

    if threshold > 0.0 {
        let luma = dot(delayed, vec3<f32>(0.299, 0.587, 0.114));
        delayed = select(vec3<f32>(0.0), delayed, luma > threshold);
    }

    let blended   = blend_colors(cur.rgb, delayed * intensity, blend_mode);
    let final_rgb = mix(blended, cur.rgb, input_mix);

    return vec4<f32>(final_rgb, cur.a);
}
