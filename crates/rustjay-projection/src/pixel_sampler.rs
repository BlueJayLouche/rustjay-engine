//! `PixelSampler` — atlas-based GPU pixel sampler for lighting output.
//!
//! Wraps a [`HeadlessOutput`] readback target and a [`SampleStage`] so that one
//! lighting output can pack all of its segments into a single small BGRA8 atlas
//! readback per frame.

use crate::headless::HeadlessOutput;
use crate::sample_stage::{AtlasLayout, SampleStage};

/// Stable identity for a pixel sampler. Survives output reordering/removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SamplerId(pub u64);

/// Owns the GPU resources for sampling one lighting output's segments.
pub struct PixelSampler {
    headless: HeadlessOutput,
    sample_stage: SampleStage,
    layout: AtlasLayout,
}

impl PixelSampler {
    /// Create a sampler for the given atlas layout.
    pub fn new(device: &wgpu::Device, layout: AtlasLayout) -> Self {
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let sample_stage = SampleStage::new(device, format, layout.clone());
        let headless = HeadlessOutput::new(
            device,
            layout.size[0].max(1),
            layout.size[1].max(1),
            Vec::new(), // we render via render_stage, not the internal chain
        );
        Self {
            headless,
            sample_stage,
            layout,
        }
    }

    /// Update the atlas layout, resizing the GPU target if necessary.
    pub fn set_layout(&mut self, device: &wgpu::Device, layout: AtlasLayout) {
        if self.layout == layout {
            return;
        }
        self.headless.resize(
            device,
            layout.size[0].max(1),
            layout.size[1].max(1),
        );
        self.sample_stage.set_layout(device, layout.clone());
        self.layout = layout;
    }

    /// Set per-segment source view overrides (aligned to the atlas tiles). A
    /// `Some` entry makes that segment sample the given texture (e.g. a mixer
    /// channel) instead of the master composite; `None` falls back to master.
    pub fn set_tile_sources(&mut self, sources: &[Option<std::sync::Arc<wgpu::TextureView>>]) {
        self.sample_stage.set_tile_sources(sources);
    }

    /// Render the source into the atlas and enqueue an async CPU readback.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_texture: Option<&wgpu::Texture>,
        source_size: [u32; 2],
    ) {
        self.headless.render_stage(
            device,
            queue,
            &mut self.sample_stage,
            source_view,
            source_texture,
            source_size,
        );
    }

    /// Latest completed atlas readback plus its layout.
    pub fn latest_atlas(&self) -> Option<(&[u8], &AtlasLayout)> {
        self.headless.latest_frame().map(|b| (b, &self.layout))
    }

    /// Current atlas size in pixels.
    pub fn size(&self) -> [u32; 2] {
        self.headless.size()
    }
}
