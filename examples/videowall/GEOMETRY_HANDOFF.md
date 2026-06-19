# Video-wall geometry refactor — handoff spec

## The one rule
The calibrated wall layout (per-screen **aspect, position, orientation**) is fixed and
**aspect-neutral** — independent of the calibration photo's aspect. When applied to any
input (4:3 photo, 16:9 webcam, the matrix output), the whole layout is placed via a
**single uniform transform** (one scalar scale + translation). The total sample area may
be uniformly resized/repositioned to maximise coverage, but the layout is **never
reformed** — no per-axis stretch, ever. Each cell's pixel aspect on any target == the
screen's display aspect; relative positions are preserved.

This is what failed before and why:
- **Baking a frame-relative UV rect** (current `custom_source_rect`, photo-normalized):
  applied to a different-aspect input it stretches. ❌
- **Per-cell, per-axis compensation** (`w_uv = h_uv·display_aspect/target_aspect` with
  raw normalized centres): fixed each box's *size* aspect but left *positions* as raw
  normalized → centres spread apart on a wider input. ❌
- **Correct:** transform positions AND sizes through ONE uniform fit of the layout's
  bounding box into the target. ✅

## Coordinate model
Store per cell, computed once at detection, **aspect-neutral** (normalize everything by
the detection image HEIGHT — pixels are square, so /height keeps real proportions and
bakes the photo aspect into the x-range only):
- `wall_center: [f32;2]` = `[center_px.x / img_h, center_px.y / img_h]`
- `wall_size:   [f32;2]` = `[w_px / img_h, h_px / img_h]`  (so `wall_size.x/wall_size.y` == display aspect; portrait when rotated)
- keep `aspect_ratio`, `orientation` (orientation drives the shader's sampling rotation;
  aspect_ratio is informational/UI).

`w_px`/`h_px` are the detected bezel dims (landscape `long×short`, swapped for rotated) —
the same numbers screen_from_detection already produces, just normalized by `img_h` for
BOTH axes instead of mixed `img_w`/`img_h`.

## Resolve source rects for a target of aspect `A = target_w/target_h`
Compute once per frame over all **enabled** cells:
1. Wall bbox over all enabled cells: `min=(minx,miny)`, `max=(maxx,maxy)`,
   `bbox_w=maxx-minx`, `bbox_h=maxy-miny` (wall units).
2. Uniform scale (target-height-units per wall-unit), maximise coverage:
   `S = min(A / bbox_w, 1.0 / bbox_h)`
3. Centre: `ox = (A - bbox_w*S)/2`, `oy = (1.0 - bbox_h*S)/2`  (target-height-units)
4. Per cell → input UV:
   ```
   x_thu = (wall_center.x - wall_size.x/2 - minx)*S + ox
   y_thu = (wall_center.y - wall_size.y/2 - miny)*S + oy
   w_thu = wall_size.x * S
   h_thu = wall_size.y * S
   source_rect_uv = Rect{ x: x_thu/A, y: y_thu, width: w_thu/A, height: h_thu }
   ```
   Invariant: cell pixel-aspect on the target == `wall_size.x/wall_size.y` for ANY `A`;
   centres scale uniformly. (Single empty/degenerate cell → fall back to grid cell.)

This **replaces** the current direct `custom_source_rect` sampling and the deleted
`source_rect_for(target)`/`input_aspect` machinery. Keep `custom_source_rect` only as an
optional manual override (used verbatim if present, after the wall model).

## What stays as-is (already correct — do NOT touch)
- **Aspect classification**: tag own-frame `w/h` classified DIRECTLY, never inverted for
  rotation (`videowall.rs::classify_aspect` + `screen_from_detection`).
- **Output letterbox**: `VideoMatrixConfig.output_aspect` + the shader letterbox so
  resizing the projector WINDOW scales uniformly. (Separate axis from this refactor.)
- **Per-cell brightness/contrast/gamma**; tag-grid calibration; photo/live detect + enhance.

## Where things are
- `crates/rustjay-projection/src/matrix.rs` — `GridCellMapping` (add wall_center/wall_size),
  `source_rect` (→ needs target aspect + the config's wall bbox), `CellMappingGpu::from_mapping`,
  `MatrixStage` (compute target aspect from its **input texture** — `engine.render_target`,
  the content — and resolve all source rects; this uniform fit IS allowed and wanted, unlike
  the rejected per-axis compensation), `MatrixSync`, shader `shaders/matrix.wgsl`.
- `crates/rustjay-projection/src/videowall.rs` — `DetectedScreen` (carry wall_center/wall_size),
  `screen_from_detection` (normalize by img_h), `suggest_config` (store wall geom; keep
  `output_aspect = calib_aspect`).
- `examples/videowall/src/ui.rs` — `draw_preview` overlay (resolve source rects for the shown
  background's aspect via the same uniform fit, so photo and webcam both show the layout
  undistorted); per-cell nudge (edit wall_center/wall_size, aspect-locked).
- Test assets: `examples/videowall/testFiles/IMG_1054.JPG` (4:3 photo, 4 tags ids 0/1/2/4;
  id2 is a rotated 4:3) and `test002.json`.

## Tests to add/keep (crate `rustjay-projection`, feature `videowall`)
- Layout invariance: a set of cells resolved for A=16:9, 4:3, 9:16 → every cell's
  pixel-aspect == its display aspect, AND the relative ordering/spacing of centres is
  identical up to a uniform scale (no per-axis spread).
- Rotated 4:3 cell → portrait (wall_size.x < wall_size.y).
- Keep: `rotated_tall_tag_classifies_4_3`, `gpu_struct_sizes`, the golden `matrix_left_half_snapshot`
  (update to the new model).

## Build / verify
```
cargo test  -p rustjay-projection --features videowall
cargo clippy -p videowall
cargo build  -p videowall
```
Cannot verify egui visually here — the human will load IMG_1054 / test002 and switch the
preview between the photo and the live webcam: the green cells must keep identical shape
and relative layout on both (only uniformly scaled/centred). macOS: new leaf binaries need
the Syphon-rpath `build.rs` (already present).

## Hard "do not" list (we looped on these)
- Do NOT bake a frame-relative source rect and sample it directly (stretches on a
  different input).
- Do NOT do per-axis per-cell compensation with raw normalized centres (spreads positions).
- Do NOT invert aspect for rotated tags.
- Positions and sizes must share ONE uniform scale from the wall-bbox fit.
