struct Vertex {
    position: vec2<f32>,
    texcoord: vec2<f32>,
};

struct SputnikUniforms {
    displacement_scale: f32,
    rotation: f32,
    zoom: f32,
    aspect_ratio: f32,
    audio_bands_a: vec4<f32>,
    audio_bands_b: vec4<f32>,
    mvp: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: SputnikUniforms;
@group(1) @binding(0) var<storage, read_write> vertices: array<Vertex>;

// Simple pseudo-random hash.
fn hash2(p: vec2<f32>) -> f32 {
    let q = vec2<f32>(dot(p, vec2<f32>(127.1, 311.7)), dot(p, vec2<f32>(269.5, 183.3)));
    return fract(sin(q.x + q.y) * 43758.5453);
}

@compute @workgroup_size(256, 1, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x;
    if (index >= arrayLength(&vertices)) {
        return;
    }

    let v = &vertices[index];

    // Add subtle noise-based displacement to the base mesh position.
    let noise = hash2((*v).texcoord * 100.0) * 0.02;
    (*v).position.y = (*v).position.y + noise;
}
