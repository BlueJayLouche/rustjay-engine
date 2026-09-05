# MadMapper Laser Materials

Built, except for hardware verification and one wiring line. This replaces the
earlier planning version of this file, which got several things wrong — see
[What I had wrong](#what-i-had-wrong).

Spec: [madmappersoftware/MadMapper-Materials][repo], Apache-2.0.
`LaserMaterialsDoc.md` there is unusually complete and answers most questions
this file does not.

[repo]: https://github.com/madmappersoftware/MadMapper-Materials

## What a laser material is

Not a pixel shader — a path generator.

```glsl
void laserMaterialFunc(int pointNumber, int pointCount,
                       out vec2 pos, out vec4 color,
                       out int shapeNumber, out vec4 userData)
```

Called once per *sample point*, emitting one point of a 2D path: position in
−1..1, a colour, and a `shapeNumber` that starts a new stroke whenever it
changes. Alpha is ignored — you cannot composite a beam. 12 of MadMapper's 109
use `vectorMaterialFunc`, the same minus `userData`.

MadMapper documents the texture it writes into, and it falls out of an ordinary
fragment pass:

```
target: POINT_COUNT wide × 3 tall, Rgba32Float
  row 0:  rg = pos (−1..1),  b = shapeNumber
  row 1:  rgba = colour
  row 2:  userData → next frame's mm_LastFrameData
```

`gl_FragCoord.x` is the point number, `.y` picks the row. `RENDERSIZE.x` is the
point count, which is how the host tells the shader its budget.

## Status

| | where | state |
|---|---|---|
| Dialect (entry point, bounds, `RENDER_SETTINGS`, shape library) | `rustjay-isf` | done — **98/109 compile**, was 1 |
| Render, ping-pong, readback, budget | `rustjay-laser` | done |
| Optimiser (blanking, corner dwell, stroke repeats) | `rustjay-laser::optimise` | done, **values unverified** |
| Safety (arm, blackout, scan-fail) | `rustjay-laser::safety` | done, **thresholds unverified** |
| DAC transport | `rustjay-laser::dac` | done, **never run against hardware** |
| Preview panel | `kovvboj::ui::laser_tab` | done, **not wired into the shell** |

Commits: `fc9b25c` (dialect), `0b9000a` (crate), `f5ab27d` (transport + panel).

## The one thing left to wire

`LaserTab` is written and compiles but nothing constructs it, because
`shell.rs` was mid-edit. Three lines when that lands:

1. a field on `KovvbojShell` — `laser: LaserTab`, built with `LaserTab::new()`;
2. a `VIEW_TABS` entry drawing it through `tab(&mut self.laser, …)`;
3. `self.laser.pump(device, queue, encoder, &engine, quad, sampler)` once per
   frame from the render hook in `lib.rs`.

Build with `--features laser` (pipeline + preview) or `--features laser-dac`
(adds hardware). Neither is on by default: `laser-dac` pulls libusb and CMake.

## Decisions, and why

| Decision | Why |
|---|---|
| `laser-dac` over `nannou_laser` | Helios, Ether Dream, IDN, LaserCube vs. Ether Dream only. Costs us the within-frame optimiser, which nannou has and laser-dac does not. |
| Default features off | Its `default` adds an audio backend and ASIO, which wants the Steinberg SDK on Windows. Narrowed to the four DACs. |
| Minimal optimiser, ours | The shader already controls point density along the path and `shapeNumber` says where the jumps are, so most of MadMapper's nine knobs have nothing to compute. Three are implemented. |
| Parallel pipeline, not a mixer source | Laser materials generate geometry; they never enter a channel or FX chain. |
| Dedicated preview panel | The only way to develop without hardware, and you cannot aim a laser to find out what a shader does. |
| Budget = pps ÷ refresh, capped by `POINT_COUNT` | At 30 kpps, MadMapper's default 8192 points redraws at 3.7 Hz. All 109 materials divide by the count they're handed, so a smaller budget draws the same shape at lower density. |
| Budget fixed at load | It is the target's width, and feedback materials index history by it. Retune reallocates and clears history — deliberate, never mid-set. |
| Generators as accumulators, not `TIME × speed` | `effect.rs` already documents why: the multiply jumps when the speed changes, which is the discontinuity generators exist to prevent. |
| `Vec<LaserDeck>`, one shown | The save format is the expensive thing to change later; the UI is not. |
| Reuse `IsfEffect` | Generators, parameters, uniform packing and hot reload come free. It needed three hooks: offscreen size, offscreen format, and `mm_LastFrameData` as the primary texture. |

**IDN is the interop hedge.** Helios and LaserCube need per-device USB drivers
and Ether Dream is its own protocol, but IDN is the open standard — so
[ILDAWaveX16](https://github.com/stanleyondrus/ILDAWaveX16) and the OpenIDN
world work with no driver of their own. If only one backend ever gets verified
by hand, make it IDN.

## What is not done

**Hardware.** Nothing here has met a projector. Specifically unverified: whether
the settling values in `Optimiser` look right on real galvos, whether
`MIN_LIT_EXTENT` blanks the right frames, and whether ILDAWaveX16's IDN dialect
matches `laser-dac`'s. The code says so where it is true, rather than implying
otherwise.

What *was* verified without hardware: the frame decode, the budget arithmetic,
the optimiser's insertions, the safety gate, and the unit conversion — 33 tests
in `rustjay-laser`, plus 98/109 of the corpus compiling.

**`auto_all.glsl`** — 9 of the 109. Not a library: MadMapper generates a copy
per material, ships it inside the material's folder, and it carries **its own
ISF header declaring inputs**. Supporting it needs includes resolved relative to
the shader's directory (`compile()` currently takes `&str`, never a path) and a
header-merge pass, and it reopens the property in `resolve_include` that a
shader cannot reach the filesystem. Deferred deliberately; the 9 fail with a
message naming what is missing.

**2 shaders** are documentation templates with prose where the header goes.
Correctly unsupported.

**The other six `RENDER_SETTINGS`** — `MAX_SPEED`, `PRESERVE_ORDER`, the dwell
thresholds, the repeats and fades. Parsed where the corpus sets them, honoured
only for `ANGLE_OPTIMIZATION` and `SKIP_BLACK`. Add them when a real beam looks
wrong, and `nannou_laser`'s Abderyim implementation is the reference if the
minimal optimiser stops being enough.

## What I had wrong

The planning version of this file, written before any of it was built:

- **"The output half is a project — ILDA, path optimisation, safety."** Most of
  it exists: `laser-dac` handles five DAC families and the between-frame work.
  What was left was unit conversion, a minimal optimiser and the gate.
- **"`MadLaserMaterialShapeLibrary.glsl` is one line away."** True, and nearly
  pointless — 2 of 109 include it. `MadNoise` (75) and `MadCommon` (66) were
  the ones that mattered, and both were already vendored.
- **"`auto_all.glsl` is not bundled."** It is not a library at all. It ships per
  material and carries its own header.
- **The scan-fail measure.** The plan said path length; a beam scribbling
  tightly in one place has plenty of path length and concentrates just as much
  energy on one spot. It measures the extent of the lit points instead.

## Measuring

```sh
# Corpus pass rate and what the failures are
MM_CORPUS_DIR=<checkout>/LaserMaterials \
  cargo test -p rustjay-isf --test zz_mm_corpus -- --nocapture

# The merged GLSL, when shaderc cites a line that exists nowhere on disk
ISF_DUMP_GLSL=1 cargo test -p rustjay-isf --test laser
```
