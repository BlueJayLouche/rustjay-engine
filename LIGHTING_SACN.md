# Lighting Control — Pixel-Mapped DMX Output (sACN / Art-Net)

Status: **design / not implemented** · Owner: vjarda example · Date: 2026-06-12

## 1. Summary

Add a lighting-control output to the **vjarda** example that samples the colour of
rendered surfaces and transmits it as DMX over the network — **sACN (E1.31)** and
**Art-Net**. The canonical use-case: author a surface (e.g. an `18 × 1` strip),
the system samples it into 18 fixture colours and streams them to LED fixtures.

The four design decisions taken up front push this well beyond "read 18 pixels":

| Axis | Decision | Consequence |
|------|----------|-------------|
| Channel format | **Full per-fixture profile** | Arbitrary channel layouts (RGB, RGBW, +dimmer/strobe/static), not just raw colour. |
| Universe patching | **Per-segment explicit patch table** | Each segment carries its own `universe + start channel`, auto-wrapping across universes. |
| Code home | **New `rustjay-lighting` crate** | Reusable, dependency-light, unit-testable protocol + patch + colour layer. |
| Sampling | **Separate grid + downsample** | Fixture count is decoupled from render resolution; GPU box-downsample into a small atlas. |
| Transport (added) | **sACN + Art-Net** | One DMX-universe buffer, two pluggable transports. |

This is, in effect, a compact **pixel-mapping subsystem** (cf. MadMapper / Resolume
LED mapping, or a lighting-console patch).

## 2. Why this fits the existing architecture

The data path already exists end-to-end; we are adding a leaner tap plus a network sink.

- **GPU→CPU readback** is solved. `HeadlessOutput` (`crates/rustjay-projection/src/headless.rs`)
  performs async `copy_texture_to_buffer` + `map_async` + non-blocking poll and exposes
  `latest_frame()` as tightly-packed **BGRA8**, row-major, at exact `width × height`.
  The in-flight/poll/resize-safety logic is battle-tested and is the skeleton for the new
  sampler.
- **Subsystem surface**: `RenderSubsystem::headless_frame(index)` (`projection.rs:758`)
  already hands BGRA pixels to the example layer.
- **Lifecycle pattern**: `start_headless_ndi` / `stop_headless_ndi` / `is_headless_ndi`
  (`projection.rs:977+`) and the level-triggered reconcile loop in
  `examples/vjarda/src/lib.rs:1654` are the exact template for starting/stopping a DMX sink
  per `OutputType`.
- **`OutputType` enum** (`examples/vjarda/src/stage/mod.rs:486`) is where cross-platform
  `Sacn` / `ArtNet` variants are added (no OS gate, unlike Syphon/Spout/V4L2).
- **Protocol code is portable**: stageLX's `build_sacn` (`sacn.rs:49`) and `build_artdmx`
  (`artnet.rs:26`) are pure `(universe, &[u8;512]) -> Vec<u8>` functions; their bevy/crossbeam
  coupling lives only in the system layer, not the builders. We port the builders, not the glue.
- **Deps**: `crossbeam` and `tokio` are already workspace deps; only `socket2` (socket tuning,
  as stageLX uses) needs adding.

### Reuse vs build (MECE)

- **Reuse verbatim (port):** E1.31 + Art-Net packet builders, CID/ACN constants,
  `multicast_addr`, UDP TX background-thread sink shape.
- **Generalise from existing:** `HeadlessOutput` readback skeleton → `PixelSampler`
  (atlas target + tiny readback).
- **Build new:** fixture-profile model, per-segment patch + universe packing, colour
  pipeline (gamma/dimmer/white extraction), GPU downsample-to-atlas stage, transport trait,
  UI patch editor.

## 3. Component architecture

