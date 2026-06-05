//! Projection subsystem — manages extra projector windows and stage chains.
//!
//! Only compiled when the `projection` feature is enabled.

use rustjay_projection::stage::ProjectionStage;
use rustjay_projection::HeadlessOutput;
use std::sync::Arc;
use winit::window::Window;

/// Opaque handle type for sharing the projection subsystem with the plugin.
pub type ProjectionSubsystemHandle = Arc<std::sync::Mutex<ProjectionSubsystem>>;

/// A single projector output window with its own stage chain.
pub struct ProjectorOutput {
    #[allow(dead_code)]
    window: Arc<Window>,
    /// The winit window ID for event routing.
    pub window_id: winit::window::WindowId,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    /// Post-processing stages applied to the main render target before
    /// presentation on this projector.  Each stage is 1-in / 1-out.
    stages: Vec<Box<dyn ProjectionStage>>,
    /// Ping-pong textures for the stage chain.
    textures: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    width: u32,
    height: u32,
    /// Dummy vertex buffer for `RenderCtx` (projection stages may ignore it).
    _dummy_vb: wgpu::Buffer,
}

impl ProjectorOutput {
    /// Create a new projector output from an existing window and wgpu surface.
    pub fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        stages: Vec<Box<dyn ProjectionStage>>,
    ) -> anyhow::Result<Self> {
        let window_id = window.id();
        let size = window.inner_size();
        let surface = instance.create_surface(Arc::clone(&window))?;

        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == wgpu::TextureFormat::Bgra8UnormSrgb || *f == wgpu::TextureFormat::Bgra8Unorm)
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow::anyhow!("No surface formats available for projector"))?;

        let present_mode = if caps.present_modes.contains(&wgpu::PresentMode::Immediate) {
            wgpu::PresentMode::Immediate
        } else {
            wgpu::PresentMode::Fifo
        };
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(device, &config);

        // Allocate ping-pong textures.  For N stages we need up to 2 intermediates
        // (the output of stage i becomes input of stage i+1).
        let mut textures = Vec::new();
        let mut views = Vec::new();
        if stages.len() > 1 {
            for _ in 0..2 {
                let t = create_intermediate(device, size.width, size.height, format);
                let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                textures.push(t);
                views.push(v);
            }
        } else if stages.len() == 1 {
            let t = create_intermediate(device, size.width, size.height, format);
            let v = t.create_view(&wgpu::TextureViewDescriptor::default());
            textures.push(t);
            views.push(v);
        }

        let dummy_vb = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Projector Dummy VB"),
            size: 64,
            usage: wgpu::BufferUsages::VERTEX,
            mapped_at_creation: false,
        });

        Ok(Self {
            window,
            window_id,
            surface,
            surface_config: config,
            stages,
            textures,
            views,
            width: size.width,
            height: size.height,
            _dummy_vb: dummy_vb,
        })
    }

    /// Resize the projector surface.
    pub fn resize(&mut self, width: u32, height: u32, device: &wgpu::Device) {
        if width > 0 && height > 0 && (width != self.width || height != self.height) {
            self.width = width;
            self.height = height;
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(device, &self.surface_config);

            // Recreate ping-pong textures at new size.
            let format = self.surface_config.format;
            self.textures.clear();
            self.views.clear();
            if self.stages.len() > 1 {
                for _ in 0..2 {
                    let t = create_intermediate(device, width, height, format);
                    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                    self.textures.push(t);
                    self.views.push(v);
                }
            } else if self.stages.len() == 1 {
                let t = create_intermediate(device, width, height, format);
                let v = t.create_view(&wgpu::TextureViewDescriptor::default());
                self.textures.push(t);
                self.views.push(v);
            }

            for stage in &mut self.stages {
                stage.on_input_changed(device, [width, height]);
            }
        }
    }

    /// Render the given input texture through the stage chain and present.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        input_view: &wgpu::TextureView,
        input_texture: Option<&wgpu::Texture>,
        _input_size: [u32; 2],
    ) {
        let surface_texture = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(st)
            | wgpu::CurrentSurfaceTexture::Suboptimal(st) => st,
            err => {
                log::warn!("Projector surface unavailable: {err:?}");
                return;
            }
        };

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Projector Stage Chain"),
        });

        let n = self.stages.len();
        if n == 0 {
            // No stages — just present (surface shows whatever was there).
            queue.submit(std::iter::once(encoder.finish()));
            surface_texture.present();
            return;
        }

        let surface_view = surface_texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let views = &self.views;

        for (i, stage) in self.stages.iter_mut().enumerate() {
            let is_first = i == 0;
            let is_last = i == n - 1;

            let in_view: &wgpu::TextureView = if is_first { input_view } else { &views[(i - 1) % views.len()] };
            let in_tex: Option<&wgpu::Texture> = if is_first { input_texture } else { Some(&self.textures[(i - 1) % views.len()]) };
            let out_view: &wgpu::TextureView = if is_last { &surface_view } else { &views[i % views.len()] };

            let mut ctx = rustjay_core::RenderCtx {
                device,
                queue,
                encoder: &mut encoder,
                vertex_buffer: &self._dummy_vb,
            };

            stage.render(&mut ctx, in_view, in_tex, out_view, [self.width, self.height]);
        }

        queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }

    /// Returns true if this projector owns the given window ID.
    pub fn owns_window(&self, id: winit::window::WindowId) -> bool {
        self.window_id == id
    }
}

