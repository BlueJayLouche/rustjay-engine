//! FFmpeg D3D12VA frames consumed directly by wgpu's DX12 backend.
//!
//! FFmpeg and wgpu share one `ID3D12Device`: wgpu owns the device and FFmpeg
//! adopts it through a caller-allocated `AVHWDeviceContext`. The D3D12VA pool
//! allocates one committed NV12 resource per frame (not shareable across
//! devices — same-device adoption is the only interop that exists), each with
//! its own decode fence. Consumption imports the resource's two planes as
//! wgpu textures and pairs the decode fence with the exact conversion
//! submission through [`crate::SharedQueue`].
//!
//! Resource states never need manual barriers: FFmpeg transitions each
//! resource COMMON → VIDEO_DECODE_WRITE → COMMON around a decode, and the
//! imported planes are registered with wgpu in the `RESOURCE` state so wgpu
//! emits no transitions of its own — sampling rides D3D12's implicit
//! read-only promotion from COMMON, which decays back at the end of each
//! `ExecuteCommandLists`.

use crate::VideoFrame;
use crate::frame_lease::{LeaseBudget, LeasePermit, expanded_pool_size};
use crate::submit_queue::DecodeFence;
use ffmpeg_next::ffi;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12Fence, ID3D12Resource};
use windows::core::Interface;

/// The D3D12VA pool has no texture-array axis, but each pool slot is a whole
/// committed NV12 resource (~9 MB at 5120x1216), so keep the expansion bound
/// modest.
const MAX_POOL_RESOURCES: u32 = 64;

#[repr(C)]
struct AvD3d12VaDeviceContext {
    device: *mut c_void,
    video_device: *mut c_void,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

#[repr(C)]
struct AvD3d12VaSyncContext {
    fence: *mut c_void,
    event: *mut c_void,
    fence_value: u64,
}

#[repr(C)]
struct AvD3d12VaFrame {
    texture: *mut c_void,
    sync_ctx: AvD3d12VaSyncContext,
}

#[repr(C)]
struct AvD3d12VaFramesContext {
    format: u32,
    flags: u32,
}

const _: () = {
    if cfg!(target_pointer_width = "64") {
        assert!(std::mem::size_of::<AvD3d12VaDeviceContext>() == 40);
        assert!(std::mem::offset_of!(AvD3d12VaDeviceContext, device) == 0);
        assert!(std::mem::offset_of!(AvD3d12VaDeviceContext, video_device) == 8);
        assert!(std::mem::offset_of!(AvD3d12VaDeviceContext, lock) == 16);
        assert!(std::mem::offset_of!(AvD3d12VaDeviceContext, unlock) == 24);
        assert!(std::mem::offset_of!(AvD3d12VaDeviceContext, lock_ctx) == 32);

        assert!(std::mem::size_of::<AvD3d12VaSyncContext>() == 24);
        assert!(std::mem::size_of::<AvD3d12VaFrame>() == 32);
        assert!(std::mem::offset_of!(AvD3d12VaFrame, texture) == 0);
        assert!(std::mem::offset_of!(AvD3d12VaFrame, sync_ctx) == 8);

        assert!(std::mem::size_of::<AvD3d12VaFramesContext>() == 8);
        assert!(std::mem::offset_of!(AvD3d12VaFramesContext, format) == 0);
        assert!(std::mem::offset_of!(AvD3d12VaFramesContext, flags) == 4);
    }
};

pub(crate) struct InteropDevice {
    device: wgpu::Device,
    raw_device: ID3D12Device,
    adapter_name: String,
}

impl InteropDevice {
    pub(crate) fn new(
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> Result<Self, String> {
        let raw_device = {
            let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Dx12>() }
                .ok_or_else(|| "DX12 HAL device is unavailable".to_string())?;
            hal_device.raw_device().clone()
        };
        Ok(Self {
            device: device.clone(),
            raw_device,
            adapter_name: adapter.get_info().name,
        })
    }

