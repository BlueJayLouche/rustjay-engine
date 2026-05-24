
// Block 1 Shader - ported from BLUEJAY_WAAAVES shader1.frag

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
var<uniform> uniforms: BlockAUniforms;

// Input textures
@group(0) @binding(0)
var ch1_tex: texture_2d<f32>;
@group(0) @binding(1)
var ch1_sampler: sampler;
@group(0) @binding(2)
var ch2_tex: texture_2d<f32>;
@group(0) @binding(3)
var ch2_sampler: sampler;

// Feedback textures
@group(2) @binding(0)
var fb1_tex: texture_2d<f32>;
@group(2) @binding(1)
var fb1_sampler: sampler;
@group(2) @binding(2)
var temporal_tex: texture_2d<f32>;
@group(2) @binding(3)
var temporal_sampler: sampler;

// vec4<f32> is used for all Align16 Rust fields (both are 16 bytes, AlignOf 16).
// Only .xyz is meaningful; .w is the internal Rust pad byte.
struct BlockAUniforms {
    width: f32,
    height: f32,
    inv_width: f32,
    inv_height: f32,

    ch1_input_width: f32,
    ch1_input_height: f32,
    ch2_input_width: f32,
    ch2_input_height: f32,

    ch1_aspect: f32,
    ch1_crib_x: f32,
    ch1_scale: f32,
    ch1_hd_zcrib: f32,
    ch1_xy_displace: vec2<f32>,
    ch1_z_displace: f32,
    ch1_rotate: f32,
    ch1_hsb_attenuate: vec4<f32>,   // Align16 → vec4; use .xyz
    ch1_posterize: f32,
    ch1_posterize_inv: f32,
    ch1_kaleidoscope: f32,
    ch1_kaleidoscope_slice: f32,
    ch1_blur_amount: f32,
    ch1_blur_radius: f32,
    ch1_sharpen_amount: f32,
    ch1_sharpen_radius: f32,
    ch1_filters_boost: f32,
    ch1_switches: u32,
    ch1_geo_overflow: i32,
    ch1_hd_aspect_on: i32,
    _pad1: f32,

    ch2_mix_amount: f32,
    ch2_key_value: vec4<f32>,       // Align16 → vec4; use .xyz
    ch2_key_threshold: f32,
    ch2_key_soft: f32,
    ch2_mix_type: i32,
    ch2_mix_overflow: i32,
    ch2_key_order: i32,
    ch2_key_mode: i32,

    ch2_aspect: f32,
    ch2_crib_x: f32,
    ch2_scale: f32,
    ch2_hd_zcrib: f32,
    ch2_xy_displace: vec2<f32>,
    ch2_z_displace: f32,
    ch2_rotate: f32,
    ch2_hsb_attenuate: vec4<f32>,   // Align16 → vec4; use .xyz
    ch2_posterize: f32,
    ch2_posterize_inv: f32,
    ch2_kaleidoscope: f32,
    ch2_kaleidoscope_slice: f32,
    ch2_blur_amount: f32,
    ch2_blur_radius: f32,
    ch2_sharpen_amount: f32,
    ch2_sharpen_radius: f32,
    ch2_filters_boost: f32,
    ch2_switches: u32,
    ch2_geo_overflow: i32,
    ch2_hd_aspect_on: i32,
    _pad3: f32,

    fb1_mix_amount: f32,
    fb1_key_value: vec4<f32>,       // Align16 → vec4; use .xyz
    fb1_key_threshold: f32,
    fb1_key_soft: f32,
    fb1_mix_type: i32,
    fb1_mix_overflow: i32,
    fb1_key_order: i32,
    _pad4: i32,

