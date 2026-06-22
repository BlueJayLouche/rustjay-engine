//! # CV LED Mapping
//!
//! Map addressable LEDs (ws281x et al.) by flashing a calibration pattern and
//! recovering each LED's position from a camera frame, then export a
//! [`LedMap`](format::LedMap) that the engine can sample. See `DESIGN.md`.
//!
//! - [`format`] — the `ledmap.json` interchange types + save/load.
//! - [`detect`] — blob detection (threshold → connected components → subpixel
//!   centroid), dependency-free.
//! - [`calibrate`] — sequential single-LED flash controller: drives the pattern
//!   through `rustjay-lighting` (sACN), ingests captured frames, builds a map.
//! - [`sampler`] — [`PointMap`](sampler::PointMap): play a recovered map back,
//!   sampling rendered pixels at each LED's `(u,v)` → a `DmxFrame`.
//!
//! Wire packing and transport are reused from `rustjay-lighting`
//! (`pack_fixtures`, `SacnTransport`, `DmxSender`); this crate owns the CV and
//! the format, not the protocol.

pub mod calibrate;
pub mod detect;
pub mod format;
pub mod sampler;
pub mod session;

pub use calibrate::SequentialCalibrator;
pub use detect::{brightest_blob, detect_blobs, Blob};
pub use format::{Led, LedMap, Source, Space, LEDMAP_VERSION};
pub use sampler::{PointMap, IDENTITY_QUAD};
pub use session::{CalibrationSession, Tick};
