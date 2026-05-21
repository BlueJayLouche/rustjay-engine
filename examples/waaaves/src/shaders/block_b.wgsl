
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
var<uniform> uniforms: BlockBUniforms;

@group(0) @binding(0)
var input_tex: texture_2d<f32>;
@group(0) @binding(1)
var input_sampler: sampler;
@group(2) @binding(0)
var fb2_tex: texture_2d<f32>;
@group(2) @binding(1)
var fb2_sampler: sampler;
@group(2) @binding(2)
var temporal_tex: texture_2d<f32>;
@group(2) @binding(3)
var temporal_sampler: sampler;

struct BlockBUniforms {
    width: f32,
    height: f32,
    inv_width: f32,
    inv_height: f32,
    
    input_aspect: f32,
    input_crib_x: f32,
    input_scale: f32,
    input_hd_zcrib: f32,
    input_xy_displace: vec2<f32>,
    input_z_displace: f32,
    input_rotate: f32,
    input_hsb_attenuate: vec4<f32>,
    input_posterize: f32,
    input_posterize_inv: f32,
    input_kaleidoscope: f32,
    input_kaleidoscope_slice: f32,
    input_blur_amount: f32,
    input_blur_radius: f32,
    input_sharpen_amount: f32,
    input_sharpen_radius: f32,
    input_filters_boost: f32,
    input_switches: u32,
    input_posterize_switch: i32,
    input_solarize: i32,
    input_geo_overflow: i32,
    input_hd_aspect_on: i32,
    _pad1: f32,
    
    fb2_mix_amount: f32,
    fb2_key_value: vec4<f32>,
    fb2_key_threshold: f32,
    fb2_key_soft: f32,
    fb2_mix_type: i32,
    fb2_mix_overflow: i32,
    fb2_key_order: i32,
    _pad2: f32,
    
    fb2_xy_displace: vec2<f32>,
    fb2_z_displace: f32,
    fb2_rotate: f32,
    fb2_shear_matrix: vec4<f32>,
    fb2_kaleidoscope: f32,
    fb2_kaleidoscope_slice: f32,
    fb2_hsb_offset: vec4<f32>,
    fb2_hsb_attenuate: vec4<f32>,
    fb2_hsb_powmap: vec4<f32>,
    fb2_hue_shaper: f32,
    fb2_posterize: f32,
    fb2_posterize_inv: f32,
    fb2_blur_amount: f32,
    fb2_blur_radius: f32,
    fb2_sharpen_amount: f32,
    fb2_sharpen_radius: f32,
    fb2_temporal1_amount: f32,
    fb2_temporal1_res: f32,
    fb2_temporal2_amount: f32,
    fb2_temporal2_res: f32,
    fb2_filters_boost: f32,
    fb2_switches: u32,
    fb2_posterize_switch: i32,
    fb2_rotate_mode: i32,
    fb2_geo_overflow: i32,
    
    // Input selection (0=block1, 1=input1, 2=input2)
    block2_input_select: i32,
    // tail padding to 352 bytes is implicit
}

// Helper functions
const TWO_PI: f32 = 6.28318530718;

fn get_switch(switches: u32, bit: u32) -> bool {
    return (switches & (1u << bit)) != 0u;
}

fn rgb2hsb(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(0.0, -1.0 / 3.0, 2.0 / 3.0, -1.0);
    let p = mix(vec4<f32>(c.bg, K.wz), vec4<f32>(c.gb, K.xy), step(c.b, c.g));
    let q = mix(vec4<f32>(p.xyw, c.r), vec4<f32>(c.r, p.yzx), step(p.x, c.r));
    let d = q.x - min(q.w, q.y);
    let e = 1.0e-10;
    return vec3<f32>(abs(q.z + (q.w - q.y) / (6.0 * d + e)), d / (q.x + e), q.x);
}

fn hsb2rgb(c: vec3<f32>) -> vec3<f32> {
    let K = vec4<f32>(1.0, 2.0 / 3.0, 1.0 / 3.0, 3.0);
    let p = abs(fract(c.xxx + K.xyz) * 6.0 - K.www);
    return c.z * mix(K.xxx, clamp(p - K.xxx, vec3<f32>(0.0), vec3<f32>(1.0)), c.y);
}

fn apply_hue_shaper(hue: f32, shaper: f32) -> f32 {
    if (shaper <= 0.0) { return 0.0; }
    let base_hue = fract(abs(hue));
    return pow(base_hue, shaper);
}

