//! Projection subsystem — manages extra projector windows and stage chains.
//!
//! Only compiled when the `projection` feature is enabled.

use rustjay_projection::stage::ProjectionStage;
use rustjay_projection::{AtlasLayout, HeadlessOutput, PixelSampler, SamplerId};
use rustjay_io::{OutputManager, RecorderCodec};
use std::path::Path;
use std::sync::Arc;
use winit::window::Window;

/// Opaque handle type for sharing the projection subsystem with the plugin.
pub type ProjectionSubsystemHandle = Arc<std::sync::Mutex<ProjectionSubsystem>>;

/// A single projector output window with its own stage chain.
pub struct ProjectorOutput {
    window: Arc<Window>,
    pub window_id: winit::window::WindowId,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,
    /// Post-processing stages applied to the main render target before
    /// presentation on this projector.  Each stage is 1-in / 1-out.
    stages: Vec<Box<dyn ProjectionStage>>,
    textures: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    width: u32,
    height: u32,
    /// Dummy vertex buffer for `RenderCtx` (projection stages may ignore it).
    _dummy_vb: wgpu::Buffer,
    fullscreen: bool,
    /// Offscreen texture used for recording (copy from surface before present).
    record_texture: Option<wgpu::Texture>,
    record_manager: Option<OutputManager>,
}