    pub(crate) fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Allocates an FFmpeg D3D12VA device context adopting wgpu's
    /// `ID3D12Device`. The returned buffer owns one device reference; FFmpeg
    /// releases it when the context is freed.
    pub(crate) unsafe fn create_hw_device_ctx(&self) -> Result<*mut ffi::AVBufferRef, String> {
        let device_ref =
            unsafe { ffi::av_hwdevice_ctx_alloc(ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_D3D12VA) };
        if device_ref.is_null() {
            return Err("av_hwdevice_ctx_alloc(d3d12va) failed".into());
        }
        let hw_device = unsafe { &mut *((*device_ref).data.cast::<ffi::AVHWDeviceContext>()) };
        let d3d12 = unsafe {
            hw_device
                .hwctx
                .cast::<AvD3d12VaDeviceContext>()
                .as_mut()
                .ok_or_else(|| "FFmpeg D3D12VA device hwctx is null".to_string())
        };
        let d3d12 = match d3d12 {
            Ok(d3d12) => d3d12,
            Err(reason) => {
                let mut device_ref = device_ref;
                unsafe { ffi::av_buffer_unref(&mut device_ref) };
                return Err(reason);
            }
        };
        // Transfer one COM reference; av_hwdevice_ctx uninit releases it.
        d3d12.device = self.raw_device.clone().into_raw();
        if unsafe { ffi::av_hwdevice_ctx_init(device_ref) } < 0 {
            let mut device_ref = device_ref;
            unsafe { ffi::av_buffer_unref(&mut device_ref) };
            return Err("av_hwdevice_ctx_init(d3d12va) rejected the wgpu device".into());
        }
        Ok(device_ref)
    }

    fn import_pool(&self, frames: Arc<FramesContextRef>) -> Result<Arc<PoolInterop>, String> {
        let (width, height) = {
            let frames_context = unsafe { frames.frames() };
            (frames_context.width, frames_context.height)
        };
        let width = u32::try_from(width).map_err(|_| "pool width does not fit u32".to_string())?;
        let height =
            u32::try_from(height).map_err(|_| "pool height does not fit u32".to_string())?;
        Ok(Arc::new(PoolInterop {
            device: self.device.clone(),
            _frames: frames,
            allocated: (width, height),
            imports: Mutex::new(HashMap::new()),
        }))
    }
}

pub(crate) struct FramesContextRef(*mut ffi::AVBufferRef);

unsafe impl Send for FramesContextRef {}
unsafe impl Sync for FramesContextRef {}

impl FramesContextRef {
    unsafe fn from_borrowed(reference: *mut ffi::AVBufferRef) -> Option<Self> {
        let reference = unsafe { ffi::av_buffer_ref(reference) };
        (!reference.is_null()).then_some(Self(reference))
    }

    pub(crate) unsafe fn frames(&self) -> &ffi::AVHWFramesContext {
        unsafe { &*((*self.0).data.cast::<ffi::AVHWFramesContext>()) }
    }
}

impl Drop for FramesContextRef {
    fn drop(&mut self) {
        unsafe { ffi::av_buffer_unref(&mut self.0) };
    }
}

enum PoolSetup {
    Pending,
    Ready(Arc<PoolInterop>),
    Failed(String),
}

#[derive(Clone)]
pub(crate) struct DirectPoolRequest {
    setup: Arc<Mutex<PoolSetup>>,
    device: Arc<InteropDevice>,
    budget: Arc<LeaseBudget>,
    canary_needed: Arc<AtomicBool>,
}

impl DirectPoolRequest {
    pub(crate) fn new(device: Arc<InteropDevice>) -> Self {
        Self {
            setup: Arc::new(Mutex::new(PoolSetup::Pending)),
            device,
            budget: LeaseBudget::new(),
            canary_needed: Arc::new(AtomicBool::new(true)),
        }
    }

    pub(crate) fn interop_device(&self) -> &Arc<InteropDevice> {
        &self.device
    }

    pub(crate) fn failure(&self) -> Option<String> {
        match &*self.setup.lock().unwrap_or_else(|error| error.into_inner()) {
            PoolSetup::Failed(reason) => Some(reason.clone()),
            _ => None,
        }
    }

