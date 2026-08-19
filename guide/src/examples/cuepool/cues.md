# Cue Reference

## Properties every cue has

| Property | Meaning |
|---|---|
| `#` (QID) | The cue number — a decimal, so 1.5 sorts between 1 and 2. Referenced by Stop/Volume/Goto cues and OSC commands. |
| Name / Description | Free text shown in the list and inspector. |
| Colour | Row tint in the cue list. |
| Trigger | **Go** (waits for the GO button), **WithLast** (fires together with the previous cue), **AfterLast** (fires when the previous cue finishes). |
| Delay | Seconds to wait after being triggered before actually starting. |
| Enabled | Disabled cues are skipped. |
| Loop | **OneShot**, **Looped** (× Loop Count), **LoopedInfinite**, **HoldLast** (hold the final frame/state). |
| Retriggerable | When off, firing the cue again while it is still playing is ignored — prevents stacked audio or flashing video from a double GO. |
| Remote Node | Fire this cue on a named [remote node](show-control.md#remote-nodes) instead of locally. Pick from the detected machines rather than typing — the Inspector flags a name nothing answers to. |
| Triggers | Optional per-cue hotkey / MIDI / wall-clock / timecode triggers — see [Show Control](show-control.md#per-cue-triggers). |

## Cue types

### Sound

Plays an audio file (anything symphonia decodes: WAV, FLAC, MP3, OGG, AAC,
ALAC, …). Properties: file, start time and duration (clip trim), volume (dB),
pan, fade in/out with a selectable curve, a per-cue
[EQ](audio.md#per-cue-eq), and [output routing](audio.md#routing).

### Video

Plays a video file (anything FFmpeg decodes) onto the
[projection canvas](video.md), with the same audio properties as a Sound cue
for its soundtrack.

### Image / Text

Draw a still image or a text overlay on the canvas. Text cues have a font
size, colour, and an optional font file (`.ttf`/`.otf`; leave unset for the
built-in font — packed with the project like other media). Both cue types
have a *Fit* mode that scales the content (for text, the rendered text
block) onto the canvas: **Stretch**, **Fit** (letterbox), or **Fill**
(center-crop).

Text renders on an overlay layer *above* whatever video or image is playing
(supertitles over a running clip, for example). A new Text cue replaces the
overlay; a Stop cue targeting the text (or Stop All) clears it.

### Group

A container. Drag cues onto the group header to nest them; the group fires
its members according to their trigger modes.

### Stop

Stops another cue: target Q#, **Immediate** or **LoopEnd** (finish the
current loop first), with an optional fade-out time and curve. A video cue's
picture and its audio track fade out together. Targeting a Group stops every
member, nested groups included, and a stop also cancels a target still
waiting on its Delay. **Stop All** stops everything instead of one target —
with a fade it brings the whole show down gently, while the transport's Stop
button is always an instant cut.

### Volume

Fades a playing Sound/Video cue to a target level: target Q#, target dB,
fade time, fade curve.

### Goto

Moves the playhead to the target Q#, so the next GO fires from there. Useful
for skips and loops in the show structure.

### OSC

Sends an OSC message when fired. The command format is
`/address,arg1,arg2,…` — e.g. `/mixer/scene,3`. Arguments are parsed as
ints, floats, or strings. See [Show Control](show-control.md#osc).

### TimeCode

Starts (and scrubs) the show clock that
[timecode triggers](show-control.md#per-cue-triggers) fire against, with a
start time and duration.

### Lighting

Crossfades patched DMX fixtures to a saved look. Fixtures not included in
the cue's snapshot keep their current state (LTP tracking). Tick **🔴 Live**
in the inspector to see edits on the rig as you program. See
[Lighting](lighting.md#lighting-cues).

### DMX Show

Plays a recorded DMX stream (`.dmxrec`) to the lighting output, merged
against Lighting-cue looks by sACN-style priority. See
[DMX Show cues](lighting.md#dmx-show-cues).

### PixelMap

Plays a video or still into the dedicated pixel-map texture that LED
[segments](lighting.md#pixel-map-segments) sample by default — LED content
independent of the projector picture. Looping follows the cue's Loop mode; a
OneShot end blanks to black.

### Dummy

Does nothing. A placeholder / structural marker.

## Fade curves

Wherever a fade has a *Type*, the options are **Linear**, **S-Curve**
(default), **Square**, and **Inverse Square**.
