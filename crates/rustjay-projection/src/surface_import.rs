//! Surface import — contour-detection pipeline (REQ-06).
//!
//! Ports Varda's `surface/{detect,import}.rs` contour pipeline into the
//! projection crate. Detects geometric contours from SVG, DXF, and raster
//! (PNG/JPG) sources, normalises them to `[0..1]`, and converts them into
//! [`Surface`] polygons and [`WarpMesh`]es (via ear-clipping triangulation) so
//! that imported geometry can be assigned to projector outputs.
//!
//! All functions are gated behind the `surface-import` feature.
//!
//! ## Live-safety
//! Raster detection delegates to third-party image-processing routines
//! (`imageproc` Canny / blur). Those are wrapped in
//! [`std::panic::catch_unwind`] so a malformed input frame can never crash the
//! host render thread (REQ-06.3).

use crate::warp::{MeshPoint, WarpMesh};
use std::io::Cursor;
use std::path::Path;

use usvg::tiny_skia_path;

// ── Public types ───────────────────────────────────────────────────────────

/// A single detected contour with computed geometry metadata.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectedContour {
    /// Polygon vertices in normalized `[0..1]` coordinates.
    pub vertices: Vec<[f32; 2]>,
    /// Polygon area in normalized coordinates (shoelace).
    pub area: f32,
    /// Whether the contour approximates a circle.
    pub is_circular: bool,
    /// If circular, the fitted `(center, radius)` in normalized coords.
    pub circle_fit: Option<([f32; 2], f32)>,
    /// Auto-generated name based on position (e.g. "top-left-1").
    pub suggested_name: String,
}

impl DetectedContour {
    /// Convert this contour to a [`Surface`] polygon.
    pub fn to_surface(&self, index: usize) -> Surface {
        Surface {
            name: self.suggested_name.clone(),
            vertices: self.vertices.clone(),
            index,
            is_circular: self.is_circular,
        }
    }

    /// Triangulate this contour into a [`WarpMesh`] (see [`contour_to_warp_mesh`]).
    pub fn to_warp_mesh(&self) -> WarpMesh {
        contour_to_warp_mesh(&self.vertices)
    }
}

/// A polygon surface produced by import: an ordered ring of normalized
/// `[0..1]` vertices that content can be routed to.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Surface {
    /// Display name (suggested from contour position).
    pub name: String,
    /// Ordered polygon vertices in normalized canvas coordinates `[0..1]`.
    pub vertices: Vec<[f32; 2]>,
    /// Index within the source detection result (sorted by area, descending).
    pub index: usize,
    /// Whether the source contour approximated a circle.
    pub is_circular: bool,
}

impl Surface {
    /// Triangulate this surface polygon into a [`WarpMesh`].
    pub fn to_warp_mesh(&self) -> WarpMesh {
        contour_to_warp_mesh(&self.vertices)
    }
}

/// Method used to produce the binary image for raster contour detection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum DetectionMethod {
    /// Canny edge detector (good for line-art / SVG-like inputs).
    Canny,
    /// Simple threshold (good for camera feeds with controlled lighting).
    #[default]
    Threshold,
}

/// Post-processing hull mode applied after simplification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum HullMode {
    /// Keep the simplified polygon as-is.
    #[default]
    None,
    /// Replace with convex hull (removes concavities).
    ConvexHull,
}

/// Parameters controlling the raster contour-detection pipeline.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct DetectionParams {
    /// Canny edge detector low threshold.
    pub canny_low: u8,
    /// Canny edge detector high threshold.
    pub canny_high: u8,
    /// Gaussian blur radius applied before edge detection.
    pub blur_radius: u32,
    /// Douglas-Peucker simplification tolerance (normalized).
    pub simplify_tolerance: f32,
    /// Minimum polygon area to keep (normalized).
    pub min_area: f32,
    /// Minimum vertex count after simplification.
    pub min_vertices: usize,
    /// Detection method: Canny or Threshold.
    pub detection_method: DetectionMethod,
    /// Threshold value for binary image creation (0-255).
    pub threshold: u8,
    /// Invert the threshold (foreground becomes background).
    pub invert: bool,
    /// Morphological close kernel radius (0 = disabled).
    pub morph_size: u32,
    /// Post-processing hull mode.
    pub hull_mode: HullMode,
}

impl Default for DetectionParams {
    fn default() -> Self {
        Self {
            canny_low: 50,
            canny_high: 150,
            blur_radius: 1,
            simplify_tolerance: 0.005,
            min_area: 0.001,
            min_vertices: 3,
            detection_method: DetectionMethod::default(),
            threshold: 127,
            invert: false,
            morph_size: 0,
            hull_mode: HullMode::default(),
        }
    }
}

/// Result of running contour detection on a source file/image.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DetectionResult {
    /// Detected contours, sorted by area descending.
    pub contours: Vec<DetectedContour>,
    /// Width of the source in pixels (or normalized bbox width for vector sources).
    pub source_width: u32,
    /// Height of the source in pixels (or normalized bbox height for vector sources).
    pub source_height: u32,
}

impl DetectionResult {
    /// The largest contour (first, since results are sorted by area descending).
    pub fn largest(&self) -> Option<&DetectedContour> {
        self.contours.first()
    }

    /// Convert all detected contours into [`Surface`] polygons.
    pub fn to_surfaces(&self) -> Vec<Surface> {
        self.contours
            .iter()
            .enumerate()
            .map(|(i, c)| c.to_surface(i))
            .collect()
    }
}

// ── Error type ─────────────────────────────────────────────────────────────