```
                         vjarda (example)
  ┌───────────────────────────────────────────────────────────────────┐
  │ VardaStage.lighting_outputs: Vec<LightingOutput>                   │
  │   each = transport cfg + Vec<Segment> + fixture-profile refs       │
  │                                                                     │
  │  reconcile loop (lib.rs ~1654, mirrors headless NDI/Syphon)        │
  │     start/stop sinks by OutputType::{Sacn,ArtNet}                  │
  └─────────────┬──────────────────────────────────┬──────────────────┘
                │ per frame                          │ start/stop
                ▼                                    ▼
   RenderSubsystem (engine)                rustjay-lighting (new crate)
  ┌──────────────────────────┐            ┌────────────────────────────────┐
  │ PixelSampler             │            │ DmxFrame  (universe→[u8;512])   │
  │  - SampleStage (GPU)     │  pixels    │ FixtureProfile / ChannelRole    │
  │  - atlas tex + readback  │ ─────────► │ Segment patch + universe pack   │
  │  - latest_atlas() BGRA8  │            │ ColorPipeline (gamma/dim/white) │
  └──────────────────────────┘            │ DmxTransport trait              │
                                          │   ├─ SacnTransport  (E1.31)     │
                                          │   └─ ArtNetTransport (ArtDMX)   │
                                          │ Tx background thread (UDP)      │
                                          └────────────────────────────────┘
```

The crate `rustjay-lighting` is **pure CPU + networking** (no `wgpu` dependency). All GPU
work (sampling/downsample) stays in `rustjay-projection`; the crate only consumes packed
pixel bytes. This keeps it trivially unit-testable and reusable by any engine consumer.

## 4. `rustjay-lighting` crate

### 4.1 Protocol layer (`e131`, `artnet`)

Ported, pure, no async:

```rust
// e131.rs
pub fn build_sacn(universe: u16, priority: u8, sequence: u8, data: &[u8; 512]) -> Vec<u8>;
pub fn multicast_addr(universe: u16) -> Ipv4Addr;   // 239.255.hi.lo
pub const SACN_PORT: u16 = 5568;

// artnet.rs
pub fn build_artdmx(net: u8, subnet: u8, universe: u8, seq: u8, data: &[u8; 512]) -> Vec<u8>;
pub const ARTNET_PORT: u16 = 6454;
```

