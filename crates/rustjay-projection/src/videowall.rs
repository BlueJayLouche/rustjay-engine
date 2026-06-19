//! AprilTag auto-detection for the video matrix (feature `videowall`).
//!
//! Detects AprilTag markers in the input texture and **suggests** a
//! [`VideoMatrixConfig`]: which output cell each screen is (tag id), plus a
//! first-guess aspect and orientation. The suggestion is meant to be shown in
//! the UI and confirmed/overridden by the user before commit (hybrid flow).
//!
//! ## Geometry (the output-authoritative fix)
//!
//! Detection runs on the input texture, so image pixels == input UV after
//! dividing by image size. That removes the old "photo aspect vs input aspect"
//! correction entirely. The pipeline is, in order:
//!
//! 1. **Identify output id** — tag id → output cell.
//! 2. **Orientation** — from the tag's rotation angle (a suggestion).
//! 3. **Aspect** — classify the tag's own-frame width/height **directly**. The tag
//!    edges rotate WITH the screen, so w/h already reflects the screen's squish in
//!    its own frame; inverting it for rotation double-corrects (a tall 4:3 tag would
//!    flip to 21:9 — the original mapper geometry bug).
//! 4. **Apply size + orientation** — the tag is square and fills the screen's
//!    short side; long side = short × aspect. A single landscape/portrait swap
//!    handles rotation. The `source_rect` stays **axis-aligned**; the shader
//!    rotates sampling exactly once via [`Orientation`].

use crate::identity::BlitPipeline;
use crate::matrix::{
    AspectRatio, GridCellMapping, GridPosition, GridSize, InputGridConfig, Orientation, Rect,
    VideoMatrixConfig,
};
use crate::stage::ProjectionStage;
use apriltag::{Detector, Family};
use image::{GrayImage, Rgba, RgbaImage};
use rustjay_core::RenderCtx;
use std::sync::{Arc, Mutex};

/// AprilTag families we support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AprilTagFamily {
    #[default]
    Tag36h11,
    Tag25h9,
    Tag16h5,
}

impl AprilTagFamily {
    fn to_family(self) -> Family {
        match self {
            Self::Tag36h11 => Family::tag_36h11(),
            Self::Tag25h9 => Family::tag_25h9(),
            Self::Tag16h5 => Family::tag_16h5(),
        }
    }
}

/// One detected marker. Corner order (AprilTag, image coords, Y down):
/// `[0]=left-bottom, [1]=right-bottom, [2]=right-top, [3]=left-top`.
#[derive(Debug, Clone)]
pub struct AprilTagDetection {
    pub id: u32,
    pub corners: [[f32; 2]; 4],
    pub center: [f32; 2],
    pub decision_margin: f32,
}

/// Detector wrapper around the `apriltag` crate.
pub struct AprilTagDetector {
    detector: Detector,
}

impl AprilTagDetector {
    pub fn new(family: AprilTagFamily) -> Self {
        let detector = Detector::builder()
            .add_family_bits(family.to_family(), 1)
            .build()
            .expect("failed to build AprilTag detector");
        Self { detector }
    }

    pub fn detect(&mut self, image: &GrayImage) -> Vec<AprilTagDetection> {
        let (w, h) = image.dimensions();
        let mut at_image = apriltag::Image::zeros_with_alignment(w as usize, h as usize, 96)
            .expect("failed to allocate AprilTag image");
        for (x, y, p) in image.enumerate_pixels() {
            at_image[(x as usize, y as usize)] = p[0];
        }
        self.detector
            .detect(&at_image)
            .into_iter()
            .map(|d| {
                let c = d.corners();
                let center = d.center();
                AprilTagDetection {
                    id: d.id() as u32,
                    corners: [
                        [c[0][0] as f32, c[0][1] as f32],
                        [c[1][0] as f32, c[1][1] as f32],
                        [c[2][0] as f32, c[2][1] as f32],
                        [c[3][0] as f32, c[3][1] as f32],
                    ],
                    center: [center[0] as f32, center[1] as f32],
                    decision_margin: d.decision_margin(),
                }
            })
            .collect()
    }
}

