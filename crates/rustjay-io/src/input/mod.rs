use anyhow::Result;
use std::sync::mpsc;

#[cfg(feature = "ndi")]
pub mod ndi;
#[cfg(feature = "ndi")]
pub use ndi::{list_ndi_sources, NdiPixelLayout, NdiReceiver};

#[cfg(not(feature = "ndi"))]
#[allow(dead_code)]
pub struct NdiReceiver;
#[cfg(not(feature = "ndi"))]
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NdiPixelLayout {
    Bgra,
    Uyvy,
}
#[cfg(not(feature = "ndi"))]
#[allow(dead_code)]
pub struct NdiFrame {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub layout: NdiPixelLayout,
}
#[cfg(not(feature = "ndi"))]
#[allow(dead_code)]
pub fn list_ndi_sources(_timeout_ms: u64) -> Vec<String> {
    vec![]
}

#[cfg(feature = "webcam")]
pub mod webcam;
#[cfg(feature = "webcam")]
pub use webcam::{list_cameras, WebcamCapture, WebcamFrame};

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg;
#[cfg(all(feature = "ffmpeg", target_os = "macos"))]
pub mod videotoolbox;
#[cfg(feature = "ffmpeg")]
#[allow(unused_imports)]
pub use ffmpeg::{detect_hap_codec, FfmpegDecoder, LoopMode, VideoFrame};

#[cfg(not(feature = "ffmpeg"))]
#[allow(dead_code)]
pub struct FfmpegDecoder;
#[cfg(not(feature = "ffmpeg"))]
#[allow(dead_code)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}
#[cfg(not(feature = "ffmpeg"))]
#[allow(dead_code)]
pub enum LoopMode {
    None,
    Loop,
    PingPong,
}

#[cfg(target_os = "macos")]
pub mod syphon_input;
#[cfg(target_os = "macos")]
pub use syphon_input::{SyphonDiscovery, SyphonInputReceiver, SyphonServerInfo};

#[cfg(target_os = "windows")]
pub mod spout_input;
#[cfg(target_os = "windows")]
pub use spout_input::{SpoutDiscovery, SpoutInputReceiver, SpoutSenderInfo};

// Note: V4L2 input on Linux is handled by nokhwa (input-native maps to V4L2).
// A separate v4l2_input module is only needed if nokhwa proves insufficient.

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone)]
pub struct SpoutSenderInfo {
    pub name: String,
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone)]
pub struct SyphonServerInfo {
    pub name: String,
    pub app_name: String,
    pub uuid: String,
}

use rustjay_core::InputType;

/// Results returned from the background discovery thread.
///
/// NOTE: Syphon servers (macOS) are intentionally excluded — `SyphonServerDirectory`
/// must be accessed from the main thread only (it uses AppKit internals that conflict
/// with Metal's drawable pool when called off-thread, causing a deadlock).
struct DiscoveryResults {
    webcam: Vec<String>,
    #[cfg(feature = "ndi")]
    ndi: Vec<String>,
    #[cfg(target_os = "windows")]
    spout: Vec<SpoutSenderInfo>,
    #[cfg(target_os = "linux")]
    v4l2_capture: Vec<crate::v4l2_devices::V4l2DeviceInfo>,
    #[cfg(target_os = "linux")]
    v4l2_output: Vec<crate::v4l2_devices::V4l2DeviceInfo>,
}

/// Manages a single video input source with hot-swappable backends
pub struct InputManager {
    /// Current input type
    input_type: InputType,
    /// Whether input is active
    active: bool,
    /// Has new frame available
    has_new_frame: bool,
    /// Current resolution
    resolution: (u32, u32),