/// Errors produced by the surface-import pipeline.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// Failed to load/decode a raster image.
    #[error("Failed to load image: {0}")]
    ImageLoad(String),
    /// Failed to parse an SVG document.
    #[error("Failed to parse SVG: {0}")]
    SvgParse(String),
    /// Failed to parse a DXF drawing.
    #[error("Failed to parse DXF: {0}")]
    DxfParse(String),
    /// File extension is not a recognised format.
    #[error("Unsupported file format: {0}")]
    UnsupportedFormat(String),
    /// No usable contours were found.
    #[error("No contours detected")]
    NoContours,
    /// The detection pipeline panicked and was recovered (live-safety).
    #[error("Detection panicked: {0}")]
    InternalPanic(String),
}

// ── File path dispatch ─────────────────────────────────────────────────────

/// Detect surfaces from a file, dispatching by extension.
pub fn detect_from_file(
    path: &Path,
    params: &DetectionParams,
) -> Result<DetectionResult, ImportError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let data = std::fs::read(path).map_err(|e| ImportError::ImageLoad(e.to_string()))?;
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "bmp" | "gif" | "tiff" | "tif" | "webp" => {
            detect_from_image(&data, params)
        }
        "svg" => detect_from_svg(&data),
        "dxf" => detect_from_dxf(&data),
        other => Err(ImportError::UnsupportedFormat(other.to_string())),
    }
}

// ── Raster image import ────────────────────────────────────────────────────

/// Detect surfaces from a raster image (PNG/JPG bytes).
///
/// Pipeline: grayscale → Gaussian blur → Canny/Threshold → morphological close
/// (optional) → border following → Douglas-Peucker simplification. The
/// `imageproc`-backed inner stage is wrapped in `catch_unwind` for live-safety.
pub fn detect_from_image(
    image_data: &[u8],
    params: &DetectionParams,
) -> Result<DetectionResult, ImportError> {
    let img =
        image::load_from_memory(image_data).map_err(|e| ImportError::ImageLoad(e.to_string()))?;
    let gray = img.to_luma8();
    run_detect_contours(&gray, params)
}

/// Detect surfaces from raw RGBA pixel data (e.g. a camera frame).
///
/// Converts RGBA → grayscale directly, avoiding a PNG round-trip. Detection is
/// wrapped in `catch_unwind` so third-party panics never reach the caller — this
/// is critical for live-performance safety (REQ-06.3).
pub fn detect_from_rgba(
    rgba: &[u8],
    w: u32,
    h: u32,
    params: &DetectionParams,
) -> Result<DetectionResult, ImportError> {
    let expected = (w as usize) * (h as usize) * 4;
    if rgba.len() < expected {
        return Err(ImportError::ImageLoad(format!(
            "RGBA buffer too small: expected {} bytes, got {}",
            expected,
            rgba.len()
        )));
    }
    let gray_pixels: Vec<u8> = rgba
        .chunks_exact(4)
        .map(|px| {
            let r = px[0] as f32;
            let g = px[1] as f32;
            let b = px[2] as f32;
            (0.299 * r + 0.587 * g + 0.114 * b) as u8
        })
        .collect();
    let gray = image::GrayImage::from_raw(w, h, gray_pixels).ok_or_else(|| {
        ImportError::ImageLoad("Failed to create grayscale image from RGBA".into())
    })?;
    run_detect_contours(&gray, params)
}

/// Run `detect_contours` inside `catch_unwind` and map empty/panic to errors.
fn run_detect_contours(
    gray: &image::GrayImage,
    params: &DetectionParams,
) -> Result<DetectionResult, ImportError> {
    let params_clone = params.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        detect_contours(gray, &params_clone)
    }));
    match result {
        Ok(result) => {
            if result.contours.is_empty() {
                Err(ImportError::NoContours)
            } else {
                Ok(result)
            }
        }
        Err(panic_info) => {
            let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = panic_info.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic in detection pipeline".to_string()
            };
            log::debug!("Detection pipeline panicked (recovered): {msg}");
            Err(ImportError::InternalPanic(msg))
        }
    }
}