impl Default for AprilTagDetector {
    fn default() -> Self {
        Self::new(AprilTagFamily::default())
    }
}

/// A detected screen — a *suggestion* for one output cell.
///
/// Geometry is stored **aspect-neutral**: `wall_center` / `wall_size` are detection
/// pixels divided by the image HEIGHT for BOTH axes (pixels are square, so /height
/// bakes the photo aspect into the x-range only). Resolved into a source rect
/// per-target via [`crate::matrix::resolve_source_rects`].
#[derive(Debug, Clone)]
pub struct DetectedScreen {
    pub screen_id: u32,
    /// Centre in aspect-neutral wall units (px / img_h).
    pub wall_center: [f32; 2],
    /// Size in aspect-neutral wall units; `wall_size.x / wall_size.y` == display aspect.
    pub wall_size: [f32; 2],
    pub aspect_ratio: AspectRatio,
    pub orientation: Orientation,
    pub decision_margin: f32,
    /// Aspect (W/H) of the detection image — the wall/calibration aspect (kept as
    /// the default `output_aspect`; the source rects no longer depend on it).
    pub calib_aspect: f32,
}

impl DetectedScreen {
    /// Axis-aligned source rect in **wall units** (orientation applied separately;
    /// not in UV — use the uniform fit to resolve a target rect).
    pub fn wall_rect(&self) -> Rect {
        Rect::new(
            self.wall_center[0] - self.wall_size[0] / 2.0,
            self.wall_center[1] - self.wall_size[1] / 2.0,
            self.wall_size[0],
            self.wall_size[1],
        )
    }
}

/// Tuning for auto-detection.
#[derive(Debug, Clone)]
pub struct AutoDetectConfig {
    pub family: AprilTagFamily,
    /// Fraction of the screen's short side covered by the **detected** tag pattern
    /// — the rest is the tag's white border plus any screen margin. Lower → bigger
    /// cell. Tune to how your tags were displayed; ~0.5 is typical. (Replaces the
    /// old `tag_size_ratio` × marker-border fudge, which assumed a full-screen tag.)
    pub tag_fill: f32,
    /// Discard detections below this decision margin.
    pub min_confidence: f32,
    /// Sample a slightly larger region than measured (1.0 = exact).
    pub screen_scale: f32,
    /// Uniformly scale all screens so their bounding box fills the input frame.
    pub fit_to_frame: bool,
    /// Auto-enhance the image before detection (darken → brighten → contrast) so
    /// AprilTags pop out of bright/washed-out photos. See [`enhance_for_detection`].
    pub enhance: bool,
}

impl Default for AutoDetectConfig {
    fn default() -> Self {
        Self {
            family: AprilTagFamily::Tag36h11,
            tag_fill: 0.5,
            min_confidence: 10.0,
            screen_scale: 1.0,
            fit_to_frame: true,
            enhance: true,
        }
    }
}

/// Pre-process a detection image so AprilTags pop (the mapper's iPhone-style
/// auto-enhance): darken (×0.2) **first** to avoid highlight blowout, then
/// brighten(+100), then contrast(+100).
pub fn enhance_for_detection(img: &GrayImage) -> GrayImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        p[0] = (p[0] as f32 * 0.2) as u8;
    }
    let out = image::imageops::brighten(&out, 100);
    image::imageops::contrast(&out, 100.0)
}

/// Auto-detector: image → suggested screens → suggested config.
pub struct AprilTagAutoDetector {
    config: AutoDetectConfig,
}

impl AprilTagAutoDetector {
    pub fn new() -> Self {
        Self { config: AutoDetectConfig::default() }
    }
    pub fn with_config(config: AutoDetectConfig) -> Self {
        Self { config }
    }
    pub fn config(&self) -> &AutoDetectConfig {
        &self.config
    }

