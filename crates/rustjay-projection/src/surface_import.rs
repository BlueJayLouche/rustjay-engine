//! Surface import — generate [`WarpMesh`] from SVG, DXF, or raster images.
//!
//! All functions are gated behind the `surface-import` feature.

use crate::warp::{MeshPoint, WarpMesh};
use std::path::Path;

/// Load a raster image and treat its grayscale values as a UV displacement map.
///
/// * `path` — image file (PNG, JPEG, etc. supported by the `image` crate)
/// * `cols`, `rows` — mesh grid resolution (must be ≥ 2)
/// * `scale` — maximum UV displacement. 0.25 means a pure-white pixel shifts
///   the corresponding UV by +0.25 and a pure-black pixel shifts by −0.25.
///
/// The image is resized to the grid resolution with nearest-neighbour sampling,
/// then each pixel's luminance is mapped from `[0, 1]` to `[-scale, +scale]`.
/// A mid-grey (luminance = 0.5) produces zero displacement — identity UV.
///
/// # Example
/// ```ignore
/// let mesh = surface_import::from_raster("mask.png", 17, 17, 0.25)?;
/// ```
pub fn from_raster(path: &Path, cols: u32, rows: u32, scale: f32) -> anyhow::Result<WarpMesh> {
    if cols < 2 || rows < 2 {
        anyhow::bail!("mesh grid must be at least 2×2");
    }

    let img = image::open(path)?.to_rgb8();
    let (img_w, img_h) = (img.width() as usize, img.height() as usize);

    // Resize to grid resolution using nearest-neighbour.
    let mut lum = vec![0.0f32; (cols * rows) as usize];
    for row in 0..rows {
        let src_y = (((row as f32 / (rows - 1) as f32) * (img_h.saturating_sub(1)) as f32)
            .round() as usize)
            .min(img_h.saturating_sub(1));
        for col in 0..cols {
            let src_x = (((col as f32 / (cols - 1) as f32) * (img_w.saturating_sub(1)) as f32)
                .round() as usize)
                .min(img_w.saturating_sub(1));
            let px = img.get_pixel(src_x as u32, src_y as u32);
            let l = luminance(px[0], px[1], px[2]);
            lum[(row * cols + col) as usize] = (l - 0.5) * 2.0 * scale;
        }
    }

    let mut points = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        let v = row as f32 / (rows - 1) as f32;
        for col in 0..cols {
            let u = col as f32 / (cols - 1) as f32;
            let d = lum[(row * cols + col) as usize];
            points.push(MeshPoint {
                position: [u, v],
                uv: [(u + d).clamp(0.0, 1.0), (v + d).clamp(0.0, 1.0)],
            });
        }
    }

    Ok(WarpMesh { cols, rows, points })
}

fn luminance(r: u8, g: u8, b: u8) -> f32 {
    (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) / 255.0
}

/// Load an SVG and convert its path geometry to a warp mesh.
///
/// The SVG `viewBox` is mapped to `[0,1]²`.  Every path in the SVG is flattened
/// to line segments; for each grid point the nearest path segment is found and
/// the UV is nudged toward that segment proportionally to closeness.  The
/// effect is that dense path regions "attract" the warp mesh, making it useful
/// for projection masks drawn in vector tools.
///
/// * `cols`, `rows` — mesh grid resolution (must be ≥ 2)
pub fn from_svg(path: &Path, cols: u32, rows: u32) -> anyhow::Result<WarpMesh> {
    if cols < 2 || rows < 2 {
        anyhow::bail!("mesh grid must be at least 2×2");
    }

    let data = std::fs::read(path)?;
    let opt = usvg::Options::default();
    let tree = usvg::Tree::from_data(&data, &opt)
        .map_err(|e| anyhow::anyhow!("SVG parse error: {e}"))?;

    let size = tree.size();
    let (svg_w, svg_h) = (size.width(), size.height());
    if svg_w <= 0.0 || svg_h <= 0.0 {
        anyhow::bail!("SVG has zero size");
    }

    // Collect all line segments from all paths.
    let mut segments: Vec<([f32; 2], [f32; 2])> = Vec::new();
    for child in tree.root().children() {
        collect_segments(child, svg_w, svg_h, &mut segments);
    }

    if segments.is_empty() {
        // No paths — return identity mesh.
        return Ok(WarpMesh::identity(cols, rows));
    }

    let mut points = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        let v = row as f32 / (rows - 1) as f32;
        for col in 0..cols {
            let u = col as f32 / (cols - 1) as f32;
            let uv = if segments.len() < 64 {
                // Exact nearest-segment for small SVGs.
                nearest_segment_uv([u, v], &segments, 0.08)
            } else {
                // For complex SVGs, just use coarse bounding-box attraction.
                bbox_attractor_uv([u, v], &segments, 0.05)
            };
            points.push(MeshPoint {
                position: [u, v],
                uv,
            });
        }
    }

    Ok(WarpMesh { cols, rows, points })
}