fn do_rotate(coord: vec2<f32>, angle: f32, mode: i32) -> vec2<f32> {
    if (angle == 0.0) {
        return coord;
    }
    let c = cos(angle);
    let s = sin(angle);

    var rotate_coord = vec2<f32>(0.0, 0.0);

    // Mode 0: spiral effect (original)
    if (mode == 0) {
        let delta = coord - vec2<f32>(0.5, 0.5);
        rotate_coord.x = delta.x * c - delta.y * s + 0.5;
        rotate_coord.y = delta.x * s + delta.y * c + 0.5;
    }
    // Mode 1: preserve aspect ratio
    else {
        let center_coord = coord - vec2<f32>(0.5, 0.5);
        rotate_coord.x = center_coord.x * c - center_coord.y * s + 0.5;
        rotate_coord.y = center_coord.x * s + center_coord.y * c + 0.5;
    }

    return rotate_coord;
}

fn do_kaleidoscope(coord: vec2<f32>, segments: f32, slice: f32) -> vec2<f32> {
    if (segments <= 0.0) {
        return coord;
    }
    var result = do_rotate(coord, slice, 1);
    let centered = result * 2.0 - 1.0;
    let radius = length(centered);
    var angle = atan2(centered.y, centered.x);
    let segment_angle = TWO_PI / segments;
    angle = angle - segment_angle * floor(angle / segment_angle);
    angle = min(angle, segment_angle - angle);
    result = radius * vec2<f32>(cos(angle), sin(angle));
    result = result * 0.5 + 0.5;
    return do_rotate(result, -slice, 1);
}

fn color_quantize(in_color: vec3<f32>, amount: f32, amount_inv: f32) -> vec3<f32> {
    var result = in_color * amount;
    result = floor(result);
    result = result * amount_inv;
    return result;
}

fn solarize(in_bright: f32) -> f32 {
    if (in_bright > 0.5) {
        return 1.0 - in_bright;
    }
    return in_bright;
}

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

