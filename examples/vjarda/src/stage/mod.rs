//! Stage — surfaces, outputs, and projection mapping state.
//!
//! Delegates to `rustjay-projection` for warp, edge-blend, and dome.
//! See VARDA_PORT.md Phase 7–8.

use serde::{Deserialize, Serialize};

#[cfg(feature = "projection")]
use bytemuck;

/// How a surface maps its source texture onto its geometry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ContentMapping {
    /// Source texture is scaled to fill the surface independently.
    #[default]
    Fill,
    /// Surface position on the stage canvas determines the UV crop.
    /// Multiple surfaces on the same canvas tile a single render.
    Mapped,
}

impl ContentMapping {
    pub fn label(&self) -> &'static str {
        match self {
            ContentMapping::Fill => "Fill",
            ContentMapping::Mapped => "Mapped",
        }
    }
}

/// Which graph output a surface samples from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum SurfaceSource {
    /// The master mix (post-crossfader composite).
    #[default]
    Master,
    /// A specific mixer channel by UUID.
    Channel(String),
    /// A specific deck inside a channel.
    Deck {
        channel_uuid: String,
        deck_uuid: String,
    },
    /// Domemaster fisheye output.
    Domemaster,
}

impl SurfaceSource {
    pub fn label(&self) -> String {
        match self {
            SurfaceSource::Master => "Master".to_string(),
            SurfaceSource::Channel(uuid) => format!("Channel {}", &uuid[..uuid.len().min(6)]),
            SurfaceSource::Deck {
                channel_uuid: _,
                deck_uuid,
            } => {
                format!("Deck {}", &deck_uuid[..deck_uuid.len().min(6)])
            }
            SurfaceSource::Domemaster => "Domemaster".to_string(),
        }
    }
}

/// One surface on the stage: a polygonal or circular region with a source
/// and a warp transform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VardaSurface {
    /// Display name.
    pub name: String,
    /// Stable identity.
    pub uuid: String,
    /// Vertices in normalized stage coordinates [0..1].
    /// For circles, this is a single vertex at the center + a radius stored
    /// in the first vertex's z (unused; we use `is_circular` + a separate
    /// radius field instead).
    pub vertices: Vec<[f32; 2]>,
    /// True if this surface is a circle (first vertex = center).
    pub is_circular: bool,
    /// Radius for circular surfaces (normalized stage units).
    pub radius: f32,
    /// What this surface displays.
    pub source: SurfaceSource,
    /// How the source texture is mapped onto the surface geometry.
    #[serde(default)]
    pub content_mapping: ContentMapping,
    /// Additional contour outlines (e.g. cutouts, frames) that are rendered
    /// as dashed lines but not part of the primary warp geometry.
    #[serde(default)]
    pub extra_contours: Vec<Vec<[f32; 2]>>,
    /// UV crop rectangle `[min_u, min_v, max_u, max_v]` in normalized source
    /// texture space. Edited by corner handles on the stage canvas.
    #[serde(default = "full_uv_crop")]
    pub uv_crop_rect: [f32; 4],
    /// Warp mode (corner-pin or mesh).
    #[cfg(feature = "projection")]
    pub warp: rustjay_projection::WarpMode,
    #[cfg(not(feature = "projection"))]
    pub warp: (),
}

/// Default UV crop covering the full texture.
fn full_uv_crop() -> [f32; 4] {
    [0.0, 0.0, 1.0, 1.0]
}

