# Video & Projection

Video, Image, and Text cues all render onto a shared **canvas** — a virtual
surface (default 1920×1080) that one or more output windows display regions
of. Video is decoded by FFmpeg on a worker thread with vsync-paced
backpressure; YUV→RGB conversion happens on the GPU.

## The Video Output window

*Window → Video Output* opens a single window showing the whole canvas — the
simplest setup: drag it onto the projector and fullscreen it.

## Projection mapping

*Window → Projection Mapping* is the multi-output editor. Set the canvas
size, then add **outputs** — one per projector. Each output has:

| Property | Meaning |
|---|---|
| Source region | The rectangle of the canvas this projector shows (x, y, w, h in canvas pixels). |
| Output size | The projector's native resolution. |
| Fullscreen monitor | Which monitor the output window fullscreens onto. Assignments are remembered by monitor identity (name + geometry), so they survive unplugs and OS re-ordering. |
| Edge blend | Per-edge (left/right/top/bottom) feathering: enable, blend width in pixels, and gamma. |

Overlap the source regions of adjacent projectors by the blend width and
enable edge blending on the touching edges to get a seamless wide image. The
**3×1 edge-blend preset** sets up a 5400×1080 canvas across three 1920×1080
projectors with the overlaps pre-configured — a good starting point to study.

## Content fit

Video/Image/Text cues have a *Fit* mode controlling how their content maps
to the canvas:

- **Fit** (default) — preserve aspect ratio, letterbox/pillarbox.
- **Fill** — preserve aspect ratio, cover the whole canvas, center-crop the
  overflow.
- **Stretch** — fill the canvas exactly, ignoring aspect ratio.

The canvas is also what lighting [pixel-map segments](lighting.md#pixel-map-segments)
sample by default, so LEDs can mirror the projector picture.
