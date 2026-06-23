# videowall — HDMI-Matrix / Video-Wall Mapper

`examples/videowall` drives **N projector or screen outputs**, each compositing
regions of one live source onto a grid of cells (a `MatrixStage` /
`ProjectionStage`). It's the tool for a wall of monitors fed from a single HDMI
matrix, or several projectors tiling one image.

```sh
cargo run -p videowall          # default: egui + webcam + NDI + AprilTag calibration
```

## What it does

- One shared **source** (picked in the built-in Input tab) is sampled per output.
- Each output is a `GridSize` of cells (seeded as one 3×3, add more at runtime).
- The **Matrix** tab manages outputs and per-cell source→screen mapping.
- Geometry is **output-authoritative**: each output window owns its own grid, so
  cells line up to physical screens rather than to a global canvas.

## AprilTag auto-calibration

With the default `videowall` feature, the app detects **AprilTags** shown on
each screen and solves the tag grid into a mapping automatically — no manual
corner-dragging. This pulls in a C toolchain (the detector is C-FFI):

```sh
cargo run -p videowall --no-default-features --features egui,webcam,ndi
```

drops AprilTag if you only want manual mapping and no C compiler.

## Where to look

| Area | File |
|---|---|
| App assembly, output sync, render plugin | `src/app.rs` |
| Matrix tab UI | `src/ui.rs` |
| Per-cell passthrough shader | `src/passthrough.wgsl` |

The matrix mapping itself lives in `rustjay-projection` (`MatrixStage`,
`GridSize`); videowall is the multi-window app wired around it.