impl VardaSurface {
    /// Create a default rectangular surface covering the full stage.
    pub fn full_frame(name: impl Into<String>, uuid: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            uuid: uuid.into(),
            vertices: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
            is_circular: false,
            radius: 0.0,
            source: SurfaceSource::Master,
            content_mapping: ContentMapping::Fill,
            extra_contours: Vec::new(),
            uv_crop_rect: full_uv_crop(),
            #[cfg(feature = "projection")]
            warp: rustjay_projection::WarpMode::identity(),
            #[cfg(not(feature = "projection"))]
            warp: (),
        }
    }

    /// Create a circular surface.
    pub fn circle(
        name: impl Into<String>,
        uuid: impl Into<String>,
        center: [f32; 2],
        radius: f32,
    ) -> Self {
        Self {
            name: name.into(),
            uuid: uuid.into(),
            vertices: vec![center],
            is_circular: true,
            radius,
            source: SurfaceSource::Master,
            content_mapping: ContentMapping::Fill,
            extra_contours: Vec::new(),
            uv_crop_rect: full_uv_crop(),
            #[cfg(feature = "projection")]
            warp: rustjay_projection::WarpMode::identity(),
            #[cfg(not(feature = "projection"))]
            warp: (),
        }
    }

    /// Axis-aligned bounding box of the surface in normalized stage space.
    /// Unions over the primary contour and all extra contours.
    pub fn bounding_box(&self) -> [f32; 4] {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        let mut has_geometry = false;

        if self.is_circular && !self.vertices.is_empty() {
            let c = self.vertices[0];
            min_x = min_x.min(c[0] - self.radius);
            min_y = min_y.min(c[1] - self.radius);
            max_x = max_x.max(c[0] + self.radius);
            max_y = max_y.max(c[1] + self.radius);
            has_geometry = true;
        } else {
            for v in &self.vertices {
                min_x = min_x.min(v[0]);
                min_y = min_y.min(v[1]);
                max_x = max_x.max(v[0]);
                max_y = max_y.max(v[1]);
                has_geometry = true;
            }
        }

        for contour in &self.extra_contours {
            for v in contour {
                min_x = min_x.min(v[0]);
                min_y = min_y.min(v[1]);
                max_x = max_x.max(v[0]);
                max_y = max_y.max(v[1]);
                has_geometry = true;
            }
        }

        if has_geometry {
            [min_x, min_y, max_x, max_y]
        } else {
            [0.0, 0.0, 1.0, 1.0]
        }
    }

    /// UV crop rect `[min_u, min_v, max_u, max_v]`.
    /// For Fill mode returns the full `[0,1]` rect unless an explicit
    /// `uv_crop_rect` has been edited.
    pub fn uv_crop(&self) -> [f32; 4] {
        match self.content_mapping {
            ContentMapping::Fill => self.uv_crop_rect,
            ContentMapping::Mapped => self.uv_crop_rect,
        }
    }

    /// Convert vertices to a `WarpMesh` (only when projection feature is on).
    #[cfg(feature = "projection")]
    pub fn to_warp_mesh(&self) -> rustjay_projection::WarpMesh {
        if self.is_circular {
            // Approximate circle with a 16-segment polygon.
            let mut poly = Vec::with_capacity(16);
            let center = self.vertices[0];
            for i in 0..16 {
                let angle = (i as f32) * std::f32::consts::TAU / 16.0;
                poly.push([
                    center[0] + self.radius * angle.cos(),
                    center[1] + self.radius * angle.sin(),
                ]);
            }
            rustjay_projection::surface_import::contour_to_warp_mesh(&poly)
        } else {
            rustjay_projection::surface_import::contour_to_warp_mesh(&self.vertices)
        }
    }
}

/// The stage holds all surfaces, projector configs, and headless output configs.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VardaStage {
    pub surfaces: Vec<VardaSurface>,
    /// Stage canvas size in pixels (logical design resolution).
    pub canvas_size: [u32; 2],
    /// Projector output windows.
    pub projectors: Vec<VardaProjector>,
    /// Headless (offscreen) outputs.
    pub headless_outputs: Vec<VardaHeadlessConfig>,
    /// Index of the currently selected surface in the UI (not serialized).
    #[serde(skip)]
    pub selected_surface_index: usize,
    /// Cached channel/deck source options for the Geometry tab source selector.
    /// Updated when the mixer lock is successfully acquired; used as fallback
    /// when the mixer is contended during render.
    #[serde(skip)]
    pub cached_source_options: Vec<(String, SurfaceSource)>,
    /// Shared warp state for the Master-routed surface, read each frame by the
    /// projector's [`VardaWarpStage`]. Injected by the plugin (`default_state`);
    /// the GUI publishes edits into it via [`VardaStage::publish_warp`]. Not
    /// serialized — it's a live render bridge, not persisted scene data.
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub warp_sync: Option<std::sync::Arc<std::sync::Mutex<WarpSync>>>,
    /// Shared dome state, read by [`VardaDomeStage`]. Injected by the plugin.
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub dome_sync: Option<std::sync::Arc<std::sync::Mutex<DomeSync>>>,
    /// Shared edge-blend state, read by [`VardaEdgeBlendStage`]. Injected by the plugin.
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub edge_blend_sync: Option<std::sync::Arc<std::sync::Mutex<EdgeBlendSync>>>,
    /// Per-projector source texture override. Each projector's [`VardaSourceStage`]
    /// reads its slot to determine which texture to sample (Master = passthrough,
    /// Channel = override). Injected by the plugin; grown/shrunk with projectors.
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub source_syncs: Vec<std::sync::Arc<std::sync::Mutex<SourceSync>>>,
    /// Per-projector output rotation. Each projector's [`RotationStage`] reads
    /// the rotation value from here. Grown/shrunk with projectors.
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub rotation_syncs: Vec<std::sync::Arc<std::sync::Mutex<rustjay_projection::RotationSync>>>,
}

