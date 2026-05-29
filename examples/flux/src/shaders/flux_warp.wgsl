// flux_warp.wgsl — warp the webcam by the flow field and blend with feedback.
//
// Binding layout must match FluxEffect::build_warp_pipeline():
//   group(0): webcam, flow, accum, sampler
//   group(1): uniforms (same FluxUniforms block)

@group(0) @binding(0) var input_tex:  texture_2d<f32>; // current webcam frame
@group(0) @binding(1) var flow_tex:   texture_2d<f32>; // encoded flow (from flux_flow)
@group(0) @binding(2) var accum_tex:  texture_2d<f32>; // previous accumulated output
@group(0) @binding(3) var tex_sampler: sampler;

struct FluxUniforms {
    flow_lambda:    f32,
    flow_smooth:    f32,
    flow_scale:     f32,
    _pad0:          f32,
    warp_strength:  f32,
    drift_strength: f32,
    feedback_decay: f32,
    webcam_mix:     f32,
    flow_viz:       f32,
    flow_viz_scale: f32,
    _pad1:          f32,
    _pad2:          f32,
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

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
    let c = v * s;
    let x = c * (1.0 - abs(fract(h * 6.0) * 2.0 - 1.0));
    let m = v - c;
    let hi = i32(h * 6.0) % 6;
    var rgb: vec3<f32>;
    switch hi {
        case 0: { rgb = vec3<f32>(c, x, 0.0); }
        case 1: { rgb = vec3<f32>(x, c, 0.0); }
        case 2: { rgb = vec3<f32>(0.0, c, x); }
        case 3: { rgb = vec3<f32>(0.0, x, c); }
        case 4: { rgb = vec3<f32>(x, 0.0, c); }
        default: { rgb = vec3<f32>(c, 0.0, x); }
    }
    return rgb + m;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;

    // Decode flow from [0,1] storage back to signed floats
    let enc  = textureSample(flow_tex, tex_sampler, uv).xy;
    let flow = (enc - 0.5) * 2.0;
    let mag  = length(flow);

    // Audio modulation: bass drives warp amount
    let warp = u.warp_strength * (1.0 + u.bass * 0.8);

    // Warp: sample webcam at UV displaced opposite to flow
    // (where this pixel came from in the previous frame)
    let warp_uv = fract(uv - flow * warp);
    let webcam  = textureSample(input_tex, tex_sampler, warp_uv);

    // Feedback: previous output drifts along the flow field
    let drift_uv = fract(uv + flow * u.drift_strength);
    let accum    = textureSample(accum_tex, tex_sampler, drift_uv);

    // Decay accumulation and blend with warped webcam
    let decay   = u.feedback_decay * (1.0 - u.treble * 0.1);
    let faded   = accum * clamp(decay, 0.0, 0.9999);
    var color   = mix(faded, webcam, clamp(u.webcam_mix, 0.0, 1.0));

    // Flow visualisation overlay: direction → hue, magnitude → brightness
    if u.flow_viz > 0.0 && mag > 0.001 {
        let angle     = atan2(flow.y, flow.x) / (2.0 * 3.14159265) + 0.5;
        let brightness = clamp(mag * u.flow_viz_scale, 0.0, 1.0);
        let flow_col  = vec4<f32>(hsv_to_rgb(angle, 0.85, brightness), 1.0);
        let blend_amt = u.flow_viz * clamp(brightness, 0.0, 1.0);
        color = mix(color, flow_col, blend_amt);
    }

    return color;
}
