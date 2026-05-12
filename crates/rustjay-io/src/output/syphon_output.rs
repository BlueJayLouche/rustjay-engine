//! # Syphon Output (macOS)
//!
//! GPU texture sharing output for macOS using Syphon framework.

use std::sync::Arc;

/// Syphon output server
pub struct SyphonOutput {
    server_name: String,
    width: u32,
    height: u32,
    wgpu_device: Option<Arc<wgpu::Device>>,
    wgpu_queue: Option<Arc<wgpu::Queue>>,
    inner: Option<syphon_wgpu::SyphonWgpuOutput>,
    initialized: bool,
}

impl SyphonOutput {
    /// Create a new Syphon output
    pub fn new(
        server_name: &str,
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            server_name: server_name.to_string(),
            width: 0,
            height: 0,
            wgpu_device: Some(device),
            wgpu_queue: Some(queue),
            inner: None,
            initialized: false,
        })
    }

    /// Initialize the Syphon server with dimensions
    pub fn initialize(&mut self, width: u32, height: u32) -> anyhow::Result<()> {
        if self.initialized {
            if self.width == width && self.height == height {
                return Ok(());
            }
            self.shutdown();
        }

        self.width = width;
        self.height = height;

        if let (Some(ref device), Some(ref queue)) = (&self.wgpu_device, &self.wgpu_queue) {
            match syphon_wgpu::SyphonWgpuOutput::new(&self.server_name, device, queue, width, height) {
                Ok(output) => {
                    log::info!("Syphon server '{}' created ({}x{})", self.server_name, width, height);
                    self.inner = Some(output);
                    self.initialized = true;
                    Ok(())
                }
                Err(e) => {
                    log::error!("Failed to create Syphon server: {}", e);
                    Err(anyhow::anyhow!("Failed to create Syphon output: {}", e))
                }
            }
        } else {
            Err(anyhow::anyhow!("Missing wgpu device or queue"))
        }
    }

    /// Submit a frame to Syphon (zero-copy)
    pub fn submit_frame(
        &mut self,
        texture: &wgpu::Texture,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> anyhow::Result<()> {
        if !self.initialized {
            self.initialize(texture.width(), texture.height())?;
        }

        if texture.width() != self.width || texture.height() != self.height {
            self.initialize(texture.width(), texture.height())?;
        }

        if let Some(ref mut inner) = self.inner {
            inner.publish(texture, device, queue);
        }

        Ok(())
    }

    /// Shutdown the Syphon server
    pub fn shutdown(&mut self) {
        if self.initialized {
            log::info!("Syphon server shutdown: {}", self.server_name);
        }
        self.inner = None;
        self.initialized = false;
        self.width = 0;
        self.height = 0;
    }
}

impl Drop for SyphonOutput {
    fn drop(&mut self) {
        self.shutdown();
    }
}
