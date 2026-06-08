//! # `rustjay-projection` — Output post-processor for projection mapping
//!
//! Consumes the final composited frame from any [`EffectInstance`] and warps /
//! edge-blends / slices it to physical projector outputs.
//!
//! Feature-gated and off by default in `rustjay-engine`.

#![warn(missing_docs)]

/// The `ProjectionStage` trait and related types.
pub mod stage;
pub use stage::ProjectionStage;

/// Identity passthrough stage and shared blit pipeline.
pub mod identity;
pub use identity::{BlitPipeline, IdentityStage};

/// Dome projection stage (cubemap → fisheye).
pub mod dome;
pub use dome::{DomeStage, DomemasterConfig, DomemasterParams, DomemasterResolution};

/// Warp projection stage (mesh / corner-pin).
pub mod warp;
pub use warp::{compute_forward_homography, MeshPoint, WarpMesh, WarpMode, WarpStage};

/// Output rotation stage (0°/90°/180°/270°).
pub mod rotation;
pub use rotation::{RotationStage, RotationSync};

/// Edge blend projection stage.
pub mod edge_blend;
pub use edge_blend::{blend_alpha, EdgeBlendConfig, EdgeBlendEdge, EdgeBlendStage};

/// Dome slicer — per-projector warp mesh generation from dome geometry.
pub mod slicer;
pub use slicer::{
    compute_dome_meshes, compute_projector_mesh, DomeGeometry, DomePreset, DomeSetup,
    ProjectorConfig, SLICER_GRID_COLS, SLICER_GRID_ROWS,
};

/// Surface import — SVG / DXF / raster → contours → `Surface` → `WarpMesh`.
#[cfg(feature = "surface-import")]
pub mod surface_import;
#[cfg(feature = "surface-import")]
pub use surface_import::{
    contour_to_warp_mesh, detect_from_dxf, detect_from_file, detect_from_image, detect_from_rgba,
    detect_from_svg, DetectedContour, DetectionMethod, DetectionParams, DetectionResult, HullMode,
    ImportError, Surface,
};

/// Headless output with async GPU→CPU readback.
pub mod headless;
pub use headless::HeadlessOutput;

/// Auto edge-blend computer (CPU-side overlap detection).
#[cfg(feature = "auto-edge-blend")]
pub mod auto_blend;
#[cfg(feature = "auto-edge-blend")]
pub use auto_blend::{compute_auto_edge_blend, AutoBlendComputer, AutoBlendResult};

#[cfg(test)]
pub(crate) mod test_harness;