fn create_intermediate(device: &wgpu::Device, width: u32, height: u32, format: wgpu::TextureFormat) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Projector Intermediate"),
        size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Subsystem that manages all projector outputs.
pub struct ProjectionSubsystem {
    /// Active projector windows.
    pub projectors: Vec<ProjectorOutput>,
    /// Active headless outputs (offscreen texture + async readback).
    pub headless_outputs: Vec<HeadlessOutput>,
    /// Staged projectors to create on next `resumed()`.
    pending: Vec<PendingProjector>,
    /// Last time projector outputs were rendered (for throttling).
    last_render: Option<std::time::Instant>,
    /// Shared GPU device, set once `create_pending()` has run.
    device: Option<Arc<wgpu::Device>>,
}

type StageFactory = Box<dyn FnOnce(&wgpu::Device, wgpu::TextureFormat) -> Vec<Box<dyn ProjectionStage>> + Send>;

struct PendingProjector {
    window_attrs: winit::window::WindowAttributes,
    stage_factory: StageFactory,
}

impl Default for ProjectionSubsystem {
    fn default() -> Self {
        Self::new()
    }
}

impl ProjectionSubsystem {
    /// Create an empty projection subsystem.
    pub fn new() -> Self {
        Self {
            projectors: Vec::new(),
            headless_outputs: Vec::new(),
            pending: Vec::new(),
            last_render: None,
            device: None,
        }
    }

    /// Queue a projector window to be created at the next opportunity.
    ///
    /// `stage_factory` is called once the wgpu surface format is known
    /// (inside `resumed()`), so stages can be constructed with the correct
    /// target format.
    pub fn add_projector(
        &mut self,
        window_attrs: winit::window::WindowAttributes,
        stage_factory: impl FnOnce(&wgpu::Device, wgpu::TextureFormat) -> Vec<Box<dyn ProjectionStage>> + Send + 'static,
    ) {
        self.pending.push(PendingProjector {
            window_attrs,
            stage_factory: Box::new(stage_factory),
        });
    }

