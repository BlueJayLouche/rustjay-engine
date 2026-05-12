//! Syphon Client - Receives frames from a Syphon server

use crate::{Result, SyphonError};
use crate::directory::ServerInfo;

#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use objc::runtime::{Class, Object};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};
#[cfg(target_os = "macos")]
use objc_id::ShareId;

/// A frame received from a Syphon server
pub struct Frame {
    #[cfg(target_os = "macos")]
    pub(crate) surface: io_surface::IOSurface,
    /// Retained `newFrameImage` Metal texture — kept alive so Syphon doesn't
    /// recycle the underlying IOSurface back to the server until this frame
    /// is fully consumed (blit complete).  Released in `Drop`.
    #[cfg(target_os = "macos")]
    frame_texture: *mut objc::runtime::Object,
    pub width: u32,
    pub height: u32,
}

#[cfg(target_os = "macos")]
unsafe impl Send for Frame {}
#[cfg(target_os = "macos")]
unsafe impl Sync for Frame {}

impl Drop for Frame {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        unsafe {
            if !self.frame_texture.is_null() {
                let _: () = objc::msg_send![self.frame_texture, release];
            }
        }
    }
}

impl Frame {
    #[cfg(target_os = "macos")]
    pub fn iosurface_id(&self) -> io_surface::IOSurfaceID {
        self.surface.get_id()
    }

    /// Zero-copy access: create a Metal texture directly from this surface.
    #[cfg(target_os = "macos")]
    pub fn iosurface(&self) -> &io_surface::IOSurface {
        &self.surface
    }

    /// Raw pointer to the `id<MTLTexture>` returned by `newFrameImage`.
    ///
    /// This is the authoritative texture managed by `SyphonMetalClient` — it
    /// was created on the *same* Metal device as the client, so it is safe to
    /// use as a blit source on that device.  The texture is retained for the
    /// lifetime of this `Frame`; Metal command buffers additionally retain
    /// every resource they reference, so it is safe to commit a blit and then
    /// drop the frame before the GPU work completes.
    ///
    /// Returns `null` if `newFrameImage` returned `nil` (server stopped, etc.).
    #[cfg(target_os = "macos")]
    pub fn metal_texture_ptr(&self) -> *mut objc::runtime::Object {
        self.frame_texture
    }

    #[cfg(target_os = "macos")]
    pub fn iosurface_ref(&self) -> io_surface::IOSurfaceRef {
        self.surface.as_concrete_TypeRef()
    }

    /// Lock the surface for CPU reading. Returns `(base_addr, seed)`.
    /// Must be paired with [`unlock`](Frame::unlock).
    #[cfg(target_os = "macos")]
    pub fn lock(&mut self) -> Result<(*mut u8, u32)> {
        use crate::iosurface_ext::{IOSurfaceLock, kIOSurfaceLockReadOnly, kIOSurfaceLockAvoidSync};

        unsafe {
            let surface_ref = self.surface.as_CFTypeRef() as io_surface::IOSurfaceRef;
            let mut seed = 0u32;

            let result = IOSurfaceLock(surface_ref, kIOSurfaceLockReadOnly, &mut seed);
            if result != 0 {
                let result2 = IOSurfaceLock(
                    surface_ref,
                    kIOSurfaceLockReadOnly | kIOSurfaceLockAvoidSync,
                    &mut seed,
                );
                if result2 != 0 {
                    return Err(SyphonError::LockFailed);
                }
            }

            let addr = crate::iosurface_ext::IOSurfaceGetBaseAddress(surface_ref);
            if addr.is_null() {
                let _ = self.unlock(seed);
                return Err(SyphonError::LockFailed);
            }

            Ok((addr as *mut u8, seed))
        }
    }

    #[cfg(target_os = "macos")]
    pub fn unlock(&mut self, seed: u32) -> Result<()> {
        use crate::iosurface_ext::IOSurfaceUnlock;

        unsafe {
            let surface_ref = self.surface.as_CFTypeRef() as io_surface::IOSurfaceRef;
            let mut seed_copy = seed;
            let result = IOSurfaceUnlock(surface_ref, 0, &mut seed_copy);
            if result != 0 {
                log::trace!("[Frame] IOSurfaceUnlock failed ({}), ignoring", result);
                return Err(SyphonError::LockFailed);
            }
            Ok(())
        }
    }