/// Run the full contour-detection pipeline on a grayscale image.
pub fn detect_contours(img: &image::GrayImage, params: &DetectionParams) -> DetectionResult {
    let (w, h) = img.dimensions();
    let wf = w as f32;
    let hf = h as f32;

    // Pad image by 2px on each side to work around imageproc Canny boundary bugs.
    let pad = 2u32;
    let padded_w = w + pad * 2;
    let padded_h = h + pad * 2;
    let mut padded = image::GrayImage::new(padded_w, padded_h);
    for y in 0..h {
        for x in 0..w {
            padded.put_pixel(x + pad, y + pad, *img.get_pixel(x, y));
        }
    }

    // 1. Gaussian blur
    let sigma = (params.blur_radius as f32).max(0.1);
    let blurred = if params.blur_radius == 0 {
        padded.clone()
    } else {
        imageproc::filter::gaussian_blur_f32(&padded, sigma)
    };

    // 2. Binary image
    let binary = match params.detection_method {
        DetectionMethod::Canny => {
            let canny_lo = f32::from(params.canny_low);
            let canny_hi = f32::from(params.canny_high).max(canny_lo);
            imageproc::edges::canny(&blurred, canny_lo, canny_hi)
        }
        DetectionMethod::Threshold => threshold_binary(&blurred, params.threshold, params.invert),
    };

    // 3. Optional morphological close
    let cleaned = if params.morph_size > 0 {
        morphological_close(&binary, params.morph_size)
    } else {
        binary
    };

    // 4. Border following
    let raw_contours = follow_borders(&cleaned);

    // 5. Process each contour
    let pad_f = pad as f32;
    let mut contours: Vec<DetectedContour> = Vec::new();
    for (idx, raw) in raw_contours.iter().enumerate() {
        let points: Vec<[f32; 2]> = raw
            .iter()
            .map(|&(x, y)| [(x as f32 - pad_f).max(0.0), (y as f32 - pad_f).max(0.0)])
            .collect();

        let pixel_tolerance = params.simplify_tolerance * wf.max(hf);
        let simplified = douglas_peucker(&points, pixel_tolerance);

        let final_shape = match params.hull_mode {
            HullMode::None => simplified,
            HullMode::ConvexHull => convex_hull(&simplified),
        };

        if final_shape.len() < params.min_vertices {
            continue;
        }

        let pixel_area = shoelace_area(&final_shape);
        let norm_area = pixel_area / (wf * hf);
        if norm_area < params.min_area {
            continue;
        }

        let vertices: Vec<[f32; 2]> = final_shape
            .iter()
            .map(|p| [(p[0] / wf).clamp(0.0, 1.0), (p[1] / hf).clamp(0.0, 1.0)])
            .collect();

        let circle_fit = check_circularity(&vertices, norm_area);
        let is_circular = circle_fit.is_some();

        let cx: f32 = vertices.iter().map(|v| v[0]).sum::<f32>() / vertices.len() as f32;
        let cy: f32 = vertices.iter().map(|v| v[1]).sum::<f32>() / vertices.len() as f32;
        let suggested_name = suggest_name([cx, cy], idx);

        contours.push(DetectedContour {
            vertices,
            area: norm_area,
            is_circular,
            circle_fit,
            suggested_name,
        });
    }

    contours.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    DetectionResult {
        contours,
        source_width: w,
        source_height: h,
    }
}

// ── SVG import ─────────────────────────────────────────────────────────────

/// Detect surfaces from SVG data (extracts geometric paths directly).
pub fn detect_from_svg(svg_data: &[u8]) -> Result<DetectionResult, ImportError> {
    let tree = usvg::Tree::from_data(svg_data, &usvg::Options::default())
        .map_err(|e| ImportError::SvgParse(e.to_string()))?;

    let mut polylines: Vec<Vec<[f32; 2]>> = Vec::new();
    walk_svg_group(tree.root(), &mut polylines);

    if polylines.is_empty() {
        return Err(ImportError::NoContours);
    }

    let (mut min_x, mut min_y) = (f32::MAX, f32::MAX);
    let (mut max_x, mut max_y) = (f32::MIN, f32::MIN);
    for poly in &polylines {
        for pt in poly {
            min_x = min_x.min(pt[0]);
            min_y = min_y.min(pt[1]);
            max_x = max_x.max(pt[0]);
            max_y = max_y.max(pt[1]);
        }
    }
    let width = (max_x - min_x).max(1e-6);
    let height = (max_y - min_y).max(1e-6);

    let mut contours = Vec::new();
    for (i, poly) in polylines.iter().enumerate() {
        let normalized: Vec<[f32; 2]> = poly
            .iter()
            .map(|pt| [(pt[0] - min_x) / width, (pt[1] - min_y) / height])
            .collect();
        if normalized.len() < 3 {
            continue;
        }
        let area = shoelace_area(&normalized);
        if area < 0.001 {
            continue;
        }
        let circle_fit = check_circularity(&normalized, area);
        let is_circular = circle_fit.is_some();
        let cx: f32 = normalized.iter().map(|v| v[0]).sum::<f32>() / normalized.len() as f32;
        let cy: f32 = normalized.iter().map(|v| v[1]).sum::<f32>() / normalized.len() as f32;
        let suggested_name = suggest_name([cx, cy], i);
        contours.push(DetectedContour {
            vertices: normalized,
            area,
            is_circular,
            circle_fit,
            suggested_name,
        });
    }

    contours.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if contours.is_empty() {
        return Err(ImportError::NoContours);
    }
    Ok(DetectionResult {
        contours,
        source_width: width as u32,
        source_height: height as u32,
    })
}

/// Recursively walk a usvg Group, extracting polylines from Path nodes.
fn walk_svg_group(group: &usvg::Group, out: &mut Vec<Vec<[f32; 2]>>) {
    for node in group.children() {
        match node {
            usvg::Node::Group(g) => walk_svg_group(g, out),
            usvg::Node::Path(p) => {
                let polyline = flatten_svg_path(p.data());
                if polyline.len() >= 3 {
                    out.push(polyline);
                }
            }
            _ => {}
        }
    }
}

