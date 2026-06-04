struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var out: VertexOutput;
    let x = f32((vertex_index & 1u) << 2u);
    let y = f32((vertex_index & 2u) << 1u);
    out.position = vec4<f32>(x - 1.0, 1.0 - y, 0.0, 1.0);
    out.uv = vec2<f32>(x * 0.5, y * 0.5);
    return out;
}

struct DomemasterParams {
    fov: f32,
    tilt: f32,
    content_az: f32,
    content_el: f32,
    content_roll: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var texture_sampler: sampler;
@group(0) @binding(1) var face_front: texture_2d<f32>;
@group(0) @binding(2) var face_right: texture_2d<f32>;
@group(0) @binding(3) var face_back: texture_2d<f32>;
@group(0) @binding(4) var face_left: texture_2d<f32>;
@group(0) @binding(5) var face_top: texture_2d<f32>;
@group(0) @binding(6) var<uniform> params: DomemasterParams;

fn sample_cubemap(dir: vec3<f32>) -> vec4<f32> {
    let abs_dir = abs(dir);
    if abs_dir.y >= abs_dir.x && abs_dir.y >= abs_dir.z && dir.y > 0.0 {
        let u = dir.x / abs_dir.y * 0.5 + 0.5;
        let v = -dir.z / abs_dir.y * 0.5 + 0.5;
        return textureSample(face_top, texture_sampler, vec2<f32>(u, v));
    }
    if abs_dir.z >= abs_dir.x && abs_dir.z >= abs_dir.y {
        if dir.z > 0.0 {
            let u = dir.x / abs_dir.z * 0.5 + 0.5;
            let v = -dir.y / abs_dir.z * 0.5 + 0.5;
            return textureSample(face_front, texture_sampler, vec2<f32>(u, v));
        } else {
            let u = -dir.x / abs_dir.z * 0.5 + 0.5;
            let v = -dir.y / abs_dir.z * 0.5 + 0.5;
            return textureSample(face_back, texture_sampler, vec2<f32>(u, v));
        }
    }
    if dir.x > 0.0 {
        let u = -dir.z / abs_dir.x * 0.5 + 0.5;
        let v = -dir.y / abs_dir.x * 0.5 + 0.5;
        return textureSample(face_right, texture_sampler, vec2<f32>(u, v));
    }
    let u = dir.z / abs_dir.x * 0.5 + 0.5;
    let v = -dir.y / abs_dir.x * 0.5 + 0.5;
    return textureSample(face_left, texture_sampler, vec2<f32>(u, v));
}

@fragment
fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let centered = uv * 2.0 - vec2<f32>(1.0, 1.0);
    let r = length(centered);
    if r > 1.0 {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let half_fov = params.fov * 0.5;
    let angle_from_zenith = r * half_fov;
    let azimuth = atan2(centered.x, -centered.y);
    let sin_angle = sin(angle_from_zenith);
    let cos_angle = cos(angle_from_zenith);
    var dir = vec3<f32>(
        sin_angle * sin(azimuth),
        cos_angle,
        sin_angle * cos(azimuth)
    );
    let cos_tilt = cos(params.tilt);
    let sin_tilt = sin(params.tilt);
    let tilted_y = dir.y * cos_tilt - dir.z * sin_tilt;
    let tilted_z = dir.y * sin_tilt + dir.z * cos_tilt;
    dir = vec3<f32>(dir.x, tilted_y, tilted_z);
    let cr = cos(-params.content_roll);
    let sr = sin(-params.content_roll);
    dir = vec3<f32>(dir.x * cr - dir.y * sr, dir.x * sr + dir.y * cr, dir.z);
    let ce = cos(-params.content_el);
    let se = sin(-params.content_el);
    dir = vec3<f32>(dir.x, dir.y * ce - dir.z * se, dir.y * se + dir.z * ce);
    let ca = cos(-params.content_az);
    let sa = sin(-params.content_az);
    dir = vec3<f32>(dir.x * ca + dir.z * sa, dir.y, -dir.x * sa + dir.z * ca);
    return sample_cubemap(dir);
}