    /// Detect screens in a grayscale input image (its dimensions ARE the input UV space).
    pub fn detect_screens(&self, image: &GrayImage) -> Vec<DetectedScreen> {
        // Optionally pre-enhance so tags survive bright/washed-out photos.
        let enhanced;
        let image: &GrayImage = if self.config.enhance {
            enhanced = enhance_for_detection(image);
            &enhanced
        } else {
            image
        };
        let (w, h) = image.dimensions();
        let mut detector = AprilTagDetector::new(self.config.family);
        let mut screens: Vec<DetectedScreen> = detector
            .detect(image)
            .into_iter()
            .filter(|d| d.decision_margin >= self.config.min_confidence)
            .map(|d| self.screen_from_detection(&d, w as f32, h as f32))
            .collect();
        screens.sort_by_key(|s| s.screen_id);
        if self.config.fit_to_frame && !screens.is_empty() {
            fit_screens_to_frame(&mut screens);
        }
        screens
    }

    /// Map detected screens onto an output grid. Cell = `id % cols, id / cols`;
    /// out-of-range ids are skipped (logged). Source rects come from detection.
    pub fn suggest_config(
        &self,
        screens: &[DetectedScreen],
        output_grid: GridSize,
    ) -> VideoMatrixConfig {
        let cols = output_grid.columns.max(1);
        let max_id = output_grid.total();
        let mut input_grid = InputGridConfig::new(GridSize::new(screens.len().max(1) as u32, 1));
        for (idx, s) in screens.iter().enumerate() {
            if s.screen_id >= max_id {
                log::warn!(
                    "screen id {} exceeds {}×{} output grid, skipping",
                    s.screen_id,
                    output_grid.columns,
                    output_grid.rows
                );
                continue;
            }
            let pos = GridPosition::new(
                (s.screen_id % cols) as f32,
                (s.screen_id / cols) as f32,
                1.0,
                1.0,
            );
            input_grid.add_mapping(
                GridCellMapping::new(idx, pos)
                    .with_aspect_ratio(s.aspect_ratio)
                    .with_orientation(s.orientation)
                    .with_display_id(s.screen_id)
                    // Store the aspect-neutral wall geometry; the source rect is
                    // resolved per-target via the uniform bbox fit, so the layout
                    // never distorts on an off-aspect input.
                    .with_wall_geometry(s.wall_center, s.wall_size),
            );
        }
        // Anchor the output to the wall/calibration aspect so the layout maps 1:1.
        let output_aspect = screens.first().map(|s| s.calib_aspect).unwrap_or(16.0 / 9.0);
        VideoMatrixConfig {
            input_grid,
            output_grid,
            background_color: [0.0, 0.0, 0.0, 1.0],
            output_aspect,
        }
    }

    fn screen_from_detection(&self, d: &AprilTagDetection, img_w: f32, img_h: f32) -> DetectedScreen {
        // Aspect-neutral wall units: normalise BOTH axes by HEIGHT. Pixels are
        // square, so /height keeps real proportions and bakes the photo aspect
        // into the x-range only (centre.x can exceed 1.0 on a wide photo).
        let h = img_h.max(1.0);
        let center = [d.center[0] / h, d.center[1] / h];

        // 2. orientation suggestion
        let orientation = detect_orientation(&d.corners);
        let rotated = matches!(orientation, Orientation::Rotated90 | Orientation::Rotated270);

        // 3. aspect suggestion. The tag's edge lengths are measured along its OWN
        // (rotated) axes, which rotate WITH the screen — so width/height already
        // reflects the screen's squish in the screen's frame. Classify it DIRECTLY;
        // do NOT invert for rotation (that double-corrects: a rotated 4:3 screen's
        // tall tag flips to 21:9).
        let aspect = classify_aspect(tag_aspect_px(&d.corners));

        // 4. size: the detected tag covers `tag_fill` of the screen's short side
        // (rotation-invariant tag measure). Reconstruct the screen's box AS IT
        // APPEARS in the detection image so the source rect matches the screen's
        // bezel: long = short × aspect, with one landscape/portrait swap for
        // rotation. (Do NOT reshape to the live input aspect — that breaks the
        // bezel match; output-side aspect correction is a separate concern.)
        let tag_px = tag_size_px(&d.corners);
        let short_px = tag_px / self.config.tag_fill.clamp(0.05, 1.0);
        let long_px = short_px * aspect.as_f32();
        let (w_px, h_px) = if rotated { (short_px, long_px) } else { (long_px, short_px) };
        let s = self.config.screen_scale;

        DetectedScreen {
            screen_id: d.id,
            wall_center: center,
            // Both axes / height → wall_size.x/wall_size.y == display aspect.
            wall_size: [(w_px / h) * s, (h_px / h) * s],
            aspect_ratio: aspect,
            orientation,
            decision_margin: d.decision_margin,
            calib_aspect: img_w / h,
        }
    }
}

