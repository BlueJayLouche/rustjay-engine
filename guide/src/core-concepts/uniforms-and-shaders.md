# Uniforms & Shaders

## The standard binding layout

Every rustjay-engine shader shares a consistent binding layout. If you deviate from it, the engine won't be able to set up the bind groups it needs.

### Group 0 — Video input

```wgsl
@group(0) @binding(0) var input_tex:     texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;
```

The engine always provides these two bindings. `input_tex` is the current video frame from whatever source is active (webcam, NDI, Syphon, etc.). `input_sampler` is a bilinear clamp sampler.

When a [RenderGraph](../rendering/render-graph.md) with feedback is active, two more bindings are added at group 0:

```wgsl
@group(0) @binding(2) var feedback_tex:     texture_2d<f32>; // previous frame output
@group(0) @binding(3) var feedback_sampler: sampler;
```

### Group 1 — Your uniforms

```wgsl
@group(1) @binding(0) var<uniform> u: MyUniforms;
```

One uniform buffer containing your `Uniforms` struct. The engine uploads whatever `build_uniforms()` returns each frame.

## Uniform struct rules

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MyUniforms {
    intensity: f32,
    hue:       f32,
    _pad:      [f32; 2],   // pad to 16 bytes
}
```

Three requirements:
1. `#[repr(C)]` — guarantees stable field ordering and no padding surprises
2. `bytemuck::Pod` — allows safe transmutation to `&[u8]` for the GPU upload
3. `bytemuck::Zeroable` — allows zeroing the buffer before your first `build_uniforms()` call

**16-byte alignment:** wgpu requires uniform buffers to be multiples of 16 bytes in size. Structs smaller than 16 bytes need explicit padding fields. The `_pad: [f32; N]` pattern is idiomatic.

## The vertex shader

The engine provides a full-screen quad (two triangles covering the NDC unit square). Your vertex shader receives:

```wgsl
@location(0) position: vec2<f32>  // NDC position [-1, 1]
@location(1) texcoord: vec2<f32>  // UV [0, 1], (0,0) = top-left
```

The standard vertex shader is boilerplate and almost never changes:

```wgsl
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec2<f32>,
    @location(1) texcoord: vec2<f32>,
) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(position, 0.0, 1.0);
    out.texcoord = texcoord;
    return out;
}
```

## Sampling the video input

```wgsl
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let col = textureSample(input_tex, input_sampler, in.texcoord);
    // ... process col ...
    return col;
}
```

`in.texcoord` is `[0,1]² ` with `(0,0)` at the top-left. The sampler clamps at the border — sampling outside `[0,1]` returns the edge pixel colour.

## Encoding data in uniforms

### Floats

Direct: `intensity: f32`, `bpm: f32`, etc.

### Booleans

wgpu doesn't support `bool` in uniform buffers. Use `u32`:

```rust
struct MyUniforms { enabled: u32, _pad: [f32; 3] }
// In build_uniforms:
enabled: if s.enabled { 1 } else { 0 },
```

```wgsl
struct MyUniforms { enabled: u32 };
// In shader:
if u.enabled != 0u { /* ... */ }
```

### Enums / modes

Use `u32` for discrete choices:

```rust
struct MyUniforms { blend_mode: u32, _pad: [f32; 3] }
```

```wgsl
switch u.blend_mode {
    case 0u: { /* Replace */ }
    case 1u: { /* Add */     }
    default: { /* ... */     }
}
```

### Colours

Use `vec4<f32>` (RGBA) or pack channels into a `[f32; 4]` array:

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct MyUniforms {
    values: [f32; 4],  // e.g. [hue_shift, saturation, brightness, unused]
}
```

```wgsl
struct MyUniforms { values: vec4<f32> };
let hue = u.values.x;
let sat = u.values.y;
```

## UV helpers

Common UV transforms:

```wgsl
// Flip V (some sources come upside-down)
let flipped = vec2<f32>(in.texcoord.x, 1.0 - in.texcoord.y);

// Centre coordinates ([-0.5, 0.5]² )
let centred = in.texcoord - vec2<f32>(0.5);

// Aspect-correct coordinates (account for non-square input)
// Requires width/height in uniforms
let aspect = f32(u.width) / f32(u.height);
let uv = vec2<f32>((in.texcoord.x - 0.5) * aspect, in.texcoord.y - 0.5);
```
