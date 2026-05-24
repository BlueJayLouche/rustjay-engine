// Sputnik vertex + fragment shader.
//
// The vertex stage reads the compute-displaced mesh position and adds a
// second layer of video-luminance + audio displacement along Y / Z before
// projecting through the MVP matrix.
//
// Luminance is scaled logarithmically to match the original sputnikMesh
// feel: bright = 2 * log(1 + luma).

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0)       texcoord: vec2<f32>,
}

// Must match SputnikUniforms in main.rs exactly (208 bytes).
struct SputnikUniforms {
    displacement_scale: f32,
    bright_invert:      u32,
    x_offset:           f32,
    y_offset:           f32,

    audio_bands_a: vec4<f32>,
    audio_bands_b: vec4<f32>,

    x_lfo_arg:   f32,
    x_lfo_amp:   f32,
    x_lfo_freq:  f32,
    x_lfo_shape: u32,

    y_lfo_arg:   f32,
    y_lfo_amp:   f32,
    y_lfo_freq:  f32,
    y_lfo_shape: u32,

    z_lfo_arg:   f32,
    z_lfo_amp:   f32,
    z_lfo_freq:  f32,
    z_lfo_shape: u32,

    x_phasemod: u32,
    x_ringmod:  u32,
    y_phasemod: u32,
    y_ringmod:  u32,

    z_phasemod:  u32,
    z_ringmod:   u32,
    tex_width:   f32,
    tex_height:  f32,

    z_offset:          f32,
    audio_reactivity:  f32,
    pad0:              u32,
    pad1:              u32,

    mvp: mat4x4<f32>,
}

@group(0) @binding(0) var input_tex:     texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(1) @binding(0) var<uniform> u:    SputnikUniforms;

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) texcoord: vec2<f32>,
) -> VertexOutput {
    var out: VertexOutput;

    // `position` is the LFO-displaced mesh position written by the compute pass.
    // Sample the video texture here to add a second displacement layer.
    // textureSampleLevel is required in vertex stage (no screen-space derivatives).
    let color = textureSampleLevel(input_tex, input_sampler, texcoord, 0.0);
    var bright = dot(color.rgb, vec3<f32>(0.299, 0.587, 0.114));
    if u.bright_invert != 0u {
        bright = 1.0 - bright;
    }
    // Logarithmic brightness scaling — matches original sputnikMesh.
    bright = 2.0 * log(1.0 + bright);

    // Map each column to one of 8 frequency bands for audio reactivity.
    let bands = array<f32, 8>(
        u.audio_bands_a.x, u.audio_bands_a.y,
        u.audio_bands_a.z, u.audio_bands_a.w,
        u.audio_bands_b.x, u.audio_bands_b.y,
        u.audio_bands_b.z, u.audio_bands_b.w,
    );
    let band_idx  = clamp(u32(texcoord.x * 8.0), 0u, 7u);
    let audio_lift = bands[band_idx] * u.audio_reactivity;

    // Video + audio displacement applied to Y; Z carries the depth component.
    let displacement = (bright + audio_lift) * u.displacement_scale;
    let pos3 = vec3<f32>(position.x, position.y + displacement, displacement * 0.5);

    out.position = u.mvp * vec4<f32>(pos3, 1.0);
    out.texcoord = texcoord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(input_tex, input_sampler, in.texcoord);
}