    fb1_xy_displace: vec2<f32>,
    fb1_z_displace: f32,
    fb1_rotate: f32,
    fb1_shear_matrix: vec4<f32>,
    fb1_kaleidoscope: f32,
    fb1_kaleidoscope_slice: f32,
    fb1_hsb_offset: vec4<f32>,      // Align16 → vec4; use .xyz
    fb1_hue_shaper: f32,
    fb1_hsb_attenuate: vec4<f32>,   // Align16 → vec4; use .xyz
    fb1_hsb_powmap: vec4<f32>,      // Align16 → vec4; use .xyz
    fb1_posterize: f32,
    fb1_posterize_inv: f32,
    fb1_blur_amount: f32,
    fb1_blur_radius: f32,
    fb1_sharpen_amount: f32,
    fb1_sharpen_radius: f32,
    fb1_temporal1_amount: f32,
    fb1_temporal1_res: f32,
    fb1_temporal2_amount: f32,
    fb1_temporal2_res: f32,
    fb1_filters_boost: f32,
    fb1_switches: u32,
    fb1_rotate_mode: i32,
    fb1_geo_overflow: i32,
    _pad5: f32,

    ch1_input_select: i32,
    ch2_input_select: i32,
    // tail pad: Rust has [f32;3] here; WGSL rounds struct to 544 implicitly
}

const PI: f32 = 3.1415926535;
const TWO_PI: f32 = 6.2831855;

