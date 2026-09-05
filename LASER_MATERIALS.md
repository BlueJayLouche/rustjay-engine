# MadMapper Laser Materials

Notes for picking this up later. Nothing here is implemented — the MadMapper
*Materials* dialect landed in `rustjay-isf` (commits `d98285c`, `5546b9f`,
`6246c6e`, 599/638 of that corpus compiling); laser materials are a separate
thing that happens to share the header format.

Source of truth: [madmappersoftware/MadMapper-Materials][repo], Apache-2.0.
`LaserMaterialsDoc.md` in that repo is the spec and is unusually complete —
read it first, it answers most of what follows.

[repo]: https://github.com/madmappersoftware/MadMapper-Materials

## The thing to understand first

A laser material is not a pixel shader. It is a **path generator**.

```glsl
void laserMaterialFunc(int pointNumber, int pointCount,
                       out vec2 pos, out vec4 color,
                       out int shapeNumber, out vec4 userData)
```

It is invoked once per *sample point* — `POINT_COUNT` of them, 8192 by default —
and each invocation emits one point on a 2D path: position in −1..1, a colour,
and a `shapeNumber` that starts a new path whenever it changes from the previous
sample. Alpha is ignored; you cannot composite a laser beam.

12 of the 109 shaders use `vectorMaterialFunc` instead, identical minus the
`userData` out-parameter.

## Where it stands today

Running MadMapper's own `LaserMaterials/` through the current pipeline:

```
MM_CORPUS_DIR=<checkout>/LaserMaterials \
  cargo test -p rustjay-isf --test zz_mm_corpus -- --nocapture
```

**1 of 109 compiles.** What the other 108 trip over:

| n | failure | what it is |
|---|---|---|
| 46 | missing entry point | no `main`; needs the `laserMaterialFunc` bridge |
| 42 | `expected an array of length 2` | a `point2D` input with scalar `MIN`/`MAX` |
| 8 | `auto_all.glsl` not bundled | MadMapper live-coding aggregate, not in the published `Libraries/` |
| 7 | undeclared identifier | unexamined |
| 2 | `MadLaserMaterialShapeLibrary.glsl` | published, just not vendored yet |
| 2 | header JSON | unexamined |
| 1 | overload | unexamined |

Note the 42: that is the same shape as the `floatRange` broadcast already in
`header::repair` — a scalar written where a 2-vector is wanted. Widening
`widen_float_range` into a general "broadcast a scalar bound to the arity the
type wants" is a handful of lines and would be the cheapest win on the board.

I said earlier that `MadLaserMaterialShapeLibrary.glsl` was "one line away".
True, and nearly pointless: 2 of 109 shaders include it. The includes that
actually matter are `MadNoise.glsl` (75) and `MadCommon.glsl` (66), both
already vendored.

## How it maps onto this pipeline

Better than expected. `LaserMaterialsDoc.md` documents the output texture
layout exactly, and it falls out of a normal fragment pass:

```
target: POINT_COUNT wide × 3 tall, float
  row 0:  rg = pos (−1..1),  b = shapeNumber,  a = 0
  row 1:  rgba = colour
  row 2:  rgba = userData
```

So `gl_FragCoord.x` is `pointNumber` and `gl_FragCoord.y` selects which of the
three rows this fragment is writing. One shader, one draw, no MRT — the bridge
calls `laserMaterialFunc` once and then selects the output by row:

```glsl
void main() {
    int pointNumber = int(gl_FragCoord.x);
    vec2 pos; vec4 color; int shapeNumber; vec4 userData;
    laserMaterialFunc(pointNumber, POINT_COUNT, pos, color, shapeNumber, userData);
    int row = int(gl_FragCoord.y);
    FragColor = row == 0 ? vec4(pos, float(shapeNumber), 0.0)
              : row == 1 ? color
              : userData;
}
```

`mm_LastFrameData` — used by 97 of the 109 — is simply the previous frame's
version of that texture, read with `texelFetch(mm_LastFrameData, ivec2(pointNumber, row), 0)`.
That is a ping-pong of the render target, which this codebase already does
elsewhere; mind the generation-change invalidation gotcha that bit the channel
output path.

## Work items, in order

1. **Scalar-bound broadcast in `header::repair`.** Unblocks 42. Cheapest thing here.
2. **`laserMaterialFunc` / `vectorMaterialFunc` bridge** in `compile.rs`, alongside
   `has_material_fn`. Needs `POINT_COUNT` from `RENDER_SETTINGS` (read it the way
   `header::generators` reads `GENERATORS` — `isf::Isf` has nowhere to put it either).
   Unblocks 46.
3. **Vendor `MadLaserMaterialShapeLibrary.glsl`** — one line in `LIBRARIES`. Unblocks 2.
4. **Render target + ping-pong** for the 3-row float texture and `mm_LastFrameData`.
   This is where it stops being a shader change: the ISF effect path assumes an
   image-shaped target sized to the output resolution.
5. **Readback and the actual output.** See below.

Generators already work and 95 of the 109 use them, so that carries over free.

## The part that is not shader work

Steps 1–4 get you a texture full of path data. Nothing in this repo can do
anything with it yet.

- **Getting it off the GPU.** A per-frame readback of POINT_COUNT×3 floats at
  laser frame rate. `rustjay-lighting` has no equivalent; the DMX path builds
  its frames on the CPU.
- **Protocol.** ILDA is the wire format; the usual consumer DACs are Ether Dream
  (Ethernet) and Helios (USB). None of them are sACN/Art-Net, so `rustjay-lighting`
  is a neighbour, not a home — likely a `rustjay-laser` crate.
- **Path optimisation.** `RENDER_SETTINGS` carries `MAX_SPEED`, `ANGLE_OPTIMIZATION`,
  `ANGLE_THRESHOLD`, `FIRST_POINT_REPEAT`, `LAST_POINT_REPEAT`, `POLY_FADE_IN`,
  `SKIP_BLACK`, `PRESERVE_ORDER`, `MIN_ILDA_POINTS_PER_POLYLINE`. These are not
  shader concerns at all — they describe how the host reorders and paces the
  path so real galvanometers can track it without overshooting. A laser output
  that ignores them will look wrong and can be hard on the hardware.
- **Safety.** Worth saying out loud before any of this drives a real projector.

**So the honest sizing:** the dialect work is a day and mostly mirrors what is
already in `compile.rs`. The output path is a project, and it only pays off if
there is a laser on the other end of it. If the goal is just to *see* these
shaders, step 4 can render the paths as lines into an ordinary texture and skip
the entire second half.

## Preview without a laser

A cheap intermediate worth considering: after step 2, draw the point list as a
line strip into a normal texture — break the strip wherever `shapeNumber`
changes — and it becomes an ordinary visual source in kovvboj. That turns 100+
MadMapper laser materials into vector-looking generators with none of the DAC,
protocol, or safety work, and it is a reasonable place to stop unless real laser
output is the point.