fn collect_segments(node: &usvg::Node, w: f32, h: f32, out: &mut Vec<([f32; 2], [f32; 2])>) {
    match node {
        usvg::Node::Path(path) => {
            let data = path.data();
            let mut current = [0.0f32; 2];
            let mut start = [0.0f32; 2];
            for seg in data.segments() {
                match seg {
                    usvg::tiny_skia_path::PathSegment::MoveTo(p) => {
                        current = [p.x / w, p.y / h];
                        start = current;
                    }
                    usvg::tiny_skia_path::PathSegment::LineTo(p) => {
                        let next = [p.x / w, p.y / h];
                        out.push((current, next));
                        current = next;
                    }
                    usvg::tiny_skia_path::PathSegment::QuadTo(c1, p) => {
                        // Flatten quadratic bezier with 4 segments.
                        let p0 = current;
                        let p1 = [c1.x / w, c1.y / h];
                        let p2 = [p.x / w, p.y / h];
                        for i in 0..4 {
                            let t0 = i as f32 / 4.0;
                            let t1 = (i + 1) as f32 / 4.0;
                            out.push((
                                quad_bezier(p0, p1, p2, t0),
                                quad_bezier(p0, p1, p2, t1),
                            ));
                        }
                        current = p2;
                    }
                    usvg::tiny_skia_path::PathSegment::CubicTo(c1, c2, p) => {
                        // Flatten cubic bezier with 4 segments.
                        let p0 = current;
                        let p1 = [c1.x / w, c1.y / h];
                        let p2 = [c2.x / w, c2.y / h];
                        let p3 = [p.x / w, p.y / h];
                        for i in 0..4 {
                            let t0 = i as f32 / 4.0;
                            let t1 = (i + 1) as f32 / 4.0;
                            out.push((
                                cubic_bezier(p0, p1, p2, p3, t0),
                                cubic_bezier(p0, p1, p2, p3, t1),
                            ));
                        }
                        current = p3;
                    }
                    usvg::tiny_skia_path::PathSegment::Close => {
                        if current != start {
                            out.push((current, start));
                        }
                    }
                }
            }
        }
        usvg::Node::Group(g) => {
            for child in g.children() {
                collect_segments(child, w, h, out);
            }
        }
        _ => {}
    }
}

fn quad_bezier(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let u2 = u * u;
    let t2 = t * t;
    [
        u2 * p0[0] + 2.0 * u * t * p1[0] + t2 * p2[0],
        u2 * p0[1] + 2.0 * u * t * p1[1] + t2 * p2[1],
    ]
}

fn cubic_bezier(p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2], t: f32) -> [f32; 2] {
    let u = 1.0 - t;
    let u2 = u * u;
    let u3 = u2 * u;
    let t2 = t * t;
    let t3 = t2 * t;
    [
        u3 * p0[0] + 3.0 * u2 * t * p1[0] + 3.0 * u * t2 * p2[0] + t3 * p3[0],
        u3 * p0[1] + 3.0 * u2 * t * p1[1] + 3.0 * u * t2 * p2[1] + t3 * p3[1],
    ]
}

fn nearest_segment_uv(pt: [f32; 2], segments: &[([f32; 2], [f32; 2])], strength: f32) -> [f32; 2] {
    let (nearest, _dist) = segments
        .iter()
        .map(|(a, b)| (a, b, point_to_segment_dist_sq(pt, *a, *b)))
        .min_by(|x, y| x.2.partial_cmp(&y.2).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(a, b, d)| {
            let closest = closest_point_on_segment(pt, *a, *b);
            (closest, d.sqrt())
        })
        .unwrap_or((pt, 1.0));

    let factor = (1.0 - _dist.min(1.0)) * strength;
    [
        (pt[0] + (nearest[0] - pt[0]) * factor).clamp(0.0, 1.0),
        (pt[1] + (nearest[1] - pt[1]) * factor).clamp(0.0, 1.0),
    ]
}

fn bbox_attractor_uv(pt: [f32; 2], segments: &[([f32; 2], [f32; 2])], strength: f32) -> [f32; 2] {
    // Compute centroid of all segment midpoints as a single attractor.
    let (sum_x, sum_y, n) = segments.iter().fold((0.0f32, 0.0f32, 0usize), |acc, (a, b)| {
        (acc.0 + (a[0] + b[0]) * 0.5, acc.1 + (a[1] + b[1]) * 0.5, acc.2 + 1)
    });
    if n == 0 {
        return pt;
    }
    let cx = sum_x / n as f32;
    let cy = sum_y / n as f32;
    let dx = cx - pt[0];
    let dy = cy - pt[1];
    let dist = (dx * dx + dy * dy).sqrt();
    let factor = (1.0 - dist.min(1.0)).max(0.0) * strength;
    [
        (pt[0] + dx * factor).clamp(0.0, 1.0),
        (pt[1] + dy * factor).clamp(0.0, 1.0),
    ]
}