impl ProjectorOutput {
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
        // Recording copies from the surface texture (`copy_texture_to_texture`),
        // which requires COPY_SRC on the surface. Request it when the platform
        // supports it; otherwise recording is skipped at copy time rather than
        // panicking the whole app (see the `record_manager` block in `render`).
        let mut surface_usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            surface_usage |= wgpu::TextureUsages::COPY_SRC;
        } else {
            log::warn!(
                "Projector surface does not support COPY_SRC; output recording will be unavailable"
            );
        }
        let config = wgpu::SurfaceConfiguration {
            usage: surface_usage,
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

    pub fn resize(&mut self, width: u32, height: u32, device: &wgpu::Device) {
        if width > 0 && height > 0 && (width != self.width || height != self.height) {
            self.width = width;
            self.height = height;
            self.surface_config.width = width;
            self.surface_config.height = height;
            self.surface.configure(device, &self.surface_config);
            // Drop stale record texture so it is recreated at the new size.
            self.record_texture = None;

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

        // Copy surface to the output texture before presenting, if any output
        // sink (recorder, NDI, Syphon, …) is active.
        // Guard on COPY_SRC: without it `copy_texture_to_texture` from the
        // surface is a fatal wgpu validation error, so skip the copy instead.
        let has_output = self
            .record_manager
            .as_ref()
            .map_or(false, |m| m.has_active_output());
        if has_output
            && self
                .surface_config
                .usage
                .contains(wgpu::TextureUsages::COPY_SRC)
        {
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

    /// The projector's output manager, lazily created. Hosts the recorder and
    /// all senders (NDI/Syphon/Spout/V4L2) — one frame is fanned out to every
    /// active output by `OutputManager::submit_frame`.
    fn output_manager(&mut self) -> &mut OutputManager {
        self.record_manager.get_or_insert_with(OutputManager::new)
    }

    pub fn start_recording(
        &mut self,
        path: &Path,
        fps: f32,
        codec: RecorderCodec,
    ) -> anyhow::Result<()> {
        let (w, h) = (self.width, self.height);
        let mgr = self.output_manager();
        if mgr.is_recording() {
            return Ok(());
        }
        mgr.start_recording(path, w, h, fps, codec)?;
        log::info!(
            "Started projector recording {}x{} @ {:.2} fps → {}",
            w,
            h,
            fps,
            path.display()
        );
        Ok(())
    }

    /// Stop recording this projector's output (leaves other sinks running).
    pub fn stop_recording(&mut self) {
        if let Some(manager) = self.record_manager.as_mut() {
            if manager.is_recording() {
                manager.stop_recording();
                log::info!("Stopped projector recording");
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.record_manager
            .as_ref()
            .map_or(false, |m| m.is_recording())
    }

    pub fn start_ndi(&mut self, name: &str) -> anyhow::Result<()> {
        let (w, h) = (self.width, self.height);
        let mgr = self.output_manager();
        if mgr.is_ndi_active() {
            return Ok(());
        }
        mgr.start_ndi(name, w, h, false)
    }

    /// Stop the NDI sender (leaves other sinks running).
    pub fn stop_ndi(&mut self) {
        if let Some(manager) = self.record_manager.as_mut() {
            manager.stop_ndi();
        }
    }

    pub fn is_ndi(&self) -> bool {
        self.record_manager
            .as_ref()
            .map_or(false, |m| m.is_ndi_active())
    }

    #[cfg(target_os = "macos")]
    pub fn start_syphon(
        &mut self,
        name: &str,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> anyhow::Result<()> {
        let mgr = self.output_manager();
        if mgr.is_syphon_active() {
            return Ok(());
        }
        mgr.start_syphon(name, device, queue)
    }

    /// Stop the Syphon server (macOS; leaves other sinks running).
    #[cfg(target_os = "macos")]
    pub fn stop_syphon(&mut self) {
        if let Some(manager) = self.record_manager.as_mut() {
            manager.stop_syphon();
        }
    }

    pub fn is_syphon(&self) -> bool {
        self.record_manager
            .as_ref()
            .map_or(false, |m| m.is_syphon_active())
    }

    /// Start publishing this projector's output via Spout (Windows).
    #[cfg(target_os = "windows")]
    pub fn start_spout(&mut self, name: &str) -> anyhow::Result<()> {
        let mgr = self.output_manager();
        if mgr.is_spout_active() {
            return Ok(());
        }
        mgr.start_spout(name)
    }

    /// Stop the Spout sender (Windows; leaves other sinks running).
    #[cfg(target_os = "windows")]
    pub fn stop_spout(&mut self) {
        if let Some(manager) = self.record_manager.as_mut() {
            manager.stop_spout();
        }
    }

    pub fn is_spout(&self) -> bool {
        self.record_manager
            .as_ref()
            .map_or(false, |m| m.is_spout_active())
    }

    #[cfg(target_os = "linux")]
    pub fn start_v4l2(&mut self, device_path: &str) -> anyhow::Result<()> {
        let (w, h) = (self.width, self.height);
        let mgr = self.output_manager();
        if mgr.is_v4l2_active() {
            return Ok(());
        }
        mgr.start_v4l2(device_path, w, h)
    }

    /// Stop the V4L2 sender (Linux; leaves other sinks running).
    #[cfg(target_os = "linux")]
    pub fn stop_v4l2(&mut self) {
        if let Some(manager) = self.record_manager.as_mut() {
            manager.stop_v4l2();
        }
    }

    pub fn is_v4l2(&self) -> bool {
        self.record_manager
            .as_ref()
            .map_or(false, |m| m.is_v4l2_active())
    }

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

    pub fn hide_cursor(&self) {
        self.window.set_cursor_visible(false);
    }

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
    pub projectors: Vec<ProjectorOutput>,
    /// Active headless outputs (offscreen texture + async readback).
    pub headless_outputs: Vec<HeadlessOutput>,
    /// Per-headless output managers (recorder + NDI/Syphon/Spout/V4L2 senders),
    /// fed the BGRA offscreen texture each frame. Parallel to `headless_outputs`.
    headless_managers: Vec<Option<OutputManager>>,
    /// Pixel samplers for lighting output: one atlas per lighting output that
    /// packs all of its segments into a single small BGRA8 readback. Separate
    /// from `headless_outputs` so lighting samplers don't appear as user outputs.
    sampler_outputs: std::collections::HashMap<SamplerId, PixelSampler>,
    next_sampler_id: u64,
    /// Staged projectors to create on next `resumed()`.
    pending: Vec<PendingProjector>,
    /// Last time projector outputs were rendered (for throttling).
    last_render: Option<std::time::Instant>,
    device: Option<Arc<wgpu::Device>>,
    /// Shared GPU queue, set once `create_pending()` has run. Needed to start
    /// Syphon output senders (which require owned `Arc` handles).
    queue: Option<Arc<wgpu::Queue>>,
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
    pub fn new() -> Self {
        Self {
            projectors: Vec::new(),
            headless_outputs: Vec::new(),
            headless_managers: Vec::new(),
            sampler_outputs: std::collections::HashMap::new(),
            next_sampler_id: 1,
            pending: Vec::new(),
            last_render: None,
            device: None,
            queue: None,
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

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    pub fn clear_pending(&mut self) {
        self.pending.clear();
    }

    pub fn create_pending(
        &mut self,
        event_loop: &winit::event_loop::ActiveEventLoop,
        instance: &wgpu::Instance,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        adapter: &wgpu::Adapter,
    ) {
        self.device = Some(Arc::clone(&device));
        self.queue = Some(queue);

        for pending in self.pending.drain(..) {
            let window = match event_loop.create_window(pending.window_attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    log::error!("Failed to create projector window: {e}");
                    continue;
                }
            };

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
                    winit::event::WindowEvent::KeyboardInput { event, .. } => {
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
        self.headless_managers.push(None);
    }

    pub fn remove_headless_output(&mut self, index: usize) {
        if index < self.headless_outputs.len() {
            self.headless_outputs.remove(index);
            if index < self.headless_managers.len() {
                if let Some(mut mgr) = self.headless_managers.remove(index) {
                    mgr.shutdown();
                }
            }
        }
    }

    /// The output manager for headless output `index`, created on demand. `None`
    /// if the index is out of range.
    fn ensure_headless_manager(&mut self, index: usize) -> Option<&mut OutputManager> {
        if index >= self.headless_outputs.len() {
            return None;
        }
        while self.headless_managers.len() < self.headless_outputs.len() {
            self.headless_managers.push(None);
        }
        Some(self.headless_managers[index].get_or_insert_with(OutputManager::new))
    }

    pub fn headless_frame(&self, index: usize) -> Option<&[u8]> {
        self.headless_outputs.get(index)?.latest_frame()
    }

    /// Add a pixel sampler for the given atlas layout. Returns a stable id that
    /// survives output reordering/removal, or `None` if the GPU device is not yet
    /// available.
    pub fn add_pixel_sampler(&mut self, layout: AtlasLayout) -> Option<SamplerId> {
        let Some(device) = self.device.clone() else {
            log::error!("add_pixel_sampler called before create_pending — no GPU device available");
            return None;
        };
        let id = SamplerId(self.next_sampler_id);
        self.next_sampler_id += 1;
        self.sampler_outputs.insert(id, PixelSampler::new(&device, layout));
        Some(id)
    }

    pub fn update_pixel_sampler(&mut self, id: SamplerId, layout: AtlasLayout) {
        let Some(device) = self.device.clone() else {
            return;
        };
        if let Some(s) = self.sampler_outputs.get_mut(&id) {
            s.set_layout(&device, layout);
        }
    }

    /// Set per-segment source view overrides for a sampler (aligned to its atlas
    /// tiles). Lets each segment sample its surface's source texture (e.g. a
    /// mixer channel) instead of the master composite. `None` entries fall back
    /// to master.
    pub fn set_sampler_tile_sources(
        &mut self,
        id: SamplerId,
        sources: &[Option<Arc<wgpu::TextureView>>],
    ) {
        if let Some(s) = self.sampler_outputs.get_mut(&id) {
            s.set_tile_sources(sources);
        }
    }

    pub fn remove_pixel_sampler(&mut self, id: SamplerId) {
        self.sampler_outputs.remove(&id);
    }

    pub fn remove_stale_pixel_samplers(&mut self, active: &std::collections::HashSet<SamplerId>) {
        self.sampler_outputs.retain(|id, _| active.contains(id));
    }

    pub fn pixel_sampler_atlas(&self, id: SamplerId) -> Option<(&[u8], &AtlasLayout)> {
        self.sampler_outputs.get(&id)?.latest_atlas()
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
        for i in 0..self.headless_outputs.len() {
            self.headless_outputs[i].render(device, queue, source_view, source_texture, source_size);
            // The BGRA offscreen feeds the recorder and every sender directly,
            // no swizzle. `submit_frame` does its own async readback for the
            // CPU-path sinks (NDI/V4L2/recorder) and hands Syphon the texture.
            let active = self
                .headless_managers
                .get(i)
                .and_then(|m| m.as_ref())
                .map_or(false, |m| m.has_active_output());
            if active {
                let tex = self.headless_outputs[i].output_texture();
                if let Some(Some(mgr)) = self.headless_managers.get_mut(i) {
                    mgr.submit_frame(tex, device, queue);
                }
            }
        }
        // Pixel samplers: pack each output's segments into an atlas, render it,
        // and enqueue a readback. `pixel_sampler_atlas` exposes the result to the
        // lighting reconcile loop.
        for s in self.sampler_outputs.values_mut() {
            s.render(device, queue, source_view, source_texture, source_size);
        }
    }

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

    pub fn stop_projector_recording(&mut self, index: usize) {
        if let Some(proj) = self.projectors.get_mut(index) {
            proj.stop_recording();
        }
    }

    pub fn is_projector_recording(&self, index: usize) -> bool {
        self.projectors.get(index).map_or(false, |p| p.is_recording())
    }

    // ── Projector output senders (NDI / Syphon / Spout / V4L2) ──────────────
    // Index is the position within the *enabled* projector list, matching the
    // recording methods above.

    pub fn start_projector_ndi(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let proj = self
            .projectors
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("Projector index {index} out of range"))?;
        proj.start_ndi(name)
    }

    pub fn stop_projector_ndi(&mut self, index: usize) {
        if let Some(proj) = self.projectors.get_mut(index) {
            proj.stop_ndi();
        }
    }

    pub fn is_projector_ndi(&self, index: usize) -> bool {
        self.projectors.get(index).map_or(false, |p| p.is_ndi())
    }

    #[cfg(target_os = "macos")]
    pub fn start_projector_syphon(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let device = self
            .device
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Projection device not ready"))?;
        let queue = self
            .queue
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Projection queue not ready"))?;
        let proj = self
            .projectors
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("Projector index {index} out of range"))?;
        proj.start_syphon(name, device, queue)
    }

    #[cfg(target_os = "macos")]
    pub fn stop_projector_syphon(&mut self, index: usize) {
        if let Some(proj) = self.projectors.get_mut(index) {
            proj.stop_syphon();
        }
    }

    pub fn is_projector_syphon(&self, index: usize) -> bool {
        self.projectors.get(index).map_or(false, |p| p.is_syphon())
    }

    #[cfg(target_os = "windows")]
    pub fn start_projector_spout(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let proj = self
            .projectors
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("Projector index {index} out of range"))?;
        proj.start_spout(name)
    }

    #[cfg(target_os = "windows")]
    pub fn stop_projector_spout(&mut self, index: usize) {
        if let Some(proj) = self.projectors.get_mut(index) {
            proj.stop_spout();
        }
    }

    pub fn is_projector_spout(&self, index: usize) -> bool {
        self.projectors.get(index).map_or(false, |p| p.is_spout())
    }

    #[cfg(target_os = "linux")]
    pub fn start_projector_v4l2(&mut self, index: usize, device_path: &str) -> anyhow::Result<()> {
        let proj = self
            .projectors
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("Projector index {index} out of range"))?;
        proj.start_v4l2(device_path)
    }

    #[cfg(target_os = "linux")]
    pub fn stop_projector_v4l2(&mut self, index: usize) {
        if let Some(proj) = self.projectors.get_mut(index) {
            proj.stop_v4l2();
        }
    }

    pub fn is_projector_v4l2(&self, index: usize) -> bool {
        self.projectors.get(index).map_or(false, |p| p.is_v4l2())
    }

    pub fn start_headless_recording(
        &mut self,
        index: usize,
        path: &Path,
        fps: f32,
        codec: RecorderCodec,
    ) -> anyhow::Result<()> {
        let [width, height] = self
            .headless_outputs
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?
            .size();
        let mgr = self
            .ensure_headless_manager(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?;
        if mgr.is_recording() {
            return Ok(());
        }
        mgr.start_recording(path, width, height, fps, codec)?;
        log::info!(
            "Started headless recording {}x{} @ {:.2} fps → {}",
            width,
            height,
            fps,
            path.display()
        );
        Ok(())
    }

    /// Stop recording a headless output by index (leaves other sinks running).
    pub fn stop_headless_recording(&mut self, index: usize) {
        if let Some(Some(mgr)) = self.headless_managers.get_mut(index) {
            mgr.stop_recording();
        }
    }

    pub fn is_headless_recording(&self, index: usize) -> bool {
        self.headless_managers
            .get(index)
            .and_then(|m| m.as_ref())
            .map_or(false, |m| m.is_recording())
    }

    // ── Headless output senders (NDI / Syphon / Spout / V4L2) ───────────────

    pub fn start_headless_ndi(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let [w, h] = self
            .headless_outputs
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?
            .size();
        let mgr = self
            .ensure_headless_manager(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?;
        if mgr.is_ndi_active() {
            return Ok(());
        }
        mgr.start_ndi(name, w, h, false)
    }

    pub fn stop_headless_ndi(&mut self, index: usize) {
        if let Some(Some(mgr)) = self.headless_managers.get_mut(index) {
            mgr.stop_ndi();
        }
    }

    pub fn is_headless_ndi(&self, index: usize) -> bool {
        self.headless_managers
            .get(index)
            .and_then(|m| m.as_ref())
            .map_or(false, |m| m.is_ndi_active())
    }

    #[cfg(target_os = "macos")]
    pub fn start_headless_syphon(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let device = self
            .device
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Projection device not ready"))?;
        let queue = self
            .queue
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Projection queue not ready"))?;
        let mgr = self
            .ensure_headless_manager(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?;
        if mgr.is_syphon_active() {
            return Ok(());
        }
        mgr.start_syphon(name, device, queue)
    }

    #[cfg(target_os = "macos")]
    pub fn stop_headless_syphon(&mut self, index: usize) {
        if let Some(Some(mgr)) = self.headless_managers.get_mut(index) {
            mgr.stop_syphon();
        }
    }

    pub fn is_headless_syphon(&self, index: usize) -> bool {
        self.headless_managers
            .get(index)
            .and_then(|m| m.as_ref())
            .map_or(false, |m| m.is_syphon_active())
    }

    #[cfg(target_os = "windows")]
    pub fn start_headless_spout(&mut self, index: usize, name: &str) -> anyhow::Result<()> {
        let mgr = self
            .ensure_headless_manager(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?;
        if mgr.is_spout_active() {
            return Ok(());
        }
        mgr.start_spout(name)
    }

    #[cfg(target_os = "windows")]
    pub fn stop_headless_spout(&mut self, index: usize) {
        if let Some(Some(mgr)) = self.headless_managers.get_mut(index) {
            mgr.stop_spout();
        }
    }

    pub fn is_headless_spout(&self, index: usize) -> bool {
        self.headless_managers
            .get(index)
            .and_then(|m| m.as_ref())
            .map_or(false, |m| m.is_spout_active())
    }

    #[cfg(target_os = "linux")]
    pub fn start_headless_v4l2(&mut self, index: usize, device_path: &str) -> anyhow::Result<()> {
        let [w, h] = self
            .headless_outputs
            .get(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?
            .size();
        let mgr = self
            .ensure_headless_manager(index)
            .ok_or_else(|| anyhow::anyhow!("Headless index {index} out of range"))?;
        if mgr.is_v4l2_active() {
            return Ok(());
        }
        mgr.start_v4l2(device_path, w, h)
    }

    #[cfg(target_os = "linux")]
    pub fn stop_headless_v4l2(&mut self, index: usize) {
        if let Some(Some(mgr)) = self.headless_managers.get_mut(index) {
            mgr.stop_v4l2();
        }
    }

    pub fn is_headless_v4l2(&self, index: usize) -> bool {
        self.headless_managers
            .get(index)
            .and_then(|m| m.as_ref())
            .map_or(false, |m| m.is_v4l2_active())
    }

    /// Remove a projector output by its window ID, dropping the surface and window.
    pub fn remove_output(&mut self, window_id: winit::window::WindowId) {
        self.projectors.retain(|p| p.window_id != window_id);
    }
}