CID is a fixed UUID4 for rustjay (regenerate, do **not** reuse stageLX's). Source name
`"vjarda"`. Unit tests assert exact byte layout against the known-good 638-byte E1.31 frame
and Art-Net `ArtDmx` header (mirror stageLX's constants as golden vectors).

### 4.2 DMX frame + transport

```rust
/// Sparse set of universes for one network tick.
pub struct DmxFrame { universes: BTreeMap<u16, [u8; 512]> }

pub trait DmxTransport: Send {
    /// Push the current frame; transport handles seq numbers + packetisation.
    fn send(&mut self, frame: &DmxFrame);
}

pub struct SacnTransport   { socket, dest: Dest, priority: u8, seq: HashMap<u16,u8>, source_name }
pub struct ArtNetTransport { socket, dest: Dest, seq: HashMap<u16,u8> }
pub enum Dest { Multicast, Broadcast, Unicast(Ipv4Addr) }  // sACN default Multicast, Art-Net default Broadcast
```

- One **background TX thread per LightingOutput**, owning the transport and a
  `crossbeam::channel` receiver of `DmxFrame` snapshots (bounded, capacity 1, latest-wins —
  same back-pressure shape as stageLX's `bounded(1)` TX sink).
- Thread paces output at the output's **target FPS** (default **44 Hz**; sACN/DMX practical
  ceiling) independent of the render loop. Sends keep-alive frames (re-send last frame) so
  fixtures don't time out when content is static, throttled to e.g. 1 Hz idle.
- Per-universe sequence counter (E1.31 §6.7).

### 4.3 Fixture profile (full per-fixture profile)

```rust
pub enum ChannelRole {
    Red, Green, Blue, White, Amber, Uv,   // sampled from the fixture's pixel
    Dimmer,            // driven by the segment master dimmer (0..=255)
    Static(u8),        // constant byte (e.g. shutter open, mode select)
    // future: Strobe, ColorMacro, Fine(<role>) for 16-bit
}

pub struct FixtureProfile {
    pub id: ProfileId,
    pub name: String,            // "RGB", "RGBW", "RGB+Dimmer", "Pixel tape GRB", …
    pub channels: Vec<ChannelRole>,   // len = footprint (channels per fixture)
}
```

- Colour order is simply the ordering of `Red/Green/Blue` in `channels` → the "selectable
  colour order" requirement is subsumed (`GRB`, `BGR`, `RGBW`, `GRBW`, …).
- A small built-in profile library ships (RGB, GRB, BGR, RGBW, RGB+Dimmer); users can add
  custom profiles, persisted with the scene.

### 4.4 Segment (per-segment explicit patch)

```rust
pub struct Segment {
    pub name: String,
    pub enabled: bool,

    // ── sampling source ───────────────────────────────
    pub source: SegmentSource,        // Master | Surface(index)  (MVP: Master)
    pub region: [f32; 4],             // u0,v0,u1,v1 normalised sub-rect of the source
    pub grid: [u16; 2],               // cols × rows  = fixture count
    pub sample_mode: SampleMode,      // Box (area average) | Point (nearest)
    pub scan: ScanOrder,              // start corner + serpentine + primary axis

    // ── patch ─────────────────────────────────────────
    pub profile: ProfileId,
    pub start: PatchAddr,             // { universe: u16, channel: u16 (1-based) }
    // fixtures laid out sequentially; auto-wrap into universe+1, +2 … on overflow

    // ── colour ────────────────────────────────────────
    pub color: SegmentColor,
}

pub struct ScanOrder { pub start_corner: Corner, pub serpentine: bool, pub primary: Axis }

pub struct SegmentColor {
    pub brightness: f32,      // 0..=1 global scale
    pub gain: [f32; 3],       // per-channel R,G,B white-balance trim
    pub master_dimmer: f32,   // 0..=1, drives ChannelRole::Dimmer
    /// RGBW white extraction. Default: MinSubtract (colour-accurate, matches WLED "Accurate").
    pub white: WhiteMode,     // for RGBW: Off | Min { amount } | MinSubtract { amount }
}
```

**Universe packing** (pure fn, the heart of the patch): given a segment's ordered fixture
colours + profile footprint + `start`, write bytes sequentially; when `channel + footprint`
exceeds 512 within a universe, advance to the next universe (channel resets to 1). The
"per-segment explicit patch" means each segment owns its `start` independently; multiple
segments may share a universe (later segment's `start` deliberately offset) — overlaps are a
user concern, surfaced as a UI warning.

### 4.5 Colour pipeline (per fixture cell)

1. Atlas readback gives **BGRA8** in display/sRGB space → reorder to RGB.
2. Apply the output's `gamma` (encode for LED perceptual response), then segment `gain` (white balance) and `brightness`.
3. `WhiteMode` for the `White` channel: `w = min(r,g,b) * amount`; `MinSubtract` additionally
   removes that white from R/G/B (colour-accurate RGBW) vs `Min` (additive, brighter).
4. `Dimmer` role ← `master_dimmer`; `Static(v)` ← `v`.
5. Emit bytes in `ChannelRole` order into the universe buffer.

All steps are pure and unit-tested on synthetic pixel inputs (no GPU).

## 5. GPU sampling — `PixelSampler` (rustjay-projection)

Sized for **minimal readback bandwidth** (relevant to the known perf profile: readback /
StagingBelt is a hot path — see `project_perf_analysis_2026_05_23`). We never read back a
full 1080p frame for lighting.

- A new `SampleStage` (a `ProjectionStage`) renders **each segment's `region` into a
  `grid.cols × grid.rows` tile** of a single packed **sample atlas** texture (BGRA8). One
  output owns one atlas = `sum(cols*rows over segments)` texels — typically a few hundred,
  worst case a few thousand.
- `SampleMode::Box`: proper area-average downsample. MVP uses source mip generation +
  trilinear sample (≈ box); a later multi-tap box-filter shader is the quality upgrade.
  `Point`: nearest.
- `PixelSampler` reuses `HeadlessOutput`'s readback skeleton (in-flight flag, fresh
  `AtomicU8` map-state per submit, resize-safety) — **factor that logic into a shared
  `ReadbackTarget` helper** rather than copy-paste.
- Subsystem API (parallel to headless):
  ```rust
  fn add_pixel_sampler(&mut self, atlas_layout: AtlasLayout) -> SamplerId;
  fn update_sampler_layout(&mut self, id, AtlasLayout);   // segments edited
  fn sampler_atlas(&self, id) -> Option<(&[u8], &AtlasLayout)>;  // BGRA8 + tile offsets
  fn remove_pixel_sampler(&mut self, id);
  ```
- Source texture: MVP samples the **master composite** (always available). M5 extends
  `SegmentSource::Surface(i)` by feeding per-surface output textures (reuse
  `cached_source_options` / `SurfaceSource` routing already in `stage/mod.rs`).

Per-frame flow (render/update thread): `sampler_atlas()` → for each segment, demux its tile →
colour pipeline → universe packing into a `DmxFrame` → push snapshot to that output's TX
thread (latest-wins). The TX thread paces to the wire.

## 6. Data model & persistence

`VardaStage` gains a dedicated collection (not overloaded onto `headless_outputs`, whose
full-frame readback is wasteful for lighting):

```rust
pub struct VardaStage {
    // …
    pub lighting_outputs: Vec<LightingOutput>,
    pub fixture_profiles: Vec<FixtureProfile>,   // scene-level library
}

pub struct LightingOutput {
    pub name: String,
    pub enabled: bool,
    pub output_type: OutputType,     // Sacn | ArtNet  (drives transport)
    pub transport: TransportCfg,     // dest, priority, source_name, fps
    /// Output-level gamma encode for the LED perceptual response (sRGB → LED).
    /// Applied to every sampled fixture cell before the fixture profile mapping.
    #[serde(default = "default_gamma")]
    pub gamma: f32,
    pub segments: Vec<Segment>,
    #[serde(skip)] pub sampler_id: Option<SamplerId>,
    #[serde(skip)] pub tx: Option<TxHandle>,
}

fn default_gamma() -> f32 { 2.2 }
```

- `OutputType` (`stage/mod.rs:486`) gains cross-platform `Sacn` and `ArtNet` variants +
  `label()` arms. They appear in the top-bar output pills (see
  `project_projector_output_senders`).
- All of `LightingOutput`/`Segment`/`FixtureProfile`/`SegmentColor` are plain serde data →
  persisted in workspace + presets exactly like the rest of the stage (cf.
  `project_vjarda_topology_persistence`). Runtime handles (`sampler_id`, `tx`) are
  `#[serde(skip)]` and rebuilt by the reconcile loop on load.
- `#[serde(default)]` on new fields for backward-compatible scene loading.

## 7. Lifecycle integration

Mirror the headless reconcile loop (`lib.rs:1654`) — level-triggered, idempotent:

```
for each LightingOutput:
    want = enabled && output_type ∈ {Sacn, ArtNet}
    ensure PixelSampler exists & layout matches segments (add/update/remove)
    if want && tx.is_none():   start TX thread (transport from output_type + cfg)
    if !want && tx.is_some():  stop  TX thread
    if want: build DmxFrame from sampler_atlas() and push to tx
```

Start/stop methods on the subsystem mirror `start_headless_ndi` & friends so the example
code stays symmetric.

## 8. UI (egui)

New **"Lighting"** panel (sub-tab under Stage/Outputs; reference stageLX's
`design_handoff_stagelx_ui` for patch-editor patterns). Sections:

1. **Outputs list** — add/remove; per-output: name, enable, protocol (sACN/Art-Net),
   destination (multicast / broadcast / unicast IP), priority (sACN), FPS, gamma encode
   (default 2.2), live TX rate + universe-count meter.
2. **Segment patch table** — per segment: source + region picker (draw a rect on the Stage
   preview — reuse the live Stage preview from `project_vjarda_stage_preview`), grid `cols×rows`,
   profile dropdown, `start` universe/channel, scan order (start corner, serpentine), colour
   (brightness/dimmer/white). Show computed universe span + overlap warnings.
3. **Fixture profile library** — add/edit profiles (ordered channel-role list).
4. **Activity** — per-universe last-value strip / simple meter for debugging patch.

## 9. Performance considerations

- **Readback is tiny by construction** (atlas = fixture count, not screen res). This is the
  single most important perf decision and directly avoids the StagingBelt/readback hot path
  flagged in `project_perf_analysis_2026_05_23`.
- **One readback per output per frame** (atlas packs all segments), one `map_async` in flight.
- **TX decoupled** from render fps on its own thread; `bounded(1)` latest-wins channel — no
  unbounded growth, render thread never blocks on the socket.
- Tuned UDP socket (port `socket2` setup from stageLX: send-buffer size).

## 9a. Implementation status (living section)

**M0 — Network spine: DONE (2026-06-12).** `crates/rustjay-lighting` exists and is
green. Modules: `e131` (build/parse_sacn, rustjay CID, `source_name` param),
`artnet` (build/parse_artdmx, `seq` param), `dmx` (`DmxFrame` = `BTreeMap<u16,[u8;512]>`),
`socket` (`tx_socket`/`rx_socket`), `transport` (`Dest`, `DmxTransport`, `SacnTransport`
multicast-default, `ArtNetTransport` broadcast-default, per-universe seq, `.with_dest_port`
test hook), `tx` (`DmxSender` latest-wins cell + paced 44 Hz keep-alive thread),
`patch` (`pack_fixtures` universe-packing). 21 unit tests + sACN unicast loopback + doctest
green; clippy clean. Example `examples/sacn_smoke.rs` (`sacn`|`artnet` arg) verified
transmitting.

**M1 — Single-segment RGB: DONE (2026-06-12).**
Decision: reuse the existing `HeadlessOutput` as the GPU downsampler (render master
into an `N×1` offscreen → tiny readback) instead of building the atlas `PixelSampler`
now; the dedicated atlas sampler is deferred to **M3** when multi-segment packing
needs it.
- **Engine** (`crates/rustjay-engine/src/app/projection.rs`): added a dedicated
  `sampler_outputs: Vec<HeadlessOutput>` (separate from user `headless_outputs`) +
  `add_sampler`/`remove_sampler`/`resize_sampler`/`sampler_count`/`sampler_size`/
  `sampler_frame`; samplers are rendered from the master `source` in `render()`.
- **vjarda stage** (`examples/vjarda/src/stage/mod.rs`): `OutputType` gained
  cross-platform `Sacn` + `ArtNet` variants (+ `label()` arms); new `LightingOutput`,
  `LightingTransport`, `LightingSegment` structs; `VardaStage.lighting_outputs:
  Vec<LightingOutput>` (`#[serde(default)]`).
- **vjarda reconcile** (`examples/vjarda/src/lib.rs`): `VardaAppState.lighting_senders:
  Vec<Option<DmxSender>>` (`#[serde(skip)]`, projection-gated); a lighting reconcile
  block after the headless loop (1:1 sampler per output, start/stop sender by
  `enabled && type∈{Sacn,ArtNet}`, submit each frame, push a top-bar sink label);
  module-level helpers `build_dmx_sender` and `build_dmx_frame` (BGRA→RGB→`pack_fixtures`).
- **vjarda UI** (`examples/vjarda/src/ui/mod.rs`): added a Lighting Outputs panel in
  the Outputs tab to add/remove/edit outputs and their segment/transport fields.
  `Sacn`/`ArtNet` are only offered in the Lighting Outputs protocol dropdown, not in
  the projector/headless dropdowns (those outputs have no reconcile path for lighting
  protocols).
- **Runtime verify**: GUI ran with `--features projection`; an 18×1 sACN output on
  universe 1, channel 1 streamed moving colour to a multicast receiver.
- **Dependency**: `rustjay-lighting` added to workspace deps + as an optional vjarda dep
  enabled by the `projection` feature.
- Builds clean: `cargo check -p vjarda --features projection` → EXIT 0. M0 crate tests
  all pass.

## 10. Phasing / milestones

- **M0 — Network spine.** `rustjay-lighting` crate; port E1.31 + Art-Net builders; `DmxFrame`,
  `DmxTransport`, both transports; TX thread. Send a hard-coded test pattern to one universe.
  Golden-vector byte tests. *(No GPU, no vjarda yet.)*
- **M1 — Single-segment RGB end-to-end.** `PixelSampler` + `SampleStage`; one segment samples
  the master into `cols×1`; RGB profile; one universe; `OutputType::Sacn` wired through the
  reconcile loop. **The `18×1` demo lights up.**
- **M2 — Fixture profiles + colour pipeline.** Full profile model, RGBW + white extraction,
  gamma/brightness/dimmer/gain. Profile library + persistence. Keep reusing `HeadlessOutput`
  for sampling; the dedicated atlas `PixelSampler` is still deferred to M3.
- **M3 — Multi-segment patch + universe spanning + Art-Net.** Per-segment patch table, atlas
  packs N segments, universe auto-wrap, scan/serpentine order, `ArtNetTransport`.
  Also fix sampler index aliasing: today removing output *i* drops the last sampler and
  resizes samplers to match; with an atlas, sampler identity (tile offsets) matters, so
  remove/update the specific sampler instead.
- **M4 — UI.** Patch editor, profile editor, region-draw on Stage preview, activity meters.
- **M5 — Polish.** `SegmentSource::Surface(i)`; proper box-filter downsample; sACN sync
  packets / per-universe sync; unicast/broadcast options; multi-NIC interface selection;
  source-loss timing.

## 11. Open questions / risks

- **Downsample quality**: bilinear-to-small aliases; box/mip is the correct answer — acceptable
  to defer true box filter to M5?
- **Colour space**: readback is sRGB/display; default LED gamma 2.2 exposed **per-output**, not per-segment. (HDR/linear surfaces are out of scope for M2.)
- **FPS vs render rate**: 44 Hz cap is standard; confirm acceptable (fixtures rarely exceed it).
- **Multi-NIC / interface binding**: stageLX binds `UNSPECIFIED`; venues with multiple NICs may
  need an explicit egress interface for multicast — add to `TransportCfg` if needed.
- **Universe overlap policy**: **warn-only** in the UI; do not hard-prevent overlapping patches.
- **White extraction for RGBW**: default to `MinSubtract` (colour-accurate, equivalent to WLED's "Accurate" RGBW mode). Keep `Min` as an opt-in for additive/brightness-maximising use cases.
- **Art-Net universe addressing**: 15-bit Net/Subnet/Universe vs sACN 16-bit — `PatchAddr`
  stores a flat `u16`; map to Art-Net Net/Subnet/Universe at the transport boundary.
- **CID stability**: generate one fixed rustjay CID constant (not per-run) so consoles see a
  stable source identity.

## 12. Testing

- **Unit (crate, no GPU)**: packet golden vectors (E1.31 638-byte frame, Art-Net header);
  universe-packing across overflow boundary (e.g. 200 RGB fixtures → 2 universes);
  colour pipeline (gamma/white-extraction) on synthetic pixels; scan/serpentine ordering.
- **Integration (GPU, like `headless.rs` tests)**: render a known gradient surface, sample to
  a grid, assert per-fixture bytes after readback (un-padding + BGRA→role mapping).
- **Loopback**: bind a local UDP receiver (reuse stageLX `parse_sacn`/`parse_artnet`) and assert
  the on-wire bytes round-trip the patched values.
- **Manual**: sACNView / Art-Net monitor (e.g. `sACNView`, `Resolume`, `QLC+`) against the live
  output.
```