impl VardaStage {
    pub fn new() -> Self {
        Self {
            surfaces: Vec::new(),
            canvas_size: [1920, 1080],
            projectors: Vec::new(),
            headless_outputs: Vec::new(),
            selected_surface_index: 0,
            cached_source_options: Vec::new(),
            #[cfg(feature = "projection")]
            warp_sync: None,
            #[cfg(feature = "projection")]
            dome_sync: None,
            #[cfg(feature = "projection")]
            edge_blend_sync: None,
            #[cfg(feature = "projection")]
            source_syncs: Vec::new(),
            #[cfg(feature = "projection")]
            rotation_syncs: Vec::new(),
        }
    }

    pub fn with_default_surface() -> Self {
        let mut stage = Self::new();
        stage
            .surfaces
            .push(VardaSurface::full_frame("Main", "main"));
        // One default projector
        stage.projectors.push(VardaProjector::default());
        stage.selected_surface_index = 0;
        stage.cached_source_options = Vec::new();
        stage
    }

    /// Push dome config into the shared [`DomeSync`] so the projector's
    /// [`VardaDomeStage`] picks it up on the next frame.
    #[cfg(feature = "projection")]
    pub fn publish_dome(
        &self,
        enabled: bool,
        config: rustjay_projection::DomemasterConfig,
        rotation: [f32; 3],
    ) {
        if let Some(sync) = &self.dome_sync {
            if let Ok(mut g) = sync.lock() {
                g.enabled = enabled;
                g.config = config;
                g.content_rotation = rotation;
                g.version = g.version.wrapping_add(1);
            }
        }
    }

    /// Push edge-blend config into the shared [`EdgeBlendSync`] so the projector's
    /// [`VardaEdgeBlendStage`] picks it up on the next frame.
    #[cfg(feature = "projection")]
    pub fn publish_edge_blend(&self, config: rustjay_projection::EdgeBlendConfig) {
        if let Some(sync) = &self.edge_blend_sync {
            if let Ok(mut g) = sync.lock() {
                g.config = config;
                g.version = g.version.wrapping_add(1);
            }
        }
    }

    /// Push the warp of the Master-routed surface (or the first surface) into
    /// the shared [`WarpSync`] so the projector's [`VardaWarpStage`] picks it up
    /// on the next frame. Bumps the version so the projector only re-applies on
    /// an actual edit. Call after the GUI mutates a surface's warp.
    #[cfg(feature = "projection")]
    pub fn publish_warp(&self) {
        let surf = self
            .surfaces
            .iter()
            .find(|s| s.source == SurfaceSource::Master)
            .or_else(|| self.surfaces.first());
        if let (Some(sync), Some(surf)) = (&self.warp_sync, surf) {
            if let Ok(mut g) = sync.lock() {
                g.mode = surf.warp.clone();
                g.uv_crop = surf.uv_crop();
                g.version = g.version.wrapping_add(1);
            }
        }
    }
}

