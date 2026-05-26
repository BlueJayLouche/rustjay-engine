# Built-in Tabs

rustjay-engine's control window comes with a set of tabs that cover the most common live-performance needs. You don't have to implement any of these yourself — they work out of the box.

## What the built-in tabs do

| Tab | Purpose |
|---|---|
| **Input** | Select and configure the video input source |
| **Audio** | Live FFT, beat detection, BPM, tap tempo, sync source selection |
| **LFO** | Configure 3 LFO banks — waveform, rate, depth, target parameter |
| **MIDI** | Device list, CC learn mode, current parameter mappings |
| **OSC** | Display OSC server address; confirm parameter paths |
| **Output** | Enable/configure NDI, Syphon, Spout, V4L2 output |
| **Presets** | Save, load, and quick-slot presets |
| **Sync** | Tempo source selector (only visible with `link` or `prodj` feature) |

## Hiding tabs you don't need

If your effect doesn't use colour parameters, hiding the Color tab keeps the UI focused:

```rust
fn hidden_tabs(&self) -> Vec<GuiTab> {
    vec![GuiTab::Color]
}
```

The full list of hide-able tabs mirrors the constants in `GuiTab`.

## Parameter sliders

When you declare parameters in `parameters()`, they appear automatically in the built-in control UI — no extra code needed. The engine groups them by `ParamCategory`:

| Category | Placement |
|---|---|
| `ParamCategory::Color` | Color tab |
| `ParamCategory::Motion` | Custom/Effect tab |
| `ParamCategory::Timing` | Audio tab's parameter section |
| `ParamCategory::Custom` | A generated "Effect" section |

Each declared parameter gets:
- A slider in the appropriate tab
- An entry in the MIDI learn list
- An OSC address (`/rustjay/<id>`)
- Availability as an LFO target in the LFO tab
- Slot in the audio routing matrix

Next: [Custom Tabs](custom-tabs.md) — adding your own panel to the control window.
