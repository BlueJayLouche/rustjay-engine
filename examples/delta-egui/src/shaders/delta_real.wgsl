// Delta — RGB delay / motion extraction.
// Algorithm ported from rustjay-delta/src/engine/shaders/motion_extraction.wgsl.
// Binding layout matches DeltaEffect::init(): all four textures first, then sampler.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

// Group 0: four history/input textures + one shared sampler
@group(0) @binding(0) var red_tex:    texture_2d<f32>; // red channel history frame
@group(0) @binding(1) var green_tex:  texture_2d<f32>; // green channel history frame
@group(0) @binding(2) var blue_tex:   texture_2d<f32>; // blue channel history frame
@group(0) @binding(3) var input_tex:  texture_2d<f32>; // current input frame
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

fn rgb_to_luma(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.299, 0.587, 0.114));
}

fn blend_colors(base: vec3<f32>, layer: vec3<f32>, mode: i32) -> vec3<f32> {
    switch mode {
        case 1: { return min(base + layer, vec3<f32>(1.0)); }
        case 2: { return base * layer; }
        case 3: { return 1.0 - (1.0 - base) * (1.0 - layer); }
        case 4: { return abs(base - layer); }
        case 5: {
            return mix(
                2.0 * base * layer,
                1.0 - 2.0 * (1.0 - base) * (1.0 - layer),
                step(vec3<f32>(0.5), base)
            );
        }
        case 6: { return max(base, layer); }
        case 7: { return min(base, layer); }
        default: { return layer; }
    }
}

// Soft threshold: suppresses values below threshold and renormalizes the remaining range.
fn apply_threshold(color: vec3<f32>, threshold: f32) -> vec3<f32> {
    if threshold <= 0.0 {
        return color;
    }
    let t = clamp(threshold, 0.0, 0.99);
    return max(color - vec3<f32>(t), vec3<f32>(0.0)) / (1.0 - t);
}

// Negative gain inverts the signal instead of clamping to zero.
fn apply_channel_gain(value: f32, gain: f32) -> f32 {
    if gain >= 0.0 {
        return clamp(value * gain, 0.0, 1.0);
    }
    return clamp((1.0 - value) * -gain, 0.0, 1.0);
}

// Core motion extraction: differences adjacent history frames per channel.
// Static content cancels out; only change energy passes through.
fn extract_motion(
    current_sample: vec4<f32>,
    sample_0: vec4<f32>,
    sample_1: vec4<f32>,
    sample_2: vec4<f32>,
) -> vec3<f32> {
    var motion: vec3<f32>;

    if u.settings.z > 0.5 {
        // Grayscale mode: convert to luminance then difference
        let cl  = rgb_to_luma(current_sample.rgb);
        let l0  = rgb_to_luma(sample_0.rgb);
        let l1  = rgb_to_luma(sample_1.rgb);
        let l2  = rgb_to_luma(sample_2.rgb);
        motion = vec3<f32>(abs(l0 - l1), abs(l1 - l2), abs(cl - l2));
    } else {
        // Per-channel mode: difference individual colour channels
        motion = vec3<f32>(
            abs(sample_0.r - sample_1.r),
            abs(sample_1.g - sample_2.g),
            abs(current_sample.b - sample_2.b),
        );
    }

    motion.r = apply_channel_gain(motion.r, u.channel_gain.x);
    motion.g = apply_channel_gain(motion.g, u.channel_gain.y);
    motion.b = apply_channel_gain(motion.b, u.channel_gain.z);

    return motion;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.texcoord;

    let s0  = textureSample(red_tex,   tex_sampler, uv);
    let s1  = textureSample(green_tex, tex_sampler, uv);
    let s2  = textureSample(blue_tex,  tex_sampler, uv);
    let cur = textureSample(input_tex, tex_sampler, uv);

    var motion = extract_motion(cur, s0, s1, s2);

    motion = apply_threshold(motion, u.mix_options.z);

    // Spatial smoothing: average four cardinal neighbours with the centre sample.
    let smoothing = u.mix_options.w;
    if smoothing > 0.0 {
        let off = smoothing * 0.01;
        let tp = uv + vec2<f32>(off, 0.0);
        let tn = uv - vec2<f32>(off, 0.0);
        let tr = uv + vec2<f32>(0.0, off);
        let tl = uv - vec2<f32>(0.0, off);

        let smoothed = (
            extract_motion(
                textureSample(input_tex, tex_sampler, tp),
                textureSample(red_tex,   tex_sampler, tp),
                textureSample(green_tex, tex_sampler, tp),
                textureSample(blue_tex,  tex_sampler, tp),
            ) +
            extract_motion(
                textureSample(input_tex, tex_sampler, tn),
                textureSample(red_tex,   tex_sampler, tn),
                textureSample(green_tex, tex_sampler, tn),
                textureSample(blue_tex,  tex_sampler, tn),
            ) +
            extract_motion(
                textureSample(input_tex, tex_sampler, tr),
                textureSample(red_tex,   tex_sampler, tr),
                textureSample(green_tex, tex_sampler, tr),
                textureSample(blue_tex,  tex_sampler, tr),
            ) +
            extract_motion(
                textureSample(input_tex, tex_sampler, tl),
                textureSample(red_tex,   tex_sampler, tl),
                textureSample(green_tex, tex_sampler, tl),
                textureSample(blue_tex,  tex_sampler, tl),
            )
        ) * 0.25;
        motion = mix(motion, smoothed, smoothing);
    }

    let blend_mode = i32(u.settings.y);
    let intensity  = u.settings.x;
    let input_mix  = u.mix_options.x;
    let trail_fade = u.mix_options.y;

    let blended = blend_colors(cur.rgb, motion, blend_mode);
    var output  = mix(cur.rgb * input_mix, blended, intensity);

    // Trail fade: gamma-like boost that makes dim motion trails more visible.
    if trail_fade > 0.0 {
        output = pow(output, vec3<f32>(1.0 - trail_fade * 0.5));
    }

    return vec4<f32>(output, 1.0);
}