/// Configuration for one projector output window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VardaProjector {
    pub name: String,
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// `None` = windowed; `Some(index)` = fullscreen on monitor N.
    pub fullscreen_monitor: Option<usize>,
    /// Which surface this projector displays (`None` = master / no override).
    pub surface_index: Option<usize>,
    /// Runtime window ID for live management (not persisted).
    #[serde(skip)]
    pub window_id: Option<winit::window::WindowId>,
    /// Output rotation for physically mounted projectors.
    #[serde(default)]
    pub rotation: OutputRotation,
    /// How this output delivers frames.
    #[serde(default)]
    pub output_type: OutputType,
    /// Use the global warp/dome/edge-blend syncs, or per-projector overrides.
    pub use_global_warp: bool,
    pub use_global_dome: bool,
    pub use_global_edge_blend: bool,
    /// Per-projector overrides (only used when `use_global_*` is false).
    #[cfg(feature = "projection")]
    #[serde(default)]
    pub warp_mode: Option<rustjay_projection::WarpMode>,
    #[cfg(not(feature = "projection"))]
    #[serde(skip)]
    pub warp_mode: Option<()>,
    pub dome_enabled: Option<bool>,
    #[cfg(feature = "projection")]
    #[serde(default)]
    pub edge_blend_config: Option<rustjay_projection::EdgeBlendConfig>,
    #[cfg(not(feature = "projection"))]
    #[serde(skip)]
    pub edge_blend_config: Option<()>,
}

impl Default for VardaProjector {
    fn default() -> Self {
        Self {
            name: "Projector".to_string(),
            // Projectors default to enabled because a typical venue has at
            // least one output window that should appear at startup.
            enabled: true,
            width: 1920,
            height: 1080,
            fullscreen_monitor: None,
            surface_index: Some(0),
            window_id: None,
            rotation: OutputRotation::default(),
            output_type: OutputType::Display,
            use_global_warp: true,
            use_global_dome: true,
            use_global_edge_blend: true,
            #[cfg(feature = "projection")]
            warp_mode: None,
            #[cfg(not(feature = "projection"))]
            warp_mode: None,
            dome_enabled: None,
            #[cfg(feature = "projection")]
            edge_blend_config: None,
            #[cfg(not(feature = "projection"))]
            edge_blend_config: None,
        }
    }
}

/// Output rotation for physically mounted projectors.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OutputRotation {
    /// No rotation (default).
    #[default]
    Deg0,
    /// 90° clockwise.
    Deg90,
    /// 180°.
    Deg180,
    /// 270° clockwise.
    Deg270,
}

impl OutputRotation {
    /// All rotation variants for UI dropdowns.
    pub const ALL: [OutputRotation; 4] = [
        OutputRotation::Deg0,
        OutputRotation::Deg90,
        OutputRotation::Deg180,
        OutputRotation::Deg270,
    ];

    /// GPU-side index (0–3) for the shader uniform.
    pub fn index(&self) -> u32 {
        match self {
            OutputRotation::Deg0 => 0,
            OutputRotation::Deg90 => 1,
            OutputRotation::Deg180 => 2,
            OutputRotation::Deg270 => 3,
        }
    }

    /// Human-readable label for UI display.
    pub fn label(&self) -> &'static str {
        match self {
            OutputRotation::Deg0 => "0°",
            OutputRotation::Deg90 => "90°",
            OutputRotation::Deg180 => "180°",
            OutputRotation::Deg270 => "270°",
        }
    }
}

/// How a projector or headless output delivers its rendered frames.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum OutputType {
    /// Render to a display window (default).
    #[default]
    Display,
    /// Send frames over NDI (requires `ndi` feature).
    Ndi,
    /// Record to disk (requires recording backend).
    Recording,
}

impl OutputType {
    pub fn label(&self) -> &'static str {
        match self {
            OutputType::Display => "Display",
            OutputType::Ndi => "NDI",
            OutputType::Recording => "Recording",
        }
    }
}

/// Configuration for a headless (offscreen) output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VardaHeadlessConfig {
    pub name: String,
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Which surface this output displays (`None` = first surface).
    pub surface_index: Option<usize>,
    /// How this output delivers frames.
    #[serde(default)]
    pub output_type: OutputType,
    /// Whether this headless output has already been pushed to the
    /// projection subsystem. Not serialized — reset on app restart.
    #[serde(skip)]
    pub pushed: bool,
}

