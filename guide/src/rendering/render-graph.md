# Multi-Pass with RenderGraph

`RenderGraph` lets you chain multiple shader passes without managing intermediate textures yourself. Each pass writes to a texture that the engine creates automatically; the last pass writes to the render target.

## When to use RenderGraph

- Two-stage effects: blur → mix, threshold → edge-detect → composite
- Feedback loops: each frame reads its own previous output
- Post-processing chains where each stage is a self-contained WGSL shader

If you need to manage your own GPU resources or read from a ring buffer of past frames, use [Frame History & Custom Pipelines](frame-history.md) instead.

## Defining a graph

Return a `RenderGraph` from `render_graph()`:

```rust
impl EffectPlugin for MyEffect {
    fn render_graph(&self) -> Option<RenderGraph> {
        Some(
            RenderGraph::new()
                .with_pass(Pass {
                    label: "Blur",
                    shader: include_str!("shaders/blur.wgsl"),
                    input: PassInput::EngineInput,
                })
                .with_pass(Pass {
                    label: "Composite",
                    shader: include_str!("shaders/composite.wgsl"),
                    input: PassInput::PreviousPass,
                }),
        )
    }
}
```

The engine executes passes in declaration order. Intermediate textures are managed automatically.

## Pass input sources

| `PassInput` | What it binds at `@group(0) @binding(0/1)` |
|---|---|
| `PassInput::EngineInput` | The live video frame |
| `PassInput::PreviousPass` | The output of the immediately preceding pass |
| `PassInput::Feedback` | The previous frame's final output |

## Enabling feedback

Add `.with_feedback()` to the graph to enable the feedback texture:

```rust
RenderGraph::new()
    .with_pass(Pass {
        label: "Distort",
        shader: include_str!("shaders/distort.wgsl"),
        input: PassInput::EngineInput,
    })
    .with_pass(Pass {
        label: "Feedback Mix",
        shader: include_str!("shaders/feedback.wgsl"),
        input: PassInput::PreviousPass,
    })
    .with_feedback()
```

When feedback is enabled, every pass gets two additional bindings:

```wgsl
@group(0) @binding(2) var feedback_tex:     texture_2d<f32>;
@group(0) @binding(3) var feedback_sampler: sampler;
```

`feedback_tex` always contains the final output of the *previous frame*. Passes that don't use feedback simply omit those declarations.

## Per-pass uniforms

By default, `build_pass_uniforms()` delegates to `build_uniforms()`, so a single uniform struct serves all passes.

Override `build_pass_uniforms()` to send different values to each pass:

```rust
fn build_pass_uniforms(
    &self,
    pass_index: usize,
    s: &MyState,
    engine: &EngineState,
) -> MyUniforms {
    match pass_index {
        0 => MyUniforms { radius: s.blur_radius, .. },
        1 => MyUniforms { mix: s.feedback_amount, .. },
        _ => self.build_uniforms(s, engine),
    }
}
```

## Single-pass fallback

The engine still compiles `shader_source()` for its default pipeline even when `render_graph()` returns `Some`. If the graph isn't available (e.g. the feature is disabled at compile time), the engine falls back to single-pass.

In practice this means `shader_source()` can be a minimal pass-through shader for RenderGraph effects.

## Example: waaaves

`examples/waaaves` demonstrates a 3-pass feedback pipeline with complex per-pass bind groups and dual ring buffers. Study it for a complete, production-ready multi-pass implementation.

```sh
cargo run -p waaaves
```

Passes:
1. **Pipeline A** — initial video processing
2. **Pipeline B** — spatial transformation with feedback
3. **Pipeline C** — colour mixing and output
