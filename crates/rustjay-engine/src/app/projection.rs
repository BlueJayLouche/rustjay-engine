//! Projection subsystem — manages extra projector windows and stage chains.
//!
//! Only compiled when the `projection` feature is enabled.

use rustjay_projection::stage::ProjectionStage;
use rustjay_projection::HeadlessOutput;
use rustjay_io::{OutputManager, Recorder, RecorderCodec};
use std::path::Path;
use std::sync::Arc;
use winit::window::Window;

/// Opaque handle type for sharing the projection subsystem with the plugin.
pub type ProjectionSubsystemHandle = Arc<std::sync::Mutex<ProjectionSubsystem>>;

/// A single projector output window with its own stage chain.
pub struct ProjectorOutput {
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
    /// Whether this projector is currently fullscreen.
    fullscreen: bool,
    /// Offscreen texture used for recording (copy from surface before present).
    record_texture: Option<wgpu::Texture>,
    /// Output manager with active recorder for this projector.
    record_manager: Option<OutputManager>,
}

impl ProjectorOutput {
    /// Create a new projector output from an existing window and wgpu surface.
    pub fn new(
        window: Arc<Window>,
        instance: &wgpu::Instance,
        device: &wgpu::Device,
        adapter: &wgpu::Adapter,
        stages: Vec<Box<dyn ProjectionStage>>,
        fullscreen: bool,
    ) -> anyhow::Result<Self> {
        let window_id = window.id();
        let size = window.inner_size();
        let surface = instance.create_surface(Arc::clone(&window))?;

        let caps = surface.get_capabilities(adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                *f == wgpu::TextureFormat::Bgra8UnormSrgb || *f == wgpu::TextureFormat::Bgra8Unorm
            })
            .or_else(|| caps.formats.first().copied())
            .ok_or_else(|| anyhow::anyhow!("No surface formats available for projector"))?;

        // Use Fifo (vsync) for projector outputs to eliminate tearing.
        // Immediate would lower latency but causes visible tearing on multi-monitor setups.
        let present_mode = wgpu::PresentMode::Fifo;
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
            fullscreen,
            record_texture: None,
            record_manager: None,
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
            // Drop stale record texture so it is recreated at the new size.
            self.record_texture = None;

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

        let active_count = self.stages.iter().filter(|s| s.is_active()).count();
        if active_count == 0 {
            // No active stages — just present (surface shows whatever was there).
            queue.submit(std::iter::once(encoder.finish()));
            surface_texture.present();
            return;
        }

        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let views = &self.views;

        let mut active_idx = 0;
        let mut current_input_view = input_view;
        let mut current_input_texture = input_texture;

        for stage in self.stages.iter_mut() {
            if !stage.is_active() {
                continue;
            }

            let is_last_active = active_idx == active_count - 1;
            let out_view: &wgpu::TextureView = if is_last_active {
                &surface_view
            } else {
                &views[active_idx % views.len()]
            };

            let mut ctx = rustjay_core::RenderCtx {
                device,
                queue,
                encoder: &mut encoder,
                vertex_buffer: &self._dummy_vb,
            };

            stage.render(
                &mut ctx,
                current_input_view,
                current_input_texture,
                out_view,
                [self.width, self.height],
            );

            current_input_view = out_view;
            if !is_last_active {
                current_input_texture = Some(&self.textures[active_idx % views.len()]);
            }
            active_idx += 1;
        }

        queue.submit(std::iter::once(encoder.finish()));

        // Copy surface to record texture before presenting, if recording.
        if self.record_manager.is_some() {
            if self.record_texture.is_none() {
                self.record_texture = Some(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Projector Record Texture"),
                    size: wgpu::Extent3d {
                        width: self.width.max(1),
                        height: self.height.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: self.surface_config.format,
                    usage: wgpu::TextureUsages::COPY_DST
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC,
                    view_formats: &[],
                }));
            }
            if let Some(ref record_tex) = self.record_texture {
                let mut copy_encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Projector Record Copy"),
                });
                copy_encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &surface_texture.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: record_tex,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: self.width,
                        height: self.height,
                        depth_or_array_layers: 1,
                    },
                );
                queue.submit(std::iter::once(copy_encoder.finish()));
                if let Some(ref mut manager) = self.record_manager {
                    manager.submit_frame(record_tex, device, queue);
                }
            }
        }

        surface_texture.present();
    }

    /// Start recording this projector's output to disk.
    pub fn start_recording(
        &mut self,
        path: &Path,
        fps: f32,
        codec: RecorderCodec,
    ) -> anyhow::Result<()> {
        if self.record_manager.is_some() {
            return Err(anyhow::anyhow!("Projector already recording"));
        }
        let mut manager = OutputManager::new();
        manager.start_recording(path, self.width, self.height, fps, codec)?;
        self.record_manager = Some(manager);
        log::info!(
            "Started projector recording {}x{} @ {:.2} fps → {}",
            self.width,
            self.height,
            fps,
            path.display()
        );
        Ok(())
    }

    /// Stop recording this projector's output.
    pub fn stop_recording(&mut self) {
        if let Some(mut manager) = self.record_manager.take() {
            manager.stop_recording();
            log::info!("Stopped projector recording");
        }
    }

    /// Whether this projector is currently recording.
    pub fn is_recording(&self) -> bool {
        self.record_manager.is_some()
    }

    /// Toggle fullscreen for this projector window.
    pub fn toggle_fullscreen(&mut self) {
        self.fullscreen = !self.fullscreen;
        let mode = if self.fullscreen {
            Some(winit::window::Fullscreen::Borderless(None))
        } else {
            None
        };
        self.window.set_fullscreen(mode);
        self.window.set_cursor_visible(false);
    }

    /// Hide the cursor on this projector window.
    pub fn hide_cursor(&self) {
        self.window.set_cursor_visible(false);
    }

    /// Returns true if this projector owns the given window ID.
    pub fn owns_window(&self, id: winit::window::WindowId) -> bool {
        self.window_id == id
    }
}