    pub(crate) fn pool(&self) -> Option<Arc<PoolInterop>> {
        match &*self.setup.lock().unwrap_or_else(|error| error.into_inner()) {
            PoolSetup::Ready(pool) => Some(Arc::clone(pool)),
            _ => None,
        }
    }

    pub(crate) fn take_canary(&self) -> bool {
        self.canary_needed.swap(false, Ordering::AcqRel)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn frame(
        &self,
        frame: *const ffi::AVFrame,
        width: u32,
        height: u32,
        pts: f64,
        full_range: bool,
        bt709: bool,
        canary_readback: Option<VideoFrame>,
    ) -> Result<VideoFrame, String> {
        let pool = self
            .pool()
            .ok_or_else(|| "D3D12VA pool is not initialized".to_string())?;
        let hw_frame = unsafe { (*frame).data[0].cast::<AvD3d12VaFrame>().as_ref() }
            .ok_or_else(|| "decoded frame carries no AVD3D12VAFrame".to_string())?;
        if hw_frame.texture.is_null() {
            return Err("decoded D3D12 texture is null".into());
        }
        if hw_frame.sync_ctx.fence.is_null() {
            return Err("decoded D3D12 frame carries no fence".into());
        }
        let fence = unsafe { ID3D12Fence::from_raw_borrowed(&hw_frame.sync_ctx.fence) }
            .ok_or_else(|| "decoded D3D12 fence is not an ID3D12Fence".to_string())?
            .clone();
        let planes = pool.import(hw_frame.texture)?;
        let permit = self
            .budget
            .try_acquire()
            .ok_or_else(|| "zero-copy frame lease budget exhausted".to_string())?;
        let frame_lease = unsafe { RawFrameLease::new(frame) }?;
        Ok(VideoFrame::d3d12_nv12(
            width,
            height,
            pts,
            D3d12Frame::new(
                pool,
                planes,
                DecodeFence {
                    fence,
                    value: hw_frame.sync_ctx.fence_value,
                },
                full_range,
                bt709,
                frame_lease,
                permit,
                canary_readback,
            ),
        ))
    }

    fn fail(&self, reason: impl Into<String>) {
        *self.setup.lock().unwrap_or_else(|error| error.into_inner()) =
            PoolSetup::Failed(reason.into());
    }

    fn ready(&self, pool: Arc<PoolInterop>) {
        *self.setup.lock().unwrap_or_else(|error| error.into_inner()) = PoolSetup::Ready(pool);
    }
}

/// Configures the decoder's hardware frames context from the `get_format`
/// callback, correcting the pool dimensions to the coded size.
///
/// Upstream FFmpeg (8.0/8.1/master) sizes the D3D12VA pool and decoder heap
/// from the display dimensions (`avctx->width/height`), while HEVC decodes at
/// coded size; any stream whose 16-aligned display height is below its coded
/// height then fails on NVIDIA. The venue 5120x1200 masters code 1216 rows
/// with a 16-row conformance crop and hit this exactly, so the override below
/// is load-bearing, not defensive.
pub(crate) unsafe fn configure_pool(
    codec: *mut ffi::AVCodecContext,
    request: &DirectPoolRequest,
) -> Result<ffi::AVPixelFormat, String> {
    if crate::zero_copy::direct_path_poisoned() {
        request.fail(crate::zero_copy::DIRECT_PATH_POISONED_REASON);
        return Err(crate::zero_copy::DIRECT_PATH_POISONED_REASON.into());
    }
    // FFmpeg changed the AVD3D12VA struct layouts inside the libavutil 60
    // series (the 8.1.2+ texture-array rework), so an additive-looking DLL
    // swap can silently change the ABI these mirrors were built against.
    // Decline the direct path unless the runtime matches the build exactly at
    // major.minor; readback and software are unaffected by the rework.
    let runtime = ffmpeg_next::util::version();
    let built = ((ffi::LIBAVUTIL_VERSION_MAJOR as u32) << 16)
        | ((ffi::LIBAVUTIL_VERSION_MINOR as u32) << 8);
    if runtime >> 8 != built >> 8 {
        let reason = format!(
            "FFmpeg runtime libavutil {}.{} differs from build {}.{}; \
             D3D12VA struct layouts may not match",
            runtime >> 16,
            (runtime >> 8) & 0xff,
            built >> 16,
            (built >> 8) & 0xff,
        );
        request.fail(reason.clone());
        return Err(reason);
    }
    let mut frames_ref = std::ptr::null_mut();
    let result = unsafe {
        ffi::avcodec_get_hw_frames_parameters(
            codec,
            (*codec).hw_device_ctx,
            ffi::AVPixelFormat::AV_PIX_FMT_D3D12,
            &mut frames_ref,
        )
    };
    if result < 0 || frames_ref.is_null() {
        request.fail("avcodec_get_hw_frames_parameters rejected the D3D12VA pool");
        return Err("avcodec_get_hw_frames_parameters failed".into());
    }

    let configured = unsafe { configure_pool_ref(frames_ref, codec) };
    if let Err(reason) = configured {
        request.fail(reason.clone());
        unsafe { ffi::av_buffer_unref(&mut frames_ref) };
        return Err(reason);
    }

    if unsafe { ffi::av_hwframe_ctx_init(frames_ref) } < 0 {
        let reason = "av_hwframe_ctx_init rejected the D3D12VA pool".to_string();
        request.fail(reason.clone());
        unsafe { ffi::av_buffer_unref(&mut frames_ref) };
        return Err(reason);
    }

    let retained = match unsafe { FramesContextRef::from_borrowed(frames_ref) } {
        Some(retained) => retained,
        None => {
            let reason = "av_buffer_ref failed for the D3D12VA pool".to_string();
            request.fail(reason.clone());
            unsafe { ffi::av_buffer_unref(&mut frames_ref) };
            return Err(reason);
        }
    };
    unsafe { ffi::av_buffer_unref(&mut (*codec).hw_frames_ctx) };
    unsafe { (*codec).hw_frames_ctx = frames_ref };
    let pool = request
        .device
        .import_pool(Arc::new(retained))
        .inspect_err(|reason| request.fail(reason.clone()))?;
    request.ready(pool);
    Ok(ffi::AVPixelFormat::AV_PIX_FMT_D3D12)
}

unsafe fn configure_pool_ref(
    frames_ref: *mut ffi::AVBufferRef,
    codec: *const ffi::AVCodecContext,
) -> Result<(), String> {
    let frames = unsafe { &mut *((*frames_ref).data.cast::<ffi::AVHWFramesContext>()) };
    if frames.format != ffi::AVPixelFormat::AV_PIX_FMT_D3D12 {
        return Err("D3D12VA pool format is not AV_PIX_FMT_D3D12".into());
    }
    if frames.sw_format != ffi::AVPixelFormat::AV_PIX_FMT_NV12 {
        return Err("D3D12VA pool software format is not NV12".into());
    }
    let coded_width = unsafe { (*codec).coded_width };
    let coded_height = unsafe { (*codec).coded_height };
    if coded_width <= 0 || coded_height <= 0 || coded_width % 2 != 0 || coded_height % 2 != 0 {
        return Err("D3D12VA pool has invalid coded dimensions".into());
    }
    frames.width = coded_width;
    frames.height = coded_height;
    // D3D12VA pools are dynamic: avcodec reports initial_pool_size 0 and the
    // pool allocates per-frame committed resources on demand (the lease budget
    // bounds how many stay checked out). Only a preallocated pool needs the
    // in-flight expansion.
    if frames.initial_pool_size > 0 {
        frames.initial_pool_size = expanded_pool_size(frames.initial_pool_size, MAX_POOL_RESOURCES)
            .ok_or_else(|| {
                "D3D12VA pool size is invalid or exceeds the resource bound".to_string()
            })?;
    }
    Ok(())
}

struct ImportedPlanes {
    y_view: wgpu::TextureView,
    uv_view: wgpu::TextureView,
    _y_texture: wgpu::Texture,
    _uv_texture: wgpu::Texture,
}

pub(crate) struct PoolInterop {
    device: wgpu::Device,
    _frames: Arc<FramesContextRef>,
    allocated: (u32, u32),
    imports: Mutex<HashMap<usize, Arc<ImportedPlanes>>>,
}

impl std::fmt::Debug for PoolInterop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PoolInterop")
            .field("allocated", &self.allocated)
            .finish_non_exhaustive()
    }
}