    #[cfg(target_os = "macos")]
    pub fn bytes_per_row(&self) -> usize {
        use crate::iosurface_ext::IOSurfaceGetBytesPerRow;
        unsafe {
            IOSurfaceGetBytesPerRow(self.surface.as_CFTypeRef() as io_surface::IOSurfaceRef)
        }
    }

    /// CPU-copy the frame data into a `Vec<u8>`.
    #[cfg(target_os = "macos")]
    pub fn to_vec(&mut self) -> Result<Vec<u8>> {
        use std::slice;

        let (addr, seed) = self.lock()?;
        let height = self.height as usize;
        let stride = self.bytes_per_row();

        unsafe {
            let data = slice::from_raw_parts(addr, height * stride).to_vec();
            let _ = self.unlock(seed);
            Ok(data)
        }
    }
}

// ---------------------------------------------------------------------------

/// Wraps `block::RcBlock` to make it `Send + Sync`.
///
/// SAFETY: The block captures only `SyncSender<()>`, which is `Send + Sync`.
/// The Syphon framework may fire the handler from any thread, so the block
/// itself must be `Send`. We never invoke the block concurrently from Rust —
/// only the framework does — so `Sync` is also sound.
#[cfg(target_os = "macos")]
struct FrameHandlerBlock(
    block::RcBlock<(*mut objc::runtime::Object,), ()>,
);
#[cfg(target_os = "macos")]
unsafe impl Send for FrameHandlerBlock {}
#[cfg(target_os = "macos")]
unsafe impl Sync for FrameHandlerBlock {}

/// A Syphon client that receives frames from a named server.
pub struct SyphonClient {
    #[cfg(target_os = "macos")]
    inner: ShareId<Object>,
    info: ServerInfo,
    /// Keeps the ObjC block alive for the lifetime of the client.
    /// Only set when the client was created via `connect_with_channel`.
    #[cfg(target_os = "macos")]
    _handler_block: Option<Box<dyn std::any::Any>>,
}

// SAFETY: `SyphonMetalClient` is backed by an ARC-managed ObjC object.
// The Syphon framework documents that `newFrameHandler` blocks may fire on
// any thread, implying the client's internal state is protected under a lock.
// The selectors we call — `hasNewFrame`, `newSurface`, `newFrameImage`,
// `isValid`, `stop` — are all safe to invoke from any thread.
// `info` is a plain Rust value written once at construction and never mutated.
// Sync: all our ObjC calls are individually thread-safe, so concurrent shared
// references (e.g. two threads calling `has_new_frame()`) are sound.
#[cfg(target_os = "macos")]
unsafe impl Send for SyphonClient {}
#[cfg(target_os = "macos")]
unsafe impl Sync for SyphonClient {}