/// Flatten a `tiny_skia_path::Path` into a polyline by sampling curve segments.
fn flatten_svg_path(path: &tiny_skia_path::Path) -> Vec<[f32; 2]> {
    let mut points: Vec<[f32; 2]> = Vec::new();
    let mut last = [0.0f32; 2];

    for seg in path.segments() {
        match seg {
            tiny_skia_path::PathSegment::MoveTo(pt) => {
                last = [pt.x, pt.y];
                points.push(last);
            }
            tiny_skia_path::PathSegment::LineTo(pt) => {
                last = [pt.x, pt.y];
                points.push(last);
            }
            tiny_skia_path::PathSegment::QuadTo(ctrl, pt) => {
                const STEPS: usize = 8;
                for i in 1..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    let inv = 1.0 - t;
                    let x = inv * inv * last[0] + 2.0 * inv * t * ctrl.x + t * t * pt.x;
                    let y = inv * inv * last[1] + 2.0 * inv * t * ctrl.y + t * t * pt.y;
                    points.push([x, y]);
                }
                last = [pt.x, pt.y];
            }
            tiny_skia_path::PathSegment::CubicTo(c1, c2, pt) => {
                const STEPS: usize = 12;
                for i in 1..=STEPS {
                    let t = i as f32 / STEPS as f32;
                    let inv = 1.0 - t;
                    let x = inv * inv * inv * last[0]
                        + 3.0 * inv * inv * t * c1.x
                        + 3.0 * inv * t * t * c2.x
                        + t * t * t * pt.x;
                    let y = inv * inv * inv * last[1]
                        + 3.0 * inv * inv * t * c1.y
                        + 3.0 * inv * t * t * c2.y
                        + t * t * t * pt.y;
                    points.push([x, y]);
                }
                last = [pt.x, pt.y];
            }
            tiny_skia_path::PathSegment::Close => {
                if let Some(&first) = points.first() {
                    if (last[0] - first[0]).abs() > 1e-4 || (last[1] - first[1]).abs() > 1e-4 {
                        points.push(first);
                    }
                }
            }
        }
    }
    points
}

// ── DXF import ─────────────────────────────────────────────────────────────

const DXF_MIN_AREA: f32 = 0.001;
const ARC_SEGMENTS: usize = 32;
const CLOSE_TOLERANCE: f64 = 1e-4;