// Switch bit extraction
fn get_switch(switches: u32, bit: u32) -> bool {
    return (switches & (1u << bit)) != 0u;
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

// Apply hue shaping
fn apply_hue_shaper(in_hue: f32, shaper: f32) -> f32 {
    return fract(abs(in_hue + shaper * sin(in_hue * 0.3184713)));
}

// Color quantization
fn color_quantize(in_color: vec3<f32>, amount: f32, amount_inv: f32) -> vec3<f32> {
    var c = in_color * amount;
    c = floor(c);
    return c * amount_inv;
}

// Solarize effect
fn apply_solarize(in_bright: f32) -> f32 {
    if (in_bright > 0.5) {
        return 1.0 - in_bright;
    }
    return in_bright;
}

// Rotate coordinates — UV-space, aspect-preserving (mode 1)
fn do_rotate(coord: vec2<f32>, angle: f32) -> vec2<f32> {
    if (angle == 0.0) {
        return coord;
    }
    let centered = coord - vec2<f32>(0.5, 0.5);
    let c = cos(angle);
    let s = sin(angle);
    let rotated_x = centered.x * c - centered.y * s;
    let rotated_y = centered.x * s + centered.y * c;
    return vec2<f32>(rotated_x + 0.5, rotated_y + 0.5);
}

// Rotate in pixel-space (non-aspect-preserving, mode 0): stretches x by aspect
// ratio before rotating, producing the same "always circular" distortion as the
// original GLSL mode 0 which operated in raw pixel coordinates.
fn do_rotate_mode0(coord: vec2<f32>, angle: f32) -> vec2<f32> {
    if (angle == 0.0) {
        return coord;
    }
    let aspect = uniforms.width / uniforms.height;
    let centered = coord - vec2<f32>(0.5, 0.5);
    let c = cos(angle);
    let s = sin(angle);
    let sx = centered.x * aspect;
    let rx = sx * c - centered.y * s;
    let ry = sx * s + centered.y * c;
    return vec2<f32>(rx / aspect + 0.5, ry + 0.5);
}

// Kaleidoscope effect — asymmetric rotation matching original shader3:
// pre-rotation uses mode 1 (aspect-preserving), counter-rotation uses mode 0
// (pixel-space stretch), which creates the characteristic "twist" distortion.
fn do_kaleidoscope(coord: vec2<f32>, segments: f32, slice: f32) -> vec2<f32> {
    if (segments <= 0.0) {
        return coord;
    }
    var result = do_rotate(coord, slice);
    let centered = result * 2.0 - 1.0;
    let radius = length(centered);
    var angle = atan2(centered.y, centered.x);
    let segment_angle = TWO_PI / segments;
    angle = angle - segment_angle * floor(angle / segment_angle);
    angle = min(angle, segment_angle - angle);
    result = radius * vec2<f32>(cos(angle), sin(angle));
    result = result * 0.5 + 0.5;
    return do_rotate_mode0(result, -slice);
}

// Wrap coordinates
fn wrap_coord(coord: vec2<f32>) -> vec2<f32> {
        return fract(coord);
    }

// Mirror function
fn mirror_val(a: f32) -> f32 {
    if (a > 0.0) {
        return a;
    }
    return -(1.0 + a);
}

// Mirror coordinates
fn mirror_coord(coord: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(
        1.0 - mirror_val(coord.x % 2.0 - 1.0),
        1.0 - mirror_val(coord.y % 2.0 - 1.0)
    );
}

// Blur and sharpen function
fn blur_and_sharpen(tex: texture_2d<f32>, tex_sampler: sampler, coord: vec2<f32>,
                    sharpen_amount: f32, sharpen_radius: f32, sharpen_boost: f32,
                    blur_radius: f32, blur_amount: f32) -> vec4<f32> {
    let original_color = textureSample(tex, tex_sampler, coord);

    // Early exit if no filters
    if (blur_amount < 0.001 && sharpen_amount < 0.001) {
        return original_color;
    }

    let blur_size = vec2<f32>(blur_radius) * vec2<f32>(uniforms.inv_width, uniforms.inv_height);
    let sharpen_size = vec2<f32>(sharpen_radius) * vec2<f32>(uniforms.inv_width, uniforms.inv_height);
    
    // Box blur (8 samples)
    var color_blur = original_color;
    if (blur_amount >= 0.001) {
        color_blur = textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>( 1.0, 1.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>( 0.0, 1.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>(-1.0, 1.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>(-1.0, 0.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>(-1.0, -1.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>( 0.0, -1.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>( 1.0, -1.0))
                   + textureSample(tex, tex_sampler, coord + blur_size * vec2<f32>( 1.0, 0.0));
        color_blur *= 0.125;
        color_blur = mix(original_color, color_blur, blur_amount);
    }
    
    // Sharpen
    var color_blur_hsb = rgb2hsb(color_blur.rgb);
    if (sharpen_amount >= 0.001) {
        let lum_weights = vec3<f32>(0.299, 0.587, 0.114);
        var color_sharpen_bright = 
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>( 1.0, 0.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>(-1.0, 0.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>( 0.0, 1.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>( 0.0, -1.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>( 1.0, 1.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>(-1.0, 1.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>( 1.0, -1.0)).rgb, lum_weights) +
            dot(textureSample(tex, tex_sampler, coord + sharpen_size * vec2<f32>(-1.0, -1.0)).rgb, lum_weights);
        color_sharpen_bright *= 0.125;
        color_blur_hsb.z -= sharpen_amount * color_sharpen_bright;
    }
    
    // Boost
    let boost_factor = mix(1.0, 1.0 + sharpen_amount + sharpen_boost, step(0.001, sharpen_amount));
    color_blur_hsb.z *= boost_factor;
    
    // Clamp HSB values before converting back to RGB
    color_blur_hsb.x = fract(color_blur_hsb.x);
    color_blur_hsb.y = clamp(color_blur_hsb.y, 0.0, 1.0);
    color_blur_hsb.z = clamp(color_blur_hsb.z, 0.0, 1.0);
    
    return vec4<f32>(hsb2rgb(color_blur_hsb), 1.0);
}

// Shear transformation
fn shear_coord(coord: vec2<f32>, shear_matrix: vec4<f32>) -> vec2<f32> {
    if (shear_matrix.x == 1.0 && shear_matrix.y == 0.0 && shear_matrix.z == 0.0 && shear_matrix.w == 1.0) {
        return coord;
    }
    let center = vec2<f32>(0.5, 0.5);
    var result = coord - center;
    let rx = shear_matrix.x * result.x + shear_matrix.y * result.y;
    let ry = shear_matrix.z * result.x + shear_matrix.w * result.y;
    return vec2<f32>(rx, ry) + center;
}

// Wrap value 0-1
fn wrap01(v: f32) -> f32 {
    if (v < 0.0) {
        return 1.0 - abs(v);
    }
    if (v > 1.0) {
        return fract(v);
    }
    return v;
}

// Foldover
fn fold01(v: f32) -> f32 {
    var r = v;
    if (r < 0.0) {
        r = abs(r);
    }
    if (r > 1.0) {
        r = 1.0 - fract(r);
    }
    if (r < 0.0) {
        r = abs(r);
    }
    return r;
}

// Calculate key mix amount based on chroma distance
fn calculate_key_mix(color: vec3<f32>, key_value: vec3<f32>, threshold: f32, softness: f32) -> f32 {
    if (threshold < 0.001) {
        return 0.0;
    }
    let chroma_distance = distance(key_value, color);
    if (chroma_distance < threshold) {
        // Key amount increases as we get closer to the key color
        return smoothstep(threshold, threshold * (1.0 - softness), chroma_distance);
    }
    return 0.0;
}

// Mix two colors with a specific blend mode and overflow handling
fn mix_with_mode(fg: vec4<f32>, bg: vec4<f32>, amount: f32, mix_type: i32, mix_overflow: i32) -> vec4<f32> {
    var out_rgb: vec3<f32>;
    
    // Mix modes
    switch(mix_type) {
        case 0: { // lerp
            out_rgb = mix(fg.rgb, bg.rgb, amount);
        }
        case 1: { // add
            out_rgb = fg.rgb + amount * bg.rgb;
        }
        case 2: { // diff
            out_rgb = abs(fg.rgb - amount * bg.rgb);
        }
        case 3: { // mult
            out_rgb = mix(fg.rgb, fg.rgb * bg.rgb, amount);
        }
        case 4: { // dodge
            out_rgb = mix(fg.rgb, fg.rgb / (1.00001 - bg.rgb), amount);
        }
        default: {
            out_rgb = mix(fg.rgb, bg.rgb, amount);
        }
    }
    
    // Overflow modes
    switch(mix_overflow) {
        case 0: { // clamp
            out_rgb = clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        }
        case 1: { // wrap
            out_rgb = vec3<f32>(wrap01(out_rgb.x), wrap01(out_rgb.y), wrap01(out_rgb.z));
        }
        case 2: { // fold
            out_rgb = vec3<f32>(fold01(out_rgb.x), fold01(out_rgb.y), fold01(out_rgb.z));
        }
        default: {
            out_rgb = clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0));
        }
    }
    
    return vec4<f32>(out_rgb, 1.0);
}

// Mix and key function with OF-style integrated keying
// key_order: 0=Key First Then Mix, 1=Mix First Then Key
// mix_type: 0=lerp, 1=add, 2=diff, 3=mult, 4=dodge
fn mix_and_key(fg: vec4<f32>, bg: vec4<f32>, amount: f32, mix_type: i32, 
               key_threshold: f32, key_soft: f32, key_value: vec3<f32>,
               key_order: i32, mix_overflow: i32) -> vec4<f32> {
    
    var out_color: vec4<f32>;
    
    if (key_order == 0) {
        // Key First Then Mix: Key the foreground, then mix with background
        let key_amount = calculate_key_mix(fg.rgb, key_value, key_threshold, key_soft);
        let keyed_fg = mix(fg, bg, key_amount);
        out_color = mix_with_mode(keyed_fg, bg, amount, mix_type, mix_overflow);
    } else {
        // Mix First Then Key: Mix first, then key the result
        out_color = mix_with_mode(fg, bg, amount, mix_type, mix_overflow);
        let key_amount = calculate_key_mix(out_color.rgb, key_value, key_threshold, key_soft);
        out_color = mix(out_color, bg, key_amount);
    }
    
    return out_color;
}

// Process channel
fn process_channel(uv: vec2<f32>, coords: vec2<f32>, 
                   tex: texture_2d<f32>, tex_sampler: sampler,
                   input_width: f32, input_height: f32,
                   aspect: f32, crib_x: f32, scale: f32, hd_zcrib: f32,
                   xy_displace: vec2<f32>, z_displace: f32, rotate: f32,
                   hsb_attenuate: vec3<f32>, posterize: f32, posterize_inv: f32,
                   kaleidoscope: f32, kaleidoscope_slice: f32,
                   blur_amount: f32, blur_radius: f32, 
                   sharpen_amount: f32, sharpen_radius: f32, filters_boost: f32,
                   switches: u32, geo_overflow: i32, hd_aspect_on: i32) -> vec4<f32> {
    
    var ch_coords = coords;
    
    // Apply aspect ratio
    ch_coords.x *= aspect;
    ch_coords.x -= crib_x;
    
    // Scale around center
    ch_coords -= vec2<f32>(0.5, 0.5);
    ch_coords *= scale + hd_zcrib;
    ch_coords += vec2<f32>(0.5, 0.5);
    
    // HD aspect fix
    if (hd_aspect_on == 1) {
        ch_coords = uv * vec2<f32>(1.0, 1.0);
    }
    
    // H/V Flip
    if (get_switch(switches, 2u)) { // h_flip
        ch_coords.x = 1.0 - ch_coords.x;
    }
    if (get_switch(switches, 3u)) { // v_flip
        ch_coords.y = 1.0 - ch_coords.y;
    }
    
    // H/V Mirror
    if (get_switch(switches, 0u)) { // h_mirror
        if (ch_coords.x > 0.5) {
            ch_coords.x = abs(1.0 - ch_coords.x);
        }
    }
    if (get_switch(switches, 1u)) { // v_mirror
        if (ch_coords.y > 0.5) {
            ch_coords.y = abs(1.0 - ch_coords.y);
        }
    }
    
    // Kaleidoscope
    ch_coords = do_kaleidoscope(ch_coords, kaleidoscope, kaleidoscope_slice);
    
    // Displace
    ch_coords += xy_displace;
    
    // Z displace (zoom)
    ch_coords -= vec2<f32>(0.5, 0.5);
    ch_coords *= z_displace;
    ch_coords += vec2<f32>(0.5, 0.5);
    
    // Rotate
    ch_coords = do_rotate(ch_coords, rotate);
    
    // Geo overflow
    if (geo_overflow == 1) {
        ch_coords = wrap_coord(ch_coords);
    } else if (geo_overflow == 2) {
        ch_coords = mirror_coord(ch_coords);
    }
    
    // Sample with blur/sharpen
    // For video input, simply stretch to fit the output (no special scaling)
    let ch_uv = ch_coords;
    var ch_color = blur_and_sharpen(tex, tex_sampler, ch_uv,
                                     sharpen_amount, sharpen_radius, filters_boost,
                                     blur_radius, blur_amount);
    
    // Clamp if no overflow
    if (geo_overflow == 0) {
        if (ch_coords.x > 1.0 || ch_coords.y > 1.0 || 
            ch_coords.x < 0.0 || ch_coords.y < 0.0) {
            ch_color = vec4<f32>(0.0);
        }
    }
    
    // HSB processing with early exit optimization
    // Skip HSB conversion if no HSB operations needed
    let needs_hsb = hsb_attenuate.x != 1.0 || hsb_attenuate.y != 1.0 || hsb_attenuate.z != 1.0 ||
                    get_switch(switches, 4u) || get_switch(switches, 5u) || get_switch(switches, 6u) ||
                    get_switch(switches, 8u); // solarize
    
    var ch_rgb = ch_color.rgb;
    if (needs_hsb) {
        var ch_hsb = rgb2hsb(ch_color.rgb);
        ch_hsb = pow(ch_hsb, hsb_attenuate);
        
        // Inverts
        if (get_switch(switches, 4u)) { // hue_invert
            ch_hsb.x = 1.0 - ch_hsb.x;
        }
        if (get_switch(switches, 5u)) { // sat_invert
            ch_hsb.y = 1.0 - ch_hsb.y;
        }
        if (get_switch(switches, 6u)) { // bright_invert
            ch_hsb.z = 1.0 - ch_hsb.z;
        }
        
        ch_hsb.x = fract(ch_hsb.x);
        
        // Solarize
        if (get_switch(switches, 8u)) { // solarize
            ch_hsb.z = apply_solarize(ch_hsb.z);
        }
        
        // Clamp saturation and brightness before converting back to RGB
        ch_hsb.y = clamp(ch_hsb.y, 0.0, 1.0);
        ch_hsb.z = clamp(ch_hsb.z, 0.0, 1.0);
        
        ch_rgb = hsb2rgb(ch_hsb);
    }
    
    // RGB invert
    if (get_switch(switches, 7u)) { // rgb_invert
        ch_rgb = 1.0 - ch_rgb;
    }
    
    // Posterize
    if (get_switch(switches, 9u)) { // posterize_switch
        ch_rgb = color_quantize(ch_rgb, posterize, posterize_inv);
    }
    
    ch_color = vec4<f32>(ch_rgb, ch_color.a);
    
    return ch_color;
}

@fragment
fn fs_main(@location(0) texcoord: vec2<f32>) -> @location(0) vec4<f32> {
    // All textures in this engine (webcam, render targets, ring buffers) are
    // stored top-to-bottom.  No Y flip needed — sample with raw texcoord.
    let uv = texcoord;
    let coords = texcoord;
    
    // === CHANNEL 1 Processing ===
    // ch1_input_select: 0=Input1 (ch1_tex), 1=Input2 (ch2_tex)
    var ch1_color: vec4<f32>;
    let ch1_iw = select(uniforms.ch1_input_width, uniforms.ch2_input_width, uniforms.ch1_input_select != 0);
    let ch1_ih = select(uniforms.ch1_input_height, uniforms.ch2_input_height, uniforms.ch1_input_select != 0);
    if (uniforms.ch1_input_select == 0) {
        ch1_color = process_channel(
            uv, uv, ch1_tex, ch1_sampler,
            ch1_iw, ch1_ih,
            uniforms.ch1_aspect, uniforms.ch1_crib_x, uniforms.ch1_scale, uniforms.ch1_hd_zcrib,
            uniforms.ch1_xy_displace, uniforms.ch1_z_displace, uniforms.ch1_rotate,
            uniforms.ch1_hsb_attenuate.xyz, uniforms.ch1_posterize, uniforms.ch1_posterize_inv,
            uniforms.ch1_kaleidoscope, uniforms.ch1_kaleidoscope_slice,
            uniforms.ch1_blur_amount, uniforms.ch1_blur_radius,
            uniforms.ch1_sharpen_amount, uniforms.ch1_sharpen_radius, uniforms.ch1_filters_boost,
            uniforms.ch1_switches, uniforms.ch1_geo_overflow, uniforms.ch1_hd_aspect_on
        );
    } else {
        ch1_color = process_channel(
            uv, uv, ch2_tex, ch2_sampler,
            ch1_iw, ch1_ih,
            uniforms.ch1_aspect, uniforms.ch1_crib_x, uniforms.ch1_scale, uniforms.ch1_hd_zcrib,
            uniforms.ch1_xy_displace, uniforms.ch1_z_displace, uniforms.ch1_rotate,
            uniforms.ch1_hsb_attenuate.xyz, uniforms.ch1_posterize, uniforms.ch1_posterize_inv,
            uniforms.ch1_kaleidoscope, uniforms.ch1_kaleidoscope_slice,
            uniforms.ch1_blur_amount, uniforms.ch1_blur_radius,
            uniforms.ch1_sharpen_amount, uniforms.ch1_sharpen_radius, uniforms.ch1_filters_boost,
            uniforms.ch1_switches, uniforms.ch1_geo_overflow, uniforms.ch1_hd_aspect_on
        );
    }

    var ch1_final_color = ch1_color;

    // === CHANNEL 2 Processing ===
    // ch2_input_select: 0=Input1 (ch1_tex), 1=Input2 (ch2_tex)
    var ch2_color: vec4<f32>;
    let ch2_iw = select(uniforms.ch1_input_width, uniforms.ch2_input_width, uniforms.ch2_input_select != 0);
    let ch2_ih = select(uniforms.ch1_input_height, uniforms.ch2_input_height, uniforms.ch2_input_select != 0);
    if (uniforms.ch2_input_select == 0) {
        ch2_color = process_channel(
            uv, uv, ch1_tex, ch1_sampler,
            ch2_iw, ch2_ih,
            uniforms.ch2_aspect, uniforms.ch2_crib_x, uniforms.ch2_scale, uniforms.ch2_hd_zcrib,
            uniforms.ch2_xy_displace, uniforms.ch2_z_displace, uniforms.ch2_rotate,
            uniforms.ch2_hsb_attenuate.xyz, uniforms.ch2_posterize, uniforms.ch2_posterize_inv,
            uniforms.ch2_kaleidoscope, uniforms.ch2_kaleidoscope_slice,
            uniforms.ch2_blur_amount, uniforms.ch2_blur_radius,
            uniforms.ch2_sharpen_amount, uniforms.ch2_sharpen_radius, uniforms.ch2_filters_boost,
            uniforms.ch2_switches, uniforms.ch2_geo_overflow, uniforms.ch2_hd_aspect_on
        );
    } else {
        ch2_color = process_channel(
            uv, uv, ch2_tex, ch2_sampler,
            ch2_iw, ch2_ih,
            uniforms.ch2_aspect, uniforms.ch2_crib_x, uniforms.ch2_scale, uniforms.ch2_hd_zcrib,
            uniforms.ch2_xy_displace, uniforms.ch2_z_displace, uniforms.ch2_rotate,
            uniforms.ch2_hsb_attenuate.xyz, uniforms.ch2_posterize, uniforms.ch2_posterize_inv,
            uniforms.ch2_kaleidoscope, uniforms.ch2_kaleidoscope_slice,
            uniforms.ch2_blur_amount, uniforms.ch2_blur_radius,
            uniforms.ch2_sharpen_amount, uniforms.ch2_sharpen_radius, uniforms.ch2_filters_boost,
            uniforms.ch2_switches, uniforms.ch2_geo_overflow, uniforms.ch2_hd_aspect_on
        );
    }

    // === Mix CH1 and CH2 ===
    var mixed_color = mix_and_key(
        ch1_final_color, ch2_color, uniforms.ch2_mix_amount, uniforms.ch2_mix_type,
        uniforms.ch2_key_threshold, uniforms.ch2_key_soft, uniforms.ch2_key_value.xyz,
        uniforms.ch2_key_order, uniforms.ch2_mix_overflow
    );
    
    // === FB1 Processing ===
    // fb1 is a render target (ring buffer copy of intermediate_a) — top-to-bottom,
    // so sample with raw texcoord, not the video-input-correcting uv.
    var fb1_coords = texcoord;
    
    // FB1 H/V Flip
    if (get_switch(uniforms.fb1_switches, 2u)) { // fb1_h_flip
        fb1_coords.x = 1.0 - fb1_coords.x;
    }
    if (get_switch(uniforms.fb1_switches, 3u)) { // fb1_v_flip
        fb1_coords.y = 1.0 - fb1_coords.y;
    }
    
    // FB1 H/V Mirror
    if (get_switch(uniforms.fb1_switches, 0u)) { // fb1_h_mirror
        if (fb1_coords.x > 0.5) {
            fb1_coords.x = abs(1.0 - fb1_coords.x);
        }
    }
    if (get_switch(uniforms.fb1_switches, 1u)) { // fb1_v_mirror
        if (fb1_coords.y > 0.5) {
            fb1_coords.y = abs(1.0 - fb1_coords.y);
        }
    }
    
    // FB1 Kaleidoscope
    fb1_coords = do_kaleidoscope(fb1_coords, uniforms.fb1_kaleidoscope, uniforms.fb1_kaleidoscope_slice);
    
    // FB1 Displace
    fb1_coords += uniforms.fb1_xy_displace;
    
    // FB1 Z displace
    fb1_coords -= vec2<f32>(0.5, 0.5);
    fb1_coords *= uniforms.fb1_z_displace;
    fb1_coords += vec2<f32>(0.5, 0.5);
    
    // FB1 Rotate
    fb1_coords = do_rotate(fb1_coords, uniforms.fb1_rotate);
    
    // FB1 Shear
    fb1_coords = shear_coord(fb1_coords, uniforms.fb1_shear_matrix);
    
    // FB1 Geo overflow
    if (uniforms.fb1_geo_overflow == 1) {
        fb1_coords = wrap_coord(fb1_coords);
    } else if (uniforms.fb1_geo_overflow == 2) {
        fb1_coords = mirror_coord(fb1_coords);
    }
    
    // Sample FB1
    var fb1_color = blur_and_sharpen(fb1_tex, fb1_sampler, fb1_coords,
                                      uniforms.fb1_sharpen_amount, uniforms.fb1_sharpen_radius, 
                                      uniforms.fb1_filters_boost,
                                      uniforms.fb1_blur_radius, uniforms.fb1_blur_amount);
    
    // Clamp FB1 if no overflow
    if (uniforms.fb1_geo_overflow == 0) {
        if (fb1_coords.x > 1.0 || fb1_coords.y > 1.0 ||
            fb1_coords.x < 0.0 || fb1_coords.y < 0.0) {
            fb1_color = vec4<f32>(0.0);
        }
    }
    
    // FB1 HSB processing — skip if all HSB ops are identity (REQ-11.2)
    let fb1_needs_hsb =
        uniforms.fb1_hsb_offset.x != 0.0 || uniforms.fb1_hsb_offset.y != 0.0 || uniforms.fb1_hsb_offset.z != 0.0 ||
        uniforms.fb1_hsb_attenuate.x != 1.0 || uniforms.fb1_hsb_attenuate.y != 1.0 || uniforms.fb1_hsb_attenuate.z != 1.0 ||
        uniforms.fb1_hsb_powmap.x != 1.0 || uniforms.fb1_hsb_powmap.y != 1.0 || uniforms.fb1_hsb_powmap.z != 1.0 ||
        uniforms.fb1_hue_shaper != 1.0 ||
        get_switch(uniforms.fb1_switches, 4u) || get_switch(uniforms.fb1_switches, 5u) || get_switch(uniforms.fb1_switches, 6u);
    var fb1_rgb = fb1_color.rgb;
    if (fb1_needs_hsb) {
        var fb1_hsb = rgb2hsb(fb1_color.rgb);
        fb1_hsb += uniforms.fb1_hsb_offset.xyz;
        fb1_hsb = pow(fb1_hsb, uniforms.fb1_hsb_attenuate.xyz);
        fb1_hsb = pow(fb1_hsb, uniforms.fb1_hsb_powmap.xyz);
        fb1_hsb.x = apply_hue_shaper(fb1_hsb.x, uniforms.fb1_hue_shaper);

        if (get_switch(uniforms.fb1_switches, 4u)) { fb1_hsb.x = 1.0 - fb1_hsb.x; }
        if (get_switch(uniforms.fb1_switches, 5u)) { fb1_hsb.y = 1.0 - fb1_hsb.y; }
        if (get_switch(uniforms.fb1_switches, 6u)) { fb1_hsb.z = 1.0 - fb1_hsb.z; }

        fb1_hsb.x = fract(fb1_hsb.x);
        fb1_hsb.y = clamp(fb1_hsb.y, 0.0, 1.0);
        fb1_hsb.z = clamp(fb1_hsb.z, 0.0, 1.0);
        fb1_rgb = hsb2rgb(fb1_hsb);
    }

    // FB1 Posterize
    if (get_switch(uniforms.fb1_switches, 9u)) {
        fb1_rgb = color_quantize(fb1_rgb, uniforms.fb1_posterize, uniforms.fb1_posterize_inv);
    }

    fb1_color = vec4<f32>(fb1_rgb, fb1_color.a);

    // === Mix with FB1 ===
    var final_color = mix_and_key(
        mixed_color, fb1_color, uniforms.fb1_mix_amount, uniforms.fb1_mix_type,
        uniforms.fb1_key_threshold, uniforms.fb1_key_soft, uniforms.fb1_key_value.xyz,
        uniforms.fb1_key_order, uniforms.fb1_mix_overflow
    );
    
    return final_color;
}
