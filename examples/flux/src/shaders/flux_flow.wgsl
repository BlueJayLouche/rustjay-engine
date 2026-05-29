// flux_flow.wgsl — per-pixel Lucas-Kanade optical flow estimate.
//
// Computes spatial gradients (Ix, Iy) from the current frame and the
// temporal gradient (It = current - previous) then solves the constraint
// equation per pixel.  Flow is encoded into [0,1] as 0.5 + value/scale so
// it can live in a standard Bgra8Unorm texture.
//
// Binding layout must match FluxEffect::build_flow_pipeline():
//   group(0): curr, prev, prev_flow, sampler
//   group(1): uniforms

@group(0) @binding(0) var curr_tex:      texture_2d<f32>;
@group(0) @binding(1) var prev_tex:      texture_2d<f32>;
@group(0) @binding(2) var prev_flow_tex: texture_2d<f32>;
@group(0) @binding(3) var tex_sampler:   sampler;

struct FluxUniforms {
    // flow computation
    flow_lambda:    f32,   // regularisation (prevents div-by-zero on flat regions)
    flow_smooth:    f32,   // temporal IIR blend towards previous flow  [0, 1)
    flow_scale:     f32,   // multiplier before encode — controls sensitivity
    _pad0:          f32,
    // warp / feedback
    warp_strength:  f32,
    drift_strength: f32,
    feedback_decay: f32,
    webcam_mix:     f32,
    // visualisation
    flow_viz:       f32,
    flow_viz_scale: f32,
    _pad1:          f32,
    _pad2:          f32,
    // audio
    audio_level:    f32,
    bass:           f32,
    mid:            f32,
    treble:         f32,
};
@group(1) @binding(0) var<uniform> u: FluxUniforms;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0)       uv:  vec2<f32>,
};

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.pos = vec4<f32>(pos, 0.0, 1.0);
    out.uv  = uv;
    return out;
}

fn luma(c: vec4<f32>) -> f32 {
    return dot(c.rgb, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let sz  = vec2<f32>(textureDimensions(curr_tex));
    let dx  = vec2<f32>(1.0 / sz.x, 0.0);
    let dy  = vec2<f32>(0.0, 1.0 / sz.y);
    let uv  = in.uv;

    // Spatial gradients (central differences on the current frame's luma)
    let ix = (luma(textureSample(curr_tex, tex_sampler, uv + dx))
            - luma(textureSample(curr_tex, tex_sampler, uv - dx))) * 0.5;
    let iy = (luma(textureSample(curr_tex, tex_sampler, uv + dy))
            - luma(textureSample(curr_tex, tex_sampler, uv - dy))) * 0.5;

    // Temporal gradient
    let it = luma(textureSample(curr_tex, tex_sampler, uv))
           - luma(textureSample(prev_tex,  tex_sampler, uv));

    // Per-pixel Lucas-Kanade: solve (Ix²+Iy²+λ)·[u,v] = -It·[Ix,Iy]
    let lambda = u.flow_lambda + 0.001;
    let denom  = ix * ix + iy * iy + lambda;
    let audio_boost = 1.0 + u.audio_level * 0.5;
    let vx = -it * ix / denom * u.flow_scale * audio_boost;
    let vy = -it * iy / denom * u.flow_scale * audio_boost;

    // Temporal smoothing: blend with previous flow
    let prev = textureSample(prev_flow_tex, tex_sampler, uv).xy;
    // decode previous (was stored as 0.5 + v/scale)
    let prev_decoded = (prev - 0.5) * 2.0;
    let smoothed = mix(vec2<f32>(vx, vy), prev_decoded, u.flow_smooth);

    // Encode into [0,1]: 0.5 = zero, range ±1 maps to [0,1]
    let enc = clamp(smoothed * 0.5 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0));
    let mag = clamp(length(smoothed), 0.0, 1.0);
    return vec4<f32>(enc, mag, 1.0);
}
