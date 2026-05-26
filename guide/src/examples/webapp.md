# Web App (WebGPU + WASM)

`examples/webapp` is a self-contained browser application: the delta RGB delay effect compiled to WebAssembly, rendered via WebGPU, with a React overlay for controls.

This is not a remote-control panel for the native engine — it is the engine running *inside* the browser.

```sh
cargo run -p webapp   # doesn't apply — see build steps below
```

## What it is

| Layer | Technology |
|---|---|
| GPU rendering | Rust → WASM (`cdylib`), wgpu WebGPU backend |
| Camera capture | JavaScript (`getUserMedia` + canvas) |
| Control UI | React 18 + TypeScript, built with Vite |
| Build tooling | [Trunk](https://trunkrs.dev/) (WASM bundler for Rust) |

The Rust code never runs natively — it targets `wasm32-unknown-unknown` only (`#![cfg(target_arch = "wasm32")]`). wgpu's `BROWSER_WEBGPU` backend talks directly to the browser's WebGPU API (Chrome/Edge 113+).

## Browser requirements

WebGPU is required. Check compatibility:

| Browser | WebGPU status |
|---|---|
| Chrome 113+ | ✅ Enabled by default |
| Edge 113+ | ✅ Enabled by default |
| Firefox | 🚧 Behind a flag (`dom.webgpu.enabled`) |
| Safari 18+ | ✅ Enabled by default |

The app also requests webcam access (`getUserMedia`). Serve it over HTTPS or `localhost` — browsers block camera access on plain HTTP origins.

## Building and running

### Prerequisites

```sh
# Rust WASM target
rustup target add wasm32-unknown-unknown

# Trunk (WASM bundler)
cargo install trunk

# Node.js (for the React UI build)
node --version  # 18+ recommended
```

### First run

```sh
cd examples/webapp

# Build the React UI once (Trunk does this automatically if ui/dist is missing)
cd ui && npm install && npm run build && cd ..

# Start the dev server — opens http://localhost:8080
trunk serve
```

Trunk compiles the Rust WASM, bundles it, copies the React build output from `ui/dist/` into the final bundle, and serves everything at `http://localhost:8080`.

The React UI only needs a rebuild when you change files under `ui/src/`. The Trunk hook skips the npm build if `ui/dist/` already exists, so subsequent `trunk serve` calls are fast.

### Production build

```sh
trunk build --release
# Output in dist/ (configurable in Trunk.toml)
```

The `dist/` directory is self-contained — serve it with any static HTTP server.

## Architecture

### Startup sequence

```
Browser loads index.html
  └── Trunk loads WASM module
        └── TrunkApplicationStarted fires
              ├── start() — initialise WebGPU device, textures, pipeline
              ├── getUserMedia() — open webcam
              └── requestAnimationFrame loop begins
                    ├── JS: capture webcam frame → RGBA bytes
                    ├── JS: call update_webcam_frame(data, w, h) → WASM
                    └── Rust: render frame with WebGPU
```

### Camera → GPU

The browser has no direct GPU-texture-from-camera API, so frames travel through a CPU copy each tick:

```js
// index.html — JS side
ctx.drawImage(video, 0, 0, w, h);          // draw video into offscreen canvas
const img = ctx.getImageData(0, 0, w, h);  // read RGBA pixels from canvas
update_webcam_frame(data, w, h);           // call into WASM
```

```rust
// lib.rs — Rust side
#[wasm_bindgen]
pub fn update_webcam_frame(data: &[u8], width: u32, height: u32) {
    // write_buffer → uploads RGBA bytes to the webcam GPU texture
}
```

This is the main performance ceiling for high-resolution inputs — the `getImageData` call reads from the GPU back to CPU each frame. For 1280×720 it's fine in practice.

### WASM exports (`window.rustjay`)

After startup, the JS side registers the WASM parameter setters on `window.rustjay`:

```ts
window.rustjay = { set_delay_r, set_delay_g, set_delay_b, set_mix };
```

The React component calls these directly:

```ts
// DelaySliders.tsx
const call = useCallback((fn: string, value: number) => {
    window.rustjay?.[fn]?.(value);
}, []);

// on slider change:
call('set_delay_r', newValue);
```

There is no network round-trip — the React UI and the WASM renderer run in the same browser tab, communicating through `thread_local!` state:

```rust
thread_local! {
    static PARAMS: RefCell<Params> = RefCell::new(Params::default());
}

#[wasm_bindgen]
pub fn set_delay_r(v: i32) {
    PARAMS.with(|p| p.borrow_mut().delay_r = v.clamp(-64, 64));
}
```

### Render loop

The render loop runs via `requestAnimationFrame` — no Winit, no event loop, no threads. It reads the current `PARAMS`, uploads uniforms, and runs the WebGPU render pass:

```rust
fn render_frame(app: &mut App) {
    let params = PARAMS.with(|p| *p.borrow());
    // upload uniforms, run render pass, copy to feedback texture
}
```

The feedback texture (previous frame's output) is updated with `copy_texture_to_texture` at the end of each frame so the delta shader can read it next tick.

## The effect

The effect is a simplified version of `examples/delta`: per-channel pixel offset with a mix between the live camera and the feedback texture.

```
┌──────────┐     pixel offset (R, G, B independently)
│  Webcam  │ ──────────────────────────────────────────┐
│  (live)  │                                           ↓
└──────────┘                                    ┌─────────────┐
                                                │    WGSL     │ → output
┌──────────┐     sampled with uv offset         │   shader    │
│ Feedback │ ──────────────────────────────────→│             │
│ (t-1)    │                                    └─────────────┘
└──────────┘         ↑
                     └── copy_texture_to_texture each frame
```

Parameters exposed to the React UI:

| Export | Type | Range | Effect |
|---|---|---|---|
| `set_delay_r` | `i32` | `[-64, 64]` | Red channel horizontal pixel offset |
| `set_delay_g` | `i32` | `[-64, 64]` | Green channel horizontal pixel offset |
| `set_delay_b` | `i32` | `[-64, 64]` | Blue channel horizontal pixel offset |
| `set_mix` | `f32` | `[0, 1]` | Blend between live camera and feedback |

## Extending the webapp

### Adding a parameter

**1. Add to the `Params` struct and export a setter:**

```rust
// lib.rs
pub struct Params {
    pub delay_r: i32,
    // ...
    pub brightness: f32,   // add this
}

#[wasm_bindgen]
pub fn set_brightness(v: f32) {
    PARAMS.with(|p| p.borrow_mut().brightness = v.clamp(0.0, 2.0));
}
```

**2. Pass it through uniforms:**

```rust
// delta.rs
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DeltaUniforms {
    // existing fields ...
    pub brightness: f32,
    pub _pad: [f32; 3],
}
```

**3. Use it in the shader (`src/shaders/delta.wgsl`):**

```wgsl
out = out * u.brightness;
```

**4. Add a slider in React (`ui/src/components/DelaySliders.tsx`):**

```tsx
<Slider
    label="Brightness"
    value={brightness}
    min={0} max={2} step={0.01}
    onChange={(v) => { setBrightness(v); call('set_brightness', v); }}
    color="#ffee44"
/>
```

Then rebuild the React UI (`cd ui && npm run build`) and restart `trunk serve`.

### Replacing the effect

The rendering logic lives in `src/delta.rs` and `src/shaders/delta.wgsl`. Swap in a different WGSL shader and update `DeltaUniforms` to match. The startup, camera capture, and React overlay are all independent of the specific effect.

## Project layout

```
examples/webapp/
├── src/
│   ├── lib.rs          # WASM entry point, render loop, wasm-bindgen exports
│   ├── delta.rs        # Pipeline creation, DeltaUniforms
│   ├── webcam.rs       # update_webcam helper (CPU→GPU texture upload)
│   └── shaders/
│       └── delta.wgsl  # Fragment shader
├── ui/
│   ├── src/
│   │   ├── App.tsx
│   │   └── components/
│   │       └── DelaySliders.tsx   # React control overlay
│   └── package.json
├── index.html          # Trunk entry — wires WASM init, webcam loop, React mount
└── Trunk.toml          # Build config — port 8080, React pre-build hook
```
