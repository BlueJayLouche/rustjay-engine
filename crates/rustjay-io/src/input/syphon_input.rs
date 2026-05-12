//! # Syphon Input (macOS)
//!
//! GPU texture sharing input for macOS using Syphon framework.

/// Information about a Syphon server
pub use syphon_core::ServerInfo as SyphonServerInfo;

/// Syphon input receiver using syphon-wgpu integration.
///
/// Must call `initialize(device, queue)` with the application's wgpu device
/// before calling `connect()` or `try_receive_texture()`.
pub struct SyphonInputReceiver {
    /// Created lazily in `initialize()` using the main app's wgpu device.
    #[cfg(target_os = "macos")]
    inner: Option<syphon_wgpu::SyphonWgpuInput>,
    server_name: Option<String>,
    resolution: (u32, u32),
}

impl SyphonInputReceiver {
    /// Create a new Syphon input receiver.
    /// No GPU resources are allocated here — call `initialize()` first.
    pub fn new() -> Self {
        Self {
            #[cfg(target_os = "macos")]
            inner: None,
            server_name: None,
            resolution: (1920, 1080),
        }
    }

    /// Check if Syphon is available on this system
    pub fn is_available() -> bool {
        syphon_core::is_available()
    }

    /// Initialize with the application's wgpu device and queue.
    /// Must be called before `connect()`.
    pub fn initialize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        #[cfg(target_os = "macos")]
        {
            self.inner = Some(syphon_wgpu::SyphonWgpuInput::new(device, queue));
        }
        let _ = (device, queue); // suppress unused warnings on non-macOS
    }

    /// Connect to a Syphon server by name.
    /// Returns an error if `initialize()` has not been called.
    pub fn connect(&mut self, server_name: impl Into<String>) -> anyhow::Result<()> {
        let server_name = server_name.into();

        if self.is_connected() {
            self.disconnect();
        }

        log::info!("[Syphon Input] Connecting to: {}", server_name);

        #[cfg(target_os = "macos")]
        {
            let inner = self.inner.as_mut()
                .ok_or_else(|| anyhow::anyhow!("SyphonInputReceiver not initialized — call initialize() first"))?;
            inner.connect(&server_name)
                .map_err(|e| anyhow::anyhow!("Failed to connect to '{}': {:?}", server_name, e))?;
        }

        self.server_name = Some(server_name);
        Ok(())
    }

    /// Connect to a Syphon server by UUID (unambiguous).
    /// Falls back to name-based matching if UUID lookup fails.
    /// Returns an error if `initialize()` has not been called.
    pub fn connect_by_uuid(&mut self, server_uuid: impl Into<String>, server_name: impl Into<String>) -> anyhow::Result<()> {
        let server_uuid = server_uuid.into();
        let server_name = server_name.into();

        if self.is_connected() {
            self.disconnect();
        }

        log::info!("[Syphon Input] Connecting to: {} (uuid={})", server_name, server_uuid);

        #[cfg(target_os = "macos")]
        {
            let inner = self.inner.as_mut()
                .ok_or_else(|| anyhow::anyhow!("SyphonInputReceiver not initialized — call initialize() first"))?;

            let info = syphon_core::ServerInfo {
                name: server_name.clone(),
                uuid: server_uuid.clone(),
                app_name: String::new(),
                bundle_id: String::new(),
            };

            inner.connect_by_info(&info)
                .map_err(|e| anyhow::anyhow!("Failed to connect to '{}' (uuid={}): {:?}", server_name, server_uuid, e))?;
        }

        self.server_name = Some(server_name);
        Ok(())
    }

    /// Try to receive a frame into the internal output texture.
    ///
    /// Returns `true` when a new frame was written. Access the texture with
    /// [`output_texture`](Self::output_texture).
    pub fn try_receive_texture(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> bool {
        #[cfg(target_os = "macos")]
        {
            if let Some(ref mut inner) = self.inner {
                if inner.receive_texture(device, queue) {
                    if let Some(tex) = inner.output_texture() {
                        self.resolution = (tex.width(), tex.height());
                    }
                    return true;
                }
            }
        }
        let _ = (device, queue);
        false
    }

    /// The output texture, valid after [`try_receive_texture`](Self::try_receive_texture)
    /// returns `true`.
    pub fn output_texture(&self) -> Option<&wgpu::Texture> {
        #[cfg(target_os = "macos")]
        { self.inner.as_ref().and_then(|i| i.output_texture()) }
        #[cfg(not(target_os = "macos"))]
        { None }
    }

    /// Disconnect from the server
    pub fn disconnect(&mut self) {
        #[cfg(target_os = "macos")]
        {
            if let Some(ref mut inner) = self.inner {
                inner.disconnect();
            }
        }
        self.server_name = None;
    }

    /// Check if connected to a server
    pub fn is_connected(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.inner.as_ref().map(|i| i.is_connected()).unwrap_or(false)
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    /// Get current resolution
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }
}

impl Default for SyphonInputReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SyphonInputReceiver {
    fn drop(&mut self) {
        self.disconnect();
    }
}

/// Syphon server discovery
pub struct SyphonDiscovery;

impl SyphonDiscovery {
    /// Create a new discovery instance
    pub fn new() -> Self {
        Self
    }

    /// Discover available Syphon servers
    pub fn discover_servers(&self) -> Vec<SyphonServerInfo> {
        syphon_core::SyphonServerDirectory::servers()
    }
}

impl Default for SyphonDiscovery {
    fn default() -> Self {
        Self::new()
    }
}