impl Default for AprilTagAutoDetector {
    fn default() -> Self {
        Self::new()
    }
}

// --- geometry helpers ---

fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// Orientation from the top edge (LT→RT). 0° points +x.
fn detect_orientation(c: &[[f32; 2]; 4]) -> Orientation {
    let top = [c[2][0] - c[3][0], c[2][1] - c[3][1]]; // RT - LT
    let deg = top[1].atan2(top[0]).to_degrees();
    let n = ((deg % 360.0) + 360.0) % 360.0;
    if !(45.0..315.0).contains(&n) {
        Orientation::Normal
    } else if n < 135.0 {
        Orientation::Rotated90
    } else if n < 225.0 {
        Orientation::Rotated180
    } else {
        Orientation::Rotated270
    }
}

/// width/height of the tag in pixels (>1 = wide, <1 = tall).
fn tag_aspect_px(c: &[[f32; 2]; 4]) -> f32 {
    let w = (dist(c[1], c[0]) + dist(c[2], c[3])) / 2.0; // bottom, top
    let h = (dist(c[3], c[0]) + dist(c[2], c[1])) / 2.0; // left, right
    if h > 0.0 {
        w / h
    } else {
        1.0
    }
}

/// Square-tag side length in pixels (rotation invariant).
fn tag_size_px(c: &[[f32; 2]; 4]) -> f32 {
    (dist(c[0], c[1]) + dist(c[1], c[2]) + dist(c[2], c[3]) + dist(c[3], c[0])) / 4.0
}

/// Snap a measured aspect to a standard screen ratio.
fn classify_aspect(measured: f32) -> AspectRatio {
    if measured < 0.85 {
        AspectRatio::Ratio4_3
    } else if measured < 1.20 {
        AspectRatio::Ratio16_9
    } else {
        AspectRatio::Ratio21_9
    }
}

/// Uniformly scale + recentre all screens (in wall units) so their bounding box
/// is normalised to ~unit height around (0.5, 0.5). This is a pure uniform
/// similarity transform, so it does NOT change the layout's aspects or relative
/// spacing — the real coverage fit happens per-target in
/// [`crate::matrix::resolve_source_rects`]. It just keeps the stored wall
/// coordinates tidy (near 0..1) for the override-nudge UI.
fn fit_screens_to_frame(screens: &mut [DetectedScreen]) {
    let mut min = [f32::MAX, f32::MAX];
    let mut max = [f32::MIN, f32::MIN];
    for s in screens.iter() {
        let r = s.wall_rect();
        min[0] = min[0].min(r.x);
        min[1] = min[1].min(r.y);
        max[0] = max[0].max(r.x + r.width);
        max[1] = max[1].max(r.y + r.height);
    }
    let (bw, bh) = (max[0] - min[0], max[1] - min[1]);
    if bw <= 0.0 || bh <= 0.0 {
        return;
    }
    // One scalar for BOTH axes (similarity, never per-axis).
    let scale = (1.0 / bw).min(1.0 / bh);
    let c = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0];
    for s in screens.iter_mut() {
        s.wall_center = [
            (s.wall_center[0] - c[0]) * scale + 0.5,
            (s.wall_center[1] - c[1]) * scale + 0.5,
        ];
        s.wall_size = [s.wall_size[0] * scale, s.wall_size[1] * scale];
    }
}