impl PoolInterop {
    pub(crate) fn allocated_size(&self) -> (u32, u32) {
        self.allocated
    }

    /// Imports (or reuses) the wgpu wrappers for one pool resource. The pool
    /// recycles a bounded set of committed resources, so the cache is keyed on
    /// the resource pointer and stays small; frames hold an `Arc` so eviction
    /// can never invalidate an in-flight view.
    fn import(&self, resource: *mut c_void) -> Result<Arc<ImportedPlanes>, String> {
        let key = resource as usize;
        let mut imports = self
            .imports
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(planes) = imports.get(&key) {
            return Ok(Arc::clone(planes));
        }
        let resource = unsafe { ID3D12Resource::from_raw_borrowed(&resource) }
            .ok_or_else(|| "decoded D3D12 texture is not an ID3D12Resource".to_string())?
            .clone();
        let (width, height) = self.allocated;

        let error_scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let y_texture = unsafe {
            let hal_texture = wgpu::hal::dx12::Device::texture_from_raw(
                resource.clone(),
                wgpu::TextureFormat::R8Unorm,
                wgpu::TextureDimension::D2,
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                1,
                1,
            )
            .with_plane_slice(0);
            self.device.create_texture_from_hal::<wgpu::hal::api::Dx12>(
                hal_texture,
                &plane_descriptor(width, height, wgpu::TextureFormat::R8Unorm),
                wgpu::TextureUses::RESOURCE,
            )
        };
        let uv_texture = unsafe {
            let hal_texture = wgpu::hal::dx12::Device::texture_from_raw(
                resource,
                wgpu::TextureFormat::Rg8Unorm,
                wgpu::TextureDimension::D2,
                wgpu::Extent3d {
                    width: width / 2,
                    height: height / 2,
                    depth_or_array_layers: 1,
                },
                1,
                1,
            )
            .with_plane_slice(1);
            self.device.create_texture_from_hal::<wgpu::hal::api::Dx12>(
                hal_texture,
                &plane_descriptor(width / 2, height / 2, wgpu::TextureFormat::Rg8Unorm),
                wgpu::TextureUses::RESOURCE,
            )
        };
        let planes = Arc::new(ImportedPlanes {
            y_view: y_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("d3d12va-nv12-y"),
                ..Default::default()
            }),
            uv_view: uv_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("d3d12va-nv12-uv"),
                ..Default::default()
            }),
            _y_texture: y_texture,
            _uv_texture: uv_texture,
        });
        if let Some(error) = pollster::block_on(error_scope.pop()) {
            return Err(format!("D3D12 NV12 plane import failed: {error}"));
        }
        if imports.len() >= MAX_POOL_RESOURCES as usize {
            // In-flight frames hold Arcs, so dropping stale cache entries is
            // safe; a pool that grows this far is recycling badly anyway.
            log::warn!("Video zero-copy: D3D12 import cache exceeded its bound; clearing");
            imports.clear();
        }
        imports.insert(key, Arc::clone(&planes));
        Ok(planes)
    }
}