fn wrap_coord(coord: vec2<f32>) -> vec2<f32> {
    return fract(coord);
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

fn blur_and_sharpen(tex: texture_2d<f32>, tex_sampler: sampler, coord: vec2<f32>,
                    sharpen_amount: f32, sharpen_radius: f32, sharpen_boost: f32,
                    blur_radius: f32, blur_amount: f32) -> vec4<f32> {
    let original_color = textureSample(tex, tex_sampler, coord);

    if (blur_amount < 0.001 && sharpen_amount < 0.001) {
        return original_color;
    }

    let blur_size = vec2<f32>(blur_radius) * vec2<f32>(uniforms.inv_width, uniforms.inv_height);
    let sharpen_size = vec2<f32>(sharpen_radius) * vec2<f32>(uniforms.inv_width, uniforms.inv_height);
    
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
    
    let boost_factor = mix(1.0, 1.0 + sharpen_amount + sharpen_boost, step(0.001, sharpen_amount));
    color_blur_hsb.z *= boost_factor;
    
    // Clamp HSB values before converting back to RGB
    color_blur_hsb.x = fract(color_blur_hsb.x);
    color_blur_hsb.y = clamp(color_blur_hsb.y, 0.0, 1.0);
    color_blur_hsb.z = clamp(color_blur_hsb.z, 0.0, 1.0);
    
    return vec4<f32>(hsb2rgb(color_blur_hsb), 1.0);
}

fn wrap_color(in_color: f32) -> f32 {
    var result = in_color;
    if (result < 0.0) {
        result = 1.0 - abs(result);
    }
    if (result > 1.0) {
        result = fract(result);
    }
    return result;
}

fn foldover(in_color: f32) -> f32 {
    var result = in_color;
    if (result < 0.0) {
        result = abs(result);
    }
    if (result > 1.0) {
        result = 1.0 - fract(result);
    }
    if (result < 0.0) {
        result = abs(result);
    }
    return result;
}

fn mix_and_key(fg: vec4<f32>, bg: vec4<f32>, amount: f32, mix_type: i32, 
               key_threshold: f32, key_soft: f32, key_value: vec3<f32>,
               key_order: i32, mix_overflow: i32) -> vec4<f32> {
    var foreground = fg;
    var background = bg;
    
    if (key_order == 1) {
        let temp = foreground;
        foreground = background;
        background = temp;
    }
    
    var out_color: vec3<f32>;
    
    // Mix modes: 0=lerp, 1=add/sub, 2=diff, 3=mult, 4=dodge, 5+=key
    if (mix_type == 0) { // Standard mix (lerp)
        out_color = mix(foreground.rgb, background.rgb, amount);
    } else if (mix_type == 1) { // Add/Subtract
        out_color = foreground.rgb + amount * background.rgb;
    } else if (mix_type == 2) { // Difference
        out_color = abs(foreground.rgb - amount * background.rgb);
    } else if (mix_type == 3) { // Multiply
        out_color = mix(foreground.rgb, foreground.rgb * background.rgb, amount);
    } else if (mix_type == 4) { // Dodge
        out_color = mix(foreground.rgb, foreground.rgb / (1.00001 - background.rgb), amount);
    } else {
        // Key mix (type 5+)
        let diff = length(foreground.rgb - key_value);
        let key = smoothstep(key_threshold - key_soft, key_threshold + key_soft, diff);
        out_color = mix(foreground.rgb, background.rgb, key * amount);
    }
    
    // Overflow handling: 0=clamp, 1=wrap, 2=fold
    if (mix_overflow == 0) {
        out_color = clamp(out_color, vec3<f32>(0.0), vec3<f32>(1.0));
    } else if (mix_overflow == 1) {
        out_color.r = wrap_color(out_color.r);
        out_color.g = wrap_color(out_color.g);
        out_color.b = wrap_color(out_color.b);
    } else if (mix_overflow == 2) {
        out_color.r = foldover(out_color.r);
        out_color.g = foldover(out_color.g);
        out_color.b = foldover(out_color.b);
    }
    
    return vec4<f32>(out_color, 1.0);
}

fn process_input(uv: vec2<f32>, coords: vec2<f32>) -> vec4<f32> {
    var ch_coords = coords;
    
    // Aspect and scale
    ch_coords.x *= uniforms.input_aspect;
    ch_coords.x -= uniforms.input_crib_x;
    ch_coords -= vec2<f32>(0.5, 0.5);
    ch_coords *= uniforms.input_scale + uniforms.input_hd_zcrib;
    ch_coords += vec2<f32>(0.5, 0.5);

    if (uniforms.input_hd_aspect_on == 1) {
        ch_coords = uv;
    }
    
    // H/V Flip
    if (get_switch(uniforms.input_switches, 2u)) {
        ch_coords.x = 1.0 - ch_coords.x;
    }
    if (get_switch(uniforms.input_switches, 3u)) {
        ch_coords.y = 1.0 - ch_coords.y;
    }
    
    // H/V Mirror
    if (get_switch(uniforms.input_switches, 0u)) {
        if (ch_coords.x > 0.5) {
            ch_coords.x = abs(1.0 - ch_coords.x);
        }
    }
    if (get_switch(uniforms.input_switches, 1u)) {
        if (ch_coords.y > 0.5) {
            ch_coords.y = abs(1.0 - ch_coords.y);
        }
    }
    
    // Kaleidoscope
    ch_coords = do_kaleidoscope(ch_coords, uniforms.input_kaleidoscope, uniforms.input_kaleidoscope_slice);
    
    // Displace
    ch_coords += uniforms.input_xy_displace;

    // Z displace (zoom)
    ch_coords -= vec2<f32>(0.5, 0.5);
    ch_coords *= uniforms.input_z_displace;
    ch_coords += vec2<f32>(0.5, 0.5);
    
    // Rotate (mode 0 = spiral effect)
    ch_coords = do_rotate(ch_coords, uniforms.input_rotate, 0);
    
    // Geo overflow
    if (uniforms.input_geo_overflow == 1) {
        ch_coords = wrap_coord(ch_coords);
    } else if (uniforms.input_geo_overflow == 2) {
        ch_coords = mirror_coord(ch_coords);
    }
    
    // Sample with filters
    let ch_uv = ch_coords;
    var ch_color = blur_and_sharpen(input_tex, input_sampler, ch_uv,
                                     uniforms.input_sharpen_amount, uniforms.input_sharpen_radius, uniforms.input_filters_boost,
                                     uniforms.input_blur_radius, uniforms.input_blur_amount);
    
    // Clamp if no overflow
    if (uniforms.input_geo_overflow == 0) {
        if (ch_coords.x > 1.0 || ch_coords.y > 1.0 ||
            ch_coords.x < 0.0 || ch_coords.y < 0.0) {
            ch_color = vec4<f32>(0.0);
        }
    }
    
    // HSB processing with early exit optimization
    let needs_hsb = uniforms.input_hsb_attenuate.x != 1.0 || uniforms.input_hsb_attenuate.y != 1.0 || uniforms.input_hsb_attenuate.z != 1.0 ||
                    get_switch(uniforms.input_switches, 4u) || get_switch(uniforms.input_switches, 5u) || get_switch(uniforms.input_switches, 6u) ||
                    uniforms.input_solarize == 1;
    
    var ch_rgb = ch_color.rgb;
    if (needs_hsb) {
        var ch_hsb = rgb2hsb(ch_color.rgb);
        ch_hsb = pow(ch_hsb, uniforms.input_hsb_attenuate.xyz);
        
        if (get_switch(uniforms.input_switches, 4u)) { ch_hsb.x = 1.0 - ch_hsb.x; }
        if (get_switch(uniforms.input_switches, 5u)) { ch_hsb.y = 1.0 - ch_hsb.y; }
        if (get_switch(uniforms.input_switches, 6u)) { ch_hsb.z = 1.0 - ch_hsb.z; }
        
        ch_hsb.x = fract(ch_hsb.x);
        
        // Solarize
        if (uniforms.input_solarize == 1) {
            ch_hsb.z = solarize(ch_hsb.z);
        }
        
        // Clamp saturation and brightness before converting back to RGB
        ch_hsb.y = clamp(ch_hsb.y, 0.0, 1.0);
        ch_hsb.z = clamp(ch_hsb.z, 0.0, 1.0);
        
        ch_rgb = hsb2rgb(ch_hsb);
    }
    
    if (get_switch(uniforms.input_switches, 7u)) { ch_rgb = 1.0 - ch_rgb; }
    
    // Posterize
    if (uniforms.input_posterize_switch == 1) {
        ch_rgb = color_quantize(ch_rgb, uniforms.input_posterize, uniforms.input_posterize_inv);
    }
    
    return vec4<f32>(ch_rgb, ch_color.a);
}

fn process_fb2(uv: vec2<f32>, coords: vec2<f32>) -> vec4<f32> {
    var fb_coords = coords;
    
    // H/V Flip
    if (get_switch(uniforms.fb2_switches, 2u)) {
        fb_coords.x = 1.0 - fb_coords.x;
    }
    if (get_switch(uniforms.fb2_switches, 3u)) {
        fb_coords.y = 1.0 - fb_coords.y;
    }
    
    // H/V Mirror
    if (get_switch(uniforms.fb2_switches, 0u)) {
        if (fb_coords.x > 0.5) {
            fb_coords.x = abs(1.0 - fb_coords.x);
        }
    }
    if (get_switch(uniforms.fb2_switches, 1u)) {
        if (fb_coords.y > 0.5) {
            fb_coords.y = abs(1.0 - fb_coords.y);
        }
    }
    
    // Kaleidoscope
    fb_coords = do_kaleidoscope(fb_coords, uniforms.fb2_kaleidoscope, uniforms.fb2_kaleidoscope_slice);
    
    // Displace
    fb_coords += uniforms.fb2_xy_displace;
    
    // Z displace
    fb_coords -= vec2<f32>(0.5, 0.5);
    fb_coords *= uniforms.fb2_z_displace;
    fb_coords += vec2<f32>(0.5, 0.5);
    
    // Rotate (with mode selection)
    fb_coords = do_rotate(fb_coords, uniforms.fb2_rotate, uniforms.fb2_rotate_mode);
    
    // Shear
    fb_coords = shear_coord(fb_coords, uniforms.fb2_shear_matrix);
    
    // Geo overflow
    if (uniforms.fb2_geo_overflow == 1) {
        fb_coords = wrap_coord(fb_coords);
    } else if (uniforms.fb2_geo_overflow == 2) {
        fb_coords = mirror_coord(fb_coords);
    }
    
    // Sample with filters
    let fb_uv = fb_coords / vec2<f32>(1.0, 1.0);
    var fb_color = blur_and_sharpen(fb2_tex, fb2_sampler, fb_uv,
                                     uniforms.fb2_sharpen_amount, uniforms.fb2_sharpen_radius, uniforms.fb2_filters_boost,
                                     uniforms.fb2_blur_radius, uniforms.fb2_blur_amount);
    
    // Clamp if no overflow
    if (uniforms.fb2_geo_overflow == 0) {
        if (fb_coords.x > 1.0 || fb_coords.y > 1.0 ||
            fb_coords.x < 0.0 || fb_coords.y < 0.0) {
            fb_color = vec4<f32>(0.0);
        }
    }
    
    // HSB processing with early exit (REQ-11.2)
    let fb2_needs_hsb =
        uniforms.fb2_hsb_offset.x != 0.0 || uniforms.fb2_hsb_offset.y != 0.0 || uniforms.fb2_hsb_offset.z != 0.0 ||
        uniforms.fb2_hsb_attenuate.x != 1.0 || uniforms.fb2_hsb_attenuate.y != 1.0 || uniforms.fb2_hsb_attenuate.z != 1.0 ||
        uniforms.fb2_hsb_powmap.x != 1.0 || uniforms.fb2_hsb_powmap.y != 1.0 || uniforms.fb2_hsb_powmap.z != 1.0 ||
        uniforms.fb2_hue_shaper != 1.0 ||
        get_switch(uniforms.fb2_switches, 4u) || get_switch(uniforms.fb2_switches, 5u) || get_switch(uniforms.fb2_switches, 6u);
    var fb_rgb = fb_color.rgb;
    if (fb2_needs_hsb) {
        var fb_hsb = rgb2hsb(fb_color.rgb);
        fb_hsb += uniforms.fb2_hsb_offset.xyz;
        fb_hsb = pow(fb_hsb, uniforms.fb2_hsb_attenuate.xyz);
        fb_hsb = pow(fb_hsb, uniforms.fb2_hsb_powmap.xyz);
        fb_hsb.x = apply_hue_shaper(fb_hsb.x, uniforms.fb2_hue_shaper);
        if (get_switch(uniforms.fb2_switches, 4u)) { fb_hsb.x = 1.0 - fb_hsb.x; }
        if (get_switch(uniforms.fb2_switches, 5u)) { fb_hsb.y = 1.0 - fb_hsb.y; }
        if (get_switch(uniforms.fb2_switches, 6u)) { fb_hsb.z = 1.0 - fb_hsb.z; }
        fb_hsb.x = fract(fb_hsb.x);
        fb_hsb.y = clamp(fb_hsb.y, 0.0, 1.0);
        fb_hsb.z = clamp(fb_hsb.z, 0.0, 1.0);
        fb_rgb = hsb2rgb(fb_hsb);
    }
    
    // Posterize
    if (uniforms.fb2_posterize_switch == 1) {
        fb_rgb = color_quantize(fb_rgb, uniforms.fb2_posterize, uniforms.fb2_posterize_inv);
    }
    
    return vec4<f32>(fb_rgb, fb_color.a);
}

@fragment
fn fs_main(@location(0) texcoord: vec2<f32>) -> @location(0) vec4<f32> {
    let uv = vec2<f32>(texcoord.x, 1.0 - texcoord.y);
    let coords = uv;
    
    // input_tex and fb2_tex are render targets (top-to-bottom) — use raw texcoord.
    let input_color = process_input(texcoord, texcoord);

    let fb2_color = process_fb2(texcoord, texcoord);
    
    // Mix input and FB2
    var final_color = mix_and_key(
        input_color, fb2_color, uniforms.fb2_mix_amount, uniforms.fb2_mix_type,
        uniforms.fb2_key_threshold, uniforms.fb2_key_soft, uniforms.fb2_key_value.xyz,
        uniforms.fb2_key_order, uniforms.fb2_mix_overflow
    );
    
    // Temporal Filter 1  (temporal_tex is a render target — raw texcoord)
    let temporal1_color = textureSample(temporal_tex, temporal_sampler, texcoord);
    var temporal1_hsb = rgb2hsb(temporal1_color.rgb);
    // Apply resonance to saturation and brightness
    temporal1_hsb.y = clamp(temporal1_hsb.y * (1.0 + uniforms.fb2_temporal1_res * 0.25), 0.0, 1.0);
    temporal1_hsb.z = clamp(temporal1_hsb.z * (1.0 + uniforms.fb2_temporal1_res * 0.5), 0.0, 1.0);
    let temporal1_rgb = hsb2rgb(temporal1_hsb);
    final_color = mix(final_color, vec4<f32>(temporal1_rgb, 1.0), uniforms.fb2_temporal1_amount);
    final_color = clamp(final_color, vec4<f32>(0.0), vec4<f32>(1.0));
    
    // Temporal Filter 2
    var temporal2_hsb = temporal1_hsb;  // Reuse HSB from filter 1
    temporal2_hsb.y = clamp(temporal2_hsb.y * (1.0 + uniforms.fb2_temporal2_res * 0.25), 0.0, 1.0);
    temporal2_hsb.z = clamp(temporal2_hsb.z * (1.0 + uniforms.fb2_temporal2_res * 0.5), 0.0, 1.0);
    let temporal2_rgb = hsb2rgb(temporal2_hsb);
    final_color = mix(final_color, vec4<f32>(temporal2_rgb, 1.0), uniforms.fb2_temporal2_amount);
    final_color = clamp(final_color, vec4<f32>(0.0), vec4<f32>(1.0));
    
    final_color.a = 1.0;
    return final_color;
}