/// Detect surfaces from DXF data (LWPOLYLINE / LINE / CIRCLE / ARC / ELLIPSE).
pub fn detect_from_dxf(dxf_data: &[u8]) -> Result<DetectionResult, ImportError> {
    let mut cursor = Cursor::new(dxf_data);
    let drawing =
        dxf::Drawing::load(&mut cursor).map_err(|e| ImportError::DxfParse(e.to_string()))?;

    let mut polylines: Vec<(Vec<[f64; 2]>, bool)> = Vec::new(); // (points, is_circular)

    for entity in drawing.entities() {
        match &entity.specific {
            dxf::entities::EntityType::Line(line) => {
                polylines.push((vec![[line.p1.x, line.p1.y], [line.p2.x, line.p2.y]], false));
            }
            dxf::entities::EntityType::LwPolyline(poly) => {
                let pts: Vec<[f64; 2]> = poly.vertices.iter().map(|v| [v.x, v.y]).collect();
                if pts.len() >= 2 {
                    let mut pts = pts;
                    if poly.is_closed() || close_enough(&pts) {
                        close_polyline(&mut pts);
                    }
                    polylines.push((pts, false));
                }
            }
            dxf::entities::EntityType::Circle(circle) => {
                let pts = approximate_circle(circle.center.x, circle.center.y, circle.radius);
                polylines.push((pts, true));
            }
            dxf::entities::EntityType::Arc(arc) => {
                let pts = approximate_arc(
                    arc.center.x,
                    arc.center.y,
                    arc.radius,
                    arc.start_angle,
                    arc.end_angle,
                );
                polylines.push((pts, false));
            }
            dxf::entities::EntityType::Ellipse(ellipse) => {
                let pts = approximate_ellipse(
                    ellipse.center.x,
                    ellipse.center.y,
                    ellipse.major_axis.x,
                    ellipse.major_axis.y,
                    ellipse.minor_axis_ratio,
                );
                polylines.push((pts, true));
            }
            _ => {}
        }
    }

    if polylines.is_empty() {
        return Err(ImportError::NoContours);
    }

    let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
    let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
    for (pts, _) in &polylines {
        for pt in pts {
            min_x = min_x.min(pt[0]);
            min_y = min_y.min(pt[1]);
            max_x = max_x.max(pt[0]);
            max_y = max_y.max(pt[1]);
        }
    }
    let width = (max_x - min_x).max(1e-10);
    let height = (max_y - min_y).max(1e-10);

    let mut contours = Vec::new();
    for (i, (pts, is_circular_hint)) in polylines.iter().enumerate() {
        let normalized: Vec<[f32; 2]> = pts
            .iter()
            .map(|pt| {
                [
                    ((pt[0] - min_x) / width) as f32,
                    ((pt[1] - min_y) / height) as f32,
                ]
            })
            .collect();
        if normalized.len() < 3 {
            continue;
        }
        let area = shoelace_area(&normalized);
        if area < DXF_MIN_AREA {
            continue;
        }
        let circle_fit = if *is_circular_hint {
            check_circularity(&normalized, area)
        } else {
            None
        };
        let is_circular = circle_fit.is_some() || *is_circular_hint;
        let cx: f32 = normalized.iter().map(|v| v[0]).sum::<f32>() / normalized.len() as f32;
        let cy: f32 = normalized.iter().map(|v| v[1]).sum::<f32>() / normalized.len() as f32;
        let suggested_name = suggest_name([cx, cy], i);
        contours.push(DetectedContour {
            vertices: normalized,
            area,
            is_circular,
            circle_fit,
            suggested_name,
        });
    }

    contours.sort_by(|a, b| {
        b.area
            .partial_cmp(&a.area)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if contours.is_empty() {
        return Err(ImportError::NoContours);
    }
    Ok(DetectionResult {
        contours,
        source_width: width as u32,
        source_height: height as u32,
    })
}

fn close_enough(pts: &[[f64; 2]]) -> bool {
    if pts.len() < 2 {
        return false;
    }
    let first = pts[0];
    let last = pts[pts.len() - 1];
    (first[0] - last[0]).abs() < CLOSE_TOLERANCE && (first[1] - last[1]).abs() < CLOSE_TOLERANCE
}

fn close_polyline(pts: &mut Vec<[f64; 2]>) {
    if pts.len() >= 2 && !close_enough(pts) {
        pts.push(pts[0]);
    }
}

fn approximate_circle(cx: f64, cy: f64, r: f64) -> Vec<[f64; 2]> {
    let tau = std::f64::consts::TAU;
    (0..ARC_SEGMENTS)
        .map(|i| {
            let angle = tau * i as f64 / ARC_SEGMENTS as f64;
            [cx + r * angle.cos(), cy + r * angle.sin()]
        })
        .collect()
}

fn approximate_arc(cx: f64, cy: f64, r: f64, start_deg: f64, end_deg: f64) -> Vec<[f64; 2]> {
    let start = start_deg.to_radians();
    let mut end = end_deg.to_radians();
    if end <= start {
        end += std::f64::consts::TAU;
    }
    let steps = ARC_SEGMENTS;
    (0..=steps)
        .map(|i| {
            let angle = start + (end - start) * i as f64 / steps as f64;
            [cx + r * angle.cos(), cy + r * angle.sin()]
        })
        .collect()
}

fn approximate_ellipse(
    cx: f64,
    cy: f64,
    major_x: f64,
    major_y: f64,
    minor_ratio: f64,
) -> Vec<[f64; 2]> {
    let major_len = (major_x * major_x + major_y * major_y).sqrt();
    let minor_len = major_len * minor_ratio;
    let rotation = major_y.atan2(major_x);
    let tau = std::f64::consts::TAU;
    (0..ARC_SEGMENTS)
        .map(|i| {
            let angle = tau * i as f64 / ARC_SEGMENTS as f64;
            let ex = major_len * angle.cos();
            let ey = minor_len * angle.sin();
            let rx = ex * rotation.cos() - ey * rotation.sin();
            let ry = ex * rotation.sin() + ey * rotation.cos();
            [cx + rx, cy + ry]
        })
        .collect()
}

// ── Contour → WarpMesh triangulation (REQ-06.4) ─────────────────────────────

/// Triangulate a normalized contour polygon into a [`WarpMesh`].
///
/// Uses ear-clipping (`earcutr`) over the contour vertices. The resulting
/// `WarpMesh` carries the triangle list flattened into `points` (3 entries per
/// triangle) with `cols = 3`, `rows = triangle_count`. Each `MeshPoint`'s
/// `position` is the polygon vertex (output space) and its `uv` is the same
/// normalized coordinate (source space), so the mesh samples the source 1:1 over
/// the contour's footprint. All UVs are clamped to `[0,1]²`.
///
/// Degenerate inputs (< 3 vertices, or triangulation failure) fall back to a
/// 2×2 identity mesh.
pub fn contour_to_warp_mesh(vertices: &[[f32; 2]]) -> WarpMesh {
    if vertices.len() < 3 {
        return WarpMesh::identity(2, 2);
    }

    // Flatten to the [x0, y0, x1, y1, ...] layout earcutr expects.
    let mut flat: Vec<f64> = Vec::with_capacity(vertices.len() * 2);
    for v in vertices {
        flat.push(v[0] as f64);
        flat.push(v[1] as f64);
    }

    let indices = match earcutr::earcut(&flat, &[], 2) {
        Ok(idx) if idx.len() >= 3 => idx,
        _ => return WarpMesh::identity(2, 2),
    };

    let tri_count = indices.len() / 3;
    let mut points = Vec::with_capacity(tri_count * 3);
    for &i in &indices {
        let v = vertices[i];
        let p = [v[0].clamp(0.0, 1.0), v[1].clamp(0.0, 1.0)];
        points.push(MeshPoint { position: p, uv: p });
    }

    WarpMesh {
        cols: 3,
        rows: tri_count as u32,
        points,
    }
}

// ── Geometry helpers (ported from Varda detect.rs) ──────────────────────────

/// Compute the (unsigned) area of a polygon using the shoelace formula.
pub fn shoelace_area(vertices: &[[f32; 2]]) -> f32 {
    let n = vertices.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    for i in 0..n {
        let j = (i + 1) % n;
        sum += vertices[i][0] * vertices[j][1];
        sum -= vertices[j][0] * vertices[i][1];
    }
    sum.abs() * 0.5
}

/// Douglas-Peucker polyline simplification.
pub fn douglas_peucker(points: &[[f32; 2]], tolerance: f32) -> Vec<[f32; 2]> {
    if points.len() <= 2 {
        return points.to_vec();
    }

    let first = points[0];
    let last = points[points.len() - 1];
    let mut max_dist = 0.0f32;
    let mut max_idx = 0;

    for (i, p) in points.iter().enumerate().skip(1).take(points.len() - 2) {
        let d = perpendicular_distance(p, &first, &last);
        if d > max_dist {
            max_dist = d;
            max_idx = i;
        }
    }

    if max_dist > tolerance {
        let mut left = douglas_peucker(&points[..=max_idx], tolerance);
        let right = douglas_peucker(&points[max_idx..], tolerance);
        left.pop();
        left.extend(right);
        left
    } else {
        vec![first, last]
    }
}

fn perpendicular_distance(p: &[f32; 2], a: &[f32; 2], b: &[f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let len_sq = dx * dx + dy * dy;
    if len_sq < f32::EPSILON {
        let ex = p[0] - a[0];
        let ey = p[1] - a[1];
        return (ex * ex + ey * ey).sqrt();
    }
    ((p[0] - a[0]) * dy - (p[1] - a[1]) * dx).abs() / len_sq.sqrt()
}

fn threshold_binary(img: &image::GrayImage, threshold: u8, invert: bool) -> image::GrayImage {
    let (w, h) = img.dimensions();
    let mut out = image::GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y).0[0];
            let is_fg = if invert {
                px < threshold
            } else {
                px >= threshold
            };
            out.put_pixel(x, y, image::Luma([if is_fg { 255 } else { 0 }]));
        }
    }
    out
}