impl Default for VardaHeadlessConfig {
    fn default() -> Self {
        Self {
            name: "Headless".to_string(),
            // Headless outputs default to disabled because they consume GPU
            // memory and CPU readback bandwidth; they are opt-in per use-case.
            enabled: false,
            width: 1920,
            height: 1080,
            surface_index: None,
            output_type: OutputType::Display,
            pushed: false,
        }
    }
}

/// Live warp state shared between the GUI (writer) and the projector's
/// [`VardaWarpStage`] (reader). `version` is bumped on each edit so the reader
/// re-applies only on change, not every frame.
#[cfg(feature = "projection")]
#[derive(Debug, Clone)]
pub struct WarpSync {
    pub mode: rustjay_projection::WarpMode,
    /// UV crop rect [min_u, min_v, max_u, max_v] for content mapping.
    pub uv_crop: [f32; 4],
    pub version: u64,
}

#[cfg(feature = "projection")]
impl Default for WarpSync {
    fn default() -> Self {
        Self {
            mode: rustjay_projection::WarpMode::identity(),
            uv_crop: [0.0, 0.0, 1.0, 1.0],
            version: 0,
        }
    }
}

/// Per-projector source texture override. The projector's [`VardaSourceStage`]
/// reads this to determine which texture to sample.
#[cfg(feature = "projection")]
#[derive(Debug, Clone)]
pub struct SourceSync {
    /// `None` = use the default input (master mix).
    /// `Some(view)` = sample from this texture view instead.
    pub override_view: Option<std::sync::Arc<wgpu::TextureView>>,
    /// A key representing the current source (e.g. "master", "channel:<uuid>").
    /// Used to detect source changes without bumping version every frame.
    pub source_key: Option<String>,
    pub version: u64,
}

#[cfg(feature = "projection")]
impl Default for SourceSync {
    fn default() -> Self {
        Self {
            override_view: None,
            source_key: None,
            version: 0,
        }
    }
}

/// A projector stage that warps the incoming composite using the live
/// [`WarpSync`] state. Corner-pin edits update the homography in place (cheap);
/// a mode switch or mesh edit rebuilds the inner [`rustjay_projection::WarpStage`].
#[cfg(feature = "projection")]
pub struct VardaWarpStage {
    inner: rustjay_projection::WarpStage,
    format: wgpu::TextureFormat,
    sync: std::sync::Arc<std::sync::Mutex<WarpSync>>,
    last_version: u64,
    inner_is_corner_pin: bool,
    last_mesh_cols: u32,
    last_mesh_rows: u32,
}

#[cfg(feature = "projection")]
impl VardaWarpStage {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sync: std::sync::Arc<std::sync::Mutex<WarpSync>>,
    ) -> Self {
        let (mode, version) = {
            let g = sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.mode.clone(), g.version)
        };
        let inner_is_corner_pin = matches!(mode, rustjay_projection::WarpMode::CornerPin { .. });
        let (last_mesh_cols, last_mesh_rows) = match &mode {
            rustjay_projection::WarpMode::Mesh(mesh) => (mesh.cols, mesh.rows),
            _ => (0, 0),
        };
        let inner = rustjay_projection::WarpStage::from_mode(device, format, &mode);
        Self {
            inner,
            format,
            sync,
            last_version: version,
            inner_is_corner_pin,
            last_mesh_cols,
            last_mesh_rows,
        }
    }
}

#[cfg(feature = "projection")]
impl rustjay_projection::ProjectionStage for VardaWarpStage {
    fn label(&self) -> &str {
        "varda-warp"
    }

