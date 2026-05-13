struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

struct SputnikUniforms {
    displacement_scale: f32,
    rotation: f32,
    zoom: f32,
    aspect_ratio: f32,
    audio_bands_a: vec4<f32>,
    audio_bands_b: vec4<f32>,
};

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
@group(1) @binding(0) var<uniform> u: SputnikUniforms;

@vertex
fn vs_main(@location(0) position: vec2<f32>, @location(1) texcoord: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;

    // Sample video texture at this vertex's UV to get luminance.
    // textureSampleLevel is required in the vertex stage because
    // textureSample needs screen-space derivatives (dpdx/dpdy).
    let color = textureSampleLevel(input_tex, input_sampler, texcoord, 0.0);
    let luminance = dot(color.rgb, vec3<f32>(0.299, 0.587, 0.114));

    // Sum all 8 weighted audio bands.
    var audio_sum: f32 = 0.0;
    audio_sum = audio_sum + u.audio_bands_a.x + u.audio_bands_a.y + u.audio_bands_a.z + u.audio_bands_a.w;
    audio_sum = audio_sum + u.audio_bands_b.x + u.audio_bands_b.y + u.audio_bands_b.z + u.audio_bands_b.w;

    // Displace Y based on luminance + audio reactivity.
    let displacement = (luminance + audio_sum) * u.displacement_scale;

    var pos = position;
    pos.y = pos.y + displacement;

    // Apply 2D rotation around the Z axis.
    let c = cos(u.rotation);
    let s = sin(u.rotation);
    let rx = pos.x * c - pos.y * s;
    let ry = pos.x * s + pos.y * c;
    pos = vec2<f32>(rx, ry);

    // Apply zoom.
    pos = pos * u.zoom;

    // Correct for aspect ratio so the mesh isn't stretched.
    pos.x = pos.x / u.aspect_ratio;

    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(input_tex, input_sampler, in.texcoord);
}
