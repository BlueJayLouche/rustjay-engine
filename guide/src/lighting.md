# Lighting & LED Output

The engine can drive physical fixtures and addressable LED strips from rendered
pixels — sending DMX over **sACN (E1.31)** or **Art-Net**, and mapping each LED
to a screen position with a camera. Three crates cover this:

| Crate | Role |
|---|---|
| `rustjay-lighting` | Network spine — DMX framing + sACN / Art-Net transport |
| `rustjay-ledmap` | CV LED mapping — flash a pattern, recover per-LED `(u,v)` |
| `ledmap-studio` | Standalone tool to calibrate strips and export `ledmap.json` |

## DMX output — `rustjay-lighting`

Pure CPU + networking: it knows nothing about wgpu or sampling. You build a
`DmxFrame` (universe → 512 channel bytes) and hand it to a `DmxSender`, which
paces transmission on a background thread.

```rust
use rustjay_lighting::{DmxFrame, DmxSender, SacnTransport, Dest};

let transport = SacnTransport::new(Dest::Multicast, 100, "vjarda").unwrap();
let sender = DmxSender::spawn(Box::new(transport), 44.0); // 44 Hz refresh

let mut frame = DmxFrame::new();
let u = frame.universe_mut(1);
u[0] = 255;            // fixture 1, channel 1 (e.g. red)
sender.submit(frame);
```

Fixture profiles (`FixtureProfile`, `ChannelRole`, `WhiteMode`) and
`pack_fixtures` turn per-fixture colours into channel bytes; `find_overlaps`
catches patch collisions across universes.

## LED mapping — `rustjay-ledmap`

Addressable strips (ws281x et al.) have no inherent screen position. To map
them, flash a calibration pattern — one LED at a time — and recover each LED's
location from a camera frame:

1. **`calibrate`** — `SequentialCalibrator` drives the flash pattern out through
   `rustjay-lighting` and ingests captured frames.
2. **`detect`** — dependency-free blob detection (threshold → connected
   components → subpixel centroid) finds the lit LED in each frame.
3. **`format`** — the result is a `LedMap` (`ledmap.json`), an interchange of
   per-LED `(u,v)` positions.
4. **`sampler`** — `PointMap` plays a recovered map back: sample the rendered
   frame at each LED's `(u,v)` and emit a `DmxFrame`.

Wire packing and transport are reused from `rustjay-lighting` — this crate owns
the CV and the file format, not the protocol.

## `ledmap-studio`

A standalone GUI for the calibrate-and-export loop, when you don't want to wire
the API by hand:

```sh
cargo run -p ledmap-studio
```

Calibrate your strips against a camera, then export `ledmap.json` for any
engine app to sample.

## In vjarda

vjarda's **LED Map tab** wraps the whole flow — calibration with background
subtraction (capture an unlit reference, subtract it so only the flashed LED
registers) plus live sACN playback of the rendered master output. See the
[vjarda chapter](examples/vjarda.md).

## Design notes

The full architecture spec lives in `LIGHTING_SACN.md` (DMX output: per-fixture
profiles, per-segment patch, atlas downsample) and `crates/rustjay-ledmap/DESIGN.md`.