fn morphological_close(img: &image::GrayImage, kernel_size: u32) -> image::GrayImage {
    if kernel_size == 0 {
        return img.clone();
    }
    let radius = kernel_size as i32;
    let dilated = morph_dilate(img, radius);
    morph_erode(&dilated, radius)
}

fn morph_dilate(img: &image::GrayImage, radius: i32) -> image::GrayImage {
    let (w, h) = img.dimensions();
    let mut out = image::GrayImage::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut max_val = 0u8;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h {
                        max_val = max_val.max(img.get_pixel(nx as u32, ny as u32).0[0]);
                    }
                }
            }
            out.put_pixel(x as u32, y as u32, image::Luma([max_val]));
        }
    }
    out
}

fn morph_erode(img: &image::GrayImage, radius: i32) -> image::GrayImage {
    let (w, h) = img.dimensions();
    let mut out = image::GrayImage::new(w, h);
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut min_val = 255u8;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h {
                        min_val = min_val.min(img.get_pixel(nx as u32, ny as u32).0[0]);
                    }
                }
            }
            out.put_pixel(x as u32, y as u32, image::Luma([min_val]));
        }
    }
    out
}

fn is_border_pixel(img: &image::GrayImage, x: u32, y: u32, w: u32, h: u32) -> bool {
    if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
        return true;
    }
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            if dx == 0 && dy == 0 {
                continue;
            }
            let nx = (x as i32 + dx) as u32;
            let ny = (y as i32 + dy) as u32;
            if img.get_pixel(nx, ny).0[0] == 0 {
                return true;
            }
        }
    }
    false
}

/// Follow borders using Moore neighbor tracing to extract ordered contour points.
fn follow_borders(binary: &image::GrayImage) -> Vec<Vec<(u32, u32)>> {
    const DX: [i32; 8] = [1, 1, 0, -1, -1, -1, 0, 1];
    const DY: [i32; 8] = [0, 1, 1, 1, 0, -1, -1, -1];

    let (w, h) = binary.dimensions();
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let mut visited = vec![false; (w * h) as usize];
    let mut contours = Vec::new();
    let max_steps = (w * h) as usize;

    for y in 0..h {
        for x in 0..w {
            if binary.get_pixel(x, y).0[0] == 0 {
                continue;
            }
            if !is_border_pixel(binary, x, y, w, h) {
                continue;
            }
            let idx = (y * w + x) as usize;
            if visited[idx] {
                continue;
            }
            let contour =
                trace_single_border(binary, x, y, w, h, &DX, &DY, &mut visited, max_steps);
            if !contour.is_empty() {
                contours.push(contour);
            }
        }
    }
    contours
}

#[allow(clippy::too_many_arguments)]
fn trace_single_border(
    binary: &image::GrayImage,
    sx: u32,
    sy: u32,
    w: u32,
    h: u32,
    dx: &[i32; 8],
    dy: &[i32; 8],
    visited: &mut [bool],
    max_steps: usize,
) -> Vec<(u32, u32)> {
    let mut contour = Vec::new();
    visited[(sy * w + sx) as usize] = true;
    contour.push((sx, sy));

    let mut entry_dir: usize = 4;
    for d in 0..8 {
        let nx = sx as i32 + dx[d];
        let ny = sy as i32 + dy[d];
        if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
            entry_dir = (d + 4) % 8;
            break;
        }
        if binary.get_pixel(nx as u32, ny as u32).0[0] == 0 {
            entry_dir = (d + 4) % 8;
            break;
        }
    }

    let mut cx = sx;
    let mut cy = sy;

    for _ in 0..max_steps {
        let start_search = (entry_dir + 1) % 8;
        let mut found = false;

        for i in 0..8 {
            let d = (start_search + i) % 8;
            let nx = cx as i32 + dx[d];
            let ny = cy as i32 + dy[d];

            if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                continue;
            }
            let nxu = nx as u32;
            let nyu = ny as u32;
            if binary.get_pixel(nxu, nyu).0[0] == 0 {
                continue;
            }
            if !is_border_pixel(binary, nxu, nyu, w, h) {
                continue;
            }
            if nxu == sx && nyu == sy {
                return contour;
            }
            let nidx = (nyu * w + nxu) as usize;
            if !visited[nidx] {
                visited[nidx] = true;
                contour.push((nxu, nyu));
            }
            entry_dir = (d + 4) % 8;
            cx = nxu;
            cy = nyu;
            found = true;
            break;
        }

        if !found {
            break;
        }
    }

    contour
}