    // Input backends
    #[cfg(feature = "webcam")]
    webcam: Option<WebcamCapture>,
    /// Keeps the struct one shape whether or not the feature is on; nothing
    /// reads it in that build.
    #[cfg(not(feature = "webcam"))]
    #[allow(dead_code)]
    webcam: Option<()>,
    frame_receiver: Option<mpsc::Receiver<WebcamFrame>>,
    #[cfg(feature = "ndi")]
    ndi_receiver: Option<NdiReceiver>,
    /// Set by [`initialize`](Self::initialize). Needed to hand the NDI receive
    /// thread buffers to write into, and to poll their re-maps.
    device: Option<std::sync::Arc<wgpu::Device>>,
    /// The NDI frame now in GPU-visible memory, and its row stride. Handed to
    /// the consumer by reference; the buffer goes back to the ring from here,
    /// never from the consumer, so a frame nobody uploads cannot leak one.
    #[cfg(feature = "ndi")]
    ndi_staged: Option<(wgpu::Buffer, u32)>,
    /// Staged buffers whose copy has been submitted, waiting to be mapped again.
    #[cfg(feature = "ndi")]
    ndi_pending_remap: Vec<wgpu::Buffer>,
    /// Layout of the NDI frames arriving now; a sender can change it mid-stream.
    #[cfg(feature = "ndi")]
    ndi_layout: NdiPixelLayout,

    // Syphon (macOS only)
    #[cfg(target_os = "macos")]
    syphon_receiver: Option<SyphonInputReceiver>,
    #[cfg(target_os = "macos")]
    syphon_device: Option<std::sync::Arc<wgpu::Device>>,
    #[cfg(target_os = "macos")]
    syphon_queue: Option<std::sync::Arc<wgpu::Queue>>,

    // Spout (Windows only)
    #[cfg(target_os = "windows")]
    spout_receiver: Option<SpoutInputReceiver>,

    // Current frame data (CPU path)
    current_frame: Option<Vec<u8>>,

    // Device lists — None = not yet discovered, Some([]) = discovered but none found
    webcam_devices: Option<Vec<String>>,
    #[cfg(feature = "ndi")]
    ndi_sources: Option<Vec<String>>,
    #[cfg(target_os = "macos")]
    syphon_servers: Option<Vec<SyphonServerInfo>>,
    #[cfg(target_os = "windows")]
    spout_senders: Option<Vec<SpoutSenderInfo>>,
    #[cfg(target_os = "linux")]
    v4l2_capture_devices: Option<Vec<crate::v4l2_devices::V4l2DeviceInfo>>,
    #[cfg(target_os = "linux")]
    v4l2_output_devices: Option<Vec<crate::v4l2_devices::V4l2DeviceInfo>>,

    // Background discovery
    discovery_rx: Option<mpsc::Receiver<DiscoveryResults>>,
    is_discovering: bool,
}

impl InputManager {
    /// Create a new input manager
    pub fn new() -> Self {
        Self {
            input_type: InputType::None,
            active: false,
            has_new_frame: false,
            resolution: (1920, 1080),
            #[cfg(feature = "webcam")]
            webcam: None,
            #[cfg(not(feature = "webcam"))]
            webcam: None,
            frame_receiver: None,
            #[cfg(feature = "ndi")]
            ndi_receiver: None,
            device: None,
            #[cfg(feature = "ndi")]
            ndi_staged: None,
            #[cfg(feature = "ndi")]
            ndi_pending_remap: Vec::new(),
            #[cfg(feature = "ndi")]
            ndi_layout: NdiPixelLayout::Bgra,
            #[cfg(target_os = "macos")]
            syphon_receiver: None,
            #[cfg(target_os = "macos")]
            syphon_device: None,
            #[cfg(target_os = "macos")]
            syphon_queue: None,
            #[cfg(target_os = "windows")]
            spout_receiver: None,
            current_frame: None,
            webcam_devices: None,
            #[cfg(feature = "ndi")]
            ndi_sources: None,
            #[cfg(target_os = "macos")]
            syphon_servers: None,
            #[cfg(target_os = "windows")]
            spout_senders: None,
            #[cfg(target_os = "linux")]
            v4l2_capture_devices: None,
            #[cfg(target_os = "linux")]
            v4l2_output_devices: None,
            discovery_rx: None,
            is_discovering: false,
        }
    }

    /// Get cached list of V4L2 capture devices (Linux only; empty until discovery completes)
    #[cfg(target_os = "linux")]
    pub fn v4l2_capture_devices(&self) -> &[crate::v4l2_devices::V4l2DeviceInfo] {
        self.v4l2_capture_devices.as_deref().unwrap_or(&[])
    }

