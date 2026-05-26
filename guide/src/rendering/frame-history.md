# Frame History & Custom Pipelines

Some effects need access to frames from the recent past — RGB delay, motion extraction, echo trails, temporal blur. The single-pass default only gives you the current frame.

The solution is to override `render()` and manage your own GPU pipeline, including a ring buffer of past frames.

## The pattern

1. Store textures for your history ring buffer in the plugin struct
2. Create your own pipeline in `init()`
3. Override `render()` to: copy the current input frame into the ring buffer, bind delayed frames, run your pass, and return `true`

Returning `true` from `render()` tells the engine to skip its default draw.

## Minimal skeleton

```rust
use rustjay_engine::prelude::*;

struct DelayEffect {
    pipeline:  Option<wgpu::RenderPipeline>,
    bgl:       Option<wgpu::BindGroupLayout>,
    history:   Vec<Texture>,     // ring buffer
    write_idx: usize,
}

impl EffectPlugin for DelayEffect {
    type State    = DelayState;
    type Uniforms = DelayUniforms;

    // ── The engine still requires a shader_source ──────────────────────────
    // It compiles this for its default pipeline, but since render() returns
    // true below, that pipeline is never executed. Provide a minimal stub
    // that matches the standard binding layout.
    fn shader_source(&self) -> &'static str {
        include_str!("shaders/stub.wgsl")
    }

    fn build_uniforms(&self, s: &DelayState, engine: &EngineState) -> DelayUniforms {
        DelayUniforms { /* ... */ }
    }

    // ── Create the real pipeline ───────────────────────────────────────────
    fn init(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("delay"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/delay.wgsl").into()),
        });

        // Create bind group layout with N history texture slots
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("delay-bgl"),
            entries: &[
                // slot 0: current input, slot 1: sampler,
                // slot 2..N+2: history textures and their samplers
                // ...
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
            label: None,
        });

        // create_render_pipeline(...) with your layout
        self.pipeline = Some(/* ... */);
        self.bgl      = Some(bgl);

        // Allocate history textures
        self.history = (0..8).map(|_| {
            Texture::create_render_target(device, 1920, 1080) // or your preferred size
        }).collect();
    }

    // ── Custom render ──────────────────────────────────────────────────────
    fn render(
        &mut self,
        encoder:            &mut wgpu::CommandEncoder,
        device:             &wgpu::Device,
        queue:              &wgpu::Queue,
        input_view:         Option<&wgpu::TextureView>,
        input_sampler:      Option<&wgpu::Sampler>,
        render_target_view: &wgpu::TextureView,
        app_state:          &mut DelayState,
        engine_state:       &EngineState,
        _vertex_buffer:     &wgpu::Buffer,
        input_texture:      Option<&wgpu::Texture>,   // raw texture for copies
    ) -> bool {
        // 1. Copy current input frame into history ring buffer
        if let Some(src) = input_texture {
            let dst = &self.history[self.write_idx];
            encoder.copy_texture_to_texture(
                src.as_image_copy(),
                dst.texture().as_image_copy(),
                dst.texture().size(),
            );
            self.write_idx = (self.write_idx + 1) % self.history.len();
        }

        // 2. Build a bind group from the appropriate history frame(s)
        let delay_idx = (self.write_idx + self.history.len() - app_state.delay_frames)
            % self.history.len();
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: self.bgl.as_ref().unwrap(),
            entries: &[/* bind delayed frames ... */],
            label: None,
        });

        // 3. Run the render pass
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: render_target_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            label: Some("delay-pass"),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(self.pipeline.as_ref().unwrap());
        rp.set_bind_group(0, &bind_group, &[]);
        rp.draw(0..6, 0..1); // 6 vertices = fullscreen quad

        true // skip the engine's default render pass
    }
}
```

## The stub shader

The engine still compiles `shader_source()` for its default pipeline. The stub must declare the standard binding layout even though the pipeline never runs:

```wgsl
// shaders/stub.wgsl — minimal stub, never actually drawn
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) texcoord: vec2<f32>,
};

@group(0) @binding(0) var input_tex:     texture_2d<f32>;
@group(0) @binding(1) var input_sampler: sampler;

struct Uniforms { _pad: f32 };
@group(1) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_main(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.texcoord = uv;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return textureSample(input_tex, input_sampler, in.texcoord);
}
```

## Per-channel delay

The `examples/delta` example goes further: it maintains separate delay values for the red, green, and blue channels, producing RGB colour trails. Study it for a complete, production-ready implementation of this pattern.

```sh
cargo run -p delta
```

Features demonstrated:
- 8-frame ring buffer
- Per-channel R/G/B delays (0–7 frames)
- 8 blend modes (Replace, Add, Multiply, Screen, Difference, Overlay, Lighten, Darken)
- Per-channel gain, trail fade, threshold, smoothing
