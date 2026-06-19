// Video matrix shader — composites N source regions of the input texture into
// their destination cells of one output, with per-cell 0/90/180/270 rotation.
// Unmapped output area is filled with the background colour.

struct CellMapping {
    source_rect: vec4<f32>, // x, y, w, h  (input UV)
    dest_rect: vec4<f32>,   // x, y, w, h  (output UV)
    orientation: u32,       // 0=0°, 1=90°CW, 2=180°, 3=270°CW
    enabled: u32,
    brightness: f32,        // per-display output adjustments (1.0 = no change)
    contrast: f32,
    gamma: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

// Per-display brightness / contrast / gamma, applied after sampling.
fn adjust(c: vec3<f32>, brightness: f32, contrast: f32, gamma: f32) -> vec3<f32> {
    var x = c * brightness;
    x = (x - vec3<f32>(0.5)) * contrast + vec3<f32>(0.5);
    x = max(x, vec3<f32>(0.0));
    return pow(x, vec3<f32>(1.0 / gamma));
}

struct MatrixUniforms {
    mapping_count: u32,
    output_width: u32,
    output_height: u32,
    output_aspect: f32, // fixed W/H the mapping is laid out in (letterboxed)
    background_color: vec4<f32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@group(1) @binding(0) var<uniform> uniforms: MatrixUniforms;
@group(1) @binding(1) var<storage, read> mappings: array<CellMapping, 16>;

// Fullscreen triangle from vertex_index (no vertex buffer).
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let x = select(-1.0, 3.0, vertex_index == 1u);
    let y = select(-1.0, 3.0, vertex_index == 2u);
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn apply_orientation(uv: vec2<f32>, orientation: u32) -> vec2<f32> {
    switch orientation {
        case 1u: { return vec2<f32>(1.0 - uv.y, uv.x); }       // 90° CW
        case 2u: { return vec2<f32>(1.0 - uv.x, 1.0 - uv.y); } // 180°
        case 3u: { return vec2<f32>(uv.y, 1.0 - uv.x); }       // 270° CW
        default: { return uv; }                                // 0°
    }
}

fn find_mapping_for_output(output_uv: vec2<f32>) -> i32 {
    for (var i: i32 = 0; i < i32(uniforms.mapping_count); i = i + 1) {
        let m = mappings[i];
        if (m.enabled == 0u) {
            continue;
        }
        let dest_min = m.dest_rect.xy;
        let dest_max = dest_min + m.dest_rect.zw;
        if (output_uv.x >= dest_min.x && output_uv.x < dest_max.x &&
            output_uv.y >= dest_min.y && output_uv.y < dest_max.y) {
            return i;
        }
    }
    return -1;
}

@fragment
fn fs_main(@builtin(position) frag_coord: vec4<f32>) -> @location(0) vec4<f32> {
    let output_size = vec2<f32>(f32(uniforms.output_width), f32(uniforms.output_height));
    let window_uv = frag_coord.xy / output_size;

    // Letterbox the mapping into a fixed output aspect so resizing the window
    // scales uniformly instead of stretching the layout. `output_uv` is the
    // coordinate inside the fixed-aspect content rect; outside = background bars.
    let win_aspect = output_size.x / max(output_size.y, 1.0);
    var output_uv = window_uv;
    if (win_aspect > uniforms.output_aspect) {
        let cw = uniforms.output_aspect / win_aspect; // pillarbox
        output_uv.x = (window_uv.x - (1.0 - cw) * 0.5) / cw;
    } else {
        let ch = win_aspect / uniforms.output_aspect; // letterbox
        output_uv.y = (window_uv.y - (1.0 - ch) * 0.5) / ch;
    }
    if (output_uv.x < 0.0 || output_uv.x > 1.0 || output_uv.y < 0.0 || output_uv.y > 1.0) {
        return uniforms.background_color;
    }

    let idx = find_mapping_for_output(output_uv);
    if (idx < 0) {
        return uniforms.background_color;
    }

    let m = mappings[idx];

    // UV within the destination cell, then rotate, then map into the source rect.
    var local_uv = (output_uv - m.dest_rect.xy) / m.dest_rect.zw;
    local_uv = clamp(local_uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let oriented = apply_orientation(local_uv, m.orientation);
    let source_uv = m.source_rect.xy + oriented * m.source_rect.zw;

    let color = textureSample(source_tex, source_sampler, source_uv);
    return vec4<f32>(adjust(color.rgb, m.brightness, m.contrast, m.gamma), color.a);
}
