//! Syphon Server - Publishes frames for other apps to receive
//!
//! This wraps the Objective-C SyphonMetalServer class

use crate::{Result, SyphonError};

/// Options for creating a [`SyphonServer`].
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// When `true`, the server is invisible to [`SyphonServerDirectory`] and
    /// will not appear in discovery listings. Clients must receive the server
    /// description through another channel (e.g. inter-process messaging).
    ///
    /// Corresponds to `SyphonServerOptionIsPrivate`.
    pub is_private: bool,
}

// Objective-C imports
#[cfg(target_os = "macos")]
use objc::runtime::{Class, Object};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use objc_id::ShareId;

/// A Syphon server that publishes frames
///
/// # Example
///
/// ```no_run
/// use syphon_core::SyphonServer;
///
/// let server = SyphonServer::new("My Rust App", 1920, 1080).unwrap();
/// ```
pub struct SyphonServer {
    #[cfg(target_os = "macos")]
    inner: ShareId<Object>,
    
    name: String,
    width: u32,
    height: u32,
}

// SAFETY: `SyphonMetalServer` is backed by an ObjC object wrapped in
// `ShareId<Object>` (i.e. an ARC-managed retain/release pair). The Syphon
// framework documents that `SyphonMetalServer` is thread-safe for publishing:
// `publishFrameTexture:onCommandBuffer:imageRegion:flipped:` and
// `hasClients`/`stop` can be called from any thread. The only mutable state
// (`width`, `height`, `name`) is written once at construction and never again.
#[cfg(target_os = "macos")]
unsafe impl Send for SyphonServer {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SyphonServer {}

impl SyphonServer {
    /// Create a new Syphon server using the system default Metal device.
    pub fn new(name: &str, width: u32, height: u32) -> Result<Self> {
        Self::new_with_options(name, width, height, ServerOptions::default())
    }

