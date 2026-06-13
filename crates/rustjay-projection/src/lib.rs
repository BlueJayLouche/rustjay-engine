//! # `rustjay-projection` — Output post-processor for projection mapping
//!
//! Consumes the final composited frame from any [`EffectInstance`] and warps /
//! edge-blends / slices it to physical projector outputs.
//!
//! Feature-gated and off by default in `rustjay-engine`.

pub mod stage;
pub use stage::ProjectionStage;

pub mod identity;
pub use identity::{BlitPipeline, IdentityStage};

pub mod dome;
pub use dome::{DomeStage, DomemasterConfig, DomemasterParams, DomemasterResolution};

pub mod warp;
pub use warp::{compute_forward_homography, MeshPoint, WarpMesh, WarpMode, WarpStage};

pub mod rotation;
pub use rotation::{RotationStage, RotationSync};

pub mod edge_blend;
pub use edge_blend::{blend_alpha, EdgeBlendConfig, EdgeBlendEdge, EdgeBlendStage};

pub mod slicer;
pub use slicer::{
    compute_dome_meshes, compute_projector_mesh, DomeGeometry, DomePreset, DomeSetup,
    ProjectorConfig, SLICER_GRID_COLS, SLICER_GRID_ROWS,
};

#[cfg(feature = "surface-import")]
pub mod surface_import;
#[cfg(feature = "surface-import")]
pub use surface_import::{
    contour_to_warp_mesh, detect_from_dxf, detect_from_file, detect_from_image, detect_from_rgba,
    detect_from_svg, DetectedContour, DetectionMethod, DetectionParams, DetectionResult, HullMode,
    ImportError, Surface,
};

pub mod headless;
pub use headless::HeadlessOutput;

pub mod sample_stage;
pub use sample_stage::{AtlasLayout, AtlasTile, SampleStage};

pub mod pixel_sampler;
pub use pixel_sampler::{PixelSampler, SamplerId};

#[cfg(feature = "auto-edge-blend")]
pub mod auto_blend;
#[cfg(feature = "auto-edge-blend")]
pub use auto_blend::{compute_auto_edge_blend, AutoBlendComputer, AutoBlendResult};

#[cfg(test)]
pub(crate) mod test_harness;
