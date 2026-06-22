# DESIGN: CV LED Mapping

Map addressable LEDs (ws281x et al.) by flashing a calibration pattern and
recovering each LED's position from camera/video. Export a map that
rustjay-engine/vjarda (and later stageLX) can sample.

## Decisions (locked)

| Axis | Choice |
|------|--------|
| Geometry | **2D now, 3D-ready format** — recover per-LED (u,v); schema reserves optional z |
| Sync | **Both** — app drives LEDs + captures (primary); uploaded video fallback via self-ID pattern |
| Output protocol | **sACN (E1.31)** — reuse `rustjay-lighting` (`e131`/`SacnTransport`); Art-Net deferred |
| Home | **New crate `rustjay-ledmap`** in the rustjay-engine workspace (rustjay-mapper is retired) |
| GUI | **egui tab** in `rustjay-gui` (engine's live GUI stack) |
| First consumer | **rustjay-engine / vjarda** — needs a new freeform point-sample map type (current code is grid-only) |

## Why a crate in rustjay-engine, not a new app

rustjay-engine already owns the expensive 80%; this crate adds only CV + format:
- Output: **reuse `rustjay-lighting`** — `SacnTransport` + `DmxSender` drive the
  pattern; `pack_fixtures` packs `dmx_frame()`. No hand-rolled sACN.
- Capture: `nokhwa` webcam already a workspace dep.
- GPU + display + GUI: `wgpu` + egui via `rustjay-gui` / `rustjay-render`.
- Rectify (optional): `rustjay-projection` homography, image → content space.

`rustjay-ledmap` itself depends only on `rustjay-lighting` + serde — it stays a
pure CV/format crate (no GPU, no GUI), so it's trivially testable. New code = the
calibrator + detector + format here, a `PointMap` sampler in the engine, and an
egui calibration tab.

## Pipeline (MECE)

```
emit pattern ──┐                 ┌─ live: app drives output, exact frame↔index
               ├─ CAPTURE frames ┤
external ──────┘                 └─ uploaded: video file, pattern self-identifies
                     │
              BACKGROUND SUBTRACT  (all-off reference frame)
                     │
              DETECT blobs         (threshold → connected components →
                     │              intensity-weighted subpixel centroid)
              IDENTIFY index       (sequential: frame k = LED k |
                     │              gray-code: decode on/off bits per blob)
              RECTIFY (optional)    (rustjay-projection homography → content space)
                     │
              NORMALIZE → (u,v) ∈ [0,1]
                     │
              EXPORT ledmap.json
```

## Calibration pattern — phased, one shared format

One documented temporal sequence so a live-driven run and an externally-driven
uploaded video decode identically.

- **Phase 1 (MVP, live only): sequential flash.** Light one LED per held frame,
  in patch order. Frame k ↔ LED k. No decode, dead simple, robust. O(N) frames.
- **Phase 2 (shared, scales + enables uploaded): temporal Gray code.** Each LED
  encodes its index across `ceil(log2 N)` frames via on/off; decode the bit
  pattern at each blob location. Leading **all-on** sync frame marks sequence
  start (so uploaded video aligns without external timing); trailing all-off.
  Self-identifying → same path for live and uploaded.

Hold each pattern frame for several camera frames and sample mid-hold —
defeats exposure/rolling-shutter mismatch without genlock.

`// ponytail:` no OpenCV/imageproc. Blob detection = threshold + flood-fill
connected components + weighted centroid on a plain luma slice (`detect.rs`).
Add `imageproc::region_labelling` only if dense/overlapping detection fails.

## Export format — `ledmap.json` v1 (the bridge)

Neutral source of truth; thin per-consumer adapters. Engine map is the first
adapter but the file is consumer-agnostic.

```json
{
  "version": 1,
  "space": "canvas-2d",
  "source": { "tool": "rustjay-ledmap", "captured": "2026-06-22T...", "image_wh": [1920,1080] },
  "leds": [
    { "i": 0, "u": 0.121, "v": 0.840, "universe": 1, "channel": 1, "order": "GRB", "conf": 0.98 }
  ]
}
```

- `i` = LED index in wiring order (== detection identity).
- `u,v` ∈ [0,1] in canvas space (post-rectify if AprilTags used, else raw image).
- `universe`/`channel` = patch address for output packing.
- `order` = color byte order (ws281x usually GRB).
- `conf` = detection quality; lets a consumer or a manual-fixup pass flag misses.
- 3D later: `space:"world-3d"`, add `w` (z) per LED, multi-view triangulation —
  purely additive, no v1 break.

## rustjay-engine integration (the one real consumer change)

Today `rustjay-lighting/scan.rs` assumes a **grid atlas tile + ScanOrder**
(serpentine/corner). It has no per-LED arbitrary position. Add a sibling sample
mode, don't touch the grid path:

- New `PointMap` sampler: load `ledmap.json`; for each LED sample the render
  canvas at `(u,v)` → RGB; apply `order`; emit fixture-major bytes.
- Reuse `color::color_pipeline` (BGRA→channels) and `patch::pack_fixtures`
  (universe/channel already in the map) unchanged.
- Net new engine code: one sampler that reads (u,v) instead of demuxing a tile.

## Milestones

1. **`rustjay-ledmap` crate** (DONE): format + detect + sequential calibrator
   driving `DmxFrame` via `pack_fixtures`; tested. Remaining: egui tab wiring the
   loop (drive → settle → capture luma → record → export).
2. **`PointMap` sampler + live output** (DONE): sampler maps a BGRA frame at each
   LED's `(u,v)` → `DmxFrame` (per-LED address + color order); tested.
   `examples/playback.rs` drives a strip over sACN from an animated frame.
   `rustjay-io` `OutputManager` hosts a `LedOutput` (behind the `led` feature)
   that harvests the same readback frame as NDI/V4L2 and submits via a
   `DmxSender`. Full trigger chain wired: `OutputCommand::StartLed {path,priority}`
   / `StopLed` → `commands.rs` → `WgpuEngine::start_led_output` →
   `OutputManager::start_led`; toggled from the **Playback (sACN)** section of the
   vjarda LED Map tab. End-to-end: calibrate → export → Start LED output → the
   live mix drives the mapped strip.
3. Gray-code pattern + uploaded-video ingestion (file input → same decode).
4. Projection/homography rectification toggle (image → content space) for clean (u,v).
5. Manual fixup UI: drag misdetected LEDs, re-order, mark dropouts (`conf` low).
6. (later) 3D: second camera angle, triangulate, `world-3d` export → stageLX.

## Risks / mitigations

- **Merged blobs** (distant/adjacent LEDs) → Gray code resolves identity even when
  spatially touching; zoom/resolution for spatial accuracy.
- **Ambient light / bloom** → background subtraction + threshold; drive at modest
  brightness; centroid tolerant of bloom.
- **Lens distortion** → optional `rustjay-projection` homography rectify.
- **Output protocol breadth** → reuse `rustjay-lighting`; sACN now, Art-Net is
  one `ArtNetTransport` swap, serial / WLED only when a target needs it.
- **Uploaded-video timing** → all-on sync frame + Gray code = no external genlock.

## Open / deferred

- Art-Net, serial, WLED, Pixelblaze output drivers — add per demand.
- stageLX export (3D fixtures + GDTF/MVR patch) — Milestone 6, second target.
- Multi-strip / multi-universe runs in one capture — format already supports;
  UI for assigning patch addresses per detected run is Phase 5 work.
