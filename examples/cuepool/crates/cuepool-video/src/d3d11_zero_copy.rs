use crate::VideoFrame;
use crate::frame_lease::{LeaseBudget, LeasePermit, expanded_pool_size};
use ash::vk;
use ffmpeg_next::ffi;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_DECODER, D3D11_REQ_TEXTURE2D_ARRAY_AXIS_DIMENSION,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE,
    D3D11_TEXTURE2D_DESC, ID3D11Device, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12;
use windows::Win32::Graphics::Dxgi::{
    DXGI_SHARED_RESOURCE_READ, DXGI_SHARED_RESOURCE_WRITE, IDXGIDevice, IDXGIKeyedMutex,
    IDXGIResource1,
};
use windows::core::{Interface, PCWSTR};

const D3D11_KEY: u64 = 0;
const VULKAN_KEY: u64 = 1;
const KEYED_MUTEX_TIMEOUT_MS: u32 = 1_000;

pub(crate) struct InteropDevice {
    device: wgpu::Device,
    adapter_luid: [u8; 8],
}

impl InteropDevice {
    pub(crate) fn new(
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
    ) -> Result<Self, String> {
        let hal_adapter = unsafe { adapter.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or_else(|| "Vulkan HAL adapter is unavailable".to_string())?;
        let mut id = vk::PhysicalDeviceIDProperties::default();
        let mut properties = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
        unsafe {
            hal_adapter
                .shared_instance()
                .raw_instance()
                .get_physical_device_properties2(
                    hal_adapter.raw_physical_device(),
                    &mut properties,
                );
        }
        if id.device_luid_valid != vk::TRUE {
            return Err("Vulkan adapter did not report a valid Windows LUID".into());
        }
        Ok(Self {
            device: device.clone(),
            adapter_luid: id.device_luid,
        })
    }

    fn import_pool(&self, frames: Arc<FramesContextRef>) -> Result<Arc<PoolInterop>, String> {
        unsafe { PoolInterop::import(self, frames) }.map(Arc::new)
    }
}

#[repr(C)]
struct AvD3d11VaDeviceContext {
    device: *mut c_void,
    device_context: *mut c_void,
    video_device: *mut c_void,
    video_context: *mut c_void,
    lock: Option<unsafe extern "C" fn(*mut c_void)>,
    unlock: Option<unsafe extern "C" fn(*mut c_void)>,
    lock_ctx: *mut c_void,
}

#[repr(C)]
struct AvD3d11VaFramesContext {
    texture: *mut c_void,
    bind_flags: u32,
    misc_flags: u32,
    texture_infos: *mut c_void,
}

const _: () = {
    if cfg!(target_pointer_width = "64") {
        assert!(std::mem::size_of::<AvD3d11VaDeviceContext>() == 56);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, device) == 0);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, device_context) == 8);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, video_device) == 16);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, video_context) == 24);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, lock) == 32);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, unlock) == 40);
        assert!(std::mem::offset_of!(AvD3d11VaDeviceContext, lock_ctx) == 48);

        assert!(std::mem::size_of::<AvD3d11VaFramesContext>() == 24);
        assert!(std::mem::offset_of!(AvD3d11VaFramesContext, texture) == 0);
        assert!(std::mem::offset_of!(AvD3d11VaFramesContext, bind_flags) == 8);
        assert!(std::mem::offset_of!(AvD3d11VaFramesContext, misc_flags) == 12);
        assert!(std::mem::offset_of!(AvD3d11VaFramesContext, texture_infos) == 16);
    }
};

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
            .ok_or_else(|| "shareable D3D11VA pool is not initialized".to_string())?;
        let layer = unsafe { (*frame).data[1] as usize };
        let layer = u32::try_from(layer)
            .map_err(|_| "D3D11VA array-slice index does not fit in u32".to_string())?;
        if unsafe { (*frame).data[0].cast::<c_void>() } != pool.canonical_texture_ptr() {
            return Err("decoded D3D11 texture does not match the configured pool".into());
        }
        if layer >= pool.array_size() {
            return Err(format!(
                "decoded D3D11 array slice {layer} exceeds pool size {}",
                pool.array_size()
            ));
        }
        let permit = self
            .budget
            .try_acquire()
            .ok_or_else(|| "zero-copy frame lease budget exhausted".to_string())?;
        let frame_lease = unsafe { RawFrameLease::new(frame) }?;
        Ok(VideoFrame::d3d11_nv12(
            width,
            height,
            pts,
            D3d11Frame::new(
                pool,
                layer,
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

pub(crate) unsafe fn configure_pool(
    codec: *mut ffi::AVCodecContext,
    request: &DirectPoolRequest,
) -> Result<ffi::AVPixelFormat, String> {
    if crate::zero_copy::direct_path_poisoned() {
        request.fail(crate::zero_copy::DIRECT_PATH_POISONED_REASON);
        return Err(crate::zero_copy::DIRECT_PATH_POISONED_REASON.into());
    }
    let mut frames_ref = std::ptr::null_mut();
    let result = unsafe {
        ffi::avcodec_get_hw_frames_parameters(
            codec,
            (*codec).hw_device_ctx,
            ffi::AVPixelFormat::AV_PIX_FMT_D3D11,
            &mut frames_ref,
        )
    };
    if result < 0 || frames_ref.is_null() {
        request.fail("avcodec_get_hw_frames_parameters rejected the shareable pool");
        return Err("avcodec_get_hw_frames_parameters failed".into());
    }

    let configured = unsafe { configure_pool_ref(frames_ref) };
    if let Err(reason) = configured {
        request.fail(reason.clone());
        unsafe { ffi::av_buffer_unref(&mut frames_ref) };
        return Err(reason);
    }

    if unsafe { ffi::av_hwframe_ctx_init(frames_ref) } < 0 {
        let reason = "av_hwframe_ctx_init rejected the shareable D3D11VA pool".to_string();
        request.fail(reason.clone());
        unsafe { ffi::av_buffer_unref(&mut frames_ref) };
        return Err(reason);
    }

    let retained = match unsafe { FramesContextRef::from_borrowed(frames_ref) } {
        Some(retained) => retained,
        None => {
            let reason = "av_buffer_ref failed for the shareable pool".to_string();
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
    Ok(ffi::AVPixelFormat::AV_PIX_FMT_D3D11)
}

unsafe fn configure_pool_ref(frames_ref: *mut ffi::AVBufferRef) -> Result<(), String> {
    let frames = unsafe { &mut *((*frames_ref).data.cast::<ffi::AVHWFramesContext>()) };
    if frames.format != ffi::AVPixelFormat::AV_PIX_FMT_D3D11 {
        return Err("shareable pool format is not AV_PIX_FMT_D3D11".into());
    }
    if frames.sw_format != ffi::AVPixelFormat::AV_PIX_FMT_NV12 {
        return Err("shareable pool software format is not NV12".into());
    }
    if frames.width <= 0 || frames.height <= 0 || frames.width % 2 != 0 || frames.height % 2 != 0 {
        return Err("shareable pool has invalid allocated dimensions".into());
    }
    frames.initial_pool_size = expanded_pool_size(
        frames.initial_pool_size,
        D3D11_REQ_TEXTURE2D_ARRAY_AXIS_DIMENSION,
    )
    .ok_or_else(|| "shareable pool size is invalid or exceeds the D3D11 array limit".to_string())?;

    let d3d = unsafe { &mut *frames.hwctx.cast::<AvD3d11VaFramesContext>() };
    if !d3d.texture.is_null() {
        return Err("FFmpeg unexpectedly supplied a user-allocated D3D11 texture".into());
    }
    d3d.bind_flags |= D3D11_BIND_DECODER.0 as u32;
    d3d.misc_flags |=
        (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
    Ok(())
}

struct SharedHandle(HANDLE);

impl Drop for SharedHandle {
    fn drop(&mut self) {
        if let Err(error) = unsafe { CloseHandle(self.0) } {
            log::warn!("Video zero-copy: CloseHandle failed: {error}");
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Ownership {
    Decoder,
    Vulkan,
}

pub(crate) struct PoolInterop {
    device: wgpu::Device,
    frames: Arc<FramesContextRef>,
    canonical_texture: ID3D11Texture2D,
    keyed_mutex: IDXGIKeyedMutex,
    texture_desc: D3D11_TEXTURE2D_DESC,
    _texture: wgpu::Texture,
    plane_views: Vec<(wgpu::TextureView, wgpu::TextureView)>,
    image: vk::Image,
    memory: vk::DeviceMemory,
    queue_family_index: u32,
    ownership: Mutex<Ownership>,
}

impl std::fmt::Debug for PoolInterop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PoolInterop")
            .field("size", &(self.texture_desc.Width, self.texture_desc.Height))
            .field("array_size", &self.texture_desc.ArraySize)
            .finish_non_exhaustive()
    }
}

impl PoolInterop {
    unsafe fn import(
        interop: &InteropDevice,
        frames: Arc<FramesContextRef>,
    ) -> Result<Self, String> {
        let frames_context = unsafe { frames.frames() };
        let d3d_frames = unsafe {
            frames_context
                .hwctx
                .cast::<AvD3d11VaFramesContext>()
                .as_ref()
        }
        .ok_or_else(|| "FFmpeg D3D11VA frames context is null".to_string())?;
        let canonical_texture = unsafe {
            ID3D11Texture2D::from_raw_borrowed(&d3d_frames.texture)
                .ok_or_else(|| "FFmpeg D3D11VA canonical texture is null".to_string())?
                .clone()
        };
        let mut texture_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe { canonical_texture.GetDesc(&mut texture_desc) };
        validate_texture_desc(&texture_desc, frames_context)?;

        let device_context = unsafe {
            frames_context
                .device_ctx
                .as_ref()
                .and_then(|context| context.hwctx.cast::<AvD3d11VaDeviceContext>().as_ref())
        }
        .ok_or_else(|| "FFmpeg D3D11VA device context is null".to_string())?;
        let d3d_device = unsafe {
            ID3D11Device::from_raw_borrowed(&device_context.device)
                .ok_or_else(|| "FFmpeg D3D11 device is null".to_string())?
        };
        let dxgi_device: IDXGIDevice = d3d_device
            .cast()
            .map_err(|error| format!("ID3D11Device to IDXGIDevice failed: {error}"))?;
        let d3d_luid = unsafe {
            dxgi_device
                .GetAdapter()
                .and_then(|adapter| adapter.GetDesc())
        }
        .map_err(|error| format!("DXGI adapter LUID query failed: {error}"))?
        .AdapterLuid;
        let mut d3d_luid_bytes = [0; 8];
        d3d_luid_bytes[..4].copy_from_slice(&d3d_luid.LowPart.to_ne_bytes());
        d3d_luid_bytes[4..].copy_from_slice(&d3d_luid.HighPart.to_ne_bytes());
        if d3d_luid_bytes != interop.adapter_luid {
            return Err(format!(
                "D3D11/Vulkan adapter LUID mismatch ({d3d_luid_bytes:02x?} != {:02x?})",
                interop.adapter_luid
            ));
        }

        let keyed_mutex: IDXGIKeyedMutex = canonical_texture
            .cast()
            .map_err(|error| format!("IDXGIKeyedMutex query failed: {error}"))?;
        with_device_lock(device_context, || unsafe {
            keyed_mutex
                .AcquireSync(D3D11_KEY, KEYED_MUTEX_TIMEOUT_MS)
                .map_err(|error| format!("initial IDXGIKeyedMutex::AcquireSync failed: {error}"))
        })?;

        let resource: IDXGIResource1 = canonical_texture
            .cast()
            .map_err(|error| format!("IDXGIResource1 query failed: {error}"))?;
        let shared_handle = SharedHandle(
            unsafe {
                resource.CreateSharedHandle(
                    None,
                    (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0,
                    PCWSTR::null(),
                )
            }
            .map_err(|error| format!("IDXGIResource1::CreateSharedHandle failed: {error}"))?,
        );

        let hal_desc = hal_texture_descriptor(&texture_desc);
        let (hal_texture, image, memory, queue_family_index) = unsafe {
            let hal_device = interop
                .device
                .as_hal::<wgpu::hal::api::Vulkan>()
                .ok_or_else(|| "Vulkan HAL device is unavailable".to_string())?;
            validate_external_image(&hal_device, &texture_desc)?;
            let hal_texture = hal_device
                .texture_from_d3d11_shared_handle(shared_handle.0, &hal_desc)
                .map_err(|error| format!("Vulkan D3D11 texture import failed: {error:?}"))?;
            let image = hal_texture.raw_handle();
            let memory = match hal_texture.memory() {
                wgpu::hal::vulkan::TextureMemory::Dedicated(memory) => *memory,
                _ => return Err("imported Vulkan texture did not use dedicated memory".into()),
            };
            (hal_texture, image, memory, hal_device.queue_family_index())
        };

        let public_desc = wgpu::TextureDescriptor {
            label: Some("d3d11va-nv12-pool"),
            size: hal_desc.size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::NV12,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let texture = unsafe {
            interop
                .device
                .create_texture_from_hal_with_initial_state::<wgpu::hal::api::Vulkan>(
                    hal_texture,
                    &public_desc,
                    wgpu::TextureUses::RESOURCE,
                )
        };
        let error_scope = interop
            .device
            .push_error_scope(wgpu::ErrorFilter::Validation);
        let plane_views = (0..texture_desc.ArraySize)
            .map(|layer| {
                (
                    plane_view(
                        &texture,
                        layer,
                        wgpu::TextureFormat::R8Unorm,
                        wgpu::TextureAspect::Plane0,
                    ),
                    plane_view(
                        &texture,
                        layer,
                        wgpu::TextureFormat::Rg8Unorm,
                        wgpu::TextureAspect::Plane1,
                    ),
                )
            })
            .collect();
        if let Some(error) = pollster::block_on(error_scope.pop()) {
            return Err(format!("imported NV12 plane view creation failed: {error}"));
        }

        Ok(Self {
            device: interop.device.clone(),
            frames,
            canonical_texture,
            keyed_mutex,
            texture_desc,
            _texture: texture,
            plane_views,
            image,
            memory,
            queue_family_index,
            ownership: Mutex::new(Ownership::Decoder),
        })
    }

    pub(crate) fn array_size(&self) -> u32 {
        self.texture_desc.ArraySize
    }

    pub(crate) fn allocated_size(&self) -> (u32, u32) {
        (self.texture_desc.Width, self.texture_desc.Height)
    }

    pub(crate) fn canonical_texture_ptr(&self) -> *mut c_void {
        self.canonical_texture.as_raw()
    }

    pub(crate) fn plane_views(
        &self,
        layer: u32,
    ) -> Option<(&wgpu::TextureView, &wgpu::TextureView)> {
        self.plane_views.get(layer as usize).map(|(y, uv)| (y, uv))
    }

    pub(crate) fn image(&self) -> vk::Image {
        self.image
    }

    pub(crate) fn memory(&self) -> vk::DeviceMemory {
        self.memory
    }

    pub(crate) fn queue_family_index(&self) -> u32 {
        self.queue_family_index
    }

    fn record_barrier(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        acquire: bool,
    ) -> Result<(), String> {
        let hal_device = unsafe { self.device.as_hal::<wgpu::hal::api::Vulkan>() }
            .ok_or_else(|| "Vulkan HAL device is unavailable".to_string())?;
        let queue_family = self.queue_family_index();
        let image = self.image();
        let barrier = if acquire {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .dst_queue_family_index(queue_family)
                .image(image)
                .subresource_range(nv12_subresource_range())
        } else {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_queue_family_index(queue_family)
                .dst_queue_family_index(vk::QUEUE_FAMILY_EXTERNAL)
                .image(image)
                .subresource_range(nv12_subresource_range())
        };
        unsafe {
            encoder.as_hal_mut::<wgpu::hal::api::Vulkan, _, _>(|hal_encoder| {
                let hal_encoder = hal_encoder
                    .ok_or_else(|| "Vulkan command encoder is unavailable".to_string())?;
                hal_device.raw_device().cmd_pipeline_barrier(
                    hal_encoder.raw_handle(),
                    if acquire {
                        vk::PipelineStageFlags::ALL_COMMANDS
                    } else {
                        vk::PipelineStageFlags::FRAGMENT_SHADER
                    },
                    if acquire {
                        vk::PipelineStageFlags::FRAGMENT_SHADER
                    } else {
                        vk::PipelineStageFlags::ALL_COMMANDS
                    },
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                );
                Ok(())
            })
        }
    }

    pub(crate) fn release_to_vulkan(&self) -> Result<(), String> {
        let mut ownership = self
            .ownership
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *ownership != Ownership::Decoder {
            return Err("D3D11VA pool is not owned by the decoder".into());
        }
        let context = self.device_context()?;
        with_device_lock(context, || unsafe {
            self.keyed_mutex
                .ReleaseSync(VULKAN_KEY)
                .map_err(|error| format!("IDXGIKeyedMutex::ReleaseSync failed: {error}"))
        })?;
        *ownership = Ownership::Vulkan;
        Ok(())
    }

    pub(crate) fn acquire_for_decoder(&self) -> Result<(), String> {
        let mut ownership = self
            .ownership
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if *ownership != Ownership::Vulkan {
            return Err("D3D11VA pool is not owned by Vulkan".into());
        }
        let context = self.device_context()?;
        with_device_lock(context, || unsafe {
            self.keyed_mutex
                .AcquireSync(D3D11_KEY, KEYED_MUTEX_TIMEOUT_MS)
                .map_err(|error| format!("IDXGIKeyedMutex::AcquireSync failed: {error}"))
        })?;
        *ownership = Ownership::Decoder;
        Ok(())
    }

    fn decoder_owned(&self) -> bool {
        *self
            .ownership
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            == Ownership::Decoder
    }

    fn device_context(&self) -> Result<&AvD3d11VaDeviceContext, String> {
        let frames = unsafe { self.frames.frames() };
        unsafe {
            frames
                .device_ctx
                .as_ref()
                .and_then(|context| context.hwctx.cast::<AvD3d11VaDeviceContext>().as_ref())
        }
        .ok_or_else(|| "FFmpeg D3D11VA device context disappeared".into())
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

struct D3d11FrameData {
    pool: Arc<PoolInterop>,
    layer: u32,
    full_range: bool,
    bt709: bool,
    completion: Arc<HandoffCompletion>,
    canary_readback: Mutex<Option<VideoFrame>>,
    _frame_lease: RawFrameLease,
    _permit: LeasePermit,
}

#[derive(Clone)]
pub struct D3d11Frame(Arc<D3d11FrameData>);

impl std::fmt::Debug for D3d11Frame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("D3d11Frame")
            .field("layer", &self.0.layer)
            .field("full_range", &self.0.full_range)
            .field("bt709", &self.0.bt709)
            .finish_non_exhaustive()
    }
}

impl D3d11Frame {
    #[allow(clippy::too_many_arguments)]
    fn new(
        pool: Arc<PoolInterop>,
        layer: u32,
        full_range: bool,
        bt709: bool,
        frame_lease: RawFrameLease,
        permit: LeasePermit,
        canary_readback: Option<VideoFrame>,
    ) -> Self {
        Self(Arc::new(D3d11FrameData {
            pool,
            layer,
            full_range,
            bt709,
            completion: HandoffCompletion::new(),
            canary_readback: Mutex::new(canary_readback),
            _frame_lease: frame_lease,
            _permit: permit,
        }))
    }

    pub fn layer(&self) -> u32 {
        self.0.layer
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

    pub fn take_canary_readback(&self) -> Option<VideoFrame> {
        self.0
            .canary_readback
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }

    pub(crate) fn pool(&self) -> &Arc<PoolInterop> {
        &self.0.pool
    }

    pub fn release_to_vulkan(&self) -> Result<(), String> {
        self.0.pool.release_to_vulkan()
    }

    pub(crate) fn acquire_for_decoder(&self) -> Result<(), String> {
        self.0.pool.acquire_for_decoder()
    }

    /// Records the external-to-Vulkan ownership and layout transition.
    ///
    /// # Safety
    ///
    /// The caller must retain this frame through submission completion, submit the resulting
    /// command buffer exactly once after releasing the keyed mutex to Vulkan, and include a
    /// matching release transition after all image accesses in the same queue submission.
    pub unsafe fn record_vulkan_acquire(
        &self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), String> {
        self.0.pool.record_barrier(encoder, true)
    }

    /// Records the Vulkan-to-external ownership and layout transition.
    ///
    /// # Safety
    ///
    /// The caller must submit this after all accesses to the imported image, attach this frame's
    /// keyed-mutex operations to a command buffer in the same submission, and retain the frame
    /// through submission completion.
    pub unsafe fn record_vulkan_release(
        &self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), String> {
        self.0.pool.record_barrier(encoder, false)
    }

    /// Attaches this pool's keyed-mutex acquire and release to the encoder's submission.
    ///
    /// # Safety
    ///
    /// The encoder's command buffer must be submitted exactly once, in the same submission as this
    /// frame's matching ownership transitions, after [`Self::release_to_vulkan`]. The frame must
    /// remain alive until that submission completes.
    pub unsafe fn attach_keyed_mutex(
        &self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Result<(), String> {
        let memory = self.0.pool.memory();
        unsafe {
            encoder.as_hal_submission_mut::<wgpu::hal::api::Vulkan, _, _>(|hal_encoder| {
                let hal_encoder = hal_encoder
                    .ok_or_else(|| "Vulkan submission encoder is unavailable".to_string())?;
                hal_encoder.add_win32_keyed_mutex_acquire_release(
                    &[wgpu::hal::vulkan::Win32KeyedMutexAcquire {
                        memory,
                        key: VULKAN_KEY,
                        timeout_ms: KEYED_MUTEX_TIMEOUT_MS,
                    }],
                    &[wgpu::hal::vulkan::Win32KeyedMutexRelease {
                        memory,
                        key: D3D11_KEY,
                    }],
                );
                Ok(())
            })
        }
    }

    pub fn complete(&self, result: Result<(), String>) {
        self.0.completion.finish(result);
    }

    pub fn handoff(&self) -> D3d11Handoff {
        D3d11Handoff {
            pool: Arc::clone(&self.0.pool),
            completion: Arc::clone(&self.0.completion),
        }
    }
}

#[derive(Clone)]
pub struct D3d11Handoff {
    pool: Arc<PoolInterop>,
    completion: Arc<HandoffCompletion>,
}

impl D3d11Handoff {
    pub fn wait(&self, stop: &AtomicBool) -> Result<(), String> {
        let mut result = self
            .completion
            .result
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(result) = result.take() {
                result?;
                return self.pool.acquire_for_decoder();
            }
            if stop.load(Ordering::Relaxed) && self.pool.decoder_owned() {
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

fn validate_texture_desc(
    desc: &D3D11_TEXTURE2D_DESC,
    frames: &ffi::AVHWFramesContext,
) -> Result<(), String> {
    let required_misc =
        (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0 | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32;
    if desc.Format != DXGI_FORMAT_NV12
        || desc.Width != frames.width as u32
        || desc.Height != frames.height as u32
        || desc.MipLevels != 1
        || desc.SampleDesc.Count != 1
        || desc.SampleDesc.Quality != 0
        || desc.ArraySize != frames.initial_pool_size as u32
        || desc.BindFlags & D3D11_BIND_DECODER.0 as u32 == 0
        || desc.MiscFlags & required_misc != required_misc
    {
        return Err(format!(
            "shareable D3D11 texture descriptor is invalid: {desc:?}"
        ));
    }
    Ok(())
}

fn nv12_subresource_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::PLANE_0 | vk::ImageAspectFlags::PLANE_1,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: vk::REMAINING_ARRAY_LAYERS,
    }
}

fn hal_texture_descriptor(desc: &D3D11_TEXTURE2D_DESC) -> wgpu::hal::TextureDescriptor<'static> {
    wgpu::hal::TextureDescriptor {
        label: Some("d3d11va-nv12-pool"),
        size: wgpu::Extent3d {
            width: desc.Width,
            height: desc.Height,
            depth_or_array_layers: desc.ArraySize,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::NV12,
        usage: wgpu::TextureUses::RESOURCE,
        memory_flags: wgpu::hal::MemoryFlags::empty(),
        view_formats: Vec::new(),
    }
}

unsafe fn validate_external_image(
    device: &wgpu::hal::vulkan::Device,
    desc: &D3D11_TEXTURE2D_DESC,
) -> Result<(), String> {
    let mut external_info = vk::PhysicalDeviceExternalImageFormatInfo::default()
        .handle_type(vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE);
    let image_info = vk::PhysicalDeviceImageFormatInfo2::default()
        .format(vk::Format::G8_B8R8_2PLANE_420_UNORM)
        .ty(vk::ImageType::TYPE_2D)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(vk::ImageUsageFlags::SAMPLED)
        .flags(vk::ImageCreateFlags::MUTABLE_FORMAT | vk::ImageCreateFlags::EXTENDED_USAGE)
        .push_next(&mut external_info);
    let mut external_properties = vk::ExternalImageFormatProperties::default();
    let limits = {
        let mut properties =
            vk::ImageFormatProperties2::default().push_next(&mut external_properties);
        unsafe {
            device
                .shared_instance()
                .raw_instance()
                .get_physical_device_image_format_properties2(
                    device.raw_physical_device(),
                    &image_info,
                    &mut properties,
                )
        }
        .map_err(|error| format!("Vulkan external image capability query failed: {error:?}"))?;
        properties.image_format_properties
    };
    let features = external_properties
        .external_memory_properties
        .external_memory_features;
    if !features.contains(vk::ExternalMemoryFeatureFlags::IMPORTABLE) {
        return Err("Vulkan reports the D3D11 NV12 image as non-importable".into());
    }
    if desc.Width > limits.max_extent.width
        || desc.Height > limits.max_extent.height
        || desc.ArraySize > limits.max_array_layers
        || !limits.sample_counts.contains(vk::SampleCountFlags::TYPE_1)
    {
        return Err("D3D11 texture exceeds Vulkan external image limits".into());
    }
    Ok(())
}

fn plane_view(
    texture: &wgpu::Texture,
    layer: u32,
    format: wgpu::TextureFormat,
    aspect: wgpu::TextureAspect,
) -> wgpu::TextureView {
    texture.create_view(&wgpu::TextureViewDescriptor {
        label: Some("d3d11va-nv12-plane"),
        format: Some(format),
        dimension: Some(wgpu::TextureViewDimension::D2),
        usage: Some(wgpu::TextureUsages::TEXTURE_BINDING),
        aspect,
        base_mip_level: 0,
        mip_level_count: Some(1),
        base_array_layer: layer,
        array_layer_count: Some(1),
    })
}

fn with_device_lock<T>(
    context: &AvD3d11VaDeviceContext,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    struct Unlock<'a>(&'a AvD3d11VaDeviceContext);
    impl Drop for Unlock<'_> {
        fn drop(&mut self) {
            if let Some(unlock) = self.0.unlock {
                unsafe { unlock(self.0.lock_ctx) };
            }
        }
    }

    match (context.lock, context.unlock) {
        (Some(lock), Some(_)) => unsafe { lock(context.lock_ctx) },
        (None, None) => {}
        _ => return Err("FFmpeg D3D11 device lock callbacks are incomplete".into()),
    }
    let _unlock = Unlock(context);
    operation()
}
