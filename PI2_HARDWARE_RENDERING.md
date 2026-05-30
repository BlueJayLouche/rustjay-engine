# Pi 2 Hardware Rendering — Plan

Goal: run `flux --nogui` on Pi 2 VC4 hardware without `LIBGL_ALWAYS_SOFTWARE=1`.

---

## Probe results (2026-05-30, Mesa 26.1.1-arch1.2, vc4 gallium)

```
OpenGL ES profile version:  OpenGL ES 2.0
GLSL ES version:            OpenGL ES GLSL ES 1.0.16
OpenGL (desktop) version:   2.1 Mesa 26.1.1
```

### UBO support — absent in every profile

| Profile | UBO extension | Result |
|---|---|---|
| GLES 2.0 | `GL_OES_uniform_buffer_object` | **not present** |
| GLES 2.0 | `GL_EXT_uniform_buffer_object` | **not present** |
| Desktop GL 2.1 | `GL_ARB_uniform_buffer_object` | **not present** |

UBOs were introduced in OpenGL 3.1 / GLES 3.0. vc4 hardware is locked at GL 2.1 /
GLES 2.0 and predates UBOs entirely. No driver patch, environment variable, or EGL
override can add hardware that was never there.

### GLSL ES version constraint

The highest GLSL the driver accepts is GLSL ES 1.0.16. naga's GLES backend always emits
`#version 300 es`, which the driver rejects outright. Lowering the GLSL target would
require changes to naga.

---

## Why the vendored wgpu-hal patch approach is dead

The plan was:
1. Patch `egl.rs` to try a GLES 2.0 context fallback.
2. Patch `adapter.rs` to guard UBO queries behind an OES extension check.

Step 2 fails fatally: `max_uniform_buffers_per_shader_stage` would be `0`, so wgpu
would refuse to compile any pipeline that uses a uniform buffer. Every wgpu shader uses
uniform buffers (they are how `var<uniform>` is implemented). No flux pass could be
created.

Even if context creation succeeded, shader compilation would fail: the GLSL emitted by
naga (`#version 300 es`) requires GLES 3.0 and the driver would reject it.

---

## Realistic paths forward

### Path A — Accept Pi 2 limitations (current state, no code change)

Software rendering via llvmpipe is the correct answer for Pi 2. The guide already
documents this. At 640×480 (matching the webcam capture size) llvmpipe is realtime on
Pi 2.

**Recommended if** the goal is just to run flux. Lower `internal_width/height` in
`~/.config/rustjay/flux.json` to `640`/`480`.

---

### Path B — Native GLES 2.0 renderer for flux (bypass wgpu)

Implement a second render backend for flux that uses raw EGL + GLES 2.0 directly,
skipping wgpu entirely for the three render passes. The engine handles window/surface
creation; flux opts into its own GL context for rendering.

**What this involves:**

1. **Trait extension**: add an optional `fn render_gles2(...)` method to `EffectPlugin`
   that receives the raw EGL display/surface/context handles. If implemented, the engine
   skips its wgpu render and calls this instead.

2. **Flux GLES 2.0 pass**: write `flux_flow.frag`, `flux_warp.frag`, `flux_blit.frag`
   in GLSL ES 1.00. Replace the UBO (`FluxUniforms`) with individual `uniform float`
   declarations. Use the `glow` crate (already in the dependency tree via wgpu-hal) for
   the GL calls.

3. **FBO ping-pong**: FBOs are core in GLES 2.0 (`GL_OES_framebuffer_object` is always
   present). The same ping-pong texture structure works.

4. **EGL context setup**: the engine already creates an EGL context for wgpu. For Pi 2
   we create a GLES 2.0 EGL context alongside (or instead of) wgpu's context, sharing
   resources as needed.

**Scope**: medium. Roughly 500–700 lines of new code confined to
`examples/flux/src/gles2.rs` and small additions to the engine trait + event handling.
Sputnik is unaffected.

**Gate**: `#[cfg(feature = "gles2")]` or runtime detection via `std::env::var("GLES2")`.

---

### Path C — Upgrade to Pi 4 or Pi 5 (hardware path works today)

Pi 4 (VideoCore VI) and Pi 5 (VideoCore VII) both support Vulkan via V3DV. The engine's
Vulkan path works on them without any software rendering or patches. If the goal is
hardware-accelerated flux on a Pi, a Pi 4 is the simplest answer.

---

## Decision

| Goal | Recommendation |
|---|---|
| Run flux on the Pi 2 I have now | Path A (llvmpipe at 640×480) |
| Real-time hardware rendering on Pi 2 | Path B (native GLES 2.0 renderer) — significant work |
| Hardware rendering, willing to upgrade hardware | Path C (Pi 4 / Pi 5) |

---

## If pursuing Path B — files to create/change

| File | Change |
|---|---|
| `crates/rustjay-core/src/plugin.rs` | Add optional `render_gles2` hook to `EffectPlugin` |
| `crates/rustjay-engine/src/app/events.rs` | Detect GLES 2.0 context availability; call `render_gles2` instead of wgpu render |
| `examples/flux/src/gles2.rs` | GLES 2.0 implementation of the three flux passes |
| `examples/flux/src/shaders/flux_flow_es1.frag` | GLSL ES 1.00 flow shader |
| `examples/flux/src/shaders/flux_warp_es1.frag` | GLSL ES 1.00 warp shader |
| `examples/flux/src/shaders/flux_blit_es1.frag` | GLSL ES 1.00 blit shader |
