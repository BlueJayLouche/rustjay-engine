//! Dome slicer — auto-computes per-projector warp meshes from dome geometry
//! and projector placement. Generates `WarpMesh` instances that map each
//! projector's output rectangle onto the correct region of the domemaster.

use crate::warp::{MeshPoint, WarpMesh};

/// Dome hemisphere geometry.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct DomeGeometry {
    /// Dome radius in arbitrary units (only ratios matter).
    pub radius: f32,
    /// Truncation angle in degrees from zenith.
    /// 90° = full hemisphere, 60° = truncated dome.
    pub truncation_degrees: f32,
    /// Dome tilt in degrees (0 = zenith up, positive = tilted forward).
    pub tilt_degrees: f32,
    /// Content azimuth rotation in degrees.
    #[serde(default)]
    pub content_azimuth_degrees: f32,
    /// Content elevation rotation in degrees.
    #[serde(default)]
    pub content_elevation_degrees: f32,
    /// Content roll in degrees.
    #[serde(default)]
    pub content_roll_degrees: f32,
}

impl Default for DomeGeometry {
    fn default() -> Self {
        Self {
            radius: 1.0,
            truncation_degrees: 90.0,
            tilt_degrees: 0.0,
            content_azimuth_degrees: 0.0,
            content_elevation_degrees: 0.0,
            content_roll_degrees: 0.0,
        }
    }
}

/// Projector placement and lens configuration.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct ProjectorConfig {
    /// Azimuth angle in degrees (0 = front, 90 = right, etc.)
    pub azimuth_degrees: f32,
    /// Elevation angle in degrees from the horizon.
    pub elevation_degrees: f32,
    /// Distance from dome center (normalized to dome radius).
    pub distance: f32,
    /// Horizontal field of view in degrees.
    pub fov_degrees: f32,
    /// Aspect ratio (width / height), e.g. 16.0/9.0
    pub aspect_ratio: f32,
    /// Overlap percentage with adjacent projectors (0.0–1.0).
    pub overlap_pct: f32,
}

impl Default for ProjectorConfig {
    fn default() -> Self {
        Self {
            azimuth_degrees: 0.0,
            elevation_degrees: 30.0,
            distance: 0.5,
            fov_degrees: 90.0,
            aspect_ratio: 16.0 / 9.0,
            overlap_pct: 0.15,
        }
    }
}

/// A complete dome setup: geometry + projector array.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DomeSetup {
    /// Dome geometry.
    pub geometry: DomeGeometry,
    /// Projector configurations.
    pub projectors: Vec<ProjectorConfig>,
}

/// Default mesh grid columns for slicer output (must be ≥ 2).
pub const SLICER_GRID_COLS: u32 = 17;
/// Default mesh grid rows for slicer output (must be ≥ 2).
pub const SLICER_GRID_ROWS: u32 = 17;

/// Compute the warp mesh for a single projector aimed at a dome.
///
/// Returns a `WarpMesh` where:
/// - `position` = uniform grid in projector output space [0..1]²
/// - `uv` = corresponding domemaster texture coordinates [0..1]²
pub fn compute_projector_mesh(
    geometry: &DomeGeometry,
    projector: &ProjectorConfig,
    cols: u32,
    rows: u32,
) -> WarpMesh {
    let half_fov_h = (projector.fov_degrees * 0.5).to_radians();
    let half_fov_v = ((projector.fov_degrees / projector.aspect_ratio) * 0.5).to_radians();
    let az = projector.azimuth_degrees.to_radians();
    let el = projector.elevation_degrees.to_radians();
    let dome_trunc = geometry.truncation_degrees.to_radians();
    let dome_tilt = geometry.tilt_degrees.to_radians();
    // Content rotation is NOT baked into warp meshes — it is applied
    // in the domemaster shader in real-time so slices stay fixed.
    let content_az = 0.0_f32;
    let content_el = 0.0_f32;
    let content_roll = 0.0_f32;

    let mut points = Vec::with_capacity((cols * rows) as usize);

    for row in 0..rows {
        let v = row as f32 / (rows - 1).max(1) as f32;
        let angle_v = half_fov_v * (1.0 - 2.0 * v);

        for col in 0..cols {
            let u = col as f32 / (cols - 1).max(1) as f32;
            let angle_h = half_fov_h * (2.0 * u - 1.0);

            let local_dir = normalize([angle_h.tan(), angle_v.tan(), 1.0]);
            let after_el = rotate_x(local_dir, el);
            let world_dir = rotate_y(after_el, az);

            let uv = ray_to_domemaster_uv(
                world_dir,
                dome_trunc,
                dome_tilt,
                content_az,
                content_el,
                content_roll,
            );

            points.push(MeshPoint {
                position: [u, v],
                uv,
            });
        }
    }

    WarpMesh { cols, rows, points }
}

/// Convenience: compute meshes for all projectors in a dome setup.
pub fn compute_dome_meshes(setup: &DomeSetup) -> Vec<WarpMesh> {
    setup
        .projectors
        .iter()
        .map(|p| compute_projector_mesh(&setup.geometry, p, SLICER_GRID_COLS, SLICER_GRID_ROWS))
        .collect()
}

// ── Vector math helpers ─────────────────────────────────────────────────

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-10 {
        return [0.0, 0.0, 1.0];
    }
    [v[0] / len, v[1] / len, v[2] / len]
}

fn rotate_x(v: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    [v[0], v[1] * c - v[2] * s, v[1] * s + v[2] * c]
}

fn rotate_y(v: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    [v[0] * c + v[2] * s, v[1], -v[0] * s + v[2] * c]
}