    fn render(
        &mut self,
        ctx: &mut rustjay_core::RenderCtx<'_>,
        input: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        output_size: [u32; 2],
    ) {
        let (mode, uv_crop, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.mode.clone(), g.uv_crop, g.version)
        };
        if version != self.last_version {
            self.last_version = version;
            match &mode {
                // Same mode family → cheap homography update (no rebuild on drag).
                rustjay_projection::WarpMode::CornerPin { corners } if self.inner_is_corner_pin => {
                    let src = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                    let h = rustjay_projection::compute_forward_homography(&src, corners);
                    self.inner.set_homography(ctx.queue, &h);
                    // set_homography already includes self.uv_crop, so no need
                    // for a second uniform write here.
                }
                // Same mesh dimensions → cheap vertex buffer update (no rebuild on drag).
                rustjay_projection::WarpMode::Mesh(mesh)
                    if !self.inner_is_corner_pin
                        && mesh.cols == self.last_mesh_cols
                        && mesh.rows == self.last_mesh_rows =>
                {
                    self.inner.set_mesh(ctx.queue, mesh);
                    self.inner.set_uv_crop(ctx.queue, &uv_crop);
                }
                // Mode switch or mesh dimension change → rebuild the warp stage.
                _ => {
                    self.inner =
                        rustjay_projection::WarpStage::from_mode(ctx.device, self.format, &mode);
                    self.inner_is_corner_pin =
                        matches!(mode, rustjay_projection::WarpMode::CornerPin { .. });
                    self.inner.set_uv_crop(ctx.queue, &uv_crop);
                    if let rustjay_projection::WarpMode::Mesh(mesh) = &mode {
                        self.last_mesh_cols = mesh.cols;
                        self.last_mesh_rows = mesh.rows;
                    }
                }
            }
        }
        self.inner
            .render(ctx, input, input_texture, output, output_size);
    }

    fn on_input_changed(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        self.inner.on_input_changed(device, size);
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Source stage — per-projector texture override
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "projection")]
pub struct VardaSourceStage {
    blit: rustjay_projection::identity::BlitPipeline,
    vertex_buffer: wgpu::Buffer,
    cached_bind_group: Option<wgpu::BindGroup>,
    sync: std::sync::Arc<std::sync::Mutex<SourceSync>>,
    last_version: u64,
}

#[cfg(feature = "projection")]
impl VardaSourceStage {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sync: std::sync::Arc<std::sync::Mutex<SourceSync>>,
    ) -> Self {
        use wgpu::util::DeviceExt;
        let blit = rustjay_projection::identity::BlitPipeline::new(device, format);
        let vertices: &[rustjay_projection::identity::BlitVertex] = &[
            rustjay_projection::identity::BlitVertex {
                position: [-1.0, -1.0],
                texcoord: [0.0, 1.0],
            },
            rustjay_projection::identity::BlitVertex {
                position: [1.0, -1.0],
                texcoord: [1.0, 1.0],
            },
            rustjay_projection::identity::BlitVertex {
                position: [-1.0, 1.0],
                texcoord: [0.0, 0.0],
            },
            rustjay_projection::identity::BlitVertex {
                position: [-1.0, 1.0],
                texcoord: [0.0, 0.0],
            },
            rustjay_projection::identity::BlitVertex {
                position: [1.0, -1.0],
                texcoord: [1.0, 1.0],
            },
            rustjay_projection::identity::BlitVertex {
                position: [1.0, 1.0],
                texcoord: [1.0, 0.0],
            },
        ];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Varda Source Stage VB"),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            blit,
            vertex_buffer,
            cached_bind_group: None,
            sync,
            last_version: 0,
        }
    }
}

#[cfg(feature = "projection")]
impl rustjay_projection::ProjectionStage for VardaSourceStage {
    fn label(&self) -> &str {
        "varda-source"
    }

    fn render(
        &mut self,
        ctx: &mut rustjay_core::RenderCtx<'_>,
        input: &wgpu::TextureView,
        _input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        _output_size: [u32; 2],
    ) {
        let (override_view, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.override_view.clone(), g.version)
        };

        let source = override_view.as_ref().map(|a| a.as_ref()).unwrap_or(input);

        if self.last_version != version || self.cached_bind_group.is_none() {
            self.last_version = version;
            self.cached_bind_group = Some(self.blit.create_bind_group(ctx.device, source));
        }

        let bind_group = self.cached_bind_group.as_ref().unwrap();
        self.blit
            .blit(ctx.encoder, bind_group, output, &self.vertex_buffer);
    }

    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {
        self.cached_bind_group = None;
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Dome stage wrapper — follows the WarpSync bridge pattern
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "projection")]
#[derive(Debug, Clone)]
pub struct DomeSync {
    pub enabled: bool,
    pub config: rustjay_projection::DomemasterConfig,
    pub content_rotation: [f32; 3],
    pub version: u64,
}