    /// Number of projector windows waiting to be created.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Create any pending projector windows (call from `resumed()`).
    pub fn create_pending(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
    ) {
        self.device = Some(Arc::new(device.clone()));

        for pending in self.pending.drain(..) {
            let window = match event_loop.create_window(pending.window_attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    log::error!("Failed to create projector window: {e}");
                    continue;
                }
            };

            // Create a temporary surface just to query the format for the factory.
            let temp_surface = match instance.create_surface(Arc::clone(&window)) {
                Ok(s) => s,
                Err(e) => {
                    log::error!("Failed to create projector surface: {e}");
                    continue;
                }
            };
            let caps = temp_surface.get_capabilities(adapter);
            let format = caps
                .formats
                .iter()
                .copied()
                .find(|f| *f == wgpu::TextureFormat::Bgra8UnormSrgb || *f == wgpu::TextureFormat::Bgra8Unorm)
                .or_else(|| caps.formats.first().copied())
                .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
            drop(temp_surface);

            let stages = (pending.stage_factory)(device, format);
            match ProjectorOutput::new(window, instance, device, adapter, stages) {
                Ok(proj) => {
                    log::info!(
                        "Projector window created ({}x{})",
                        proj.width,
                        proj.height
                    );
                    self.projectors.push(proj);
                }
                Err(e) => {
                    log::error!("Failed to initialise projector output: {e}");
                }
            }
        }
    }

    /// Route a window event to the matching projector, if any.
    pub fn handle_window_event(
        &mut self,
        window_id: winit::window::WindowId,
        event: &winit::event::WindowEvent,
        device: &wgpu::Device,
    ) -> bool {
        let mut remove_id: Option<winit::window::WindowId> = None;
        for proj in &mut self.projectors {
            if proj.owns_window(window_id) {
                match event {
                    winit::event::WindowEvent::Resized(size) => {
                        proj.resize(size.width, size.height, device);
                    }
                    winit::event::WindowEvent::CloseRequested => {
                        remove_id = Some(proj.window_id);
                    }
                    _ => {}
                }
                break;
            }
        }
        if let Some(id) = remove_id {
            self.remove_output(id);
            return true;
        }
        remove_id.is_some()
    }

    /// Add a headless output (offscreen texture, no window).
    ///
    /// The output format is fixed to `Rgba8Unorm` (linear, non-sRGB).
    /// Requires that `create_pending()` has already been called so the
    /// shared device is available.
    pub fn add_headless_output(
        &mut self,
        width: u32,
        height: u32,
        stages: Vec<Box<dyn ProjectionStage>>,
    ) {
        let device = self.device.as_ref()
            .expect("add_headless_output called before create_pending");
        self.headless_outputs
            .push(HeadlessOutput::new(device, width, height, stages));
    }

    /// Remove a headless output by index.
    pub fn remove_headless_output(&mut self, index: usize) {
        if index < self.headless_outputs.len() {
            self.headless_outputs.remove(index);
        }
    }

    /// Returns the latest readback frame for a headless output, if available.
    pub fn headless_frame(&self, index: usize) -> Option<&[u8]> {
        self.headless_outputs.get(index)?.latest_frame()
    }

    /// Render all projector and headless outputs from the given source texture.
    /// Throttled to ~60 Hz to avoid burning CPU/GPU on unbounded polls.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source_view: &wgpu::TextureView,
        source_texture: Option<&wgpu::Texture>,
        source_size: [u32; 2],
    ) {
        const MIN_INTERVAL: std::time::Duration = std::time::Duration::from_micros(16_666); // ~60 Hz
        let now = std::time::Instant::now();
        if let Some(last) = self.last_render {
            if now.duration_since(last) < MIN_INTERVAL {
                return;
            }
        }
        self.last_render = Some(now);
        for proj in &mut self.projectors {
            proj.render(device, queue, source_view, source_texture, source_size);
        }
        for headless in &mut self.headless_outputs {
            headless.render(device, queue, source_view, source_texture, source_size);
        }
    }

    /// Remove a projector output by its window ID, dropping the surface and window.
    pub fn remove_output(&mut self, window_id: winit::window::WindowId) {
        self.projectors.retain(|p| p.window_id != window_id);
    }
}
