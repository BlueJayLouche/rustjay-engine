//! Stage — surfaces, outputs, and projection mapping state.
//!
//! Delegates to `rustjay-projection` for warp, edge-blend, and dome.
//! See VARDA_PORT.md Phase 7–8.

use serde::{Deserialize, Serialize};

/// Assignment of a surface to an output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceAssignment {
    /// UUID of the assigned surface.
    pub surface_uuid: String,
    /// Whether this assignment is active.
    pub enabled: bool,
}

impl SurfaceAssignment {
    pub fn new(surface_uuid: String) -> Self {
        Self {
            surface_uuid,
            enabled: true,
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
    /// Warp mode (corner-pin or mesh).
    #[cfg(feature = "projection")]
    pub warp: rustjay_projection::WarpMode,
    #[cfg(not(feature = "projection"))]
    pub warp: (),
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
            #[cfg(feature = "projection")]
            warp: rustjay_projection::WarpMode::identity(),
            #[cfg(not(feature = "projection"))]
            warp: (),
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
}

impl VardaStage {
    pub fn new() -> Self {
        Self {
            surfaces: Vec::new(),
            canvas_size: [1920, 1080],
            projectors: Vec::new(),
            headless_outputs: Vec::new(),
            #[cfg(feature = "projection")]
            warp_sync: None,
            #[cfg(feature = "projection")]
            dome_sync: None,
            #[cfg(feature = "projection")]
            edge_blend_sync: None,
        }
    }

    pub fn with_default_surface() -> Self {
        let mut stage = Self::new();
        stage
            .surfaces
            .push(VardaSurface::full_frame("Main", "main"));
        // One default projector
        stage.projectors.push(VardaProjector::default());
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
    ///
    /// Also broadcasts to every per-output warp_sync so unassigned outputs pick
    /// up the edit (S1).
    #[cfg(feature = "projection")]
    pub fn publish_warp(&self) {
        let fallback = self
            .surfaces
            .iter()
            .find(|s| s.source == SurfaceSource::Master)
            .or_else(|| self.surfaces.first());
        let warp = fallback.map(|s| s.warp.clone());

        // Global sync (backward compatibility)
        if let Some(sync) = &self.warp_sync {
            resolve_and_publish_warp(sync, None, &self.surfaces, warp.as_ref());
        }

        // Broadcast to all per-projector syncs
        for proj in &self.projectors {
            if let Some(sync) = &proj.warp_sync {
                resolve_and_publish_warp(sync, Some(&proj.surface_assignments), &self.surfaces, warp.as_ref());
            }
        }

        // Broadcast to all per-headless syncs
        for hl in &self.headless_outputs {
            if let Some(sync) = &hl.warp_sync {
                resolve_and_publish_warp(sync, Some(&hl.surface_assignments), &self.surfaces, warp.as_ref());
            }
        }
    }
}

/// Resolve a warp for an output and publish it into its `WarpSync`, bumping
/// version only when the mode actually changed (S2).
#[cfg(feature = "projection")]
pub fn resolve_and_publish_warp(
    sync: &std::sync::Arc<std::sync::Mutex<WarpSync>>,
    assignments: Option<&[SurfaceAssignment]>,
    surfaces: &[VardaSurface],
    fallback_warp: Option<&rustjay_projection::WarpMode>,
) {
    let warp = if let Some(assignments) = assignments {
        assignments
            .iter()
            .filter(|a| a.enabled)
            .find_map(|a| surfaces.iter().find(|s| s.uuid == a.surface_uuid))
            .map(|s| &s.warp)
            .or(fallback_warp)
            .cloned()
    } else {
        fallback_warp.cloned()
    };

    if let Some(warp) = warp {
        if let Ok(mut g) = sync.lock() {
            if g.mode != warp {
                g.mode = warp;
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
    /// Surface assignments — which surfaces this output renders.
    /// Empty = render all surfaces (fallback to first Master surface).
    #[serde(default)]
    pub surface_assignments: Vec<SurfaceAssignment>,
    /// Per-projector warp sync (injected at runtime, not serialized).
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub warp_sync: Option<std::sync::Arc<std::sync::Mutex<WarpSync>>>,
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
            surface_assignments: Vec::new(),
            #[cfg(feature = "projection")]
            warp_sync: None,
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

/// Configuration for a headless (offscreen) output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VardaHeadlessConfig {
    pub name: String,
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Surface assignments — which surfaces this output renders.
    #[serde(default)]
    pub surface_assignments: Vec<SurfaceAssignment>,
    /// Per-output warp sync (injected at runtime, not serialized).
    #[cfg(feature = "projection")]
    #[serde(skip)]
    pub warp_sync: Option<std::sync::Arc<std::sync::Mutex<WarpSync>>>,
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
            surface_assignments: Vec::new(),
            #[cfg(feature = "projection")]
            warp_sync: None,
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
    pub version: u64,
}

#[cfg(feature = "projection")]
impl Default for WarpSync {
    fn default() -> Self {
        Self {
            mode: rustjay_projection::WarpMode::identity(),
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
        let inner = rustjay_projection::WarpStage::from_mode(device, format, &mode);
        Self {
            inner,
            format,
            sync,
            last_version: version,
            inner_is_corner_pin,
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
        let (mode, version) = {
            let g = self.sync.lock().unwrap_or_else(|e| e.into_inner());
            (g.mode.clone(), g.version)
        };
        if version != self.last_version {
            self.last_version = version;
            match &mode {
                // Same mode family → cheap homography update (no rebuild on drag).
                rustjay_projection::WarpMode::CornerPin { corners } if self.inner_is_corner_pin => {
                    let src = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
                    let h = rustjay_projection::compute_forward_homography(&src, corners);
                    self.inner.set_homography(ctx.queue, &h);
                }
                // Mode switch or mesh edit → rebuild the warp stage.
                _ => {
                    self.inner =
                        rustjay_projection::WarpStage::from_mode(ctx.device, self.format, &mode);
                    self.inner_is_corner_pin =
                        matches!(mode, rustjay_projection::WarpMode::CornerPin { .. });
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
