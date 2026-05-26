# Delta — RGB Delay / Motion Extraction

`examples/delta` implements RGB delay — a temporal video effect where the red, green, and blue channels are sampled from independently delayed frames. The result is chromatic motion trails that colour-code the direction and speed of movement in the image.

```sh
cargo run -p delta
```

A related version using the egui backend instead of ImGui is in [delta-egui](delta-egui.md).

## What it does

Each channel samples a different point in time from a 16-slot frame-history ring buffer. With red at frame 0, green at frame 2, and blue at frame 4 (the defaults), a moving object leaves a trail: its current position is in all three channels, but its recent positions are visible as distinct red, green, or blue ghosts.

The effect is inspired by [Posy's colour delay work](https://www.youtube.com/watch?v=nsqbnNa4q00) and the RGB delay patches found on analogue video synthesisers.

## Parameters

| Parameter | Type | Range | Default | Description |
|---|---|---|---|---|
| **Red Delay** | int | 0–16 frames | 0 | History slot for the red channel |
| **Green Delay** | int | 0–16 frames | 2 | History slot for the green channel |
| **Blue Delay** | int | 0–16 frames | 4 | History slot for the blue channel |
| **Intensity** | float | 0–1 | 1.0 | Overall effect strength |
| **Blend Mode** | enum | 8 modes | Replace | How delayed channels composite |
| **Grayscale Input** | bool | — | true | Desaturate input before delay processing |
| **Red / Green / Blue Gain** | float | −2–2 | 1.0 | Per-channel gain; negative values invert the channel |
| **Input Mix** | float | 0–1 | 0.0 | Blend between effect output and the raw live input |
| **Trail Fade** | float | 0–1 | 0.0 | Fade out old history frames — longer trails at higher values |
| **Threshold** | float | 0–1 | 0.0 | Cut pixels below this luminance — isolates bright motion |
| **Smoothing** | float | 0–1 | 0.0 | Temporal smoothing between frames — reduces flicker |

### Blend modes

| Mode | What it does |
|---|---|
| **Replace** | Each channel is taken directly from its delayed frame |
| **Add** | Delayed channels are added to the live frame — can bloom/clip |
| **Multiply** | Darkens where channels agree |
| **Screen** | Inverse multiply — brightens without clipping |
| **Difference** | Absolute difference — highlights what changed between delays |
| **Overlay** | Contrast-dependent blend — darks multiply, lights screen |
| **Lighten** | Per-pixel maximum of live and delayed |
| **Darken** | Per-pixel minimum of live and delayed |

`Difference` mode with matching delays and inverted gains is a clean motion-extraction technique: static areas cancel to black, moving areas glow in the delay colour.

## Architecture

Delta overrides `render()` and manages its own GPU pipeline — see [Frame History & Custom Pipelines](../rendering/frame-history.md) for the general pattern.

### FrameHistory

`FrameHistory` is a 16-slot ring buffer of GPU textures. Each slot is a full-resolution `Bgra8Unorm` render target:

```
Frame N-16  Frame N-15  ...  Frame N-1   Frame N (write head)
     ↑                              ↑
 get_frame(15)               get_frame(0) — most recent completed frame
```

Each frame:
1. `push_frame()` copies the current video input (`input_texture`) into the write slot via `copy_texture_to_texture` — a pure GPU-side copy, no CPU round-trip
2. The write index advances modulo 16
3. `get_frame(n)` looks up the slot `n` steps behind the write head

```rust
fn push_frame(&mut self, source: &wgpu::Texture, encoder: &mut wgpu::CommandEncoder) {
    encoder.copy_texture_to_texture(src, dest, size);
    self.write_index = (self.write_index + 1) % self.max_history;
}

fn get_frame(&self, frames_ago: usize) -> Option<&Texture> {
    let index = if frames_ago < self.write_index {
        self.write_index - 1 - frames_ago
    } else {
        self.max_history - 1 - (frames_ago - self.write_index)
    };
    self.frames.get(index)
}
```

`FrameHistory::resize()` detects resolution changes and reallocates all slots — this handles window resizes gracefully without a crash.

### Bind group layout

The shader receives four textures and one shared sampler on group 0:

```
@group(0) @binding(0)  red_delayed_frame
@group(0) @binding(1)  green_delayed_frame
@group(0) @binding(2)  blue_delayed_frame
@group(0) @binding(3)  live_input
@group(0) @binding(4)  sampler (shared)
```

The bind group is rebuilt each frame after looking up the three history slots — this is cheap because it's a struct of `TextureView` references, not copies.

### Uniform layout

```rust
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable)]
struct DeltaUniforms {
    delays:       [f32; 4], // red, green, blue, max_history
    settings:     [f32; 4], // intensity, blend_mode, grayscale, unused
    channel_gain: [f32; 4], // red, green, blue, unused
    mix_options:  [f32; 4], // input_mix, trail_fade, threshold, smoothing
}
```

Four `vec4<f32>` blocks — 64 bytes total, exactly 4× the 16-byte wgpu uniform alignment.

## The Motion tab

Delta ships a custom `MotionTab` that replaces the engine's built-in Motion tab:

```rust
impl AnyGuiTab for MotionTab {
    fn name(&self) -> &str { "Motion" }
    fn replaces(&self) -> Option<GuiTab> { Some(GuiTab::Motion) }
    // ...
}
```

Sliders call `engine.set_param_base(id, value)` to keep the engine's parameter registry in sync — this ensures LFO and audio routing targets stay consistent with the displayed values.

## Preset tips

Because the three delay values interact so strongly with the blend mode, saving named presets for combinations you like is worth doing. Good starting points:

- **Motion trails:** R=0, G=4, B=8, Blend=Replace, Grayscale=on
- **Chroma ghost:** R=0, G=8, B=16, Blend=Screen, Grayscale=off, all Gains=1
- **Inversion ghost:** R=0, G=2, B=4, Blend=Difference, Red Gain=−1, Blue Gain=−1
- **Smear:** R=0, G=1, B=2, Blend=Add, Trail Fade=0.3, Smoothing=0.2