fn plane_descriptor(
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureDescriptor<'static> {
    wgpu::TextureDescriptor {
        label: Some("d3d12va-nv12-plane"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    }
}

struct RawFrameLease(*mut ffi::AVFrame);

unsafe impl Send for RawFrameLease {}
unsafe impl Sync for RawFrameLease {}

impl RawFrameLease {
    unsafe fn new(source: *const ffi::AVFrame) -> Result<Self, String> {
        let frame = unsafe { ffi::av_frame_alloc() };
        if frame.is_null() {
            return Err("av_frame_alloc failed for zero-copy lease".into());
        }
        if unsafe { ffi::av_frame_ref(frame, source) } < 0 {
            let mut frame = frame;
            unsafe { ffi::av_frame_free(&mut frame) };
            return Err("av_frame_ref failed for zero-copy lease".into());
        }
        Ok(Self(frame))
    }
}

impl Drop for RawFrameLease {
    fn drop(&mut self) {
        unsafe { ffi::av_frame_free(&mut self.0) };
    }
}

struct HandoffCompletion {
    result: Mutex<Option<Result<(), String>>>,
    ready: Condvar,
}

impl HandoffCompletion {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            result: Mutex::new(None),
            ready: Condvar::new(),
        })
    }

    fn finish(&self, result: Result<(), String>) {
        let mut current = self
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if current.is_none() {
            *current = Some(result);
            self.ready.notify_all();
        }
    }
}

