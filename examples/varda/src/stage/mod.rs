//! Stage — surfaces, outputs, and projection mapping state.
//!
//! Delegates to `rustjay-projection` for warp, edge-blend, and dome.
//! See VARDA_PORT.md Phase 7–8.

/// Surface model (polygon/circle) + source selector.
pub struct Surface;

/// Output model: window/fullscreen/NDI/stream/record assignment.
pub struct Output;
