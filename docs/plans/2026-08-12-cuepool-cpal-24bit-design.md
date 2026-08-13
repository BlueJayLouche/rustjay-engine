# CuePool native 24-bit output design

## Scope

Upgrade only the nested `examples/cuepool` workspace from CPAL 0.15.3 to
0.18.1. The root workspace already uses CPAL 0.17. CuePool owns its CPAL
dependency and exposes no CPAL types across the workspace seam, so the root
workspace and `rustjay-audio` remain unchanged.

## Implementation

Keep the output-selection and conversion structure introduced in #71. Rank
supported formats as F32, I32, I24, then I16, and route I24 callbacks through
the existing generic `render_converted` path using CPAL's four-byte `I24`
in-memory sample type. CPAL handles packing those samples into native three-byte
ASIO buffers. Clamp only I24 conversion inputs to its representable range so
full-scale and over-range mix samples saturate rather than wrap. If a runtime
ASIO buffer resize exceeds the preallocated conversion scratch buffer, keep the
safe silence fallback and log the first occurrence for that stream.

Adapt only the CPAL 0.18 API changes used by `cuepool-audio`: unified errors,
device descriptions, the `SampleRate` alias, and by-value stream configs.
Preserve #71's fail-closed selection and error catalogue, apart from adding I24
to the supported-format text.

On Linux, align `midir` to 0.11 because its widened ALSA dependency permits the
same `alsa` 0.11/`alsa-sys` 0.4 pair required by CPAL 0.18. Cargo cannot link
the distinct ALSA sys-crate versions selected by `midir` 0.10 and CPAL 0.18.

CPAL's public ASIO enumeration still drops drivers whose metadata probe fails
without exposing a device or reason. Improving #80 locally would require a
backend-specific enumeration fork, so it remains an upstream CPAL concern.

## Verification

Add host-agnostic tests for I24 ranking, representation, selection, and float
conversion. Run the existing CuePool crate tests with default features and with
`--features asio` on Linux. The CuePool binary and video crate remain excluded
because this environment does not provide FFmpeg.