fn create_intermediate(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Projector Intermediate"),
        size: wgpu::Extent3d {
            width: width.max(1),
            height: height.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// Subsystem that manages all projector outputs.
pub struct ProjectionSubsystem {
    /// Active projector windows.
    pub projectors: Vec<ProjectorOutput>,
    /// Active headless outputs (offscreen texture + async readback).
    pub headless_outputs: Vec<HeadlessOutput>,
    /// Per-headless disk recorders (RGBA→BGRA swizzled).
    headless_recorders: Vec<Option<Recorder>>,
    /// Staged projectors to create on next `resumed()`.
    pending: Vec<PendingProjector>,
    /// Last time projector outputs were rendered (for throttling).
    last_render: Option<std::time::Instant>,
    /// Shared GPU device, set once `create_pending()` has run.
    device: Option<Arc<wgpu::Device>>,
}

type StageFactory =
    Box<dyn FnOnce(&wgpu::Device, wgpu::TextureFormat) -> Vec<Box<dyn ProjectionStage>> + Send>;

struct PendingProjector {
    window_attrs: winit::window::WindowAttributes,
    stage_factory: StageFactory,
    /// Monitor index for fullscreen, or `None` for windowed.
    fullscreen_monitor: Option<usize>,
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
            headless_recorders: Vec::new(),
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
        fullscreen_monitor: Option<usize>,
        stage_factory: impl FnOnce(&wgpu::Device, wgpu::TextureFormat) -> Vec<Box<dyn ProjectionStage>>
            + Send
            + 'static,
    ) {
        self.pending.push(PendingProjector {
            window_attrs,
            stage_factory: Box::new(stage_factory),
            fullscreen_monitor,
        });
    }

    /// Number of projector windows waiting to be created.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Clear all pending projector creation requests.
    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// Create any pending projector windows (call from `resumed()`).
    pub fn create_pending(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        instance: &wgpu::Instance,
        device: Arc<wgpu::Device>,
        adapter: &wgpu::Adapter,
    ) {
        self.device = Some(Arc::clone(&device));

        for pending in self.pending.drain(..) {
            let window = match event_loop.create_window(pending.window_attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    log::error!("Failed to create projector window: {e}");
                    continue;
                }
            };

            // Apply fullscreen if requested.
            let mut fullscreen = false;
            if let Some(idx) = pending.fullscreen_monitor {
                let monitors: Vec<_> = event_loop.available_monitors().collect();
                if let Some(monitor) = monitors.get(idx) {
                    window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(Some(
                        monitor.clone(),
                    ))));
                    fullscreen = true;
                    log::info!("Projector fullscreen on monitor {}", idx);
                } else {
                    log::warn!(
                        "Projector requested fullscreen on monitor {} but only {} available",
                        idx,
                        monitors.len()
                    );
                }
            }
            window.set_cursor_visible(false);

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
                .find(|f| {
                    *f == wgpu::TextureFormat::Bgra8UnormSrgb
                        || *f == wgpu::TextureFormat::Bgra8Unorm
                })
                .or_else(|| caps.formats.first().copied())
                .unwrap_or(wgpu::TextureFormat::Bgra8Unorm);
            drop(temp_surface);

            let stages = (pending.stage_factory)(&device, format);
            match ProjectorOutput::new(window, instance, &device, adapter, stages, fullscreen) {
                Ok(proj) => {
                    log::info!("Projector window created ({}x{})", proj.width, proj.height);
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
        shift_pressed: &mut bool,
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
                    winit::event::WindowEvent::CursorEntered { .. } => {
                        proj.hide_cursor();
                    }
                    winit::event::WindowEvent::KeyboardInput { ref event, .. } => {
                        if let winit::keyboard::Key::Named(winit::keyboard::NamedKey::Shift) =
                            &event.logical_key
                        {
                            *shift_pressed =
                                event.state == winit::event::ElementState::Pressed;
                        }
                        if event.state == winit::event::ElementState::Pressed {
                            if let winit::keyboard::Key::Character(ch) = &event.logical_key {
                                if *shift_pressed && ch.to_lowercase() == "f" {
                                    proj.toggle_fullscreen();
                                }
                            }
                        }
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
        let Some(device) = self.device.as_ref() else {
            log::error!(
                "add_headless_output called before create_pending — no GPU device available"
            );
            return;
        };
        self.headless_outputs
            .push(HeadlessOutput::new(device, width, height, stages));
        self.headless_recorders.push(None);
    }

    /// Remove a headless output by index.
    pub fn remove_headless_output(&mut self, index: usize) {
        if index < self.headless_outputs.len() {
            self.headless_outputs.remove(index);
            if index < self.headless_recorders.len() {
                if let Some(rec) = self.headless_recorders.remove(index) {
                    if let Err(e) = rec.finish() {
                        log::warn!("[Headless {index}] recorder finish on remove failed: {e}");
                    }
                }
            }
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
        for (i, headless) in self.headless_outputs.iter_mut().enumerate() {
            headless.render(device, queue, source_view, source_texture, source_size);
            headless.poll_readback(device);
            if let Some(ref mut rec) = self.headless_recorders.get_mut(i).and_then(|r| r.as_mut()) {
                if let Some(pixels) = headless.latest_frame() {
                    let width = headless.size()[0];
                    let height = headless.size()[1];
                    let mut bgra = vec![0u8; (width * height * 4) as usize];
                    for y in 0..height {
                        for x in 0..width {
                            let src_idx = ((y * width + x) * 4) as usize;
                            let dst_idx = ((y * width + x) * 4) as usize;
                            // RGBA -> BGRA
                            bgra[dst_idx] = pixels[src_idx + 2];
                            bgra[dst_idx + 1] = pixels[src_idx + 1];
                            bgra[dst_idx + 2] = pixels[src_idx];
                            bgra[dst_idx + 3] = pixels[src_idx + 3];
                        }
                    }
                    if !rec.encode_frame(&bgra) {
                        log::warn!("[Headless {i}] encoder failed — stopping");
                        self.headless_recorders[i] = None;
                    }
                }
            }
        }
    }

    /// Start recording a projector output by index.
    pub fn start_projector_recording(
        &mut self,
        index: usize,
        path: &Path,
        fps: f32,
        codec: RecorderCodec,
    ) -> anyhow::Result<()> {
        let proj = self.projectors.get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("Projector index {index} out of range"))?;
        proj.start_recording(path, fps, codec)
    }

    /// Stop recording a projector output by index.
    pub fn stop_projector_recording(&mut self, index: usize) {
        if let Some(proj) = self.projectors.get_mut(index) {
            proj.stop_recording();
        }
    }

    /// Whether a projector is recording.
    pub fn is_projector_recording(&self, index: usize) -> bool {
        self.projectors.get(index).map_or(false, |p| p.is_recording())
    }

    /// Start recording a headless output by index.
    pub fn start_headless_recording(
        &mut self,
        index: usize,
        path: &Path,
        fps: f32,
        codec: RecorderCodec,
    ) -> anyhow::Result<()> {
        if index >= self.headless_outputs.len() {
            return Err(anyhow::anyhow!("Headless index {index} out of range"));
        }
        let headless = &self.headless_outputs[index];
        let [width, height] = headless.size();
        let rec = Recorder::start(path, width, height, fps, codec)?;
        while self.headless_recorders.len() < self.headless_outputs.len() {
            self.headless_recorders.push(None);
        }
        self.headless_recorders[index] = Some(rec);
        log::info!(
            "Started headless recording {}x{} @ {:.2} fps → {}",
            width,
            height,
            fps,
            path.display()
        );
        Ok(())
    }

    /// Stop recording a headless output by index.
    pub fn stop_headless_recording(&mut self, index: usize) {
        if let Some(slot) = self.headless_recorders.get_mut(index) {
            if let Some(rec) = slot.take() {
                if let Err(e) = rec.finish() {
                    log::warn!("[Headless {index}] recorder finish failed: {e}");
                }
                log::info!("Stopped headless recording [{index}]");
            }
        }
    }

    /// Whether a headless output is recording.
    pub fn is_headless_recording(&self, index: usize) -> bool {
        self.headless_recorders.get(index).and_then(|r| r.as_ref()).is_some()
    }

    /// Remove a projector output by its window ID, dropping the surface and window.
    pub fn remove_output(&mut self, window_id: winit::window::WindowId) {
        self.projectors.retain(|p| p.window_id != window_id);
    }
}
