//! Auto edge-blend computer — CPU-side overlap detection between surfaces.
//!
//! Ported from Varda's edge_blend.rs auto-detection logic.

/// Maximum number of overlap zones per surface.
pub const MAX_OVERLAP_ZONES: usize = 4;

/// A single overlap zone in surface-local UV space [0..1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OverlapZone {
    /// Overlap rectangle in surface UV: [u_min, v_min, u_max, v_max].
    pub uv_rect: [f32; 4],
    /// Smoothstep gamma exponent for the blend ramp.
    pub gamma: f32,
    /// Horizontal ramp direction: +1.0 = fade toward u_max, -1.0 = fade toward u_min, 0.0 = none.
    pub ramp_x: f32,
    /// Vertical ramp direction: +1.0 = fade toward v_max, -1.0 = fade toward v_min, 0.0 = none.
    pub ramp_y: f32,
}

/// Per-surface overlap zones for Auto mode blending.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SurfaceOverlapZones {
    /// Computed overlap zones, sorted by area descending.
    pub zones: Vec<OverlapZone>,
}

impl SurfaceOverlapZones {
    /// Returns true if any overlap zones are present.
    pub fn any_enabled(&self) -> bool {
        !self.zones.is_empty()
    }

    /// Add a zone, keeping only the largest `MAX_OVERLAP_ZONES` by area.
    pub fn add_zone(&mut self, zone: OverlapZone) {
        self.zones.push(zone);
        if self.zones.len() > MAX_OVERLAP_ZONES {
            self.zones.sort_by(|a, b| {
                let area_a = (a.uv_rect[2] - a.uv_rect[0]) * (a.uv_rect[3] - a.uv_rect[1]);
                let area_b = (b.uv_rect[2] - b.uv_rect[0]) * (b.uv_rect[3] - b.uv_rect[1]);
                area_b
                    .partial_cmp(&area_a)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            self.zones.truncate(MAX_OVERLAP_ZONES);
        }
    }
}

/// Result of auto overlap-zone detection for a single surface.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoBlendResult {
    /// Index of the output this surface belongs to.
    pub output_idx: usize,
    /// Computed overlap zones for this surface.
    pub overlap_zones: SurfaceOverlapZones,
}

/// A mapped region on the canvas belonging to a specific output.
#[derive(Debug, Clone, PartialEq)]
pub struct MappedRegion {
    /// Axis-aligned bounding box [x, y, width, height] in normalized canvas coords.
    pub bbox: [f32; 4],
    /// Primary contour vertices in canvas coords (for precise polygon intersection).
    pub vertices: Vec<[f32; 2]>,
}

/// Describes one output's surface topology for auto edge-blend computation.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputSurfaceInfo {
    /// Index into the outputs array.
    pub output_idx: usize,
    /// Default gamma to apply when auto-computing blend edges.
    pub default_gamma: f32,
    /// All mapped regions assigned to this output.
    pub regions: Vec<MappedRegion>,
}

/// Compute AABB intersection. Returns `Some([x, y, w, h])` if boxes overlap, `None` otherwise.
fn aabb_intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let ax2 = a[0] + a[2];
    let ay2 = a[1] + a[3];
    let bx2 = b[0] + b[2];
    let by2 = b[1] + b[3];
    let ix = a[0].max(b[0]);
    let iy = a[1].max(b[1]);
    let ix2 = ax2.min(bx2);
    let iy2 = ay2.min(by2);
    let iw = ix2 - ix;
    let ih = iy2 - iy;
    if iw > 1e-6 && ih > 1e-6 {
        Some([ix, iy, iw, ih])
    } else {
        None
    }
}

/// Compute ramp direction for surface A's overlap zone toward surface B.
fn compute_ramp_direction(bbox_a: &[f32; 4], bbox_b: &[f32; 4]) -> (f32, f32) {
    let center_a = [bbox_a[0] + bbox_a[2] * 0.5, bbox_a[1] + bbox_a[3] * 0.5];
    let center_b = [bbox_b[0] + bbox_b[2] * 0.5, bbox_b[1] + bbox_b[3] * 0.5];
    let dx = center_b[0] - center_a[0];
    let dy = center_b[1] - center_a[1];
    let ramp_x = if dx.abs() > 1e-4 { dx.signum() } else { 0.0 };
    let ramp_y = if dy.abs() > 1e-4 { dy.signum() } else { 0.0 };
    (ramp_x, ramp_y)
}

/// Convert a stage-space AABB overlap into surface-local UV coordinates [0..1].
fn stage_to_surface_uv(region: &[f32; 4], overlap: &[f32; 4]) -> [f32; 4] {
    let rw = region[2].max(1e-6);
    let rh = region[3].max(1e-6);
    let u_min = ((overlap[0] - region[0]) / rw).clamp(0.0, 1.0);
    let v_min = ((overlap[1] - region[1]) / rh).clamp(0.0, 1.0);
    let u_max = ((overlap[0] + overlap[2] - region[0]) / rw).clamp(0.0, 1.0);
    let v_max = ((overlap[1] + overlap[3] - region[1]) / rh).clamp(0.0, 1.0);
    [u_min, v_min, u_max, v_max]
}

