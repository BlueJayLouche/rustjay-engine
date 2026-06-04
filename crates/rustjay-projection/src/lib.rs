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
pub use dome::{DomeStage, DomemasterConfig, DomemasterResolution, DomemasterParams};

/// Warp projection stage (mesh / corner-pin).
pub mod warp;
pub use warp::{WarpStage, WarpMesh, MeshPoint, WarpMode, compute_forward_homography};

/// Edge blend projection stage.
pub mod edge_blend;
pub use edge_blend::{EdgeBlendStage, EdgeBlendConfig, EdgeBlendEdge, blend_alpha};

/// Dome slicer — per-projector warp mesh generation from dome geometry.
pub mod slicer;
pub use slicer::{
    DomeGeometry, DomePreset, DomeSetup, ProjectorConfig,
    compute_projector_mesh, compute_dome_meshes,
    SLICER_GRID_COLS, SLICER_GRID_ROWS,
};

/// Surface import — SVG / DXF / raster → warp mesh.
#[cfg(feature = "surface-import")]
pub mod surface_import;
#[cfg(feature = "surface-import")]
pub use surface_import::{from_raster, from_svg, from_dxf};

/// Auto edge-blend computer (CPU-side overlap detection).
#[cfg(feature = "auto-edge-blend")]
pub mod auto_blend;
#[cfg(feature = "auto-edge-blend")]
pub use auto_blend::{AutoBlendResult, AutoBlendComputer, compute_auto_edge_blend};

#[cfg(test)]
pub(crate) mod test_harness;