struct D3d12FrameData {
    pool: Arc<PoolInterop>,
    planes: Arc<ImportedPlanes>,
    decode_fence: DecodeFence,
    full_range: bool,
    bt709: bool,
    completion: Arc<HandoffCompletion>,
    canary_readback: Mutex<Option<VideoFrame>>,
    _frame_lease: RawFrameLease,
    _permit: LeasePermit,
}

/// One decoded D3D12VA frame. Holding it parks the pool slot: FFmpeg cannot
/// recycle the underlying resource until this (and its AVFrame lease) drops,
/// which the consume thread only allows after the conversion submission is
/// observed complete.
#[derive(Clone)]
pub struct D3d12Frame(Arc<D3d12FrameData>);

impl std::fmt::Debug for D3d12Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("D3d12Frame")
            .field("fence_value", &self.0.decode_fence.value)
            .field("full_range", &self.0.full_range)
            .field("bt709", &self.0.bt709)
            .finish_non_exhaustive()
    }
}

impl D3d12Frame {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pool: Arc<PoolInterop>,
        planes: Arc<ImportedPlanes>,
        decode_fence: DecodeFence,
        full_range: bool,
        bt709: bool,
        frame_lease: RawFrameLease,
        permit: LeasePermit,
        canary_readback: Option<VideoFrame>,
    ) -> Self {
        Self(Arc::new(D3d12FrameData {
            pool,
            planes,
            decode_fence,
            full_range,
            bt709,
            completion: HandoffCompletion::new(),
            canary_readback: Mutex::new(canary_readback),
            _frame_lease: frame_lease,
            _permit: permit,
        }))
    }

    pub fn full_range(&self) -> bool {
        self.0.full_range
    }

    pub fn bt709(&self) -> bool {
        self.0.bt709
    }

    pub fn allocated_size(&self) -> (u32, u32) {
        self.0.pool.allocated_size()
    }

    pub fn plane_views(&self) -> (&wgpu::TextureView, &wgpu::TextureView) {
        (&self.0.planes.y_view, &self.0.planes.uv_view)
    }

    /// The decode fence to pair with the conversion submission via
    /// [`crate::SharedQueue::submit_with_decode_wait`].
    pub fn decode_fence(&self) -> &DecodeFence {
        &self.0.decode_fence
    }

    pub fn take_canary_readback(&self) -> Option<VideoFrame> {
        self.0
            .canary_readback
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    pub fn complete(&self, result: Result<(), String>) {
        self.0.completion.finish(result);
    }

    pub fn handoff(&self) -> D3d12Handoff {
        D3d12Handoff {
            completion: Arc::clone(&self.0.completion),
        }
    }
}

/// Decode-thread view of one frame's consumption outcome: `wait` blocks until
/// the consume thread has submitted (or declined) the frame's conversion, so
/// engage/decline decisions land on the decode thread exactly as they did on
/// the D3D11 path.
#[derive(Clone)]
pub struct D3d12Handoff {
    completion: Arc<HandoffCompletion>,
}

impl D3d12Handoff {
    pub fn wait(&self, stop: &AtomicBool) -> Result<(), String> {
        let mut result = self
            .completion
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(result) = result.take() {
                return result;
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            result = self
                .completion
                .ready
                .wait_timeout(result, std::time::Duration::from_millis(10))
                .unwrap_or_else(|error| error.into_inner())
                .0;
        }
    }
}