/// Derive per-surface overlap zones for each output from surface topology.
///
/// Algorithm:
/// 1. For each output, iterate its regions (surfaces).
/// 2. For each region, compare against regions on every other output.
/// 3. Compute AABB intersection in stage space → convert to surface-local UV rect.
/// 4. Compute ramp direction from relative surface positions.
/// 5. Collect zones, keep top `MAX_OVERLAP_ZONES` by area.
pub fn compute_auto_edge_blend(infos: &[OutputSurfaceInfo]) -> Vec<AutoBlendResult> {
    let mut results: Vec<AutoBlendResult> = Vec::new();

    for info in infos {
        let gamma = info.default_gamma;

        for region_a in &info.regions {
            let mut zones = SurfaceOverlapZones::default();

            for other in infos {
                if other.output_idx == info.output_idx {
                    continue;
                }
                for region_b in &other.regions {
                    if let Some(overlap) = aabb_intersect(region_a.bbox, region_b.bbox) {
                        let uv_rect = stage_to_surface_uv(&region_a.bbox, &overlap);
                        let (ramp_x, ramp_y) =
                            compute_ramp_direction(&region_a.bbox, &region_b.bbox);
                        zones.add_zone(OverlapZone {
                            uv_rect,
                            gamma,
                            ramp_x,
                            ramp_y,
                        });
                    }
                }
            }

            results.push(AutoBlendResult {
                output_idx: info.output_idx,
                overlap_zones: zones,
            });
        }
    }

    results
}

/// Placeholder struct for future expansion (e.g. caching, config).
pub struct AutoBlendComputer;

impl AutoBlendComputer {
    /// Create a new auto-blend computer.
    pub fn new() -> Self {
        Self
    }
}

impl Default for AutoBlendComputer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_info(idx: usize, regions: Vec<MappedRegion>) -> OutputSurfaceInfo {
        OutputSurfaceInfo {
            output_idx: idx,
            default_gamma: 2.2,
            regions,
        }
    }

    fn make_region(bbox: [f32; 4]) -> MappedRegion {
        let [x, y, w, h] = bbox;
        MappedRegion {
            bbox,
            vertices: vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]],
        }
    }

    fn find_zones(results: &[AutoBlendResult], output_idx: usize) -> &SurfaceOverlapZones {
        &results
            .iter()
            .find(|r| r.output_idx == output_idx)
            .unwrap()
            .overlap_zones
    }

    #[test]
    fn auto_blend_no_overlap() {
        let infos = vec![
            make_info(0, vec![make_region([0.0, 0.0, 0.4, 1.0])]),
            make_info(1, vec![make_region([0.5, 0.0, 0.4, 1.0])]),
        ];
        let results = compute_auto_edge_blend(&infos);
        assert_eq!(results.len(), 2);
        assert!(!results[0].overlap_zones.any_enabled());
        assert!(!results[1].overlap_zones.any_enabled());
    }

    #[test]
    fn auto_blend_horizontal_overlap() {
        let infos = vec![
            make_info(0, vec![make_region([0.0, 0.0, 0.6, 1.0])]),
            make_info(1, vec![make_region([0.4, 0.0, 0.6, 1.0])]),
        ];
        let results = compute_auto_edge_blend(&infos);
        let z0 = find_zones(&results, 0);
        let z1 = find_zones(&results, 1);

        assert_eq!(z0.zones.len(), 1);
        assert_eq!(z1.zones.len(), 1);

        let rect0 = z0.zones[0].uv_rect;
        assert!((rect0[2] - 1.0).abs() < 1e-4); // u_max = 1.0
        assert!((rect0[0] - 0.4 / 0.6).abs() < 0.01); // u_min ≈ 0.667
        assert_eq!(z0.zones[0].ramp_x, 1.0);
        assert_eq!(z0.zones[0].ramp_y, 0.0);

        let rect1 = z1.zones[0].uv_rect;
        assert!((rect1[0]).abs() < 1e-4); // u_min = 0.0
        assert!((rect1[2] - 0.2 / 0.6).abs() < 0.01); // u_max ≈ 0.333
        assert_eq!(z1.zones[0].ramp_x, -1.0);
        assert_eq!(z1.zones[0].ramp_y, 0.0);
    }

    #[test]
    fn aabb_intersect_no_overlap() {
        assert!(aabb_intersect([0.0, 0.0, 0.3, 0.5], [0.5, 0.0, 0.3, 0.5]).is_none());
    }

    #[test]
    fn aabb_intersect_touching_edges() {
        assert!(aabb_intersect([0.0, 0.0, 0.5, 1.0], [0.5, 0.0, 0.5, 1.0]).is_none());
    }

    #[test]
    fn stage_to_uv_full_overlap() {
        let region = [0.0, 0.0, 1.0, 1.0];
        let overlap = [0.0, 0.0, 1.0, 1.0];
        let uv = stage_to_surface_uv(&region, &overlap);
        assert!((uv[0]).abs() < 1e-6);
        assert!((uv[1]).abs() < 1e-6);
        assert!((uv[2] - 1.0).abs() < 1e-6);
        assert!((uv[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn overlap_zones_max_capacity() {
        let mut zones = SurfaceOverlapZones::default();
        for i in 0..6 {
            let size = 0.1 * (i as f32 + 1.0);
            zones.add_zone(OverlapZone {
                uv_rect: [0.0, 0.0, size, size],
                gamma: 2.2,
                ramp_x: 1.0,
                ramp_y: 1.0,
            });
        }
        assert_eq!(zones.zones.len(), MAX_OVERLAP_ZONES);
        let areas: Vec<f32> = zones
            .zones
            .iter()
            .map(|z| (z.uv_rect[2] - z.uv_rect[0]) * (z.uv_rect[3] - z.uv_rect[1]))
            .collect();
        for i in 0..areas.len() - 1 {
            assert!(areas[i] >= areas[i + 1]);
        }
    }
}