#[cfg(feature = "projection")]
impl Default for DomeSync {
    fn default() -> Self {
        Self {
            enabled: false,
            config: rustjay_projection::DomemasterConfig::default(),
            content_rotation: [0.0; 3],
            version: 0,
        }
    }
}

#[cfg(feature = "projection")]
pub struct VardaDomeStage {
    inner: rustjay_projection::DomeStage,
    bypass: rustjay_projection::IdentityStage,
    sync: std::sync::Arc<std::sync::Mutex<DomeSync>>,
    last_version: u64,
}

#[cfg(feature = "projection")]
impl VardaDomeStage {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sync: std::sync::Arc<std::sync::Mutex<DomeSync>>,
    ) -> Self {
        let config = {
            let g = sync.lock().unwrap_or_else(|e| e.into_inner());
            g.config.clone()
        };
        let inner = rustjay_projection::DomeStage::new(device, format, config);
        let bypass = rustjay_projection::IdentityStage::new(device, format);
        Self {
            inner,
            bypass,
            sync,
            last_version: 0,
        }
    }
}

#[cfg(feature = "projection")]
impl rustjay_projection::ProjectionStage for VardaDomeStage {
    fn label(&self) -> &str {
        "varda-dome"
    }

    fn render(
        &mut self,
        ctx: &mut rustjay_core::RenderCtx<'_>,
        input: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        output_size: [u32; 2],
    ) {
        let (enabled, config, rotation, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.enabled, g.config.clone(), g.content_rotation, g.version)
        };

        if version != self.last_version {
            self.last_version = version;
            self.inner.config = config;
            self.inner.content_rotation = rotation;
        }

        if enabled {
            self.inner
                .render(ctx, input, input_texture, output, output_size);
        } else {
            self.bypass
                .render(ctx, input, input_texture, output, output_size);
        }
    }

    fn on_input_changed(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        self.inner.on_input_changed(device, size);
    }

    fn is_active(&self) -> bool {
        let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
        g.enabled
    }
}

// ═════════════════════════════════════════════════════════════════════════════
// Edge-blend stage wrapper — follows the WarpSync bridge pattern
// ═════════════════════════════════════════════════════════════════════════════

#[cfg(feature = "projection")]
#[derive(Debug, Clone)]
pub struct EdgeBlendSync {
    pub config: rustjay_projection::EdgeBlendConfig,
    pub version: u64,
}

#[cfg(feature = "projection")]
impl Default for EdgeBlendSync {
    fn default() -> Self {
        Self {
            config: rustjay_projection::EdgeBlendConfig::default(),
            version: 0,
        }
    }
}

#[cfg(feature = "projection")]
pub struct VardaEdgeBlendStage {
    inner: rustjay_projection::EdgeBlendStage,
    bypass: rustjay_projection::IdentityStage,
    sync: std::sync::Arc<std::sync::Mutex<EdgeBlendSync>>,
    last_version: u64,
}

#[cfg(feature = "projection")]
impl VardaEdgeBlendStage {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        sync: std::sync::Arc<std::sync::Mutex<EdgeBlendSync>>,
    ) -> Self {
        let inner = rustjay_projection::EdgeBlendStage::new(device, format);
        let bypass = rustjay_projection::IdentityStage::new(device, format);
        Self {
            inner,
            bypass,
            sync,
            last_version: 0,
        }
    }
}

#[cfg(feature = "projection")]
impl rustjay_projection::ProjectionStage for VardaEdgeBlendStage {
    fn label(&self) -> &str {
        "varda-edge-blend"
    }

    fn is_active(&self) -> bool {
        self.inner.config.any_enabled()
    }

    fn render(
        &mut self,
        ctx: &mut rustjay_core::RenderCtx<'_>,
        input: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        output_size: [u32; 2],
    ) {
        let (config, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.config, g.version)
        };

        if version != self.last_version {
            self.last_version = version;
            self.inner.config = config;
        }

        if self.inner.config.any_enabled() {
            self.inner
                .render(ctx, input, input_texture, output, output_size);
        } else {
            self.bypass
                .render(ctx, input, input_texture, output, output_size);
        }
    }

    fn on_input_changed(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        self.inner.on_input_changed(device, size);
    }
}