/// Read a wgpu texture back into a grayscale image for detection.
///
/// Assumes the BGRA8 pipeline (skill discipline); luma via BT.601.
pub fn texture_to_gray_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> anyhow::Result<GrayImage> {
    let bytes_per_row = (width * 4).div_ceil(256) * 256;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("AprilTag Readback"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(std::iter::once(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let start = std::time::Instant::now();
    let mapped = loop {
        device.poll(wgpu::PollType::Poll).ok();
        match rx.try_recv() {
            Ok(r) => break r,
            Err(_) if start.elapsed() < std::time::Duration::from_secs(5) => {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Err(e) => return Err(anyhow::anyhow!("buffer map timed out: {e:?}")),
        }
    };
    mapped.map_err(|e| anyhow::anyhow!("buffer map failed: {e:?}"))?;

    let data = slice.get_mapped_range();
    let mut gray = Vec::with_capacity((width * height) as usize);
    for row in 0..height {
        let base = (row * bytes_per_row) as usize;
        for px in 0..width as usize {
            let o = base + px * 4;
            let (b, g, r) = (data[o] as f32, data[o + 1] as f32, data[o + 2] as f32);
            gray.push((0.299 * r + 0.587 * g + 0.114 * b) as u8);
        }
    }
    drop(data);
    buffer.unmap();

    GrayImage::from_raw(width, height, gray)
        .ok_or_else(|| anyhow::anyhow!("failed to build grayscale image"))
}

// ---------------------------------------------------------------------------
// Marker generation + calibration tag-grid stage
// ---------------------------------------------------------------------------

/// Render an AprilTag marker (id) to a grayscale image via the C library.
/// Works for any valid id without pre-generated assets.
pub fn generate_marker(family: AprilTagFamily, id: u32) -> anyhow::Result<GrayImage> {
    unsafe {
        let fam = match family {
            AprilTagFamily::Tag36h11 => apriltag_sys::tag36h11_create(),
            AprilTagFamily::Tag25h9 => apriltag_sys::tag25h9_create(),
            AprilTagFamily::Tag16h5 => apriltag_sys::tag16h5_create(),
        };
        if fam.is_null() {
            anyhow::bail!("failed to create apriltag family");
        }
        let img = apriltag_sys::apriltag_to_image(fam, id as i32);
        let destroy_fam = |fam| match family {
            AprilTagFamily::Tag36h11 => apriltag_sys::tag36h11_destroy(fam),
            AprilTagFamily::Tag25h9 => apriltag_sys::tag25h9_destroy(fam),
            AprilTagFamily::Tag16h5 => apriltag_sys::tag16h5_destroy(fam),
        };
        if img.is_null() {
            destroy_fam(fam);
            anyhow::bail!("apriltag_to_image returned null for id {id}");
        }
        let i = &*img;
        let (w, h, stride) = (i.width as u32, i.height as u32, i.stride as usize);
        let mut gray = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = *i.buf.add(y as usize * stride + x as usize);
                gray.put_pixel(x, y, image::Luma([v]));
            }
        }
        apriltag_sys::image_u8_destroy(img);
        destroy_fam(fam);
        Ok(gray)
    }
}

/// Compose a full-frame tag grid: cell (col,row) shows tag id `row*cols + col`,
/// each on a white quiet-zone square so detection has margin. Black background.
fn compose_tag_grid(
    family: AprilTagFamily,
    grid: GridSize,
    out_w: u32,
    out_h: u32,
    tag_ratio: f32,
) -> RgbaImage {
    let mut frame = RgbaImage::from_pixel(out_w.max(1), out_h.max(1), Rgba([0, 0, 0, 255]));
    let (cols, rows) = (grid.columns.max(1), grid.rows.max(1));
    let cell_w = out_w / cols;
    let cell_h = out_h / rows;
    let marker_px = ((cell_w.min(cell_h) as f32) * tag_ratio.clamp(0.1, 0.95)) as u32;
    if marker_px == 0 {
        return frame;
    }
    let quiet_px = (marker_px as f32 * 1.25) as u32; // white quiet zone

    for row in 0..rows {
        for col in 0..cols {
            let id = row * cols + col;
            let marker = match generate_marker(family, id) {
                Ok(m) => image::imageops::resize(
                    &m,
                    marker_px,
                    marker_px,
                    image::imageops::FilterType::Nearest,
                ),
                Err(e) => {
                    log::warn!("tag grid: marker {id} failed: {e}");
                    continue;
                }
            };
            let cx = col * cell_w + cell_w / 2;
            let cy = row * cell_h + cell_h / 2;
            // white quiet-zone square
            let qx = cx.saturating_sub(quiet_px / 2);
            let qy = cy.saturating_sub(quiet_px / 2);
            for y in qy..(qy + quiet_px).min(out_h) {
                for x in qx..(qx + quiet_px).min(out_w) {
                    frame.put_pixel(x, y, Rgba([255, 255, 255, 255]));
                }
            }
            // marker centered
            let mx = cx.saturating_sub(marker_px / 2);
            let my = cy.saturating_sub(marker_px / 2);
            for (x, y, p) in marker.enumerate_pixels() {
                let (px, py) = (mx + x, my + y);
                if px < out_w && py < out_h {
                    let v = p[0];
                    frame.put_pixel(px, py, Rgba([v, v, v, 255]));
                }
            }
        }
    }
    frame
}

/// Calibration mode handoff (UI → stage). When `active`, the projector shows the
/// tag grid instead of content so a camera can detect the wall layout.
#[derive(Debug, Clone, Default)]
pub struct CalibSync {
    pub active: bool,
    pub grid: GridSize,
    pub version: u64,
}

impl CalibSync {
    pub fn set(&mut self, active: bool, grid: GridSize) {
        self.active = active;
        self.grid = grid;
        self.version = self.version.wrapping_add(1);
    }
}

/// Projection stage that overwrites the output with an AprilTag grid while
/// calibration is active; otherwise inactive (input passes through).
pub struct TagGridStage {
    blit: BlitPipeline,
    vertex_buffer: wgpu::Buffer,
    family: AprilTagFamily,
    sync: Arc<Mutex<CalibSync>>,
    last_version: u64,
    out_size: [u32; 2],
    tex_view: Option<wgpu::TextureView>,
    cached_bg: Option<wgpu::BindGroup>,
}

impl TagGridStage {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, sync: Arc<Mutex<CalibSync>>) -> Self {
        use crate::identity::BlitVertex;
        use wgpu::util::DeviceExt;
        let verts: &[BlitVertex] = &[
            BlitVertex { position: [-1.0, -1.0], texcoord: [0.0, 1.0] },
            BlitVertex { position: [1.0, -1.0], texcoord: [1.0, 1.0] },
            BlitVertex { position: [-1.0, 1.0], texcoord: [0.0, 0.0] },
            BlitVertex { position: [-1.0, 1.0], texcoord: [0.0, 0.0] },
            BlitVertex { position: [1.0, -1.0], texcoord: [1.0, 1.0] },
            BlitVertex { position: [1.0, 1.0], texcoord: [1.0, 0.0] },
        ];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("TagGrid Vertex Buffer"),
            contents: bytemuck::cast_slice(verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            blit: BlitPipeline::new(device, format),
            vertex_buffer,
            family: AprilTagFamily::default(),
            sync,
            last_version: u64::MAX,
            out_size: [0, 0],
            tex_view: None,
            cached_bg: None,
        }
    }

    fn regenerate(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, grid: GridSize, size: [u32; 2]) {
        let frame = compose_tag_grid(self.family, grid, size[0].max(1), size[1].max(1), 0.6);
        let (w, h) = frame.dimensions();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("TagGrid Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &frame,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.tex_view = Some(texture.create_view(&wgpu::TextureViewDescriptor::default()));
        self.cached_bg = None;
    }
}

impl ProjectionStage for TagGridStage {
    fn label(&self) -> &str {
        "tag-grid"
    }

    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        _input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        output_size: [u32; 2],
    ) {
        let (active, grid, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.active, g.grid, g.version)
        };
        if !active {
            return;
        }
        if self.out_size != output_size || self.last_version != version || self.tex_view.is_none() {
            self.regenerate(ctx.device, ctx.queue, grid, output_size);
            self.out_size = output_size;
            self.last_version = version;
        }
        let view = self.tex_view.as_ref().unwrap();
        if self.cached_bg.is_none() {
            self.cached_bg = Some(self.blit.create_bind_group_nearest(ctx.device, view));
        }
        self.blit
            .blit(ctx.encoder, self.cached_bg.as_ref().unwrap(), output, &self.vertex_buffer);
    }

    fn is_active(&self) -> bool {
        self.sync.lock().unwrap_or_else(|e| e.into_inner()).active
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Square tag, [LB, RB, RT, LT], centered at `c`, half-side `s`.
    fn square_tag(id: u32, c: [f32; 2], s: f32) -> AprilTagDetection {
        AprilTagDetection {
            id,
            corners: [
                [c[0] - s, c[1] + s], // LB
                [c[0] + s, c[1] + s], // RB
                [c[0] + s, c[1] - s], // RT
                [c[0] - s, c[1] - s], // LT
            ],
            center: c,
            decision_margin: 50.0,
        }
    }

    /// Same square rotated 90° CW (top edge LT→RT points +y).
    fn square_tag_rot90(id: u32, c: [f32; 2], s: f32) -> AprilTagDetection {
        AprilTagDetection {
            id,
            corners: [
                [c[0] + s, c[1] - s], // LB
                [c[0] + s, c[1] + s], // RB
                [c[0] - s, c[1] + s], // RT
                [c[0] - s, c[1] - s], // LT
            ],
            center: c,
            decision_margin: 50.0,
        }
    }

    #[test]
    fn landscape_screen_suggestion() {
        let det = AprilTagAutoDetector::with_config(AutoDetectConfig {
            screen_scale: 1.0,
            fit_to_frame: false,
            ..Default::default()
        });
        let d = square_tag(0, [960.0, 540.0], 50.0); // 100px square in 1920×1080
        let s = det.screen_from_detection(&d, 1920.0, 1080.0);

        assert_eq!(s.orientation, Orientation::Normal);
        assert_eq!(s.aspect_ratio, AspectRatio::Ratio16_9);
        // Wall units are aspect-neutral (both axes / height), so the wall_size
        // ratio IS the display aspect directly — no per-axis image scaling.
        let ratio = s.wall_size[0] / s.wall_size[1];
        assert!((ratio - 16.0 / 9.0).abs() < 0.05, "wall aspect was {ratio}");
    }

    #[test]
    fn rotated_screen_is_portrait_footprint() {
        let det = AprilTagAutoDetector::with_config(AutoDetectConfig {
            screen_scale: 1.0,
            fit_to_frame: false,
            ..Default::default()
        });
        let d = square_tag_rot90(0, [960.0, 540.0], 50.0);
        let s = det.screen_from_detection(&d, 1920.0, 1080.0);

        assert_eq!(s.orientation, Orientation::Rotated90);
        // A rotated 16:9 screen occupies a TALL region: portrait wall footprint.
        assert!(
            s.wall_size[1] > s.wall_size[0],
            "expected portrait footprint, got {}x{}",
            s.wall_size[0],
            s.wall_size[1]
        );
        // Orientation lives on the cell; source rect stays axis-aligned.
    }

    // A 4:3 screen's wall geometry is aspect-neutral: wall_size.x/wall_size.y is
    // the display aspect (4:3) regardless of the photo's own aspect.
    #[test]
    fn source_rect_matches_screen_bezel() {
        // Tall tag (pixel ratio < 0.85) → classified 4:3.
        let (cx, cy, hw, hh) = (600.0, 450.0, 40.0, 60.0);
        let d = AprilTagDetection {
            id: 0,
            corners: [
                [cx - hw, cy + hh], // LB
                [cx + hw, cy + hh], // RB
                [cx + hw, cy - hh], // RT
                [cx - hw, cy - hh], // LT
            ],
            center: [cx, cy],
            decision_margin: 50.0,
        };
        let det = AprilTagAutoDetector::with_config(AutoDetectConfig {
            tag_fill: 0.5,
            screen_scale: 1.0,
            fit_to_frame: false,
            enhance: false,
            ..Default::default()
        });
        // 4:3 photo (1200×900): wall_size aspect must be 4:3 (the display aspect),
        // independent of the photo aspect.
        let s = det.screen_from_detection(&d, 1200.0, 900.0);
        assert_eq!(s.aspect_ratio, AspectRatio::Ratio4_3);
        let wall_ratio = s.wall_size[0] / s.wall_size[1];
        assert!((wall_ratio - 4.0 / 3.0).abs() < 0.02, "wall aspect was {wall_ratio}");
    }

    // A 4:3 screen rotated 270° shows a tag that is tall AND rotated. Aspect must
    // classify 4:3 from the tag's own-frame w/h directly — NOT inverted to 21:9.
    // (Regression for the rotated-aspect inversion bug; matches testFiles/IMG_1054
    // id2.)
    #[test]
    fn rotated_tall_tag_classifies_4_3() {
        let d = AprilTagDetection {
            id: 2,
            corners: [
                [180.0, 300.0], // LB
                [180.0, 220.0], // RB
                [300.0, 220.0], // RT
                [300.0, 300.0], // LT  → top edge (RT-LT)=(0,-80) up = 270°; w/h=80/120
            ],
            center: [240.0, 260.0],
            decision_margin: 50.0,
        };
        let det = AprilTagAutoDetector::with_config(AutoDetectConfig {
            fit_to_frame: false,
            enhance: false,
            ..Default::default()
        });
        let s = det.screen_from_detection(&d, 1920.0, 1080.0);
        assert_eq!(s.orientation, Orientation::Rotated270);
        assert_eq!(s.aspect_ratio, AspectRatio::Ratio4_3);
    }

    #[test]
    fn suggest_config_places_by_id() {
        let det = AprilTagAutoDetector::new();
        let screens = vec![
            DetectedScreen {
                screen_id: 0,
                wall_center: [0.25, 0.5],
                wall_size: [0.4, 0.4],
                aspect_ratio: AspectRatio::Ratio16_9,
                orientation: Orientation::Normal,
                decision_margin: 50.0,
                calib_aspect: 16.0 / 9.0,
            },
            DetectedScreen {
                screen_id: 3,
                wall_center: [0.75, 0.5],
                wall_size: [0.4, 0.4],
                aspect_ratio: AspectRatio::Ratio16_9,
                orientation: Orientation::Normal,
                decision_margin: 50.0,
                calib_aspect: 16.0 / 9.0,
            },
        ];
        let cfg = det.suggest_config(&screens, GridSize::new(2, 2));
        assert_eq!(cfg.input_grid.mappings.len(), 2);
        // id 3 in a 2×2 grid → (col 1, row 1)
        let m = &cfg.input_grid.mappings[1];
        assert_eq!(m.display_id, Some(3));
        assert_eq!(m.output_position.col, 1.0);
        assert_eq!(m.output_position.row, 1.0);
        assert!(m.wall_center.is_some() && m.wall_size.is_some());
    }
}
