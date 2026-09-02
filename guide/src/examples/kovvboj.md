# kovvboj — Multi-Layer VJ App

`crates/kovvboj` is the flagship application built on the engine — a complete
performance tool rather than a single-effect demo. It assembles
`rustjay-mixer`, `rustjay-isf`, `rustjay-api`, and the modulation stack into one
runnable app.

```sh
cargo run -p kovvboj                       # default: mixer + egui + webcam + LED
cargo run -p kovvboj --features projection # add the Stage / projection-mapping output
cargo run -p kovvboj --all-features        # NDI, Syphon, ProDJ, HAP, ffmpeg, recording…
```

## What it is

A **layer** is one visual: a source, its FX chain, an opacity and a blend mode
against the layers beneath it. The top of the stack composites over the
bottom, and that is the whole structure — there is no channel level above the
layers and no crossfader. In its place a pinned MASTER row carries a
**dimmer** (a real engine parameter, so MIDI, OSC and LFOs reach it) and the
master FX chain that every layer passes through on its way out.

You bring `.fs` ISF shaders and video sources; kovvboj handles the routing,
compositing, modulation, and output. The control window is a three-column egui
shell: the library on the left, the layer stack in the centre, a live preview
and the inspector on the right. The centre column switches between MIX, STAGE
and MAP modes; everything else opens as windows from the View menu.

## Key concepts

- **Layers** — each layer is a `rustjay_mixer::Channel`: the source lives in
  its `effect` slot, ISF filters in its `chain`, with opacity, one of 15 blend
  modes, solo and mute on top. Sources (`CameraSource`, `SolidColorSource`,
  ISF generators, NDI/Syphon/Spout receivers, optional
  `FfmpegSource`/`HapSource`/`StreamSource`) feed the chain. Restack the pile
  by dragging a layer's `≡` handle.
- **The library** — four groups with different verbs. DEVICES (cameras, NDI,
  Syphon, Spout), MEDIA (images, videos, streams) and GENERATORS (solid
  colour, generator shaders) each make a new layer from their ➕. EFFECTS
  (filter shaders — an ISF that declares an `inputImage`) append to the
  selected layer from theirs, and are the only rows that drag: drop one onto
  any layer's chain or the master chain. DEVICES always lists a generic
  `NDI…` / `Syphon…` / `Spout…` entry so you can build the layer before the
  sender exists — pick the actual server in the inspector, which can also
  re-point a live layer without losing its chain or mappings.
- **FX chains** — two places FX live: per layer, and master. Reorder within a
  chain or move an effect between chains by dragging its chip; click a chip's
  dot to bypass it.
- **Undo/redo** — ⌘Z / ⇧⌘Z cover structural edits: adding, removing, moving
  and re-ordering layers and effects. Parameter changes are deliberately not
  undoable — they are continuous and driven by MIDI/LFO/OSC.
- **Scene persistence** — layers and FX survive save/reload. Scenes are
  versioned: one written before the layer model is not loaded — kovvboj tells
  you the scene predates layers and leaves the file untouched. Scenes store
  *topology descriptors* and replay them with preserved UUIDs, so reloaded
  stacks reconnect to their MIDI/LFO mappings. See `scene::Scene` and
  `persistence/`.
- **Stage mode** (`--features projection`) — place output surfaces on a canvas,
  with an aspect-correct, zoomable **live preview** of the master output and
  per-surface pixel sizing. Surfaces feed `rustjay-projection`.
- **LED Map mode** — calibrate addressable LED strips and play them back over
  sACN. See [Lighting & LED](../lighting.md).
- **Outputs** — window output plus lifecycle-managed NDI / Syphon senders
  (broadcast as `kovvboj — <name>`) from the Outputs window. The top bar shows
  WEB / OSC / sink pills for whatever is live, alongside BPM and FPS readouts.
- **External control** — the Web parameter server, OSC, and MIDI all reach
  into any mapped parameter, including the master dimmer. Arm MIDI-learn or
  LFO-assign from the MIDI / Modulation windows, then click a control in the
  inspector to bind it.

## Where to look

| Area | Module |
|---|---|
| App assembly, state, render hook | `src/lib.rs` |
| Three-column shell (menus, modes, panels) | `src/shell.rs` |
| Layer stack, library, inspector | `src/ui/mod.rs` |
| Sources (camera, solid, ffmpeg, HAP, streams) | `src/sources/` |
| Scene model & save/load | `src/scene/`, `src/persistence/` |
| Stage / projection surfaces | `src/stage/` |
| LED calibration + sACN | `src/ui/ledmap_tab.rs` |
| Web API snapshot types | `src/api_state.rs` |

kovvboj is the best reference for how the engine's pieces compose into a real
app — read it alongside the [mixer](../rendering/render-graph.md) and
[lighting](../lighting.md) chapters.