fn rotate_z(v: [f32; 3], angle: f32) -> [f32; 3] {
    let (s, c) = angle.sin_cos();
    [v[0] * c - v[1] * s, v[0] * s + v[1] * c, v[2]]
}

/// Convert a world-space ray direction to domemaster UV coordinates.
fn ray_to_domemaster_uv(
    dir: [f32; 3],
    trunc_angle: f32,
    dome_tilt: f32,
    content_az: f32,
    content_el: f32,
    content_roll: f32,
) -> [f32; 2] {
    let dir = rotate_x(dir, -dome_tilt);
    let dir = rotate_z(dir, -content_roll);
    let dir = rotate_x(dir, -content_el);
    let dir = rotate_y(dir, -content_az);

    let polar = dir[1].clamp(-1.0, 1.0).acos();
    let azimuth = dir[2].atan2(dir[0]);

    let max_angle = trunc_angle.min(std::f32::consts::PI);
    let r = if max_angle > 1e-6 {
        (polar / max_angle).min(1.0)
    } else {
        0.0
    };

    let uv_x = 0.5 + r * 0.5 * azimuth.cos();
    let uv_y = 0.5 + r * 0.5 * azimuth.sin();

    [uv_x.clamp(0.0, 1.0), uv_y.clamp(0.0, 1.0)]
}

// ── Dome presets ────────────────────────────────────────────────────────

/// Standard dome projector arrangement presets.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum DomePreset {
    /// Single projector (fisheye lens, aimed at zenith)
    Single,
    /// 2 projectors (front/back split)
    Dual,
    /// 3 projectors (120° apart)
    Triple,
    /// 4 projectors (90° apart)
    Quad,
    /// 5 projectors (72° apart)
    Penta,
    /// 6 projectors (60° apart)
    Hexa,
    /// 8 projectors (45° apart)
    Octa,
}

impl DomePreset {
    /// Number of projectors in this preset.
    pub fn count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Dual => 2,
            Self::Triple => 3,
            Self::Quad => 4,
            Self::Penta => 5,
            Self::Hexa => 6,
            Self::Octa => 8,
        }
    }

    /// Generate a DomeSetup with default geometry and evenly-spaced projectors.
    pub fn to_setup(self) -> DomeSetup {
        self.to_setup_with_geometry(DomeGeometry::default())
    }

    /// Generate a DomeSetup with custom geometry and evenly-spaced projectors.
    pub fn to_setup_with_geometry(self, geometry: DomeGeometry) -> DomeSetup {
        let n = self.count();

        if n == 1 {
            return DomeSetup {
                geometry,
                projectors: vec![ProjectorConfig {
                    azimuth_degrees: 0.0,
                    elevation_degrees: 90.0,
                    distance: 0.0,
                    fov_degrees: 180.0,
                    aspect_ratio: 1.0,
                    overlap_pct: 0.0,
                }],
            };
        }

        let angle_step = 360.0 / n as f32;
        let fov = angle_step + angle_step * 0.15;
        let overlap = 0.15;

        let projectors = (0..n)
            .map(|i| ProjectorConfig {
                azimuth_degrees: i as f32 * angle_step,
                elevation_degrees: 30.0,
                distance: 0.5,
                fov_degrees: fov,
                aspect_ratio: 16.0 / 9.0,
                overlap_pct: overlap,
            })
            .collect();

        DomeSetup { geometry, projectors }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dome_geometry() {
        let g = DomeGeometry::default();
        assert!((g.radius - 1.0).abs() < f32::EPSILON);
        assert!((g.truncation_degrees - 90.0).abs() < f32::EPSILON);
    }

    #[test]
    fn preset_counts() {
        assert_eq!(DomePreset::Single.count(), 1);
        assert_eq!(DomePreset::Quad.count(), 4);
        assert_eq!(DomePreset::Octa.count(), 8);
    }

    #[test]
    fn quad_meshes() {
        let setup = DomePreset::Quad.to_setup();
        let azimuths: Vec<f32> = setup.projectors.iter().map(|p| p.azimuth_degrees).collect();
        assert!((azimuths[0] - 0.0).abs() < 1e-6);
        assert!((azimuths[1] - 90.0).abs() < 1e-6);
        assert!((azimuths[2] - 180.0).abs() < 1e-6);
        assert!((azimuths[3] - 270.0).abs() < 1e-6);
    }

    #[test]
    fn compute_dome_meshes_returns_correct_count() {
        let setup = DomePreset::Quad.to_setup();
        let meshes = compute_dome_meshes(&setup);
        assert_eq!(meshes.len(), 4);
        for mesh in &meshes {
            assert_eq!(mesh.cols, SLICER_GRID_COLS);
            assert_eq!(mesh.rows, SLICER_GRID_ROWS);
        }
    }

    #[test]
    fn mesh_uvs_are_in_unit_square() {
        let mesh = compute_projector_mesh(
            &DomeGeometry::default(),
            &ProjectorConfig::default(),
            9,
            9,
        );
        for pt in &mesh.points {
            assert!(pt.uv[0] >= 0.0 && pt.uv[0] <= 1.0);
            assert!(pt.uv[1] >= 0.0 && pt.uv[1] <= 1.0);
        }
    }

    #[test]
    fn zenith_ray_center() {
        let uv = ray_to_domemaster_uv(
            [0.0, 1.0, 0.0],
            std::f32::consts::FRAC_PI_2,
            0.0,
            0.0,
            0.0,
            0.0,
        );
        assert!((uv[0] - 0.5).abs() < 1e-4);
        assert!((uv[1] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn config_serialization_roundtrip() {
        let setup = DomePreset::Triple.to_setup();
        let json = serde_json::to_string(&setup).unwrap();
        let deserialized: DomeSetup = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.projectors.len(), 3);
    }
}
