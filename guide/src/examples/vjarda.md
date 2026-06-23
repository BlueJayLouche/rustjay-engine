# vjarda — Full Multi-Deck VJ App

`examples/vjarda` is the flagship application built on the engine — a complete
performance tool rather than a single-effect demo. It assembles
`rustjay-mixer`, `rustjay-isf`, `rustjay-api`, and the modulation stack into one
runnable app.

```sh
cargo run -p vjarda                       # default: mixer + egui + webcam + LED
cargo run -p vjarda --features projection # add the Stage / projection-mapping output
cargo run -p vjarda --all-features        # NDI, Syphon, ProDJ, HAP, ffmpeg, recording…
```

## What it is

Two (or more) **channels**, each composited through the mixer with a
crossfader, blend modes, and transitions. Every channel is driven by a **deck
graph**: a stack of sources and ISF effect instances you build at runtime. You
bring `.fs` ISF shaders and video sources; vjarda handles the routing,
compositing, modulation, and output.

## Key concepts

- **Decks & channels** — each channel owns a deck graph (`graph::Deck`,
  `DeckCompositor`). Sources (`CameraSource`, `SolidColorSource`, ISF
  generators, optional `FfmpegSource`/`HapSource`) feed a chain of ISF effects.
- **FX chains** — drag ISF filters onto a deck; parameters are live-mappable.
- **Scene / topology persistence** — decks and FX survive save/reload. The
  scene stores *topology descriptors* and replays them with preserved UUIDs, so
  reloaded graphs reconnect to their MIDI/LFO mappings. See `scene::Scene` and
  `persistence/`.
- **Stage tab** (`--features projection`) — place output surfaces on a canvas,
  with an aspect-correct, zoomable **live preview** of the master output and
  per-surface pixel sizing. Surfaces feed `rustjay-projection`.
- **LED Map tab** — calibrate addressable LED strips and play them back over
  sACN. See [Lighting & LED](../lighting.md).
- **Outputs** — window output plus lifecycle-managed NDI / Syphon senders
  (broadcast as `vjarda — <name>`). The top bar shows WEB / OSC / sink pills for
  whatever is live.
- **External control** — the Web parameter server, OSC, and MIDI all reach into
  any mapped parameter. The top bar exposes **MIDI MAP** and **LFO MAP** toggles:
  enter a map mode, then click a control to bind it.

## Where to look

| Area | Module |
|---|---|
| App assembly, state, render hook | `src/lib.rs` |
| Deck graph & compositor | `src/graph/` |
| Sources (camera, solid, ffmpeg, HAP) | `src/sources/` |
| Scene model & save/load | `src/scene/`, `src/persistence/` |
| Stage / projection surfaces | `src/stage/` |
| LED calibration + sACN tab | `src/ui/ledmap_tab.rs` |
| Web API snapshot types | `src/api_state.rs` |

vjarda is the best reference for how the engine's pieces compose into a real
app — read it alongside the [mixer](../rendering/render-graph.md) and
[lighting](../lighting.md) chapters.