    /// Get cached list of V4L2 output (loopback) devices (Linux only)
    #[cfg(target_os = "linux")]
    pub fn v4l2_output_devices(&self) -> &[crate::v4l2_devices::V4l2DeviceInfo] {
        self.v4l2_output_devices.as_deref().unwrap_or(&[])
    }

    /// Initialize with wgpu device/queue.
    ///
    /// Without this the NDI path still works, through the CPU upload; with it,
    /// frames land straight in GPU-visible memory.
    #[allow(unused_variables)] // queue is consumed only by the macOS Syphon path
    pub fn initialize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) {
        self.device = Some(std::sync::Arc::new(device.clone()));
        // ponytail: the Syphon path keeps its own handles rather than sharing
        // this one — folding them together is macOS code this machine cannot
        // compile. Merge them when someone is building there.
        #[cfg(target_os = "macos")]
        {
            self.syphon_device = Some(std::sync::Arc::new(device.clone()));
            self.syphon_queue = Some(std::sync::Arc::new(queue.clone()));
        }
    }

    pub fn webcam_devices(&self) -> &[String] {
        self.webcam_devices.as_deref().unwrap_or(&[])
    }

    /// Get cached list of NDI sources (empty until discovery completes)
    #[cfg(feature = "ndi")]
    pub fn ndi_sources(&self) -> &[String] {
        self.ndi_sources.as_deref().unwrap_or(&[])
    }

    /// NDI source names (empty; NDI feature disabled).
    #[cfg(not(feature = "ndi"))]
    pub fn ndi_sources(&self) -> &[String] {
        &[]
    }

    /// Get cached list of Syphon servers (macOS only; empty until discovery completes)
    #[cfg(target_os = "macos")]
    pub fn syphon_servers(&self) -> &[SyphonServerInfo] {
        self.syphon_servers.as_deref().unwrap_or(&[])
    }

    #[cfg(not(target_os = "macos"))]
    pub fn syphon_servers(&self) -> &[SyphonServerInfo] {
        &[]
    }

    /// Re-snapshot the Syphon server directory (cheap in-process read) and
    /// return whether the list changed.
    ///
    /// The directory populates asynchronously via run-loop notifications, so a
    /// one-shot scan at startup sees an empty list; poll this periodically
    /// (main thread only) to keep the list live without a manual refresh.
    #[cfg(target_os = "macos")]
    pub fn refresh_syphon_servers(&mut self) -> bool {
        let servers = syphon_input::SyphonDiscovery::new().discover_servers();
        let changed = match &self.syphon_servers {
            Some(cur) => {
                cur.len() != servers.len()
                    || cur
                        .iter()
                        .zip(&servers)
                        .any(|(a, b)| a.uuid != b.uuid || a.name != b.name)
            }
            None => true,
        };
        if changed {
            self.syphon_servers = Some(servers);
        }
        changed
    }

    /// Whether background discovery is currently in progress
    pub fn is_discovering(&self) -> bool {
        self.is_discovering
    }

    /// Begin async device discovery in a background thread.
    ///
    /// Returns immediately; call [`poll_discovery`](Self::poll_discovery) each frame
    /// to check when results are ready. Calling while a discovery is already in
    /// progress is a no-op.
    pub fn begin_refresh_devices(&mut self) {
        if self.is_discovering {
            return;
        }

        self.webcam_devices = None;
        #[cfg(feature = "ndi")]
        {
            self.ndi_sources = None;
        }
        // Syphon is NOT reset here — it is refreshed on the main thread below
        // (SyphonServerDirectory must not be called from a background thread).
        #[cfg(target_os = "windows")]
        {
            self.spout_senders = None;
        }
        #[cfg(target_os = "linux")]
        {
            self.v4l2_capture_devices = None;
            self.v4l2_output_devices = None;
        }

        // Syphon discovery runs on the main thread (caller's thread). The
        // SyphonServerDirectory singleton uses AppKit internals that conflict with
        // Metal's drawable pool when accessed off-thread, causing a deadlock.
        #[cfg(target_os = "macos")]
        {
            log::debug!("[InputManager] Discovering Syphon servers (main thread)...");
            let servers = syphon_input::SyphonDiscovery::new().discover_servers();
            log::debug!("[InputManager] Found {} Syphon server(s)", servers.len());
            self.syphon_servers = Some(servers);
        }

        self.is_discovering = true;
        let (tx, rx) = mpsc::channel();
        self.discovery_rx = Some(rx);

        std::thread::spawn(move || {
            #[cfg(feature = "webcam")]
            let webcam = {
                log::debug!("[InputManager] Discovering webcam devices...");
                let devices = list_cameras();
                log::debug!("[InputManager] Found {} webcam device(s)", devices.len());
                for d in &devices {
                    log::debug!("  - {}", d);
                }
                devices
            };
            #[cfg(not(feature = "webcam"))]
            let webcam: Vec<String> = Vec::new();

            #[cfg(feature = "ndi")]
            let ndi = {
                log::debug!("[InputManager] Discovering NDI sources...");
                let sources = list_ndi_sources(2000);
                log::debug!("[InputManager] Found {} NDI source(s)", sources.len());
                sources
            };
            #[cfg(not(feature = "ndi"))]
            let _ndi: Vec<String> = Vec::new();

            #[cfg(target_os = "windows")]
            let spout = {
                log::debug!("[InputManager] Discovering Spout senders...");
                let senders = spout_input::SpoutDiscovery::list_senders();
                log::debug!("[InputManager] Found {} Spout sender(s)", senders.len());
                senders
            };

            #[cfg(target_os = "linux")]
            let (v4l2_capture, v4l2_output) = {
                log::debug!("[InputManager] Discovering V4L2 devices...");
                let cap = crate::v4l2_devices::list_capture_devices();
                let out = crate::v4l2_devices::list_output_devices();
                log::debug!(
                    "[InputManager] Found {} V4L2 capture, {} V4L2 output device(s)",
                    cap.len(),
                    out.len()
                );
                for d in &cap {
                    log::debug!("  - [capture] {}", d.display_name());
                }
                for d in &out {
                    log::debug!("  - [output]  {}", d.display_name());
                }
                (cap, out)
            };

            let _ = tx.send(DiscoveryResults {
                webcam,
                #[cfg(feature = "ndi")]
                ndi,
                #[cfg(target_os = "windows")]
                spout,
                #[cfg(target_os = "linux")]
                v4l2_capture,
                #[cfg(target_os = "linux")]
                v4l2_output,
            });
        });
    }

    /// Poll for background discovery completion.
    ///
    /// Returns `true` exactly once when discovery finishes — the caller should
    /// update any caches (e.g. GUI device lists) at that point.
    pub fn poll_discovery(&mut self) -> bool {
        if !self.is_discovering {
            return false;
        }
        let result = self.discovery_rx.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = result {
            self.webcam_devices = Some(result.webcam);
            #[cfg(feature = "ndi")]
            {
                self.ndi_sources = Some(result.ndi);
            }
            // Syphon servers were populated synchronously on the main thread in
            // begin_refresh_devices() and are not part of DiscoveryResults.
            #[cfg(target_os = "windows")]
            {
                self.spout_senders = Some(result.spout);
            }
            #[cfg(target_os = "linux")]
            {
                self.v4l2_capture_devices = Some(result.v4l2_capture);
                self.v4l2_output_devices = Some(result.v4l2_output);
            }
            self.is_discovering = false;
            self.discovery_rx = None;
            true
        } else {
            false
        }
    }

    /// Start webcam capture
    #[cfg(feature = "webcam")]
    pub fn start_webcam(
        &mut self,
        device_index: usize,
        width: u32,
        height: u32,
        fps: u32,
    ) -> Result<()> {
        self.stop();

        let mut webcam = WebcamCapture::new(device_index, width, height, fps)?;
        let receiver = webcam.start()?;

        self.input_type = InputType::Webcam;
        self.resolution = (width, height);
        self.active = true;
        self.webcam = Some(webcam);
        self.frame_receiver = Some(receiver);

        log::info!(
            "Started webcam {} at {}x{}@{}fps",
            device_index,
            width,
            height,
            fps
        );
        Ok(())
    }

    /// Start webcam (placeholder when disabled)
    #[cfg(not(feature = "webcam"))]
    pub fn start_webcam(
        &mut self,
        _device_index: usize,
        _width: u32,
        _height: u32,
        _fps: u32,
    ) -> Result<()> {
        Err(anyhow::anyhow!(
            "Webcam support not compiled. Enable the 'webcam' feature."
        ))
    }

    /// Start NDI input
    #[cfg(feature = "ndi")]
    pub fn start_ndi(&mut self, source_name: impl Into<String>) -> Result<()> {
        self.stop();

        let source_name = source_name.into();
        let mut ndi = NdiReceiver::new(source_name.clone());
        ndi.start()?;

        self.input_type = InputType::Ndi;
        self.active = true;
        self.ndi_receiver = Some(ndi);

        log::info!("Started NDI input: {}", source_name);
        Ok(())
    }

    /// Start NDI input (errors; NDI feature disabled).
    #[cfg(not(feature = "ndi"))]
    pub fn start_ndi(&mut self, _source_name: impl Into<String>) -> Result<()> {
        Err(anyhow::anyhow!(
            "NDI support not compiled. Enable the 'ndi' feature."
        ))
    }

    /// Start Syphon input (macOS only)
    #[cfg(target_os = "macos")]
    pub fn start_syphon(
        &mut self,
        server_name: impl Into<String>,
        server_uuid: impl Into<String>,
    ) -> Result<()> {
        let server_name = server_name.into();
        let server_uuid = server_uuid.into();

        let device = self.syphon_device.clone();
        let queue = self.syphon_queue.clone();

        match (device, queue) { (Some(device), Some(queue)) => {
            self.stop();

            let mut receiver = SyphonInputReceiver::new();
            receiver.initialize(&device, &queue);
            receiver.connect_by_uuid(&server_uuid, &server_name)?;

            self.input_type = InputType::Syphon;
            self.active = true;
            self.syphon_receiver = Some(receiver);

            log::info!(
                "Started Syphon input: {} (uuid={})",
                server_name,
                server_uuid
            );
            Ok(())
        } _ => {
            Err(anyhow::anyhow!(
                "InputManager not initialized with wgpu device/queue"
            ))
        }}
    }

    /// Start Syphon (stub on non-macOS)
    #[cfg(not(target_os = "macos"))]
    pub fn start_syphon(&mut self, _server_name: impl Into<String>) -> Result<()> {
        Err(anyhow::anyhow!("Syphon is only available on macOS"))
    }

    /// Start Spout input (Windows only).
    #[cfg(target_os = "windows")]
    pub fn start_spout(&mut self, sender_name: impl Into<String>) -> Result<()> {
        let sender_name = sender_name.into();
        self.stop();
        let mut receiver = SpoutInputReceiver::new()
            .map_err(|e| anyhow::anyhow!("Failed to create Spout receiver: {}", e))?;
        receiver.connect(&sender_name)?;
        self.input_type = rustjay_core::InputType::Spout;
        self.active = true;
        self.spout_receiver = Some(receiver);
        log::info!("Started Spout input: {}", sender_name);
        Ok(())
    }

    /// Start Spout input (stub on non-Windows).
    #[cfg(not(target_os = "windows"))]
    pub fn start_spout(&mut self, _sender_name: impl Into<String>) -> Result<()> {
        Err(anyhow::anyhow!("Spout is only available on Windows"))
    }

    /// Get cached list of Spout senders (Windows only)
    #[cfg(target_os = "windows")]
    pub fn spout_senders(&self) -> &[SpoutSenderInfo] {
        self.spout_senders.as_deref().unwrap_or(&[])
    }

    /// Get cached list of Spout senders (empty on non-Windows).
    #[cfg(not(target_os = "windows"))]
    pub fn spout_senders(&self) -> &[SpoutSenderInfo] {
        &[]
    }

    /// Stop current input
    pub fn stop(&mut self) {
        if !self.active {
            return;
        }

        log::info!("Stopping input source ({:?})", self.input_type);

        self.active = false;
        self.has_new_frame = false;

        // Stop webcam
        #[cfg(feature = "webcam")]
        if let Some(mut webcam) = self.webcam.take() {
            let _ = webcam.stop();
        }

        // Stop NDI. The staging buffers go with the receiver: the ring they
        // belong to is inside it.
        #[cfg(feature = "ndi")]
        {
            if let Some(mut ndi) = self.ndi_receiver.take() {
                ndi.stop();
            }
            self.ndi_staged = None;
            self.ndi_pending_remap.clear();
        }

        // Stop Syphon
        #[cfg(target_os = "macos")]
        {
            self.syphon_receiver = None;
        }

        // Stop Spout
        #[cfg(target_os = "windows")]
        {
            self.spout_receiver = None;
        }

        self.frame_receiver = None;
        self.current_frame = None;
        self.input_type = InputType::None;
    }

    /// Update - poll for new frames
    pub fn update(&mut self) {
        if !self.active {
            return;
        }

        // Handle webcam frames
        if let Some(ref receiver) = self.frame_receiver {
            let mut latest_frame: Option<WebcamFrame> = None;
            while let Ok(frame) = receiver.try_recv() {
                latest_frame = Some(frame);
            }
            if let Some(frame) = latest_frame {
                self.resolution = (frame.width, frame.height);
                self.current_frame = Some(frame.data);
                self.has_new_frame = true;
            }
        }

        // Handle NDI frames
        #[cfg(feature = "ndi")]
        if let Some(ref mut ndi) = self.ndi_receiver {
            // Offer the receive thread buffers to write frames straight into, so
            // this thread only encodes a copy instead of paying for a memcpy.
            if let Some(ref device) = self.device {
                // Last frame's copies have been submitted, so those buffers can
                // be mapped again. The callbacks only run while the device is
                // polled, and nothing else on this path polls it.
                for buffer in self.ndi_pending_remap.drain(..) {
                    ndi.remap_staged(buffer);
                }
                device.poll(wgpu::PollType::Poll).ok();
                // BGRA only. Packed 4:2:2 is not colour — it needs a shader to
                // decode, and the engine's input texture is BGRA, so those
                // frames have always gone down the CPU path (where
                // InputTexture::update drops them on the size check). Offering
                // buffers for them would just have the receive thread discard
                // each one for the wrong size.
                if self.ndi_layout == NdiPixelLayout::Bgra {
                    let (width, height) = ndi.resolution();
                    ndi.provide_staging(device, width, height, NdiPixelLayout::Bgra);
                }
            }

            if let Some(frame) = ndi.get_latest_frame() {
                self.resolution = (frame.width, frame.height);
                self.ndi_layout = frame.layout;
                match frame.staged {
                    // Already in GPU-visible memory. A staged frame nobody
                    // uploaded before this one arrived goes back to the ring —
                    // dropping it would take a buffer out for good.
                    Some(staged) => {
                        if let Some((buffer, _)) = self
                            .ndi_staged
                            .replace((staged.buffer, staged.bytes_per_row))
                        {
                            self.ndi_pending_remap.push(buffer);
                        }
                    }
                    // A frame nobody took before this one arrived goes back for reuse.
                    None => {
                        if let Some(stale) = self.current_frame.replace(frame.data) {
                            ndi.recycle(stale);
                        }
                    }
                }
                self.has_new_frame = true;
            }
        }

        // Handle Syphon frames (zero-copy texture path)
        #[cfg(target_os = "macos")]
        if let (Some(ref mut syphon), Some(device), Some(queue)) = (
            self.syphon_receiver.as_mut(),
            self.syphon_device.as_ref(),
            self.syphon_queue.as_ref(),
        )
            && syphon.try_receive_texture(device, queue) {
                self.resolution = syphon.resolution();
                self.has_new_frame = true;
            }

        // Handle Spout frames (CPU path on Windows)
        // Note: pixel data stays in SpoutInputReceiver's buffer and is
        // borrowed via spout_pixels() to avoid per-frame Vec moves.
        #[cfg(target_os = "windows")]
        if let Some(ref mut spout) = self.spout_receiver {
            if spout.try_receive_texture() {
                self.resolution = spout.resolution();
                self.has_new_frame = true;
            }
        }
    }

    /// Check if there's a new frame available
    pub fn has_frame(&self) -> bool {
        self.has_new_frame
    }

    /// Take the current frame data (CPU path)
    pub fn take_frame(&mut self) -> Option<Vec<u8>> {
        self.has_new_frame = false;
        self.current_frame.take()
    }

    /// Borrow the Syphon output texture (macOS only).
    ///
    /// Valid after [`update`](Self::update) sets [`has_frame`](Self::has_frame).
    /// Call [`clear_syphon_frame`](Self::clear_syphon_frame) after consuming it.
    #[cfg(target_os = "macos")]
    pub fn syphon_output_texture(&self) -> Option<&wgpu::Texture> {
        self.syphon_receiver
            .as_ref()
            .and_then(|r| r.output_texture())
    }

    /// Reset the new-frame flag for the Syphon path.
    #[cfg(target_os = "macos")]
    pub fn clear_syphon_frame(&mut self) {
        self.has_new_frame = false;
    }

    /// The NDI frame sitting in GPU-visible memory, as `(buffer, bytes_per_row)`.
    ///
    /// `Some` only when [`initialize`](Self::initialize) supplied a device and the
    /// receive thread had a buffer free; otherwise the frame is on the CPU path and
    /// [`take_frame`](Self::take_frame) has it. Upload it with
    /// `InputTexture::update_from_buffer`, then call
    /// [`clear_ndi_frame`](Self::clear_ndi_frame).
    ///
    /// The handle is cheap to clone and the buffer returns to the ring from here on
    /// a later `update`, so a caller that takes one and never uploads it costs a
    /// frame, not a buffer.
    #[cfg(feature = "ndi")]
    pub fn ndi_staged_frame(&self) -> Option<(wgpu::Buffer, u32)> {
        self.ndi_staged
            .as_ref()
            .map(|(buffer, bytes_per_row)| (buffer.clone(), *bytes_per_row))
    }

    /// Done with the staged NDI frame: clears the new-frame flag and sends the
    /// buffer back to the ring.
    ///
    /// Call it after uploading, so the copy has been submitted by the time the
    /// buffer is mapped again on the next [`update`](Self::update). Skipping the
    /// upload is fine too — the contents are simply overwritten.
    #[cfg(feature = "ndi")]
    pub fn clear_ndi_frame(&mut self) {
        self.has_new_frame = false;
        if let Some((buffer, _)) = self.ndi_staged.take() {
            self.ndi_pending_remap.push(buffer);
        }
    }

    /// Borrow the Spout pixel buffer without moving it (Windows only).
    ///
    /// Returns `Some(&[u8])` when a Spout frame is available. The buffer
    /// is reused in-place on the next frame, avoiding per-frame allocation.
    #[cfg(target_os = "windows")]
    pub fn spout_pixels(&self) -> Option<&[u8]> {
        self.spout_receiver.as_ref().and_then(|r| r.pixels())
    }

    /// Reset the new-frame flag for the Spout path.
    #[cfg(target_os = "windows")]
    pub fn clear_spout_frame(&mut self) {
        self.has_new_frame = false;
    }

    /// Get current resolution
    pub fn resolution(&self) -> (u32, u32) {
        self.resolution
    }

    /// Get current input type
    pub fn input_type(&self) -> InputType {
        self.input_type
    }

    /// Check if input is active
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns true if the NDI source was lost (not found or too many errors)
    #[cfg(feature = "ndi")]
    pub fn is_ndi_source_lost(&self) -> bool {
        self.ndi_receiver
            .as_ref()
            .map(|r| r.is_source_lost())
            .unwrap_or(false)
    }

    /// Whether the NDI source was lost (always false; NDI feature disabled).
    #[cfg(not(feature = "ndi"))]
    pub fn is_ndi_source_lost(&self) -> bool {
        false
    }
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}

// Placeholder types when webcam is disabled
#[cfg(not(feature = "webcam"))]
pub struct WebcamFrame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
    pub timestamp: std::time::Instant,
}

#[cfg(not(feature = "webcam"))]
pub fn list_cameras() -> Vec<String> {
    Vec::new()
}
