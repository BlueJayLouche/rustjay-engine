
struct VertexInput {
    @location(0) position: vec2<f32>,
    @location(1) texcoord: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
}

@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = vec4<f32>(input.position, 0.0, 1.0);
    output.texcoord = input.texcoord;
    return output;
}


@group(1) @binding(0)
var<uniform> uniforms: BlockCUniforms;

@group(0) @binding(0)
var block1_tex: texture_2d<f32>;
@group(0) @binding(1)
var block1_sampler: sampler;
@group(0) @binding(2)
var block2_tex: texture_2d<f32>;
@group(0) @binding(3)
var block2_sampler: sampler;

// vec4<f32> is used for all Align16 Rust fields (both are 16 bytes, AlignOf 16).
// Only .xyz is meaningful; .w is the internal Rust pad byte.
struct BlockCUniforms {
    width: f32,
    height: f32,
    inv_width: f32,
    inv_height: f32,

    block1_xy_displace: vec2<f32>,
    block1_z_displace: f32,
    block1_rotate: f32,
    block1_shear_matrix: vec4<f32>,
    block1_kaleidoscope: f32,
    block1_kaleidoscope_slice: f32,
    block1_blur_amount: f32,
    block1_blur_radius: f32,
    block1_sharpen_amount: f32,
    block1_sharpen_radius: f32,
    block1_filters_boost: f32,
    block1_dither: f32,
    block1_switches: u32,
    block1_colorize_mode: i32,
    block1_dither_type: i32,
    _pad1: f32,

    block1_colorize_band1: vec4<f32>,   // Align16 → vec4; use .xyz
    block1_colorize_band2: vec4<f32>,
    block1_colorize_band3: vec4<f32>,
    block1_colorize_band4: vec4<f32>,
    block1_colorize_band5: vec4<f32>,

    block2_xy_displace: vec2<f32>,
    block2_z_displace: f32,
    block2_rotate: f32,
    block2_shear_matrix: vec4<f32>,
    block2_kaleidoscope: f32,
    block2_kaleidoscope_slice: f32,
    block2_blur_amount: f32,
    block2_blur_radius: f32,
    block2_sharpen_amount: f32,
    block2_sharpen_radius: f32,
    block2_filters_boost: f32,
    block2_dither: f32,
    block2_switches: u32,
    block2_colorize_mode: i32,
    block2_dither_type: i32,
    _pad7: f32,

    block2_colorize_band1: vec4<f32>,   // Align16 → vec4; use .xyz
    block2_colorize_band2: vec4<f32>,
    block2_colorize_band3: vec4<f32>,
    block2_colorize_band4: vec4<f32>,
    block2_colorize_band5: vec4<f32>,

    matrix_mix_type: i32,
    matrix_mix_overflow: i32,
    // 8 bytes implicit padding here before bg_into_fg_red (AlignOf=16, offset 352)
    bg_into_fg_red: vec4<f32>,          // Align16 → vec4; use .xyz
    bg_into_fg_green: vec4<f32>,
    bg_into_fg_blue: vec4<f32>,

    final_mix_amount: f32,
    // 12 bytes implicit padding here before final_key_value (AlignOf=16, offset 416)
    final_key_value: vec4<f32>,         // Align16 → vec4; use .xyz
    final_key_threshold: f32,
    final_key_soft: f32,
    final_mix_type: i32,
    final_mix_overflow: i32,
    final_key_order: i32,
    final_dither: f32,
    final_dither_type: i32,
    // 4 bytes implicit tail padding
}

// RGB to HSB conversion
fn rgb2hsb(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4<f32>(c.bg, K.wz), vec4<f32>(c.gb, K.xy), step(c.b, c.g));
    let q = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

