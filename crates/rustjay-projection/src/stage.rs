use rustjay_core::RenderCtx;

/// An object-safe stage in the output projection pipeline.
///
/// Each stage reads from an input texture and writes to an output texture.
/// Multi-input stages (e.g. dome cubemap) bind their fixed inputs as a single
/// bind group; they do not accept a slice of arbitrary inputs.
pub trait ProjectionStage: Send + 'static {
    /// Human-readable label for profiling and UI.
    fn label(&self) -> &str {
        "projection-stage"
    }

    /// Render this stage: read from `input`, write into `output`.
    ///
    /// `input` is the result of the previous stage (or the original composite
    /// for the first stage). `input_texture` is the underlying texture when
    /// available (some stages need it for copy operations). `output` is a
    /// texture the stage fully owns for this frame; it may be ping-ponged by
    /// the caller.
    fn render(
        &mut self,
        ctx: &mut RenderCtx<'_>,
        input: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        output: &wgpu::TextureView,
        output_size: [u32; 2],
    );

    /// Called when the input texture generation changes (resize, reallocation).
    fn on_input_changed(&mut self, _device: &wgpu::Device, _size: [u32; 2]) {}
}
