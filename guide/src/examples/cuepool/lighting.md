# Lighting & Pixel Mapping

CuePool sends DMX over **sACN** (E1.31) or **Art-Net**, built on the same
[`rustjay-lighting`](../../lighting.md) crate as the rest of the engine.
Everything lives in *Window → Lighting* and is saved in the project file.

## DMX output

At the top of the Lighting panel: enable output, pick the protocol, and set
an optional unicast destination IP (leave empty for sACN multicast /
Art-Net broadcast). The refresh rate defaults to 44 fps.

## Patching fixtures

*Patch* lists your rig: each fixture is a **profile** at a universe +
1-based DMX address (the channel footprint is shown next to it). Built-in
profiles:

| Profile | Channels |
|---|---|
| RGB / GRB / BGR | 3-channel colour in the named order |
| RGBW | Colour + white |
| RGB + Dimmer / Dimmer + RGB | Colour with a dimmer channel after/before |
| Dimmer | Single channel |
| Moving Head (16-bit) | Pan/tilt (16-bit) + dimmer + colour + beam |

*+ New Profile* builds a custom profile from channel roles (colour, dimmer,
pan/tilt coarse+fine, zoom, strobe, gobo, static values for fixed channels,
…). User profiles override a built-in with the same id.

## Lighting cues

A **Lighting** cue stores a *look* — dimmer, colour, white, pan/tilt, and
beam (zoom / strobe / gobo) values — for any subset of the patch, and
crossfades the rig to it over *Fade (s)* with a selectable
[curve](cues.md#fade-curves).

Fixtures **not** included in a cue keep whatever state the previous cue left
them in (LTP tracking): build your show as a sequence of partial looks, and
only the fixtures you touch change.

## Pixel-map segments

*Segments* stream video onto LED fixtures, vjarda-style. Each segment
samples a rectangle of a source texture, downsamples it to a `cols × rows`
grid, and writes one fixture-profile-worth of channels per cell starting at
a universe/address, walking the grid in the chosen scan order (row/column
order, serpentine, …).

| Property | Meaning |
|---|---|
| Source | **Canvas** (what the projectors show) or **PixelMap** (a dedicated texture fed by PixelMap cues — LED content independent of the projector picture). |
| Region | Normalized rectangle of the source to sample. |
| Grid | `cols × rows` cells — one fixture per cell. |
| Profile / U / Ch | Fixture profile, universe, and 1-based start address of the first cell. |
| Scan | Cell-to-address walking order. |
| Brightness / Gamma / White | Colour pipeline: output gamma (default 2.2) maps the display-referred canvas to LED-linear intensity; the white mode controls RGBW derivation (use **Off** for plain RGB). |

While a segment has content it streams continuously, and its channels
**override lighting-cue looks** on the same addresses.

## PixelMap cues

A [PixelMap cue](cues.md#pixelmap) plays a video or still into the dedicated
pixel-map texture. Point segments at the **PixelMap** source to drive LEDs
with it; a OneShot cue blanks the texture to black when it ends.