// HSB to RGB conversion
fn hsb2rgb(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

fn wrap01(v: f32) -> f32 {
    return fract(abs(v));
}

fn fold01(v: f32) -> f32 {
    var result = v;
    if (result < 0.0) { result = abs(result); }
    if (result > 1.0) { result = 1.0 - fract(result); }
    if (result < 0.0) { result = abs(result); }
    return result;
}

// 5-band colorize with HSB/RGB mode switch
fn apply_colorize(color: vec3<f32>, mode: i32,
                  band1: vec3<f32>, band2: vec3<f32>, band3: vec3<f32>,
                  band4: vec3<f32>, band5: vec3<f32>) -> vec3<f32> {
    let hsb = rgb2hsb(color);
    let brightness = hsb.z;

    let hsb1 = band1 + vec3<f32>(0.0, 0.0, brightness);
    let hsb2 = band2 + vec3<f32>(0.0, 0.0, brightness);
    let hsb3 = band3 + vec3<f32>(0.0, 0.0, brightness);
    let hsb4 = band4 + vec3<f32>(0.0, 0.0, brightness);
    let hsb5 = band5 + vec3<f32>(0.0, 0.0, brightness);

    let rgb_hsb1 = hsb2rgb(hsb1);
    let rgb_hsb2 = hsb2rgb(hsb2);
    let rgb_hsb3 = hsb2rgb(hsb3);
    let rgb_hsb4 = hsb2rgb(hsb4);
    let rgb_hsb5 = hsb2rgb(hsb5);

    let rgb1 = band1 + color;
    let rgb2 = band2 + color;
    let rgb3 = band3 + color;
    let rgb4 = band4 + color;
    let rgb5 = band5 + color;

    let mode_mix = f32(mode);
    let col1 = mix(rgb_hsb1, rgb1, mode_mix);
    let col2 = mix(rgb_hsb2, rgb2, mode_mix);
    let col3 = mix(rgb_hsb3, rgb3, mode_mix);
    let col4 = mix(rgb_hsb4, rgb4, mode_mix);
    let col5 = mix(rgb_hsb5, rgb5, mode_mix);

    let band_mix1 = clamp(brightness * 4.0, 0.0, 1.0);
    let band_mix2 = clamp((brightness - 0.25) * 4.0, 0.0, 1.0);
    let band_mix3 = clamp((brightness - 0.5) * 4.0, 0.0, 1.0);
    let band_mix4 = clamp((brightness - 0.75) * 4.0, 0.0, 1.0);

    var result = mix(
        mix(
            mix(col1, col2, band_mix1),
            mix(col2, col3, band_mix2),
            step(0.25, brightness)
        ),
        mix(col3, col4, band_mix3),
        step(0.5, brightness)
    );
    result = mix(result, mix(col4, col5, band_mix4), step(0.75, brightness));

    return result;
}

// ── Geo helpers ───────────────────────────────────────────────────────────────

const TWO_PI: f32 = 6.28318530718;

fn do_rotate(coord: vec2<f32>, angle: f32) -> vec2<f32> {
    if (angle == 0.0) { return coord; }
    let centered = coord - vec2<f32>(0.5, 0.5);
    let c = cos(angle);
    let s = sin(angle);
    return vec2<f32>(centered.x * c - centered.y * s + 0.5,
                     centered.x * s + centered.y * c + 0.5);
}

fn do_kaleidoscope(coord: vec2<f32>, segments: f32, slice: f32) -> vec2<f32> {
    if (segments <= 0.0) { return coord; }
    var result = do_rotate(coord, slice);
    let centered = result * 2.0 - 1.0;
    let radius = length(centered);
    var angle = atan2(centered.y, centered.x);
    let seg_angle = TWO_PI / segments;
    angle = angle - seg_angle * floor(angle / seg_angle);
    angle = min(angle, seg_angle - angle);
    result = radius * vec2<f32>(cos(angle), sin(angle));
    result = result * 0.5 + 0.5;
    return do_rotate(result, -slice);
}

fn shear_coord(coord: vec2<f32>, shear_matrix: vec4<f32>) -> vec2<f32> {
    if (shear_matrix.x == 1.0 && shear_matrix.y == 0.0 && shear_matrix.z == 0.0 && shear_matrix.w == 1.0) {
        return coord;
    }
    let center = vec2<f32>(0.5, 0.5);
    let r = coord - center;
    let rx = shear_matrix.x * r.x + shear_matrix.y * r.y;
    let ry = shear_matrix.z * r.x + shear_matrix.w * r.y;
    return vec2<f32>(rx, ry) + center;
}

fn mirror_val(a: f32) -> f32 {
    if (a > 0.0) { return a; }
    return -(1.0 + a);
}

fn mirror_coord(coord: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        1.0 - mirror_val(coord.x % 2.0 - 1.0),
        1.0 - mirror_val(coord.y % 2.0 - 1.0)
    );
}