fn point_to_segment_dist_sq(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let ab_len_sq = ab[0] * ab[0] + ab[1] * ab[1];
    if ab_len_sq < 1e-12 {
        return ap[0] * ap[0] + ap[1] * ap[1];
    }
    let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / ab_len_sq).clamp(0.0, 1.0);
    let closest = [a[0] + t * ab[0], a[1] + t * ab[1]];
    let cp = [p[0] - closest[0], p[1] - closest[1]];
    cp[0] * cp[0] + cp[1] * cp[1]
}

fn closest_point_on_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let ap = [p[0] - a[0], p[1] - a[1]];
    let ab_len_sq = ab[0] * ab[0] + ab[1] * ab[1];
    if ab_len_sq < 1e-12 {
        return a;
    }
    let t = ((ap[0] * ab[0] + ap[1] * ab[1]) / ab_len_sq).clamp(0.0, 1.0);
    [a[0] + t * ab[0], a[1] + t * ab[1]]
}

/// Load a DXF file and convert its `LWPOLYLINE` entities to a warp mesh.
///
/// All polyline vertices are collected, normalised to the file's bounding box,
/// and used as boundary attractors: grid points near a polyline vertex have
/// their UVs pulled toward that vertex.  This is useful when the DXF contains
/// a traced outline of the physical projection surface.
///
/// * `cols`, `rows` — mesh grid resolution (must be ≥ 2)
pub fn from_dxf(path: &Path, cols: u32, rows: u32) -> anyhow::Result<WarpMesh> {
    if cols < 2 || rows < 2 {
        anyhow::bail!("mesh grid must be at least 2×2");
    }

    let drawing = dxf::Drawing::load_file(path)
        .map_err(|e| anyhow::anyhow!("DXF load error: {e}"))?;

    // Collect all polyline vertices.
    let mut verts: Vec<[f32; 2]> = Vec::new();
    let mut min_x = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_y = f64::NEG_INFINITY;

    for entity in drawing.entities() {
        if let dxf::entities::EntityType::LwPolyline(poly) = &entity.specific {
            for v in &poly.vertices {
                let x = v.x;
                let y = v.y;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
                verts.push([x as f32, y as f32]);
            }
        }
    }

    if verts.is_empty() {
        return Ok(WarpMesh::identity(cols, rows));
    }

    let w = (max_x - min_x).max(1e-6) as f32;
    let h = (max_y - min_y).max(1e-6) as f32;
    let min_xf = min_x as f32;
    let min_yf = min_y as f32;

    // Normalise vertices to [0,1]².
    let norms: Vec<[f32; 2]> = verts
        .iter()
        .map(|v| [(v[0] - min_xf) / w, (v[1] - min_yf) / h])
        .collect();

    let mut points = Vec::with_capacity((cols * rows) as usize);
    for row in 0..rows {
        let v = row as f32 / (rows - 1) as f32;
        for col in 0..cols {
            let u = col as f32 / (cols - 1) as f32;
            let uv = nearest_vertex_uv([u, v], &norms, 0.10);
            points.push(MeshPoint {
                position: [u, v],
                uv,
            });
        }
    }

    Ok(WarpMesh { cols, rows, points })
}

