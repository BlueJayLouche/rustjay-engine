//! Sources — ISF, video, image, camera, NDI, streams.
//!
//! Delegates to engine crates where possible:
//! - ISF      → `rustjay-isf`
//! - Camera   → `rustjay-io/input` (webcam)
//! - NDI      → `rustjay-io/ndi_runtime`
//! - Video decode / HAP / SRT / HLS / DASH / RTMP → coverage gaps;
//!   see `PARITY.md` Phase 2 / 9 / 10 probes.

/// Source/effect registry (library). Drives the Library panel + API.
pub struct Registry;
