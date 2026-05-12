//! # Webcam Input
//!
//! Local camera capture using nokhwa.

use nokhwa::Camera;
use nokhwa::pixel_format::{RgbFormat, YuyvFormat};
use nokhwa::utils::{CameraIndex, CameraFormat, FrameFormat, RequestedFormat, RequestedFormatType, Resolution};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

/// Webcam frame data (converted to BGRA)
pub struct WebcamFrame {
    pub width: u32,
    pub height: u32,
    /// BGRA pixel data
    pub data: Vec<u8>,
    pub timestamp: Instant,
}

/// Webcam capture running on dedicated thread
pub struct WebcamCapture {
    device_index: usize,
    capture_thread: Option<JoinHandle<()>>,
    stop_signal: Option<Sender<()>>,
}

impl WebcamCapture {
    /// Create a new webcam capture configuration
    pub fn new(device_index: usize, width: u32, height: u32, fps: u32) -> anyhow::Result<Self> {
        let _ = (width, height, fps); // Unused for now
        Ok(Self {
            device_index,
            capture_thread: None,
            stop_signal: None,
        })
    }

    /// Start capturing frames on a dedicated thread
    pub fn start(&mut self) -> anyhow::Result<Receiver<WebcamFrame>> {
        if self.capture_thread.is_some() {
            return Err(anyhow::anyhow!("Webcam already started"));
        }

        let (frame_tx, frame_rx) = mpsc::channel::<WebcamFrame>();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let device_index = self.device_index;

        let thread_handle = thread::spawn(move || {
            let index = CameraIndex::Index(device_index as u32);

            let mut camera = match try_open_camera(index) {
                Ok(cam) => cam,
                Err(e) => {
                    log::error!("[Webcam] Failed to open camera {}: {:?}", device_index, e);
                    return;
                }
            };

            if let Err(e) = camera.open_stream() {
                log::error!("[Webcam] Failed to open stream: {:?}", e);
                return;
            }

            let actual_resolution = camera.resolution();
            let actual_width = actual_resolution.width() as u32;
            let actual_height = actual_resolution.height() as u32;

            log::info!("[Webcam] Camera {} opened at {}x{}",
                device_index, actual_width, actual_height);

            // Capture loop
            loop {
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                match camera.frame() {
                    Ok(frame) => {
                        let buffer = frame.buffer();

                        // Convert YUY2 to BGRA
                        let yuyv_data = buffer.to_vec();
                        let mut bgra_data = Vec::with_capacity((actual_width * actual_height * 4) as usize);

                        // YUY2 is 2 bytes per pixel, arranged as: Y0 U Y1 V.
                        // Cameras commonly deliver limited-range BT.601 YUV here,
                        // so expand luma/chroma before converting to RGB.
                        for chunk in yuyv_data.chunks_exact(4) {
                            let y0 = (chunk[0] as f32 - 16.0).max(0.0);
                            let u = chunk[1] as f32 - 128.0;
                            let y1 = (chunk[2] as f32 - 16.0).max(0.0);
                            let v = chunk[3] as f32 - 128.0;

                            // Convert first pixel (Y0, U, V)
                            let r0 = (1.164383 * y0 + 1.596027 * v).clamp(0.0, 255.0) as u8;
                            let g0 = (1.164383 * y0 - 0.391762 * u - 0.812968 * v).clamp(0.0, 255.0) as u8;
                            let b0 = (1.164383 * y0 + 2.017232 * u).clamp(0.0, 255.0) as u8;

                            // Convert second pixel (Y1, U, V)
                            let r1 = (1.164383 * y1 + 1.596027 * v).clamp(0.0, 255.0) as u8;
                            let g1 = (1.164383 * y1 - 0.391762 * u - 0.812968 * v).clamp(0.0, 255.0) as u8;
                            let b1 = (1.164383 * y1 + 2.017232 * u).clamp(0.0, 255.0) as u8;

                            // Output as BGRA for first pixel
                            bgra_data.push(b0);
                            bgra_data.push(g0);
                            bgra_data.push(r0);
                            bgra_data.push(255); // Alpha

                            // Output as BGRA for second pixel
                            bgra_data.push(b1);
                            bgra_data.push(g1);
                            bgra_data.push(r1);
                            bgra_data.push(255); // Alpha
                        }

                        let webcam_frame = WebcamFrame {
                            width: actual_width,
                            height: actual_height,
                            data: bgra_data,
                            timestamp: Instant::now(),
                        };

                        if frame_tx.send(webcam_frame).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        log::warn!("[Webcam] Frame capture error: {:?}", e);
                        thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }

            let _ = camera.stop_stream();
            log::info!("[Webcam] Camera {} stopped", device_index);
        });

        self.capture_thread = Some(thread_handle);
        self.stop_signal = Some(stop_tx);

        Ok(frame_rx)
    }

    /// Stop capturing
    pub fn stop(&mut self) -> anyhow::Result<()> {
        if let Some(stop_tx) = self.stop_signal.take() {
            let _ = stop_tx.send(());
        }

        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }

        Ok(())
    }
}

impl Drop for WebcamCapture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ---------------------------------------------------------------------------
// macOS camera authorization
// ---------------------------------------------------------------------------

/// Request camera access from macOS and block until the user responds.
///
/// On macOS, CLI binaries need to explicitly request camera access via
/// AVFoundation.  We call `AVCaptureDevice.requestAccess(for: .video)`
/// using the ObjC runtime, which triggers the system privacy dialog.
///
/// Returns `true` if access was granted.
#[cfg(target_os = "macos")]
fn ensure_camera_authorized() -> bool {
    use objc::runtime::{Class, Object};

    unsafe {
        let cls = match Class::get("AVCaptureDevice") {
            Some(c) => c,
            None => {
                log::warn!("[Webcam] AVCaptureDevice class not found");
                return true; // proceed, let nokhwa handle it
            }
        };

        // AVMediaTypeVideo = NSString "vide"
        let ns_string_cls = Class::get("NSString").unwrap();
        let media_type: *mut Object = objc::msg_send![ns_string_cls, stringWithUTF8String: b"vide\0".as_ptr() as *const std::os::raw::c_char];

        // authorizationStatusForMediaType: returns i64 (0=NotDetermined, 1=Restricted, 2=Denied, 3=Authorized)
        let status: i64 = objc::msg_send![cls, authorizationStatusForMediaType: media_type];

        match status {
            3 => {
                log::debug!("[Webcam] Camera access already authorized");
                return true;
            }
            1 | 2 => {
                log::warn!("[Webcam] Camera access denied/restricted (status={}). Grant access in System Settings > Privacy & Security > Camera.", status);
                return false;
            }
            _ => {
                log::info!("[Webcam] Camera access not determined — requesting...");
            }
        }

        // requestAccessForMediaType:completionHandler: needs an ObjC block.
        // We use the `block` support from the objc crate ecosystem.
        // Since we can't easily create blocks with objc 0.2, use a polling
        // approach: trigger the request, then poll the status.
        //
        // Passing a nil block to requestAccessForMediaType: still triggers
        // the system dialog — the completion handler is just not called.
        let nil_block: *mut Object = std::ptr::null_mut();
        let _: () = objc::msg_send![cls, requestAccessForMediaType: media_type completionHandler: nil_block];

        // Poll authorization status until it changes from NotDetermined.
        // The macOS dialog is presented asynchronously; we give the user
        // up to 30 seconds to respond.
        for i in 0..300 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let new_status: i64 = objc::msg_send![cls, authorizationStatusForMediaType: media_type];
            if new_status != 0 {
                let granted = new_status == 3;
                if granted {
                    log::info!("[Webcam] Camera access granted by user");
                } else {
                    log::warn!("[Webcam] Camera access denied by user");
                }
                return granted;
            }
            if i == 0 {
                log::info!("[Webcam] Waiting for camera permission dialog...");
            }
        }

        log::warn!("[Webcam] Camera authorization timed out (30s)");
        false
    }
}

