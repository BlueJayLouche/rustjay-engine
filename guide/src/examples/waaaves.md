# Waaaves — Multi-Pass Feedback Pipeline

`examples/waaaves` is a port of the original `rustjay-waaaves` hardware effect — a three-pass GPU pipeline with two independent feedback delay lines, ~100+ parameters split across three processing blocks, and a full custom control UI.

```sh
cargo run -p waaaves
```

This is the most complex example in the repository. It demonstrates custom multi-pass rendering, dual ring buffers, per-pass uniform buffers, and a structured parameter/tab system.

## What it does

The pipeline processes video through three successive shader passes. Each pass reads from earlier passes and its own feedback history, producing a rich accumulation of geometric distortion, colour manipulation, and temporal delay.

```
Live Input ──────────────────────────────────┐
                                             ↓
                                    ┌──────────────┐     fb1 ring buffer
                                    │   Block A    │ ←── (up to 30 frames)
                                    └──────┬───────┘
                                           │ intermediate_a
                                           ↓
                                    ┌──────────────┐     fb2 ring buffer
                                    │   Block B    │ ←── (up to 30 frames)
                                    └──────┬───────┘
                                           │ intermediate_b
                                           ↓
                                    ┌──────────────┐
                                    │   Block C    │ ──→ Output
                                    └──────────────┘
```

### Block A

Takes the live input (CH1) plus an optional second channel (CH2) and the `fb1` feedback history. Applies:
- Per-channel geometry: X/Y/Z displacement, rotation, zoom
- HSB colour adjustment, posterise, kaleidoscope, blur/sharpen
- Mirror, flip, and overflow modes
- CH2 keying (colour key with threshold and softness)
- FB1 delay mix with configurable delay time (frames)

Output feeds `intermediate_a` and is also written into the `fb1` ring buffer.

### Block B

Takes `intermediate_a` and the `fb2` feedback history. Applies the same geometry and colour processing set as Block A, plus its own feedback delay.

Output feeds `intermediate_b` and is written into the `fb2` ring buffer.

### Block C

Takes both `intermediate_a` and `intermediate_b`. Applies:
- Output geometry and colour transforms for each intermediate
- A colour matrix mixer (R→R, R→G, R→B, G→R, etc.)
- Final HSB and posterise
- A global mix amount between the two intermediates

Output is the final rendered frame.

## Parameters

Parameters are split into three blocks reflecting the three passes, each with its own tab in the control window.

### Block 1 tab (≈ 60 parameters)

Controls CH1 input processing, the CH2 key, and the FB1 delay line:

| Group | Key params |
|---|---|
| CH1 geometry | `ch1_x_displace`, `ch1_y_displace`, `ch1_z_displace`, `ch1_rotate` |
| CH1 colour | `ch1_hsb_attenuate_h/s/b`, `ch1_posterize`, `ch1_solarize` |
| CH1 filters | `ch1_blur_amount/radius`, `ch1_sharpen_amount/radius` |
| CH1 spatial | `ch1_kaleidoscope_amount/slice`, `ch1_h/v_mirror`, `ch1_h/v_flip` |
| CH2 key | `ch2_mix_amount`, `ch2_key_value_r/g/b`, `ch2_key_threshold/soft` |
| FB1 delay | `fb1_delay_time` (frames), `fb1_mix_amount` |

### Block 2 tab (≈ 40 parameters)

Controls the Block B pass geometry, colour, and the FB2 delay line.

### Block 3 tab (≈ 30 parameters)

Controls the final composite: geometry applied to each intermediate before mixing, the colour matrix, and the output blend.

## Architecture

### Dual ring buffers

The `RingBuffer` struct (`render/ring_buffer.rs`) is a circular array of GPU textures, all allocated at the same resolution:

```rust
pub struct RingBuffer {
    textures:   Vec<(wgpu::Texture, wgpu::TextureView)>,
    write_head: usize,
    capacity:   usize,
}

// Reading N frames back (minimum 1 — write head holds incomplete frame)
pub fn read_view(&self, frames_back: usize) -> &wgpu::TextureView {
    let idx = frames_back.max(1).min(self.capacity - 1);
    let i   = (self.write_head + self.capacity - idx) % self.capacity;
    &self.textures[i].1
}

// Advance after each frame
pub fn advance(&mut self) {
    self.write_head = (self.write_head + 1) % self.capacity;
}
```