impl SyphonClient {
    /// Connect to a server by display name or app name.
    ///
    /// Returns [`SyphonError::AmbiguousServerName`] when multiple servers share
    /// the same name. In that case call [`connect_by_info`](Self::connect_by_info)
    /// after listing servers with [`SyphonServerDirectory::servers()`].
    pub fn connect(server_name: &str) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            unsafe { objc::rc::autoreleasepool(|| Self::connect_by_name_macos(server_name)) }
        }
        #[cfg(not(target_os = "macos"))]
        { Err(SyphonError::NotAvailable) }
    }

    /// Connect using a [`ServerInfo`] obtained from [`SyphonServerDirectory`].
    ///
    /// Matches by UUID — the only unambiguous identifier. Prefer this over
    /// `connect()` in any production code that might encounter multiple servers.
    pub fn connect_by_info(info: &ServerInfo) -> Result<Self> {
        #[cfg(target_os = "macos")]
        {
            unsafe { objc::rc::autoreleasepool(|| Self::connect_by_info_macos(info)) }
        }
        #[cfg(not(target_os = "macos"))]
        { Err(SyphonError::NotAvailable) }
    }

    /// Connect with push-based frame delivery via a channel.
    ///
    /// Returns `(client, receiver)`. The receiver yields `()` each time the
    /// server publishes a new frame, using the Syphon `newFrameHandler` block —
    /// no CPU polling. Call [`try_receive`](Self::try_receive) after waking.
    ///
    /// The signal fires on an arbitrary thread; do not call `try_receive`
    /// from inside a handler registered elsewhere.
    ///
    /// The channel closes automatically when the client is dropped or the
    /// server stops.
    pub fn connect_with_channel(
        server_name: &str,
    ) -> Result<(Self, std::sync::mpsc::Receiver<()>)> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        #[cfg(target_os = "macos")]
        {
            let client = unsafe {
                objc::rc::autoreleasepool(|| Self::connect_by_name_with_tx(server_name, Some(tx)))
            }?;
            Ok((client, rx))
        }
        #[cfg(not(target_os = "macos"))]
        { Err(SyphonError::NotAvailable) }
    }

    /// Connect by [`ServerInfo`] with push-based frame delivery via a channel.
    ///
    /// UUID-based; prefer this over [`connect_with_channel`](Self::connect_with_channel)
    /// when multiple servers might share a name.
    pub fn connect_by_info_with_channel(
        info: &ServerInfo,
    ) -> Result<(Self, std::sync::mpsc::Receiver<()>)> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        #[cfg(target_os = "macos")]
        {
            let client = unsafe {
                objc::rc::autoreleasepool(|| Self::connect_by_info_with_tx(info, Some(tx)))
            }?;
            Ok((client, rx))
        }
        #[cfg(not(target_os = "macos"))]
        { Err(SyphonError::NotAvailable) }
    }

    // -----------------------------------------------------------------------
    // macOS internals
    // -----------------------------------------------------------------------

    /// Acquire the shared SyphonServerDirectory instance.
    #[cfg(target_os = "macos")]
    unsafe fn shared_dir() -> Result<*mut Object> {
        Class::get("SyphonServerDirectory")
            .map(|cls| { let dir: *mut Object = msg_send![cls, sharedDirectory]; dir })
            .ok_or_else(|| SyphonError::FrameworkNotFound(
                "SyphonServerDirectory not found".to_string()
            ))
    }

    /// If `dir.servers` is empty, send `requestServerAnnounce` and spin the
    /// run loop for 200 ms so incoming NSNotifications can be delivered.
    #[cfg(target_os = "macos")]
    unsafe fn ensure_servers_populated(dir: *mut Object) {
        let servers: *mut Object = msg_send![dir, servers];
        let count: usize = msg_send![servers, count];
        if count > 0 { return; }

        // Ask servers to re-announce, but do NOT spin the run loop.
        // See the matching comment in `directory.rs::servers_inner` —
        // spinning `[NSRunLoop runUntilDate:]` from inside a winit
        // ApplicationHandler callback causes winit's re-entrancy guard to panic.
        // The caller should retry if `ServerNotFound` is returned on first use.
        let _: () = msg_send![dir, requestServerAnnounce];
    }

    #[cfg(target_os = "macos")]
    unsafe fn connect_by_name_macos(name: &str) -> Result<Self> {
        Self::connect_by_name_with_tx(name, None)
    }

    #[cfg(target_os = "macos")]
    unsafe fn connect_by_info_macos(info: &ServerInfo) -> Result<Self> {
        Self::connect_by_info_with_tx(info, None)
    }

    /// Internal: find a server by name/app-name, then create a client.
    /// `tx` is `Some` when push delivery is requested.
    #[cfg(target_os = "macos")]
    unsafe fn connect_by_name_with_tx(
        name: &str,
        tx: Option<std::sync::mpsc::SyncSender<()>>,
    ) -> Result<Self> {
        let dir = Self::shared_dir()?;
        Self::ensure_servers_populated(dir);

        let servers: *mut Object = msg_send![dir, servers];
        let count: usize = msg_send![servers, count];

        let mut first_match: *mut Object = std::ptr::null_mut();
        let mut first_info: Option<ServerInfo> = None;
        let mut match_count = 0usize;

        for i in 0..count {
            let desc: *mut Object = msg_send![servers, objectAtIndex: i];
            let n = Self::str_from_desc(desc, "SyphonServerDescriptionNameKey");
            let a = Self::str_from_desc(desc, "SyphonServerDescriptionAppNameKey");

            if n == name || a == name {
                match_count += 1;
                if first_match.is_null() {
                    let u = Self::str_from_desc(desc, "SyphonServerDescriptionUUIDKey");
                    let b = Self::str_from_desc(desc, "SyphonServerDescriptionAppBundleIdentifierKey");
                    let _: () = msg_send![desc, retain];
                    first_match = desc;
                    first_info = Some(ServerInfo { name: n, uuid: u, app_name: a, bundle_id: b });
                }
            }
        }

        if first_match.is_null() {
            return Err(SyphonError::ServerNotFound(name.to_string()));
        }

        if match_count > 1 {
            let _: () = msg_send![first_match, release];
            return Err(SyphonError::AmbiguousServerName(format!(
                "{} servers match name '{}'. \
                 List servers with SyphonServerDirectory::servers() and call connect_by_info().",
                match_count, name
            )));
        }

        let info = first_info.unwrap();
        log::info!("[SyphonClient] Connecting to '{}' (uuid={})", info.display_name(), info.uuid);
        let result = Self::create_client(first_match, info, tx);
        let _: () = msg_send![first_match, release];
        result
    }

    /// Internal: find a server by UUID, then create a client.
    #[cfg(target_os = "macos")]
    unsafe fn connect_by_info_with_tx(
        info: &ServerInfo,
        tx: Option<std::sync::mpsc::SyncSender<()>>,
    ) -> Result<Self> {
        let dir = Self::shared_dir()?;
        Self::ensure_servers_populated(dir);

        let servers: *mut Object = msg_send![dir, servers];
        let count: usize = msg_send![servers, count];
        let mut found: *mut Object = std::ptr::null_mut();

        for i in 0..count {
            let desc: *mut Object = msg_send![servers, objectAtIndex: i];
            let u = Self::str_from_desc(desc, "SyphonServerDescriptionUUIDKey");
            if u == info.uuid {
                let _: () = msg_send![desc, retain];
                found = desc;
                break;
            }
        }

        if found.is_null() {
            return Err(SyphonError::ServerNotFound(format!("uuid={}", info.uuid)));
        }

        log::info!("[SyphonClient] Connecting to '{}' (uuid={})", info.display_name(), info.uuid);
        let result = Self::create_client(found, info.clone(), tx);
        let _: () = msg_send![found, release];
        result
    }

    /// Build a `SyphonMetalClient` from a retained server description pointer.
    ///
    /// When `tx` is `Some`, an ObjC block is created that signals the sender
    /// on every new frame (via `newFrameHandler:`). The block is stored on the
    /// struct so it is kept alive until the client is dropped.
    #[cfg(target_os = "macos")]
    unsafe fn create_client(
        server_desc: *mut Object,
        info: ServerInfo,
        tx: Option<std::sync::mpsc::SyncSender<()>>,
    ) -> Result<Self> {
        let device = crate::metal_device::default_device()
            .map(|d| d.raw_device)
            .ok_or_else(|| SyphonError::FrameworkNotFound("Metal not available".to_string()))?;

        let cls = Class::get("SyphonMetalClient")
            .ok_or_else(|| SyphonError::FrameworkNotFound(
                "SyphonMetalClient class not found".to_string()
            ))?;

        // Build the ObjC block if the caller wants push delivery.
        // The block captures a SyncSender<()> and calls try_send on every frame.
        // We wrap it in FrameHandlerBlock (Send+Sync) so it can live in the struct.
        let (handler_ptr, handler_block): (*mut Object, Option<Box<dyn std::any::Any>>) =
            match tx {
                Some(sender) => {
                    let blk = block::ConcreteBlock::new(move |_client: *mut Object| {
                        // Non-blocking: drop the signal if the receiver is full or gone.
                        let _ = sender.try_send(());
                    }).copy();
                    let ptr = &*blk as *const _ as *mut Object;
                    (ptr, Some(Box::new(FrameHandlerBlock(blk))))
                }
                None => (std::ptr::null_mut(), None),
            };

        let init_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let obj: *mut Object = msg_send![cls, alloc];
            let obj: *mut Object = msg_send![
                obj,
                initWithServerDescription: server_desc
                device: device
                options: std::ptr::null_mut::<Object>()
                newFrameHandler: handler_ptr
            ];
            obj
        }));

        let obj = match init_result {
            Ok(o) => o,
            Err(_) => return Err(SyphonError::CreateFailed(
                "SyphonMetalClient init threw an Objective-C exception".to_string()
            )),
        };

        if obj.is_null() {
            return Err(SyphonError::CreateFailed("SyphonMetalClient returned nil".to_string()));
        }

        let is_valid: bool = msg_send![obj, isValid];
        if !is_valid {
            return Err(SyphonError::CreateFailed(
                "SyphonMetalClient.isValid is false — server may have stopped".to_string()
            ));
        }

        Ok(Self { inner: ShareId::from_ptr(obj), info, _handler_block: handler_block })
    }

    #[cfg(target_os = "macos")]
    unsafe fn str_from_desc(desc: *mut Object, key: &str) -> String {
        use crate::utils::{to_nsstring, from_nsstring};
        let k = match to_nsstring(key) { Ok(k) => k, Err(_) => return String::new() };
        let v: *mut Object = msg_send![desc, objectForKey: k];
        if v.is_null() { String::new() } else { from_nsstring(v) }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Non-blocking frame poll. Returns `None` when no new frame is available.
    ///
    /// Safe to call from any thread — autorelease pool is managed internally.
    #[cfg(target_os = "macos")]
    pub fn try_receive(&self) -> Result<Option<Frame>> {
        unsafe {
            objc::rc::autoreleasepool(|| {
                let has_new: bool = msg_send![&*self.inner, hasNewFrame];
                if !has_new { return Ok(None); }

                // `newFrameImage` returns a RETAINED id<MTLTexture> (Cocoa "new" rule —
                // caller owns the +1 retain).  Do NOT call retain again; Frame::drop
                // calls release exactly once.
                let frame_texture: *mut Object = msg_send![&*self.inner, newFrameImage];
                if frame_texture.is_null() { return Ok(None); }

                // Get dimensions from the Metal texture directly — public API,
                // no private selectors needed.
                let width:  u64 = msg_send![frame_texture, width];
                let height: u64 = msg_send![frame_texture, height];

                // `MTLTexture.iosurface` is a property getter (get-rule: not retained).
                // wrap_under_get_rule retains once; IOSurface::drop releases once → net zero.
                let ios_ref: io_surface::IOSurfaceRef = msg_send![frame_texture, iosurface];
                if ios_ref.is_null() { return Ok(None); }
                let surface = io_surface::IOSurface::wrap_under_get_rule(ios_ref);

                Ok(Some(Frame { surface, frame_texture, width: width as u32, height: height as u32 }))
            })
        }
    }

    /// Whether a new frame has arrived since the last `try_receive` call.
    ///
    /// Safe to call from any thread — autorelease pool is managed internally.
    #[cfg(target_os = "macos")]
    pub fn has_new_frame(&self) -> bool {
        unsafe {
            objc::rc::autoreleasepool(|| msg_send![&*self.inner, hasNewFrame])
        }
    }

    /// Block until a frame is available.
    #[cfg(target_os = "macos")]
    pub fn receive(&self) -> Result<Frame> {
        loop {
            if let Some(frame) = self.try_receive()? { return Ok(frame); }
            std::thread::yield_now();
        }
    }

    /// `true` if the underlying `SyphonMetalClient.isValid` is still set.
    ///
    /// Safe to call from any thread — autorelease pool is managed internally.
    #[cfg(target_os = "macos")]
    pub fn is_connected(&self) -> bool {
        unsafe {
            objc::rc::autoreleasepool(|| msg_send![&*self.inner, isValid])
        }
    }

    /// Full server metadata.
    pub fn server_info(&self) -> &ServerInfo {
        &self.info
    }

    /// Convenience: server display name.
    pub fn server_name(&self) -> &str {
        self.info.display_name()
    }

    /// Convenience: owning application name.
    pub fn server_app(&self) -> &str {
        &self.info.app_name
    }

    pub fn stop(&self) {
        #[cfg(target_os = "macos")]
        unsafe {
            objc::rc::autoreleasepool(|| { let _: () = msg_send![&*self.inner, stop]; });
        }
    }
}

impl Drop for SyphonClient {
    fn drop(&mut self) {
        self.stop();
        log::debug!("[SyphonClient] dropped (server='{}')", self.info.display_name());
    }
}