#[cfg(not(target_os = "macos"))]
fn ensure_camera_authorized() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Camera open with multi-strategy fallback
// ---------------------------------------------------------------------------

/// Try to open a camera with progressively more permissive format strategies.
///
/// AVFoundation (macOS) rejects `lockForConfiguration` when a specific format
/// is requested that doesn't exactly match.  We try several strategies from
/// most-preferred to most-permissive.
fn try_open_camera(index: CameraIndex) -> anyhow::Result<Camera> {
    if !ensure_camera_authorized() {
        return Err(anyhow::anyhow!("Camera access not authorized. Grant access in System Settings > Privacy & Security > Camera."));
    }

    // Strategy 1: Let the camera decide (no format constraint)
    log::info!("[Webcam] Trying NoPreference strategy...");
    if let Ok(cam) = try_create_camera(&index, RequestedFormat::new::<RgbFormat>(RequestedFormatType::None)) {
        log::info!("[Webcam] Success with NoPreference");
        return Ok(cam);
    }

    // Strategy 2: Highest resolution hint
    log::info!("[Webcam] Trying HighestResolution strategy...");
    if let Ok(cam) = try_create_camera(
        &index,
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::HighestResolution(Resolution::new(1280, 720))),
    ) {
        log::info!("[Webcam] Success with HighestResolution");
        return Ok(cam);
    }

    // Strategy 3: Specific YUYV format (common for physical webcams)
    log::info!("[Webcam] Trying YUYV 1280x720@30...");
    if let Ok(cam) = try_create_camera(
        &index,
        RequestedFormat::new::<YuyvFormat>(RequestedFormatType::Closest(
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::YUYV, 30),
        )),
    ) {
        log::info!("[Webcam] Success with YUYV");
        return Ok(cam);
    }

    // Strategy 4: MJPEG format (another common format)
    log::info!("[Webcam] Trying MJPEG 1280x720@30...");
    if let Ok(cam) = try_create_camera(
        &index,
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
            CameraFormat::new(Resolution::new(1280, 720), FrameFormat::MJPEG, 30),
        )),
    ) {
        log::info!("[Webcam] Success with MJPEG");
        return Ok(cam);
    }

    // Strategy 5: Low resolution fallback (640x480 is nearly universal)
    log::info!("[Webcam] Trying 640x480 fallback...");
    if let Ok(cam) = try_create_camera(
        &index,
        RequestedFormat::new::<RgbFormat>(RequestedFormatType::Closest(
            CameraFormat::new(Resolution::new(640, 480), FrameFormat::YUYV, 30),
        )),
    ) {
        log::info!("[Webcam] Success with 640x480");
        return Ok(cam);
    }

    log::error!("[Webcam] All strategies failed for camera {:?}", index);
    Err(anyhow::anyhow!("Failed to open camera after trying all format strategies"))
}