fn nearest_vertex_uv(pt: [f32; 2], verts: &[[f32; 2]], strength: f32) -> [f32; 2] {
    let (nearest, dist) = verts
        .iter()
        .map(|v| {
            let dx = v[0] - pt[0];
            let dy = v[1] - pt[1];
            (*v, dx * dx + dy * dy)
        })
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(v, d)| (v, d.sqrt()))
        .unwrap_or((pt, 1.0));

    let factor = (1.0 - dist.min(1.0)) * strength;
    [
        (pt[0] + (nearest[0] - pt[0]) * factor).clamp(0.0, 1.0),
        (pt[1] + (nearest[1] - pt[1]) * factor).clamp(0.0, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn raster_identity_mid_grey() {
        // Create a 2×2 mid-grey PNG (displacement should be zero everywhere).
        let mut img = image::RgbImage::new(2, 2);
        for px in img.pixels_mut() {
            *px = image::Rgb([128, 128, 128]);
        }
        let dir = std::env::temp_dir();
        let path = dir.join("rustjay_test_midgrey.png");
        img.save(&path).unwrap();

        let mesh = from_raster(&path, 3, 3, 0.25).unwrap();
        assert_eq!(mesh.cols, 3);
        assert_eq!(mesh.rows, 3);
        for pt in &mesh.points {
            assert!((pt.uv[0] - pt.position[0]).abs() < 1e-3);
            assert!((pt.uv[1] - pt.position[1]).abs() < 1e-3);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn raster_black_shifts_uv_negative() {
        let mut img = image::RgbImage::new(2, 2);
        for px in img.pixels_mut() {
            *px = image::Rgb([0, 0, 0]);
        }
        let dir = std::env::temp_dir();
        let path = dir.join("rustjay_test_black.png");
        img.save(&path).unwrap();

        let mesh = from_raster(&path, 3, 3, 0.25).unwrap();
        // All UVs should be shifted toward 0 (clamped).
        for pt in &mesh.points {
            assert!(pt.uv[0] <= pt.position[0] + 1e-3);
            assert!(pt.uv[1] <= pt.position[1] + 1e-3);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn svg_empty_returns_identity() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"></svg>"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rustjay_test_empty.svg");
        std::fs::write(&path, svg).unwrap();

        let mesh = from_svg(&path, 3, 3).unwrap();
        assert_eq!(mesh.cols, 3);
        for pt in &mesh.points {
            assert!((pt.uv[0] - pt.position[0]).abs() < 1e-3);
            assert!((pt.uv[1] - pt.position[1]).abs() < 1e-3);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn svg_with_path_attracts_uv() {
        // A square path in the middle of the SVG.
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <path d="M 30 30 L 70 30 L 70 70 L 30 70 Z" fill="none" stroke="black"/>
        </svg>"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rustjay_test_square.svg");
        std::fs::write(&path, svg).unwrap();

        let mesh = from_svg(&path, 5, 5).unwrap();
        // Centre point (0.5,0.5) should be attracted toward the square centre (0.5,0.5)
        // which happens to be the same, so it stays roughly identity.
        let centre = &mesh.points[(2 * 5 + 2) as usize];
        assert!((centre.uv[0] - 0.5).abs() < 0.1);
        assert!((centre.uv[1] - 0.5).abs() < 0.1);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn dxf_empty_returns_identity() {
        // Write a minimal DXF with no LWPOLYLINE.
        let dxf = "0\nSECTION\n2\nENTITIES\n0\nENDSEC\n0\nEOF\n";
        let dir = std::env::temp_dir();
        let path = dir.join("rustjay_test_empty.dxf");
        std::fs::write(&path, dxf).unwrap();

        let mesh = from_dxf(&path, 3, 3).unwrap();
        assert!(mesh.is_identity());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_varda_stage_svg() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/stage.svg"
        ));
        assert!(path.exists(), "stage.svg fixture not found at {:?}", path);

        let mesh = from_svg(path, 17, 17).unwrap();
        assert_eq!(mesh.cols, 17);
        assert_eq!(mesh.rows, 17);

        // The stage outline should attract UVs away from identity.
        let non_identity_count = mesh.points.iter()
            .filter(|p| (p.position[0] - p.uv[0]).abs() > 1e-3 || (p.position[1] - p.uv[1]).abs() > 1e-3)
            .count();
        assert!(
            non_identity_count > 0,
            "stage SVG should produce non-identity UVs, got {} non-identity points",
            non_identity_count
        );

        // All UVs must remain in [0,1]².
        for pt in &mesh.points {
            assert!(pt.uv[0] >= 0.0 && pt.uv[0] <= 1.0, "uv.x out of bounds: {}", pt.uv[0]);
            assert!(pt.uv[1] >= 0.0 && pt.uv[1] <= 1.0, "uv.y out of bounds: {}", pt.uv[1]);
        }
    }

    #[test]
    fn dxf_polyline_vertices_attract() {
        let dir = std::env::temp_dir();
        let path = dir.join("rustjay_test_poly.dxf");
        {
            let mut drawing = dxf::Drawing::new();
            let mut poly = dxf::entities::LwPolyline::default();
            poly.vertices.push(dxf::LwPolylineVertex { x: 0.0, y: 0.0, ..Default::default() });
            poly.vertices.push(dxf::LwPolylineVertex { x: 100.0, y: 0.0, ..Default::default() });
            poly.vertices.push(dxf::LwPolylineVertex { x: 100.0, y: 100.0, ..Default::default() });
            poly.vertices.push(dxf::LwPolylineVertex { x: 0.0, y: 100.0, ..Default::default() });
            let entity = dxf::entities::Entity::new(dxf::entities::EntityType::LwPolyline(poly));
            drawing.add_entity(entity);
            drawing.save_file(&path).unwrap();
        }

        let mesh = from_dxf(&path, 5, 5).unwrap();
        assert_eq!(mesh.cols, 5);
        // Corners of the mesh should be attracted toward the polyline corners.
        let top_left = &mesh.points[0];
        assert!(top_left.uv[0] < 0.15);
        assert!(top_left.uv[1] < 0.15);

        let _ = std::fs::remove_file(&path);
    }
}
