// Text blit: a full-screen quad that inverse-maps each pixel into the text
// raster. Doing it this way means rotation and scale need no vertex work, and
// anything outside the raster is simply transparent.

struct Uniforms {
    color: vec4<f32>,
    resolution: vec2<f32>,
    center: vec2<f32>,
    scale: f32,
    tex_aspect: f32,
    angle: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var text_tex: texture_2d<f32>;
@group(0) @binding(2) var text_samp: sampler;

@vertex
fn vs_main(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / u.resolution;
    // Work in square space, or the rotation shears with the target's aspect.
    let aspect = u.resolution.x / max(u.resolution.y, 1.0);
    var p = (uv - u.center) * vec2<f32>(aspect, 1.0);
    let c = cos(-u.angle);
    let s = sin(-u.angle);
    p = vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c);

    // The raster is `scale` tall and `scale * tex_aspect` wide in that space.
    let h = max(u.scale, 1e-5);
    let t = vec2<f32>(p.x / (h * max(u.tex_aspect, 1e-5)), p.y / h) + vec2<f32>(0.5, 0.5);
    if (t.x < 0.0 || t.x > 1.0 || t.y < 0.0 || t.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    let coverage = textureSample(text_tex, text_samp, t).a;
    return vec4<f32>(u.color.rgb, u.color.a * coverage);
}