/// Helper: create camera with panic protection (nokhwa can panic on some backends).
fn try_create_camera(index: &CameraIndex, format: RequestedFormat) -> anyhow::Result<Camera> {
    match std::panic::catch_unwind(|| Camera::new(index.clone(), format)) {
        Ok(Ok(cam)) => Ok(cam),
        Ok(Err(e)) => {
            log::debug!("[Webcam] Strategy failed: {:?}", e);
            Err(anyhow::anyhow!("{:?}", e))
        }
        Err(_) => {
            log::warn!("[Webcam] Camera::new panicked");
            Err(anyhow::anyhow!("Camera::new panicked"))
        }
    }
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

/// List available webcam devices using the platform-native query API.
///
/// On macOS this uses AVFoundation device enumeration rather than trying
/// to open each camera index, which avoids silent failures.
pub fn list_cameras() -> Vec<String> {
    match std::panic::catch_unwind(|| {
        if let Some(backend) = nokhwa::native_api_backend() {
            match nokhwa::query(backend) {
                Ok(devices) => devices
                    .iter()
                    .map(|info| info.human_name().to_string())
                    .collect(),
                Err(e) => {
                    log::warn!("[Webcam] Failed to query cameras: {:?}", e);
                    Vec::new()
                }
            }
        } else {
            log::warn!("[Webcam] No native camera backend available");
            Vec::new()
        }
    }) {
        Ok(cameras) => cameras,
        Err(_) => {
            log::error!("[Webcam] Camera enumeration panicked — returning empty list");
            Vec::new()
        }
    }
}