fn blur_and_sharpen_c(tex: texture_2d<f32>, samp: sampler, coord: vec2<f32>,
                       sharpen_amount: f32, sharpen_radius: f32, boost: f32,
                       blur_radius: f32, blur_amount: f32) -> vec4<f32> {
    let orig = textureSample(tex, samp, coord);
    if (blur_amount < 0.001 && sharpen_amount < 0.001) { return orig; }

    let bs = vec2<f32>(blur_radius) * vec2<f32>(uniforms.inv_width, uniforms.inv_height);
    let ss = vec2<f32>(sharpen_radius) * vec2<f32>(uniforms.inv_width, uniforms.inv_height);

    var blurred = orig;
    if (blur_amount >= 0.001) {
        blurred = textureSample(tex, samp, coord + bs * vec2<f32>( 1.0,  1.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>( 0.0,  1.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>(-1.0,  1.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>(-1.0,  0.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>(-1.0, -1.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>( 0.0, -1.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>( 1.0, -1.0))
                + textureSample(tex, samp, coord + bs * vec2<f32>( 1.0,  0.0));
        blurred *= 0.125;
        blurred = mix(orig, blurred, blur_amount);
    }

    var b_hsb = rgb2hsb(blurred.rgb);
    if (sharpen_amount >= 0.001) {
        let lw = vec3<f32>(0.299, 0.587, 0.114);
        var sb = dot(textureSample(tex, samp, coord + ss * vec2<f32>( 1.0,  0.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>(-1.0,  0.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>( 0.0,  1.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>( 0.0, -1.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>( 1.0,  1.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>(-1.0,  1.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>( 1.0, -1.0)).rgb, lw)
               + dot(textureSample(tex, samp, coord + ss * vec2<f32>(-1.0, -1.0)).rgb, lw);
        sb *= 0.125;
        b_hsb.z -= sharpen_amount * sb;
    }
    let boost_f = mix(1.0, 1.0 + sharpen_amount + boost, step(0.001, sharpen_amount));
    b_hsb.z *= boost_f;
    b_hsb.x = fract(b_hsb.x);
    b_hsb.y = clamp(b_hsb.y, 0.0, 1.0);
    b_hsb.z = clamp(b_hsb.z, 0.0, 1.0);
    return vec4<f32>(hsb2rgb(b_hsb), 1.0);
}

// ── Ordered dither matrices ───────────────────────────────────────────────────

fn bayer4(x: u32, y: u32) -> f32 {
    let m = array<u32, 16>(
         0u,  8u,  2u, 10u,
        12u,  4u, 14u,  6u,
         3u, 11u,  1u,  9u,
        15u,  7u, 13u,  5u,
    );
    return f32(m[(y % 4u) * 4u + (x % 4u)]) / 16.0;
}

fn bayer8(x: u32, y: u32) -> f32 {
    let m = array<u32, 64>(
         0u, 32u,  8u, 40u,  2u, 34u, 10u, 42u,
        48u, 16u, 56u, 24u, 50u, 18u, 58u, 26u,
        12u, 44u,  4u, 36u, 14u, 46u,  6u, 38u,
        60u, 28u, 52u, 20u, 62u, 30u, 54u, 22u,
         3u, 35u, 11u, 43u,  1u, 33u,  9u, 41u,
        51u, 19u, 59u, 27u, 49u, 17u, 57u, 25u,
        15u, 47u,  7u, 39u, 13u, 45u,  5u, 37u,
        63u, 31u, 55u, 23u, 61u, 29u, 53u, 21u,
    );
    return f32(m[(y % 8u) * 8u + (x % 8u)]) / 64.0;
}

// ── Noise functions ───────────────────────────────────────────────────────────

fn blue_noise(x: u32, y: u32, ch: u32) -> f32 {
    var h = x ^ (x >> 4u) ^ (y * 2654435761u) ^ ch * 1013904223u;
    h ^= h >> 13u;
    h *= 1664525u;
    h ^= h >> 16u;
    return f32(h & 0xFFFFu) / 65535.0;
}

fn white_noise(x: u32, y: u32, ch: u32) -> f32 {
    let fx = f32(x) + f32(ch) * 17.0;
    return fract(sin(fx * 12.9898 + f32(y) * 78.233) * 43758.5453);
}

fn ign(x: u32, y: u32) -> f32 {
    return fract(52.9829189 * fract(0.06711056 * f32(x) + 0.00583715 * f32(y)));
}

// ── Per-channel quantization dither ──────────────────────────────────────────

fn quantize_dither(c: f32, palette: f32, threshold: f32) -> f32 {
    return clamp(floor(c * palette + threshold - 0.5) / palette, 0.0, 1.0);
}

fn dither_channel(c: f32, x: u32, y: u32, palette: f32, dtype: i32, ch: u32) -> f32 {
    var threshold: f32;
    if (dtype == 1) {
        threshold = bayer8(x, y);
    } else if (dtype == 2) {
        threshold = blue_noise(x, y, ch);
    } else if (dtype == 3) {
        threshold = white_noise(x, y, ch);
    } else if (dtype == 4) {
        threshold = ign(x, y);
    } else if (dtype == 5) {
        let row = y % 4u;
        if (row == 0u) { threshold = 0.25; }
        else if (row == 1u) { threshold = 0.75; }
        else if (row == 2u) { threshold = 0.125; }
        else { threshold = 0.625; }
    } else if (dtype == 6) {
        threshold = select(0.25, 0.75, ((x ^ y) & 1u) == 1u);
    } else if (dtype == 7) {
        threshold = f32(x % 8u) / 8.0;
    } else if (dtype == 8) {
        threshold = white_noise(x, y, ch + 8u);
    } else if (dtype == 9) {
        let noise = white_noise(x, y, ch);
        return select(0.0, 1.0, c + (noise - 0.5) * 0.25 > 0.5);
    } else if (dtype == 10) {
        let ox = select(0u, 4u, (y & 1u) == 1u);
        threshold = bayer4((x + ox) % 4u, y);
    } else if (dtype == 11) {
        threshold = bayer4(x, y) * 0.75 + blue_noise(x, y, ch) * 0.25;
    } else if (dtype == 12) {
        threshold = blue_noise(x + ch * 2u, y, 0u);
    } else {
        threshold = bayer4(x, y);
    }
    return quantize_dither(c, palette, threshold);
}

// ── Matrix Mixer ─────────────────────────────────────────────────────────────

fn matrix_mix(fg: vec3<f32>, bg: vec3<f32>) -> vec3<f32> {
    var out_color = vec3<f32>(0.0);
    let fgR = vec3<f32>(fg.r);
    let fgG = vec3<f32>(fg.g);
    let fgB = vec3<f32>(fg.b);
    let scale = vec3<f32>(0.33, 0.33, 0.33);

    if (uniforms.matrix_mix_type == 0) { // lerp
        out_color.r = dot(mix(fgR, bg, uniforms.bg_into_fg_red.xyz),   scale);
        out_color.g = dot(mix(fgG, bg, uniforms.bg_into_fg_green.xyz), scale);
        out_color.b = dot(mix(fgB, bg, uniforms.bg_into_fg_blue.xyz),  scale);
    } else if (uniforms.matrix_mix_type == 1) { // add
        out_color.r = dot(fgR + uniforms.bg_into_fg_red.xyz   * bg, scale);
        out_color.g = dot(fgG + uniforms.bg_into_fg_green.xyz * bg, scale);
        out_color.b = dot(fgB + uniforms.bg_into_fg_blue.xyz  * bg, scale);
    } else if (uniforms.matrix_mix_type == 2) { // diff
        out_color.r = dot(abs(fgR - uniforms.bg_into_fg_red.xyz   * bg), scale);
        out_color.g = dot(abs(fgG - uniforms.bg_into_fg_green.xyz * bg), scale);
        out_color.b = dot(abs(fgB - uniforms.bg_into_fg_blue.xyz  * bg), scale);
    } else if (uniforms.matrix_mix_type == 3) { // mult
        out_color.r = dot(mix(fgR, bg * fgR, uniforms.bg_into_fg_red.xyz),   scale);
        out_color.g = dot(mix(fgG, bg * fgG, uniforms.bg_into_fg_green.xyz), scale);
        out_color.b = dot(mix(fgB, bg * fgB, uniforms.bg_into_fg_blue.xyz),  scale);
    } else if (uniforms.matrix_mix_type == 4) { // dodge
        out_color.r = dot(mix(fgR, bg / (1.00001 - fgR), uniforms.bg_into_fg_red.xyz),   scale);
        out_color.g = dot(mix(fgG, bg / (1.00001 - fgG), uniforms.bg_into_fg_green.xyz), scale);
        out_color.b = dot(mix(fgB, bg / (1.00001 - fgB), uniforms.bg_into_fg_blue.xyz),  scale);
    }

    if (uniforms.matrix_mix_overflow == 0) {
        out_color = clamp(out_color, vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (uniforms.matrix_mix_overflow == 1) {
        out_color = vec3<f32>(wrap01(out_color.x), wrap01(out_color.y), wrap01(out_color.z));
    } else if (uniforms.matrix_mix_overflow == 2) {
        out_color = vec3<f32>(fold01(out_color.x), fold01(out_color.y), fold01(out_color.z));
    }
    return out_color;
}

// ── Final Mix and Key ─────────────────────────────────────────────────────────

fn final_mix(fg: vec4<f32>, bg: vec4<f32>) -> vec4<f32> {
    var out_color = fg;

    if (uniforms.final_mix_type == 0) { // lerp
        out_color = mix(fg, bg, uniforms.final_mix_amount);
    } else if (uniforms.final_mix_type == 1) { // add/sub
        out_color = vec4<f32>(fg.rgb + uniforms.final_mix_amount * bg.rgb, 1.0);
    } else if (uniforms.final_mix_type == 2) { // diff
        out_color = vec4<f32>(abs(fg.rgb - uniforms.final_mix_amount * bg.rgb), 1.0);
    } else if (uniforms.final_mix_type == 3) { // mult
        out_color = vec4<f32>(mix(fg.rgb, fg.rgb * bg.rgb, uniforms.final_mix_amount), 1.0);
    } else if (uniforms.final_mix_type == 4) { // dodge
        out_color = vec4<f32>(mix(fg.rgb, fg.rgb / (1.00001 - bg.rgb), uniforms.final_mix_amount), 1.0);
    }

    // Overflow
    if (uniforms.final_mix_overflow == 0) {
        out_color = vec4<f32>(clamp(out_color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    } else if (uniforms.final_mix_overflow == 1) {
        out_color = vec4<f32>(wrap01(out_color.r), wrap01(out_color.g), wrap01(out_color.b), 1.0);
    } else if (uniforms.final_mix_overflow == 2) {
        out_color = vec4<f32>(fold01(out_color.r), fold01(out_color.g), fold01(out_color.b), 1.0);
    }

    // Chroma key (smoothstep, consistent with block_a)
    if (uniforms.final_key_threshold > 0.001) {
        let chroma_dist = distance(uniforms.final_key_value.xyz, fg.rgb);
        if (chroma_dist < uniforms.final_key_threshold) {
            let key_amount = smoothstep(
                uniforms.final_key_threshold,
                uniforms.final_key_threshold * (1.0 - uniforms.final_key_soft),
                chroma_dist
            );
            out_color = mix(out_color, bg, key_amount);
        }
    }

    return out_color;
}

@fragment
fn fs_main(@location(0) texcoord: vec2<f32>) -> @location(0) vec4<f32> {
    let uv = texcoord;
    let px = u32(texcoord.x * uniforms.width);
    let py = u32(texcoord.y * uniforms.height);

    // block1_tex / block2_tex are render targets (top-to-bottom) — raw texcoord.
    // ── Block1 geo re-processing ───────────────────────────────────────────
    var b1 = texcoord;
    b1 = do_kaleidoscope(b1, uniforms.block1_kaleidoscope, uniforms.block1_kaleidoscope_slice);
    b1 += uniforms.block1_xy_displace;
    b1 -= vec2<f32>(0.5);
    b1 *= uniforms.block1_z_displace;
    b1 += vec2<f32>(0.5);
    b1 = do_rotate(b1, uniforms.block1_rotate);
    b1 = shear_coord(b1, uniforms.block1_shear_matrix);

    var block1_color = blur_and_sharpen_c(block1_tex, block1_sampler, b1,
        uniforms.block1_sharpen_amount, uniforms.block1_sharpen_radius, uniforms.block1_filters_boost,
        uniforms.block1_blur_radius, uniforms.block1_blur_amount);

    // ── Block2 geo re-processing ───────────────────────────────────────────
    var b2 = texcoord;
    b2 = do_kaleidoscope(b2, uniforms.block2_kaleidoscope, uniforms.block2_kaleidoscope_slice);
    b2 += uniforms.block2_xy_displace;
    b2 -= vec2<f32>(0.5);
    b2 *= uniforms.block2_z_displace;
    b2 += vec2<f32>(0.5);
    b2 = do_rotate(b2, uniforms.block2_rotate);
    b2 = shear_coord(b2, uniforms.block2_shear_matrix);

    var block2_color = blur_and_sharpen_c(block2_tex, block2_sampler, b2,
        uniforms.block2_sharpen_amount, uniforms.block2_sharpen_radius, uniforms.block2_filters_boost,
        uniforms.block2_blur_radius, uniforms.block2_blur_amount);

    // ── Colorize Block 1 ───────────────────────────────────────────────────
    if (uniforms.block1_switches != 0u) {
        block1_color = vec4<f32>(apply_colorize(
            block1_color.rgb,
            uniforms.block1_colorize_mode,
            uniforms.block1_colorize_band1.xyz,
            uniforms.block1_colorize_band2.xyz,
            uniforms.block1_colorize_band3.xyz,
            uniforms.block1_colorize_band4.xyz,
            uniforms.block1_colorize_band5.xyz
        ), block1_color.a);
    }

    // ── Dither Block 1 ─────────────────────────────────────────────────────
    if (uniforms.block1_dither > 0.001) {
        block1_color = vec4<f32>(
            dither_channel(block1_color.r, px, py, uniforms.block1_dither, uniforms.block1_dither_type, 0u),
            dither_channel(block1_color.g, px, py, uniforms.block1_dither, uniforms.block1_dither_type, 1u),
            dither_channel(block1_color.b, px, py, uniforms.block1_dither, uniforms.block1_dither_type, 2u),
            block1_color.a
        );
    }

    // ── Colorize Block 2 ───────────────────────────────────────────────────
    if (uniforms.block2_switches != 0u) {
        block2_color = vec4<f32>(apply_colorize(
            block2_color.rgb,
            uniforms.block2_colorize_mode,
            uniforms.block2_colorize_band1.xyz,
            uniforms.block2_colorize_band2.xyz,
            uniforms.block2_colorize_band3.xyz,
            uniforms.block2_colorize_band4.xyz,
            uniforms.block2_colorize_band5.xyz
        ), block2_color.a);
    }

    // ── Dither Block 2 ─────────────────────────────────────────────────────
    if (uniforms.block2_dither > 0.001) {
        block2_color = vec4<f32>(
            dither_channel(block2_color.r, px, py, uniforms.block2_dither, uniforms.block2_dither_type, 0u),
            dither_channel(block2_color.g, px, py, uniforms.block2_dither, uniforms.block2_dither_type, 1u),
            dither_channel(block2_color.b, px, py, uniforms.block2_dither, uniforms.block2_dither_type, 2u),
            block2_color.a
        );
    }

    // ── Determine fg/bg order ──────────────────────────────────────────────
    // final_key_order == 0: Block1 = FG, Block2 = BG (1 → 2)
    // final_key_order == 1: Block2 = FG, Block1 = BG (2 → 1)
    var fg = block1_color.rgb;
    var bg = block2_color.rgb;
    if (uniforms.final_key_order == 1) {
        fg = block2_color.rgb;
        bg = block1_color.rgb;
    }

    // ── Matrix Mixer ───────────────────────────────────────────────────────
    let mixed = matrix_mix(fg, bg);

    // ── Final Mix with keying ──────────────────────────────────────────────
    var final_color = final_mix(vec4<f32>(mixed, 1.0), vec4<f32>(bg, 1.0));

    // ── Final Dither ───────────────────────────────────────────────────────
    if (uniforms.final_dither > 0.001) {
        final_color = vec4<f32>(
            dither_channel(final_color.r, px, py, uniforms.final_dither, uniforms.final_dither_type, 0u),
            dither_channel(final_color.g, px, py, uniforms.final_dither, uniforms.final_dither_type, 1u),
            dither_channel(final_color.b, px, py, uniforms.final_dither, uniforms.final_dither_type, 2u),
            1.0
        );
    }

    return final_color;
}
