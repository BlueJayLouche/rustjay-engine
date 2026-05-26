# Mesh Displacement

Instead of a fullscreen quad, rustjay-engine can generate an indexed `cols × rows` mesh grid. Your vertex shader displaces each point in 3D space, turning the video frame into a displaced 3D surface.

This is how the classic **Rutt-Etra** analogue video synthesiser look is achieved — horizontal scanlines pushed out along the Z axis by video luminance.

## Enabling a mesh

Return a `MeshDescriptor` from `mesh_descriptor()`:

```rust
fn mesh_descriptor(&self, _state: &MyState) -> Option<MeshDescriptor> {
    Some(MeshDescriptor {
        cols: 320,
        rows: 240,
        topology: MeshTopology::Scanlines,
    })
}
```

The engine replaces the default two-triangle quad with a `320 × 240` indexed grid and calls your vertex shader for each vertex.

## Topologies

| `MeshTopology` | wgpu primitive | Look |
|---|---|---|
| `Scanlines` | `LineList` (horizontal lines) | Classic Rutt-Etra wire scanlines |
| `Triangles` | `TriangleList` | Solid displaced surface |
| `Wireframe` | `TriangleList` + polygon line mode | Wire-frame surface |
| `Points` | `PointList` | Particle cloud / dot-matrix |

## Letting the vertex shader sample the texture

The standard binding layout only exposes group 0 to the fragment stage. For displacement effects, you need the vertex shader to sample the video texture to compute the displacement amount.

Add this to your plugin:

```rust
fn vertex_reads_texture(&self) -> bool {
    true
}
```

The engine then adds `VERTEX | FRAGMENT` visibility to the group-0 bind group entries.

In the vertex shader:

```wgsl
@group(0) @binding(0) var input_tex:     texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    // Sample luminance at this vertex's UV
    let col  = textureSample(input_tex, input_sampler, uv);
    let luma = dot(col.rgb, vec3<f32>(0.299, 0.587, 0.114));

    // Displace along Z proportional to luminance
    let displaced = vec4<f32>(pos, luma * u.displacement_scale, 1.0);

    var out: VertexOutput;
    out.position = u.mvp * displaced;
    out.texcoord = uv;
    return out;
}
```

## MVP matrix

For 3D displacement, you need a model-view-projection matrix in your uniforms:

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MeshUniforms {
    mvp:               [[f32; 4]; 4],
    displacement_scale: f32,
    _pad:              [f32; 3],
}
```

Build the MVP matrix each frame from your camera/rotation state. The `glam` crate is convenient for this:

```rust
fn build_uniforms(&self, s: &MeshState, engine: &EngineState) -> MeshUniforms {
    let rotation = glam::Mat4::from_rotation_y(s.rot_y)
                 * glam::Mat4::from_rotation_x(s.rot_x);
    let view     = glam::Mat4::look_at_rh(
        glam::Vec3::new(0.0, 0.0, 2.0),
        glam::Vec3::ZERO,
        glam::Vec3::Y,
    );
    let proj     = glam::Mat4::perspective_rh(
        std::f32::consts::FOVN_PI_4,
        16.0 / 9.0,
        0.01, 100.0,
    );
    MeshUniforms {
        mvp: (proj * view * rotation).to_cols_array_2d(),
        displacement_scale: engine.get_param("displacement").unwrap_or(s.displacement),
        _pad: [0.0; 3],
    }
}
```

## Compute shader option

For very large meshes or complex per-vertex computations, use the compute shader path instead of the vertex shader:

```rust
fn compute_shader(&self) -> Option<&'static str> {
    Some(include_str!("shaders/displace.comp.wgsl"))
}
```

The engine dispatches the compute shader before the render pass. It receives:
- `@group(0) @binding(0)` — your uniform buffer
- `@group(1) @binding(0)` — the vertex storage buffer (`array<Vertex>`, read/write)

Workgroup size must be `@workgroup_size(256, 1, 1)`. The engine dispatches 1D groups to cover all vertices.

## Example: sputnik

`examples/sputnik` is a complete Rutt-Etra-style implementation with:
- Dynamic mesh grid (configurable resolution)
- Per-axis rotation controlled by LFOs
- Ring modulation between mesh position and LFO output
- Audio-reactive displacement depth

```sh
cargo run -p sputnik
```
