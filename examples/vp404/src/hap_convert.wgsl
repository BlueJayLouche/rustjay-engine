// HAP decode-side convert: YCoCg→RGB + dual-plane alpha, drawn fullscreen.
// Ported from varda-orig/src/internal/renderer/shaders/hap_convert.wgsl.
// hap-wgpu hands back a raw BCn texture; this turns it into sample-ready RGBA.
// ponytail: a fullscreen triangle (no vertex buffer) — vs generates its own UVs.

struct HapConvertParams {
    opacity: f32,
    /// 0.0 = passthrough RGB, 1.0 = YCoCg→RGB (HapY / HAP Q)
    do_ycocg: f32,
    /// 0.0 = alpha from colour texture, 1.0 = alpha from separate BC4 plane
    has_alpha_plane: f32,
    _pad: f32,
    /// crop the padded BC texture back to the real image (dim / padded)
    uv_scale: vec2<f32>,
    uv_offset: vec2<f32>,
}

@group(0) @binding(0) var tex_sampler: sampler;
@group(0) @binding(1) var color_texture: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: HapConvertParams;
@group(0) @binding(3) var alpha_texture: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

/// Scaled YCoCg → RGB conversion (HAP Q spec — Resolume/FFmpeg HapY files).
/// Input: BC3/DXT5 where R=Co, G=Cg, B=scale, A=Y.
/// NOTE: hap-wgpu's *own* encoder currently writes PLAIN YCoCg (no scale plane),
/// which this does NOT decode — that's a hap-wgpu encoder bug, not a bug here.
/// See 404_PORT.md §HAP / project memory.
fn ycocg_to_rgb(color: vec4<f32>) -> vec3<f32> {
    let scale = (color.b * (255.0 / 8.0)) + 1.0;
    let co = (color.r - (0.5 * 256.0 / 255.0)) / scale;
    let cg = (color.g - (0.5 * 256.0 / 255.0)) / scale;
    let y = color.a;
    return vec3<f32>(
        y + co - cg,
        y + cg,
        y - co - cg,
    );
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = in.uv * params.uv_scale + params.uv_offset;
    let color = textureSample(color_texture, tex_sampler, uv);

    var rgb: vec3<f32>;
    if (params.do_ycocg > 0.5) {
        rgb = ycocg_to_rgb(color);
    } else {
        rgb = color.rgb;
    }

    var alpha: f32;
    if (params.has_alpha_plane > 0.5) {
        alpha = textureSample(alpha_texture, tex_sampler, uv).r;
    } else {
        alpha = color.a;
    }

    return vec4<f32>(rgb, alpha * params.opacity);
}