/// Convex hull via Andrew's monotone chain.
fn convex_hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.len() <= 3 {
        return points.to_vec();
    }

    let mut sorted: Vec<[f32; 2]> = points.to_vec();
    sorted.sort_by(|a, b| {
        a[0].partial_cmp(&b[0])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a[1].partial_cmp(&b[1]).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut lower = Vec::new();
    for &p in &sorted {
        while lower.len() >= 2 && cross(&lower[lower.len() - 2], &lower[lower.len() - 1], &p) <= 0.0
        {
            lower.pop();
        }
        lower.push(p);
    }

    let mut upper = Vec::new();
    for &p in sorted.iter().rev() {
        while upper.len() >= 2 && cross(&upper[upper.len() - 2], &upper[upper.len() - 1], &p) <= 0.0
        {
            upper.pop();
        }
        upper.push(p);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn cross(o: &[f32; 2], a: &[f32; 2], b: &[f32; 2]) -> f32 {
    (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
}

/// Check if a polygon is approximately circular; if so, return `(center, radius)`.
pub fn check_circularity(vertices: &[[f32; 2]], area: f32) -> Option<([f32; 2], f32)> {
    if vertices.len() < 6 {
        return None;
    }

    let n = vertices.len() as f32;
    let cx = vertices.iter().map(|v| v[0]).sum::<f32>() / n;
    let cy = vertices.iter().map(|v| v[1]).sum::<f32>() / n;

    let max_r = vertices
        .iter()
        .map(|v| {
            let dx = v[0] - cx;
            let dy = v[1] - cy;
            (dx * dx + dy * dy).sqrt()
        })
        .fold(0.0f32, f32::max);

    if max_r < f32::EPSILON {
        return None;
    }

    let circle_area = std::f32::consts::PI * max_r * max_r;
    let ratio = area / circle_area;

    if ratio > 0.85 {
        Some(([cx, cy], max_r))
    } else {
        None
    }
}

/// Suggest a display name for a contour based on its center position.
pub fn suggest_name(center: [f32; 2], index: usize) -> String {
    let vertical = if center[1] < 0.33 {
        "top"
    } else if center[1] > 0.66 {
        "bottom"
    } else {
        "center"
    };

    let horizontal = if center[0] < 0.33 {
        "left"
    } else if center[0] > 0.66 {
        "right"
    } else {
        "center"
    };

    let position = if vertical == "center" && horizontal == "center" {
        "center".to_string()
    } else if vertical == "center" {
        horizontal.to_string()
    } else if horizontal == "center" {
        vertical.to_string()
    } else {
        format!("{vertical}-{horizontal}")
    };

    format!("{position}-{}", index + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Geometry / CPU unit tests ──────────────────────────────────────

    #[test]
    fn shoelace_area_unit_square() {
        let verts = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!((shoelace_area(&verts) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn shoelace_area_triangle() {
        let verts = [[0.0, 0.0], [1.0, 0.0], [0.5, 1.0]];
        assert!((shoelace_area(&verts) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn shoelace_area_degenerate() {
        assert_eq!(shoelace_area(&[]), 0.0);
        assert_eq!(shoelace_area(&[[0.0, 0.0]]), 0.0);
        assert_eq!(shoelace_area(&[[0.0, 0.0], [1.0, 1.0]]), 0.0);
    }

    #[test]
    fn douglas_peucker_preserves_simple_line() {
        let pts = vec![[0.0, 0.0], [1.0, 0.0]];
        assert_eq!(douglas_peucker(&pts, 0.1).len(), 2);
    }

    #[test]
    fn douglas_peucker_simplifies_collinear() {
        let pts = vec![[0.0, 0.0], [0.5, 0.0], [1.0, 0.0]];
        assert_eq!(douglas_peucker(&pts, 0.01).len(), 2);
    }

    #[test]
    fn douglas_peucker_keeps_non_collinear() {
        let pts = vec![[0.0, 0.0], [0.5, 1.0], [1.0, 0.0]];
        assert_eq!(douglas_peucker(&pts, 0.01).len(), 3);
    }

    #[test]
    fn douglas_peucker_simplifies_noisy_square() {
        // A square traced with many near-collinear points should reduce to ~5
        // vertices (4 corners + closing point).
        let mut pts = Vec::new();
        for i in 0..=10 {
            pts.push([i as f32 / 10.0, 0.0]);
        }
        for i in 0..=10 {
            pts.push([1.0, i as f32 / 10.0]);
        }
        for i in 0..=10 {
            pts.push([1.0 - i as f32 / 10.0, 1.0]);
        }
        for i in 0..=10 {
            pts.push([0.0, 1.0 - i as f32 / 10.0]);
        }
        let simplified = douglas_peucker(&pts, 0.05);
        assert!(
            simplified.len() <= 6,
            "expected <=6 vertices after simplification, got {}",
            simplified.len()
        );
    }

    #[test]
    fn check_circularity_detects_circle() {
        let n = 32;
        let (r, cx, cy) = (0.3f32, 0.5f32, 0.5f32);
        let verts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let angle = 2.0 * std::f32::consts::PI * i as f32 / n as f32;
                [cx + r * angle.cos(), cy + r * angle.sin()]
            })
            .collect();
        let area = shoelace_area(&verts);
        let (center, radius) = check_circularity(&verts, area).expect("circle expected");
        assert!((center[0] - cx).abs() < 0.01);
        assert!((center[1] - cy).abs() < 0.01);
        assert!((radius - r).abs() < 0.02);
    }

    #[test]
    fn check_circularity_rejects_rectangle() {
        let verts = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 0.1], [0.0, 0.1]];
        let area = shoelace_area(&verts);
        assert!(check_circularity(&verts, area).is_none());
    }

    #[test]
    fn suggest_name_quadrants() {
        assert!(suggest_name([0.1, 0.1], 0).contains("top"));
        assert!(suggest_name([0.9, 0.9], 0).contains("bottom"));
        assert!(suggest_name([0.5, 0.5], 0).contains("center"));
        assert!(suggest_name([0.1, 0.9], 0).contains("left"));
    }

    // ── Triangulation (REQ-06.4) ───────────────────────────────────────

    #[test]
    fn contour_to_warp_mesh_square_uvs_in_range() {
        let square = [[0.1, 0.1], [0.9, 0.1], [0.9, 0.9], [0.1, 0.9]];
        let mesh = contour_to_warp_mesh(&square);
        assert_eq!(mesh.cols, 3);
        assert!(mesh.rows >= 1, "expected at least one triangle");
        assert_eq!(mesh.points.len() as u32, mesh.cols * mesh.rows);
        for p in &mesh.points {
            assert!(p.uv[0] >= 0.0 && p.uv[0] <= 1.0);
            assert!(p.uv[1] >= 0.0 && p.uv[1] <= 1.0);
        }
    }

    #[test]
    fn contour_to_warp_mesh_degenerate_falls_back_to_identity() {
        let mesh = contour_to_warp_mesh(&[[0.0, 0.0], [1.0, 0.0]]);
        assert!(mesh.is_identity());
    }

    // ── SVG / DXF / raster detection ───────────────────────────────────

    #[test]
    fn detect_from_svg_simple_rect() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">
            <rect x="10" y="10" width="80" height="80" fill="black" stroke="black"/>
        </svg>"#;
        let det = detect_from_svg(svg).expect("svg detect");
        assert!(!det.contours.is_empty());
    }

    #[test]
    fn detect_from_svg_empty_rejects() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"></svg>"#;
        assert!(detect_from_svg(svg).is_err());
    }

    #[test]
    fn detect_from_svg_invalid_rejects() {
        assert!(detect_from_svg(b"not valid svg").is_err());
    }

    #[test]
    fn detect_from_dxf_invalid_rejects() {
        assert!(detect_from_dxf(b"not valid dxf").is_err());
    }

    #[test]
    fn detect_from_image_rejects_empty() {
        assert!(detect_from_image(&[], &DetectionParams::default()).is_err());
    }

    #[test]
    fn detect_from_image_white_rect() {
        let mut img = image::RgbaImage::new(200, 200);
        for y in 50..150 {
            for x in 50..150 {
                img.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let mut buf = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut buf);
        image::ImageEncoder::write_image(encoder, &img, 200, 200, image::ExtendedColorType::Rgba8)
            .unwrap();
        let params = DetectionParams {
            min_area: 0.01,
            min_vertices: 3,
            blur_radius: 0,
            detection_method: DetectionMethod::Threshold,
            threshold: 127,
            ..Default::default()
        };
        let det = detect_from_image(&buf, &params).expect("image detect");
        assert!(!det.contours.is_empty());
    }

    #[test]
    fn detect_from_rgba_white_rect_on_black() {
        let (w, h) = (200u32, 200u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 50..150 {
            for x in 50..150 {
                let idx = ((y * w + x) * 4) as usize;
                rgba[idx] = 255;
                rgba[idx + 1] = 255;
                rgba[idx + 2] = 255;
                rgba[idx + 3] = 255;
            }
        }
        let params = DetectionParams {
            min_area: 0.01,
            min_vertices: 3,
            blur_radius: 0,
            detection_method: DetectionMethod::Threshold,
            ..Default::default()
        };
        let det = detect_from_rgba(&rgba, w, h, &params).expect("rgba detect");
        assert!(!det.contours.is_empty());
    }

    #[test]
    fn detect_from_rgba_all_black_no_contours() {
        let (w, h) = (100u32, 100u32);
        let rgba = vec![0u8; (w * h * 4) as usize];
        assert!(matches!(
            detect_from_rgba(&rgba, w, h, &DetectionParams::default()),
            Err(ImportError::NoContours)
        ));
    }

    #[test]
    fn detect_from_rgba_buffer_too_small() {
        assert!(matches!(
            detect_from_rgba(&[0u8; 10], 100, 100, &DetectionParams::default()),
            Err(ImportError::ImageLoad(_))
        ));
    }

    #[test]
    fn detect_from_file_unsupported_extension() {
        let path = std::env::temp_dir().join("rustjay_test_detect.xyz");
        std::fs::write(&path, b"dummy").unwrap();
        let result = detect_from_file(&path, &DetectionParams::default());
        let _ = std::fs::remove_file(&path);
        assert!(matches!(result, Err(ImportError::UnsupportedFormat(_))));
    }

    // ── Acceptance test (REQ-06 / T07) ─────────────────────────────────

    #[test]
    fn import_varda_stage_svg() {
        let path = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/stage.svg"
        ));
        assert!(path.exists(), "stage.svg fixture not found at {path:?}");

        let data = std::fs::read(path).unwrap();
        let det = detect_from_svg(&data).expect("stage.svg detection");

        let largest = det.largest().expect("at least one contour");
        assert!(
            largest.area > 0.5,
            "largest contour area {} should exceed 0.5",
            largest.area
        );
        assert!(
            largest.vertices.len() >= 4,
            "largest contour should have >= 4 vertices, got {}",
            largest.vertices.len()
        );

        let mesh = largest.to_warp_mesh();
        assert!(mesh.rows >= 1, "triangulation produced no triangles");
        for p in &mesh.points {
            assert!(
                p.uv[0] >= 0.0 && p.uv[0] <= 1.0,
                "uv.x out of [0,1]: {}",
                p.uv[0]
            );
            assert!(
                p.uv[1] >= 0.0 && p.uv[1] <= 1.0,
                "uv.y out of [0,1]: {}",
                p.uv[1]
            );
        }

        // Surface conversion path also works.
        let surfaces = det.to_surfaces();
        assert!(!surfaces.is_empty());
        assert!(surfaces[0].to_warp_mesh().points.len() >= 3);
    }
}