    /// Create a new Syphon server with explicit [`ServerOptions`].
    ///
    /// Use this to create private servers (`options.is_private = true`) that
    /// are invisible to [`SyphonServerDirectory`].
    pub fn new_with_options(
        name: &str,
        width: u32,
        height: u32,
        options: ServerOptions,
    ) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            let device = Self::create_default_metal_device()
                .ok_or_else(|| SyphonError::CreateFailed(
                    "Failed to create Metal device".to_string()
                ))?;
            Self::new_macos(name, device, width, height, options)
        }
        #[cfg(not(target_os = "macos"))]
        { Err(SyphonError::NotAvailable) }
    }

    /// Create a new Syphon server with a specific Metal device.
    pub fn new_with_name_and_device(
        name: &str,
        metal_device: *mut Object,
        width: u32,
        height: u32,
    ) -> Result<Self> {
        Self::new_with_name_and_device_and_options(
            name, metal_device, width, height, ServerOptions::default()
        )
    }

    /// Create a new Syphon server with a specific Metal device and options.
    pub fn new_with_name_and_device_and_options(
        name: &str,
        metal_device: *mut Object,
        width: u32,
        height: u32,
        options: ServerOptions,
    ) -> Result<Self> {
        #[cfg(target_os = "macos")]
        { Self::new_macos(name, metal_device, width, height, options) }
        #[cfg(not(target_os = "macos"))]
        { Err(SyphonError::NotAvailable) }
    }
    
    #[cfg(target_os = "macos")]
    fn create_default_metal_device() -> Option<*mut Object> {
        unsafe {
            extern "C" {
                fn MTLCreateSystemDefaultDevice() -> *mut Object;
            }
            
            let device = MTLCreateSystemDefaultDevice();
            if device.is_null() {
                None
            } else {
                Some(device)
            }
        }
    }
    
    #[cfg(target_os = "macos")]
    fn new_macos(
        name: &str,
        metal_device: *mut Object,
        width: u32,
        height: u32,
        options: ServerOptions,
    ) -> Result<Self> {
        use crate::utils::to_nsstring;

        unsafe {
            objc::rc::autoreleasepool(|| {
                let cls = Class::get("SyphonMetalServer")
                    .or_else(|| Class::get("SyphonServer"))
                    .ok_or_else(|| SyphonError::FrameworkNotFound(
                        "SyphonMetalServer class not found".to_string()
                    ))?;

                let ns_name = to_nsstring(name)?;

                // Build an NSDictionary for options when needed, otherwise pass nil.
                let options_dict: *mut Object = if options.is_private {
                    Self::build_options_dict(&options)
                } else {
                    std::ptr::null_mut()
                };

                let obj: *mut Object = msg_send![cls, alloc];
                let obj: *mut Object = msg_send![
                    obj,
                    initWithName: ns_name
                    device: metal_device
                    options: options_dict
                ];

                // ns_name is autoreleased (from stringWithUTF8String:) —
                // do NOT release it; the enclosing autoreleasepool handles it.
                if !options_dict.is_null() {
                    let _: () = msg_send![options_dict, release];
                }

                if obj.is_null() {
                    return Err(SyphonError::CreateFailed("Failed to create SyphonServer".to_string()));
                }

                Ok(Self { inner: ShareId::from_ptr(obj), name: name.to_string(), width, height })
            })
        }
    }

    /// Build an NSDictionary from `ServerOptions` to pass to `initWithName:device:options:`.
    #[cfg(target_os = "macos")]
    unsafe fn build_options_dict(options: &ServerOptions) -> *mut Object {
        use crate::utils::to_nsstring;

        // [NSMutableDictionary dictionary] — autoreleased, so we retain+return.
        let dict_cls = Class::get("NSMutableDictionary").unwrap();
        let dict: *mut Object = msg_send![dict_cls, dictionary];
        let _: () = msg_send![dict, retain];

        if options.is_private {
            // SyphonServerOptionIsPrivate = @"SyphonServerOptionIsPrivate"
            let key = to_nsstring("SyphonServerOptionIsPrivate").unwrap();
            // NSNumber numberWithBool:YES
            let num_cls = Class::get("NSNumber").unwrap();
            let val: *mut Object = msg_send![num_cls, numberWithBool: true];
            let _: () = msg_send![dict, setObject: val forKey: key];
        }

        dict
    }
    
    /// Get the server name
    pub fn name(&self) -> &str {
        &self.name
    }
    
    /// Get dimensions
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    
    /// Publish a Metal texture to the server.
    ///
    /// # Safety
    /// `texture` and `command_buffer` must be valid objects from the same
    /// Metal device the server was created with.
    ///
    /// Safe to call from any thread — autorelease pool is managed internally.
    #[cfg(target_os = "macos")]
    pub unsafe fn publish_metal_texture(
        &self,
        texture: *mut Object,        // id<MTLTexture>
        command_buffer: *mut Object, // id<MTLCommandBuffer>
    ) {
        use cocoa::foundation::{NSRect, NSPoint, NSSize};
        use objc::rc::autoreleasepool;

        autoreleasepool(|| {
            let region = NSRect {
                origin: NSPoint::new(0.0, 0.0),
                size: NSSize::new(self.width as f64, self.height as f64),
            };
            let _: () = msg_send![
                &*self.inner,
                publishFrameTexture: texture
                onCommandBuffer: command_buffer
                imageRegion: region
                flipped: false
            ];
        });
    }

    /// Returns 1 if any client is connected, 0 otherwise.
    ///
    /// (The Syphon framework only exposes `hasClients: BOOL`, not a count.)
    #[cfg(target_os = "macos")]
    pub fn client_count(&self) -> usize {
        unsafe {
            objc::rc::autoreleasepool(|| {
                let has: bool = msg_send![&*self.inner, hasClients];
                if has { 1 } else { 0 }
            })
        }
    }

    /// `true` if at least one client is connected.
    pub fn has_clients(&self) -> bool {
        self.client_count() > 0
    }

    /// Stop the server and notify all clients.
    pub fn stop(&self) {
        #[cfg(target_os = "macos")]
        unsafe {
            objc::rc::autoreleasepool(|| { let _: () = msg_send![&*self.inner, stop]; });
        }
    }
}

impl Drop for SyphonServer {
    fn drop(&mut self) {
        // Explicitly stop before the ObjC object is released so clients
        // receive the retire notification promptly.
        self.stop();
        log::debug!("[SyphonServer] '{}' dropped", self.name);
    }
}