`fb1` and `fb2` each hold up to 30 frames (configurable via `max_delay_frames` in the state). Bind groups that reference ring buffer slots are pre-built per slot and cached — they're looked up by index rather than rebuilt each frame:

```rust
// Pre-built once (or on resize):
fb1_bind_groups: Vec<wgpu::BindGroup>,  // one per slot
fb2_bind_groups: Vec<wgpu::BindGroup>,

// Each frame — just an index lookup:
let delay = state.block1.fb1_delay_time as usize;
let bg = &self.fb1_bind_groups[ring_buffer.read_index(delay)];
```

This is more efficient than rebuilding bind groups every frame and is the recommended pattern for variable-delay feedback effects.

### Per-pass uniforms

Each of the three passes has its own uniform buffer and bind group, because the parameter blocks are independent:

```rust
struct WaaavesEffect {
    uniform_buf_a: Option<wgpu::Buffer>,
    uniform_buf_b: Option<wgpu::Buffer>,
    uniform_buf_c: Option<wgpu::Buffer>,
    uniform_bg_a:  Option<wgpu::BindGroup>,
    uniform_bg_b:  Option<wgpu::BindGroup>,
    uniform_bg_c:  Option<wgpu::BindGroup>,
}
```

All three uniform buffers are uploaded every frame inside `render()` before the passes execute.

### Bind group layout per pass

| Pass | Group 0 | Group 1 | Group 2 |
|---|---|---|---|
| **Block A** | CH1 + CH2 textures (4 bindings) | Uniform buffer | FB1 + temporal textures (4 bindings) |
| **Block B** | `intermediate_a` (2 bindings) | Uniform buffer | FB2 + temporal textures (4 bindings) |
| **Block C** | `intermediate_a` + `intermediate_b` (4 bindings) | Uniform buffer | — |

The layouts are created in `render/passes.rs` and shared across all frames.

### Resize handling

When the input resolution changes, all textures are reallocated:
- `intermediate_a`, `intermediate_b` — single textures, recreated
- `fb1`, `fb2` — ring buffers, `RingBuffer::resize()` is called, which reallocates all capacity slots and resets the write head
- All cached bind groups are rebuilt after resize

### Dummy texture

Shader slots that don't always have a real texture bound (e.g. CH2 when no second source is active) use a 1×1 black `Bgra8Unorm` texture (`dummy`). This avoids validation errors from unbound texture slots without branching in the shader.

## Pixel-pick FSM

The `PickState` field in `WaaavesState` implements a three-step finite state machine for picking a colour from the output frame to use as a key value:

```
Idle  →  Armed { target }  →  Pending { target }  →  Idle
         (button clicked)      (next render completes)
```

`target` identifies which colour key destination receives the picked value (CH2, FB1, FB2, or Final). The pixel read happens on the Rust side at the completion of the pending render, not on the GPU.

## Module layout

```
examples/waaaves/src/
├── main.rs              # WaaavesEffect struct, EffectPlugin impl
├── state.rs             # WaaavesState, PickState FSM
├── uniforms.rs          # Per-pass uniform structs
├── params/
│   ├── block1.rs        # Block1Params (~60 fields)
│   ├── block2.rs        # Block2Params (~40 fields)
│   ├── block3.rs        # Block3Params (~30 fields)
│   └── descriptors.rs   # ParameterDescriptor declarations for all params
├── render/
│   ├── passes.rs        # Pipeline + bind group layout creation
│   └── ring_buffer.rs   # RingBuffer — circular texture buffer
├── tabs/
│   ├── block1_tab.rs    # ImGui tab for Block 1
│   ├── block2_tab.rs    # ImGui tab for Block 2
│   └── block3_tab.rs    # ImGui tab for Block 3
├── lfo_ui.rs            # Custom LFO UI (hybrid: native controls + engine LFO)
└── legacy_preset.rs     # Preset compatibility with the original waaaves format
```
