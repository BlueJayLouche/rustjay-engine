use std::sync::Arc;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes, Fullscreen};
use wgpu::{Adapter, Device, Queue, Surface, SurfaceConfiguration};

/// A borderless-fullscreen output window backed by wgpu.
pub struct OutputWindow {
    pub window: Arc<Window>,
    pub surface: Surface<'static>,
    pub config: SurfaceConfiguration,
    pub device: Device,
    pub queue: Queue,
    pub adapter: Adapter,
}

impl OutputWindow {
    /// Create the output window on the primary monitor, borderless fullscreen.
    pub fn new(event_loop: &ActiveEventLoop) -> anyhow::Result<Self> {
        let window_attrs = WindowAttributes::default()
            .with_title("QPlayer Video Output")
            .with_fullscreen(Some(Fullscreen::Borderless(None)))
            .with_visible(true);

        let window = Arc::new(event_loop.create_window(window_attrs)?);

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance.create_surface(Arc::clone(&window))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .map_err(|e| anyhow::anyhow!("no wgpu adapter found: {e}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("qplayer-video-device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            },
        ))?;

        let size = window.inner_size();
        let config = surface.get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or_else(|| anyhow::anyhow!("no default surface config"))?;
        surface.configure(&device, &config);

        Ok(Self {
            window,
            surface,
            config,
            device,
            queue,
            adapter,
        })
    }

    /// Resize the surface to match the window.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }

    /// Current surface size.
    pub fn size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }
}
