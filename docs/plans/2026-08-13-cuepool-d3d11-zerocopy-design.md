# CuePool D3D11 to wgpu zero-copy video design

Issue: [#110](https://github.com/BlueJayLouche/rustjay-engine/issues/110)

## Decision

Importing CuePool's D3D11VA NV12 pool into Vulkan is feasible at the memory and texture-view layers. The pinned wgpu 29.0.4 already has a Windows D3D11 shared-handle import helper, an unsafe route from a `wgpu-hal` texture to a `wgpu::Texture`, and Vulkan NV12 plane views. It does not have a complete external-access protocol. In particular, it cannot attach a keyed-mutex operation or an imported D3D11-fence wait to the next queue submit, and `create_texture_from_hal` registers every imported texture as `UNINITIALIZED`. A direct first sample can therefore discard the decoder's contents. Repeated D3D11 to Vulkan ownership transfers are also outside wgpu's tracker.

The direct decoder-pool path must remain disabled until those gaps are closed and one synchronization protocol passes an attended NVIDIA/Vulkan test. The implementation should still be staged because the pool setup, memory import, frame leases, diagnostics, and fallback are useful and independently testable. Every stage keeps the current readback path. If direct pool sharing proves unreliable, a D3D11 GPU copy into a small ring of shareable NV12 textures is the practical workaround. That is not zero-copy, but it removes the GPU to CPU to GPU round trip.

## Verification boundary

The dependency versions are those in `examples/cuepool/Cargo.lock`: `wgpu`, `wgpu-core`, `wgpu-hal`, and `wgpu-types` 29.0.4, plus `ffmpeg-next` and `ffmpeg-sys-next` 8.1.0. `ffmpeg-sys-next` generates bindings against the target's installed FFmpeg headers. The local sysroot selected by `~/sandbox/ap/ashof/cuepool-linux-check.env` contains FFmpeg 6.1.1-3ubuntu5 headers (`libavutil` 58.29.100 and `libavcodec` 60.31.102). This note distinguishes the Rust crate version from that C ABI version.

Claims labelled **locally verified** were checked in the repository, those registry sources, or the local FFmpeg headers. D3D11 and Vulkan platform claims are labelled **spec-level, unverified locally** because this Linux worktree cannot compile or run the Windows path. The FFmpeg 6.1.1 implementation link is a source cross-check; the struct and function declarations themselves were verified in the local sysroot.

## Current path and target boundary

`VideoSource::open_with` creates an `AV_HWDEVICE_TYPE_D3D11VA` device, stores it in `AVCodecContext.hw_device_ctx`, and installs `hw_get_format`. The callback only chooses `AV_PIX_FMT_D3D11`, so FFmpeg constructs `hw_frames_ctx` and its texture pool implicitly. `VideoSource::handle_decoded` then calls `av_hwframe_transfer_data`; `plane` calls `to_vec()` for each plane. A bounded channel with `VIDEO_QUEUE_CAP == 3` carries owned CPU frames to `video_consume_thread`. `YuvConverter::upload` writes R8 and RG8 textures, then `YuvConverter::encode` samples those textures into the RGBA canvas. See `examples/cuepool/crates/cuepool-video/src/video_source.rs::{hw_get_format,VideoSource::open_with,VideoSource::handle_decoded,plane}`, `frame.rs::{VideoFrame,FramePixels}`, `yuv_converter.rs::YuvConverter::upload`, and `examples/cuepool/crates/cuepool/src/main.rs::{VIDEO_QUEUE_CAP,video_consume_thread}`. **Locally verified.**

The target replaces only the D3D11VA plus NV12 payload:

1. FFmpeg decodes into one slice of its NV12 array texture.
2. The decode message carries the pool identity, array-slice index, metadata, and a reference-counted `AVFrame` lease. It carries no plane bytes.
3. The consume thread selects the due frame, acquires that slice for Vulkan, binds its plane 0 and plane 1 views to the existing NV12 conversion shader, and releases external ownership after the conversion pass.
4. The three output render threads continue sampling the separate RGBA canvas. They never access the decoder pool.

This boundary matters for lifetime. A decoder surface only has to survive through the consume thread's YUV conversion submission, not through every projector presentation.

## FFmpeg pool ownership

### Initialize the pool in `get_format`

The current `hw_get_format` stores only the requested pixel-format integer in `AVCodecContext.opaque`. The new callback state needs the requested format, an interop policy/result sink, and the lease budget. On every `get_format` call that offers `AV_PIX_FMT_D3D11`, it should:

1. Call `avcodec_get_hw_frames_parameters(avctx, avctx->hw_device_ctx, AV_PIX_FMT_D3D11, &frames_ref)` from inside the callback.
2. Inspect the returned, uninitialized `AVHWFramesContext`. Require `format == AV_PIX_FMT_D3D11`, `sw_format == AV_PIX_FMT_NV12`, positive even allocated dimensions, and a positive fixed `initial_pool_size`.
3. Cast `AVHWFramesContext.hwctx` to `AVD3D11VAFramesContext` and preserve any values FFmpeg supplied. Set `BindFlags |= D3D11_BIND_DECODER`. Set `MiscFlags |= D3D11_RESOURCE_MISC_SHARED_NTHANDLE | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX` for the documented NT-handle recipe. The D3D11 flag combination is spec-level and unverified locally; its sources are cited under D3D11 export below.
4. Increase `initial_pool_size` by the number of additional application-held surfaces allowed by the explicit lease budget. The helper's documented size includes only one surface for the caller. CuePool can otherwise hold three channel entries, a peek/current frame, and submitted GPU work. The implementation must define one `MAX_ZERO_COPY_LEASES`, bound the channel plus retirement queue to it, and add `MAX_ZERO_COPY_LEASES - 1` to the helper's value with overflow and driver-limit checks. There must not be an unbounded completion queue.
5. Call `av_hwframe_ctx_init(frames_ref)`. On success, assign the reference to `avctx->hw_frames_ctx` before returning `AV_PIX_FMT_D3D11`. On every error, unref it and reject the direct mode.

`avcodec_get_hw_frames_parameters` is specifically documented for this callback. It supplies codec-specific format, aligned dimensions, and reference-surface count, returns an uninitialized context, and requires the caller to initialize and assign it before returning. `AVCodecContext.hw_frames_ctx` is then owned by libavcodec. See local `libavcodec/avcodec.h::{AVCodecContext.hw_frames_ctx,avcodec_get_hw_frames_parameters}` and `libavutil/hwcontext.h::{AVHWFramesContext,av_hwframe_ctx_init}`. **Locally verified.** Calling `av_hwframe_ctx_alloc` and guessing width, alignment, or codec reference count would throw away information this helper already provides.

The pool remains FFmpeg-allocated. `AVD3D11VAFramesContext.texture` stays null before initialization. With `initial_pool_size > 0`, FFmpeg creates its canonical array texture using the supplied `BindFlags` and `MiscFlags`. Local `libavutil/hwcontext_d3d11va.h::AVD3D11VAFramesContext` documents `BindFlags`, `MiscFlags`, and the array-texture behavior. FFmpeg 6.1.1's [`d3d11va_frames_init`](https://github.com/FFmpeg/FFmpeg/blob/n6.1.1/libavutil/hwcontext_d3d11va.c#L256-L322) copies those fields into `D3D11_TEXTURE2D_DESC` and uses `initial_pool_size` as `ArraySize`. **Header locally verified; implementation cross-checked against the upstream 6.1.1 tag.**

The Rust binding needs preparatory work. `ffmpeg-sys-next-8.1.0/build.rs` includes `libavutil/hwcontext.h` but not `libavutil/hwcontext_d3d11va.h`, so its generated bindings do not expose `AVD3D11VAFramesContext` in the pinned configuration. **Locally verified at `ffmpeg-sys-next-8.1.0/build.rs` bindgen header list.** The smallest maintainable fix is an upstream or pinned crate patch that includes `hwcontext_d3d11va.h` for Windows targets. A private `#[repr(C)]` mirror is a possible short-term workaround, but Windows CI must assert its size and field offsets against the target header; hand-maintained COM ABI declarations must not silently become the permanent interface.

### Identify a decoded surface

For a D3D11 hardware frame, `AVFrame.data[0]` is normally the `ID3D11Texture2D *`, and `AVFrame.data[1]` is the array index cast through `intptr_t`. Do not dereference `data[1]`. Validate that the texture is the expected pool texture and that the converted index is less than `D3D11_TEXTURE2D_DESC.ArraySize`. See local `libavutil/hwcontext_d3d11va.h::AVD3D11FrameDescriptor`. **Locally verified.**

The frame message must own references, not copy the pixels. `av_frame_ref` creates references to every `AVBufferRef`; `av_frame_unref` releases them, and buffer reference/unreference is thread-safe. See local `libavutil/frame.h::{av_frame_ref,av_frame_unref,av_frame_clone}` and `libavutil/buffer.h`. **Locally verified.** Do not use `ffmpeg_next::frame::Video::clone` for this job: in `ffmpeg-next-8.1.0/src/util/frame/video.rs::Clone`, it allocates a new video frame and calls `av_frame_copy` plus `av_frame_copy_props`. A small raw-frame RAII lease around `av_frame_ref` is the correct no-copy representation.

PTS, coded/display dimensions, crop, color range, and color space still come from the decoded `AVFrame`. The direct payload must carry the same `full_range` and BT.709 decision used by `YuvConverter`; importing the texture does not infer colorimetry.

### Rejection behavior

Pool creation can fail after `get_format` returns, even when `avcodec_get_hw_frames_parameters` succeeded; its header explicitly warns that hardware initialization may fail later. A codec or driver may also reject the shared flags, enlarged array, or NV12 combination. On any such failure, discard the attempted codec context and reopen the same source with the existing implicit D3D11VA pool and readback. If D3D11VA itself fails, keep the existing DXVA2/software candidate order. Do not retry the same rejected direct-pool configuration in a loop.

## NT handle export and Vulkan import

Everything in this section about Windows and Vulkan behavior is **spec-level, unverified locally**. The pinned wgpu implementation described later was locally inspected, but no D3D11 texture was created or imported here.

### D3D11 export

The pool texture descriptor should contain:

- `Format == DXGI_FORMAT_NV12`, `MipLevels == 1`, `SampleDesc.Count == 1`, and `ArraySize == AVHWFramesContext.initial_pool_size`.
- `BindFlags` containing `D3D11_BIND_DECODER`.
- `MiscFlags` containing `D3D11_RESOURCE_MISC_SHARED_NTHANDLE` and `D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX`.

The D3D11 docs define `SHARED_NTHANDLE` as the strict-validation NT-handle form and recommend combining it with `SHARED_KEYEDMUTEX` from Windows 8 onward. They also say a decoder output array needs `D3D11_BIND_DECODER`. See Microsoft's [`D3D11_RESOURCE_MISC_FLAG`](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_resource_misc_flag) and [`D3D11_BIND_FLAG`](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_bind_flag) pages.

Do not add `D3D11_BIND_SHADER_RESOURCE` solely for Vulkan. Vulkan image usage is declared independently, and Microsoft's bind-flag documentation says arrays created with `D3D11_BIND_DECODER` cannot be used to create D3D11 shader-resource views.

Query the canonical texture for `IDXGIResource1`, then call `CreateSharedHandle(NULL, DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE, NULL, &handle)`. An array pool has one canonical texture, so export it once per pool, not once per frame. `CreateSharedHandle` may be called only once for a resource; use `DuplicateHandle` if a second import attempt needs another handle. The caller closes its NT handle after successful Vulkan import, or on every failure path. Vulkan's imported memory retains its own payload reference. See [`IDXGIResource1::CreateSharedHandle`](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nf-dxgi1_2-idxgiresource1-createsharedhandle) and Vulkan's [`VkImportMemoryWin32HandleInfoKHR`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkImportMemoryWin32HandleInfoKHR.html).

The D3D11 device and Vulkan physical device must refer to the same adapter and driver. On multi-GPU systems, compare the DXGI adapter LUID with `VkPhysicalDeviceIDProperties.deviceLUID` when `deviceLUIDValid` is true, and create FFmpeg's D3D11 device on the matching DXGI adapter. FFmpeg 6.1.1's `d3d11va_device_create` accepts a numeric adapter string, but that detail was only cross-checked in upstream source, not in a local Windows build. An actual successful external-image capability query and import remains the final compatibility probe.

### Vulkan image and allocation

Before importing, query `vkGetPhysicalDeviceImageFormatProperties2` with `VkPhysicalDeviceExternalImageFormatInfo.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT`. Require `VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT` for the exact format, tiling, usage, and flags, then check the D3D11 extent and array-layer count against the returned image-format limits. The D3D11 texture handle type is required by the Vulkan spec to report `VK_EXTERNAL_MEMORY_FEATURE_DEDICATED_ONLY_BIT`, so the allocation must chain `VkMemoryDedicatedAllocateInfo { image }`. See [`VkPhysicalDeviceExternalImageFormatInfo`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkPhysicalDeviceExternalImageFormatInfo.html), [`VkExternalMemoryFeatureFlagBits`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkExternalMemoryFeatureFlagBits.html), and [`VkMemoryDedicatedAllocateInfo`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkMemoryDedicatedAllocateInfo.html).

Create the imported image with parameters matching `ID3D11Texture2D::GetDesc` exactly:

- `VkImageType` 2D and `VkFormat` `VK_FORMAT_G8_B8R8_2PLANE_420_UNORM`.
- Extent equal to the allocated, possibly padded D3D11 width and height; one mip; the D3D11 `ArraySize`; one sample.
- Optimal tiling, exclusive sharing, sampled usage, and initial layout `VK_IMAGE_LAYOUT_UNDEFINED` at creation.
- `VK_IMAGE_CREATE_MUTABLE_FORMAT_BIT | VK_IMAGE_CREATE_EXTENDED_USAGE_BIT`, needed for the R8 and RG8 plane views.
- `VkExternalMemoryImageCreateInfo.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT`.

Allocate device-local memory with `VkImportMemoryWin32HandleInfoKHR` for the same handle type and the dedicated-allocation structure, then bind at offset zero. The required device extension is `VK_KHR_external_memory_win32`, with the core 1.1 or KHR external-memory capability machinery. NV12 also needs the multi-planar/sampler-YCbCr capability represented by wgpu's NV12 feature. Synchronization adds either `VK_KHR_win32_keyed_mutex` or `VK_KHR_external_semaphore_win32`. The image is in `VK_IMAGE_LAYOUT_GENERAL` while D3D11 accesses it; Vulkan acquisition and release must use the external queue family and the spec's external image layout rules. See the Vulkan specification's [external image implied layouts](https://registry.khronos.org/vulkan/specs/latest/html/vkspec.html#resources-image-layouts) and [external resource sharing](https://registry.khronos.org/vulkan/specs/latest/html/vkspec.html#resources-external-sharing).

Do not substitute the visible `AVFrame.width` and `height` for the allocated texture extent. Use those visible dimensions and crop only when computing `YuvConverter` texture coordinates.

## wgpu 29.0.4 surface and gaps

### What is already present

`wgpu-29.0.4/src/lib.rs` reexports `wgpu_hal` as `wgpu::hal` in native `wgpu-core` builds. `wgpu::Device::as_hal::<wgpu::hal::api::Vulkan>` exposes the matching hal device guard, and unsafe `Device::create_texture_from_hal` wraps a hal texture as a public `wgpu::Texture`. Its safety contract requires the texture to come from the same internal device, match the public descriptor, and already be initialized. See `wgpu-29.0.4/src/api/device.rs::{Device::as_hal,Device::create_texture_from_hal}`. **Locally verified.**

The Vulkan device has the purpose-built, Windows-only unsafe helper `wgpu::hal::vulkan::Device::texture_from_d3d11_shared_handle`; the more general primitive is `texture_from_raw`. The helper's signature uses `windows::Win32::Foundation::HANDLE`, so future implementation code needs a target-specific `windows` dependency compatible with the pinned hal crate. The D3D11 helper:

- checks `Features::VULKAN_EXTERNAL_MEMORY_WIN32`;
- creates the external image with the D3D11 texture handle type;
- uses `VkImportMemoryWin32HandleInfoKHR` and `VkMemoryDedicatedAllocateInfo`;
- allocates device-local memory, binds it, and returns a hal texture owning the image and dedicated memory.

For multi-planar formats, its internal `create_image_without_memory` sets mutable-format and extended-usage flags. See `wgpu-hal-29.0.4/src/vulkan/device.rs::{texture_from_raw,create_image_without_memory,texture_from_d3d11_shared_handle}` and `wgpu-hal-29.0.4/src/lib.rs::TextureDescriptor`. **Locally verified.** The helper does not perform the external image-format capability query, synchronization, or ownership transfers, so CuePool must treat successful allocation as only part of the probe.

The app must check adapter support and request both `Features::TEXTURE_FORMAT_NV12` and `Features::VULKAN_EXTERNAL_MEMORY_WIN32` when it creates the shared device. Features cannot be enabled later. CuePool currently requests only `TEXTURE_FORMAT_16BIT_NORM` at `main.rs::main`. In `wgpu-types-29.0.4/src/features.rs`, NV12 is a native Vulkan/DX12 feature and external memory is a Vulkan/Windows feature. The Vulkan adapter maps them to NV12 format support plus `VK_KHR_sampler_ycbcr_conversion`, and `VK_KHR_external_memory_win32`, respectively, in `wgpu-hal-29.0.4/src/vulkan/adapter.rs::{PhysicalDeviceProperties::to_wgpu,required_device_extensions}`. **Locally verified.**

Pass the import helper a hal texture descriptor with the allocated size, `depth_or_array_layers = ArraySize`, one mip, one sample, dimension D2, format `TextureFormat::NV12`, usage `TextureUses::RESOURCE`, no extra memory flags, and no view formats. Pass `create_texture_from_hal` the corresponding public descriptor with `TextureUsages::TEXTURE_BINDING`. For frame slice `i`, create two D2 views with one array layer:

- plane 0: `format = R8Unorm`, `aspect = Plane0`, `base_array_layer = i`;
- plane 1: `format = Rg8Unorm`, `aspect = Plane1`, `base_array_layer = i`.

`TextureFormat::aspect_specific_format` maps those exact pairs, while `wgpu-core` validates the one-layer D2 range and passes it to the Vulkan image-view creation. Vulkan maps NV12 to `VK_FORMAT_G8_B8R8_2PLANE_420_UNORM` and the aspects to `VK_IMAGE_ASPECT_PLANE_0_BIT` and `PLANE_1_BIT`. See `wgpu-types-29.0.4/src/texture/format.rs::{TextureFormat::NV12,TextureFormat::aspect_specific_format}`, `src/texture.rs::{TextureAspect,TextureViewDescriptor}`, `wgpu-core-29.0.4/src/device/resource.rs::Device::create_texture_view`, and `wgpu-hal-29.0.4/src/vulkan/conv.rs::{PrivateCapabilities::map_texture_format,map_aspects}`. **Locally verified.** The existing NV12 shader already expects filterable R8 and RG8 D2 views, so only its binding source changes.

### Gaps that block a safe direct path

1. `wgpu-core-29.0.4/src/device/resource.rs::Device::create_texture_from_hal` inserts the texture tracker state as `TextureUses::UNINITIALIZED`. `wgpu-core-29.0.4/src/track/texture.rs` says a transition away from that state treats the contents as junk, and `wgpu-hal-29.0.4/src/vulkan/conv.rs::derive_image_layout` maps it to `VK_IMAGE_LAYOUT_UNDEFINED`. This conflicts with the public method's requirement that the imported texture already be initialized. Sampling a just-decoded frame through this route is not contents-preserving. **Locally verified.**
2. External access repeats for every frame. The real image is `GENERAL` and externally owned during D3D11 access, while wgpu's tracker remembers its last internal sampled state. The normal `wgpu-hal` texture barrier has only from/to `TextureUses`; `wgpu-hal-29.0.4/src/lib.rs::TextureBarrier` cannot name external queue-family ownership. The Vulkan device does expose `queue_family_index` and raw handles, and `wgpu::CommandEncoder::as_hal_mut` plus `wgpu-hal`'s `CommandEncoder::raw_handle` can record the acquire and release barriers, but the core tracker still needs a caller-supplied initial/current state contract. **Locally verified.**
3. No pinned feature or Vulkan code enables `VK_KHR_win32_keyed_mutex` or `VK_KHR_external_semaphore_win32`. The Vulkan queue has `add_signal_semaphore`, but no matching `add_wait_semaphore`, imported Win32 semaphore helper, or keyed-mutex submit hook. Searches for those extension symbols and operations in the four pinned crates returned no implementation. **Locally verified.** Direct `vkQueueSubmit` through `Queue::as_hal` is not an acceptable workaround: CuePool's consume thread and three output threads share one `wgpu::Queue`, and Vulkan queue submission requires external synchronization. A raw submit could race wgpu's own queue bookkeeping.

The smallest wgpu change depends on the synchronization experiment:

- Common change: add an unsafe `create_texture_from_hal` form that accepts a validated initial tracker usage, or a narrow imported-texture API. CuePool can then keep the tracker at sampled `RESOURCE` while raw barriers recorded at the start and end of the same command encoder acquire `GENERAL/external -> SHADER_READ_ONLY/internal` and release it back. The wrapper must ensure no public wgpu use occurs while the actual state is external.
- Keyed-mutex change: enable `VK_KHR_win32_keyed_mutex` and add a next-submit acquire/release record using the imported texture's `VkDeviceMemory`. `wgpu-hal-29.0.4/src/vulkan/mod.rs::{Texture::memory,Queue::submit}` already has the necessary dedicated memory and single submit construction point.
- Fence change: enable `VK_KHR_external_semaphore_win32`, import a shared `ID3D11Fence` as a Vulkan timeline semaphore using `VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D11_FENCE_BIT`, and add `Queue::add_wait_semaphore` mirroring the existing `add_signal_semaphore`. The next wgpu submission must consume that wait with the frame's fence value.

A small, reviewed fork pinned to 29.0.4 can prove these changes, but the preferred endpoint is an upstream API or an upgrade to a wgpu release that contains the equivalent. Raw queue submission in CuePool is larger and less safe than the upstream change.

## Synchronization and lifetime

### Keyed mutex

The documented NT-handle recipe creates a keyed-mutex resource. Microsoft's `D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX` documentation says the creating and opening devices must call `AcquireSync` before issuing rendering commands and `ReleaseSync` afterward. Vulkan represents the same operation by chaining `VkWin32KeyedMutexAcquireReleaseInfoKHR` to the queue submit. See Microsoft's [`D3D11_RESOURCE_MISC_FLAG`](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_resource_misc_flag) and Khronos's [`VkWin32KeyedMutexAcquireReleaseInfoKHR`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkWin32KeyedMutexAcquireReleaseInfoKHR.html). **Spec-level, unverified locally.**

`AcquireSync` and `ReleaseSync` are absent from FFmpeg 6.1.1's [`hwcontext_d3d11va.c`](https://github.com/FFmpeg/FFmpeg/blob/n6.1.1/libavutil/hwcontext_d3d11va.c) and [`dxva2.c`](https://github.com/FFmpeg/FFmpeg/blob/n6.1.1/libavcodec/dxva2.c), and from wgpu 29.0.4; this was checked by source search. **FFmpeg implementation cross-checked against the upstream 6.1.1 tag; wgpu locally verified.** Local `libavutil/hwcontext_d3d11va.h::AVD3D11VADeviceContext` documents recursive `lock` and `unlock` callbacks protecting `device_context` and `video_context`. CuePool could use that lock while bracketing decoder API calls with the keyed mutex, then make the Vulkan conversion submit acquire the released key and return the D3D key. There are two serious costs:

- The keyed mutex belongs to the whole array resource, not an individual slice. D3D11 decode and Vulkan conversion cannot overlap even when they touch different layers.
- FFmpeg may issue decoder work while sending packets, receiving frames, flushing, and reading reference surfaces. A narrow lock around only `avcodec_receive_frame` is not proved sufficient.

The correctness-first keyed prototype must therefore serialize all decoder calls that can touch the pool with the Vulkan conversion, use finite acquire timeouts, and allow at most one cross-API handoff. That may remove too much decode-ahead to meet 50 fps. The rig decides; do not optimize the locking boundary from assumption.

### External fence and semaphore

On systems exposing `ID3D11Device5`, CuePool can call [`CreateFence`](https://learn.microsoft.com/en-us/windows/win32/api/d3d11_4/nf-d3d11_4-id3d11device5-createfence), export the resulting `ID3D11Fence` through [`ID3D11Fence::CreateSharedHandle`](https://learn.microsoft.com/en-us/windows/win32/api/d3d11_3/nf-d3d11_3-id3d11fence-createsharedhandle), and use `ID3D11DeviceContext4::Signal` to place a monotonically increasing value after prior immediate-context work. Vulkan defines `VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_D3D11_FENCE_BIT` as an alias of its D3D12-fence handle and recommends importing it into a timeline semaphore with [`VkImportSemaphoreWin32HandleInfoKHR`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkImportSemaphoreWin32HandleInfoKHR.html). See Microsoft's [`ID3D11DeviceContext4::Signal`](https://learn.microsoft.com/en-us/windows/win32/api/d3d11_3/nf-d3d11_3-id3d11devicecontext4-signal) and Khronos's [`VkExternalSemaphoreHandleTypeFlagBits`](https://registry.khronos.org/vulkan/specs/latest/man/html/VkExternalSemaphoreHandleTypeFlagBits.html). **Spec-level, unverified locally.**

For frame N, signal value N after the decode work, wait for N in the Vulkan conversion submit, release the image to external ownership at the end of that submit, and do not let the decoder issue later pool accesses until the corresponding completion lease retires. This avoids a CPU wait and gives an explicit D3D11-write to Vulkan-read edge. It still needs the wgpu changes above.

The Microsoft `CreateSharedHandle` page describes both `SHARED_NTHANDLE` and `SHARED_KEYEDMUTEX` for the NT resource, so this note does not assume that a fence silently replaces the keyed-mutex obligation. Whether NVIDIA accepts an NT-shareable NV12 decoder array synchronized only by fences is **unverified** and the public documentation found for this design does not establish it. The fence-only variant may be enabled only after a Windows probe demonstrates a documented, valid resource-creation combination, or after vendor/platform confirmation. Otherwise the keyed path or GPU-copy workaround is required.

A CPU wait on the D3D11 fence before submitting Vulkan is useful only as a diagnostic stage. It can prove memory import and image interpretation without adding external semaphores, but it still does not solve the documented keyed-mutex and external-ownership obligations. It must never be presented as the finished design.

### Frame and pool leases

Every zero-copy frame owns an `av_frame_ref` lease. Keeping it prevents FFmpeg from returning that application reference to the pool. The consume thread also keeps a `PoolInterop` reference containing the imported wgpu texture, cached per-slice plane views or bind groups, the D3D11 pool/context reference, and synchronization objects.

After `queue.submit` for the conversion, move both references into a retirement record. Register `Queue::on_submitted_work_done`; its pinned documentation says the callback runs after all previous queue submissions complete, and that a later submit or `Device::poll` drives callbacks. See `wgpu-29.0.4/src/api/queue.rs::Queue::on_submitted_work_done` and `src/api/device.rs::Device::poll`. **Locally verified.** The consume loop should call `device.poll(PollType::Poll)` so pause or an idle output does not indefinitely hold decoder surfaces. The callback only sends a short retirement token; normal cleanup stays on the consume thread.

Holding the output `AVFrame` is necessary but may not be sufficient for decoder reference surfaces. A codec can retain its own reference and read that surface while decoding a later frame. The initial implementation must serialize decoder progress until Vulkan releases the handed-off surface, or prove through a codec/driver-specific integration that later reference reads are synchronized. Treating distinct array layers as automatically independent is unsafe while the resource-wide keyed mutex and external ownership rules apply.

### Epoch changes and teardown

`VideoControl.stream_epoch` already invalidates the receiver, peeked frame, and pending frame work. Zero-copy frames follow the same rule with one exception: an epoch change may drop unsent, channel, and peeked leases immediately, but it must not drop a lease or `PoolInterop` used by an already submitted command buffer. Mark those records retired-by-epoch and release them only from the completion path.

A stream's pool cache is keyed by epoch and a unique pool identity, never only by texture pointer. Windows and Vulkan handles may be reused. Stop, seek/reopen, format change, device loss, and decoder failure all stop admitting new direct frames; drain or time-bound the submitted retirement records; then close handles and destroy the imported image before releasing the last FFmpeg pool reference. A timeout freezes or blanks the canvas and reports the failure. It must not free live GPU resources to make teardown finish quickly.

## Runtime probe and fallback

The direct path is an optimization selected per stream, not a new universal frame format. The decision is made in two parts.

At device creation, mark interop available only when all of these are true:

- Windows build, wgpu backend Vulkan, and the synchronization patch compiled in.
- The adapter reports `TEXTURE_FORMAT_NV12` and `VULKAN_EXTERNAL_MEMORY_WIN32`; request both with the existing required features.
- Required raw Vulkan synchronization extensions and adapter LUID query are available.
- The operator has not set the zero-copy kill switch. Default the feature off until the attended qualification stage passes.

At stream `get_format` and pool initialization, require D3D11VA, `AV_PIX_FMT_D3D11` backed by NV12, successful shareable pool creation, matching adapter LUID, an importable exact external-image query, successful NT handle export/import, valid array-slice metadata, both plane views, and initialized synchronization. Run a first-frame canary before reporting the direct path engaged. Keep the existing `av_hwframe_transfer_data` result for that frame, run the direct and uploaded planes through the same conversion shader into separate RGBA scratch textures, and compare mapped results. This one-frame cost is acceptable at stream open and catches the most dangerous descriptor, slice, and chroma-order mistakes without depending on multi-planar texture-to-buffer copies. A debug option may repeat the check at cue boundaries.

The following cases always use the current path:

- non-Windows and non-Vulkan backends;
- software decode and DXVA2;
- hardware output whose software format is not NV12, including 10-bit formats in this issue's scope;
- a driver or codec that rejects the shareable pool or lease-sized array;
- adapter mismatch, absent extension/feature, external capability rejection, handle/import/view failure, synchronization timeout, or canary mismatch.

Pool-init rejection reopens D3D11VA with the existing implicit pool. Import or synchronization setup failure after a valid shareable frame can use `av_hwframe_transfer_data` on that frame and switch the stream to readback after retiring any submitted interop work. A detected mid-stream failure bumps the stream epoch, stops new direct submissions, keeps or blanks the last known-good canvas, drains live leases, and restarts the cue through readback at the nearest recoverable position. Log one structured reason with OS, codec, dimensions, DXGI/Vulkan adapter identity, driver version, and failed API. Do not repeatedly probe after a stream has fallen back.

Vulkan device loss is different: all CuePool rendering uses that device, so readback cannot recover presentation. Keep the existing global device-loss behavior rather than claiming the video fallback can repair it.

Automatic checks cannot reliably recognize every plausible but wrong frame. A driver can return success and show stale or torn chroma. The first-frame canary, finite synchronization timeouts, validation layers in qualification builds, an operator kill switch, and the attended soak are the available controls. The status UI must say `d3d11va zero-copy`, `d3d11va readback`, or `software`, plus the reason when direct mode was declined or disabled.

## Staged implementation plan

Each stage lands behind the existing path or a default-off feature gate.

1. Add measurements and decision reporting before interop. Record decode, `av_hwframe_transfer_data`, plane-copy, upload, conversion-submit, and starvation timing separately; show the active path and fallback reason. Add the operator kill switch. Host-independent status and decision tests run on Linux; the existing video crate still needs its FFmpeg sysroot for a full check.
2. Add bounded GPU-frame lease and retirement plumbing without using D3D11. Model the channel, peek, submitted record, completion callback, and epoch teardown with fake leases. Assert the maximum outstanding lease count and that an epoch cannot free a submitted record. These tests run on Linux. This is the consume-thread prerequisite called out in #110.
3. Own D3D11VA `hw_frames_ctx` initialization, but continue calling `av_hwframe_transfer_data`. Add the conditional `ffmpeg-sys-next` binding fix, use `avcodec_get_hw_frames_parameters`, apply the pool flags and lease allowance, and fall back by reopening when initialization fails. Linux tests cover policy and arithmetic; Linux cannot type-check the Windows FFI path. Windows CI must compile it and exercise pool rejection with a software/readback fallback. A Windows GPU runner is needed to prove shareable NV12 pool creation.
4. Add an import-only, default-off diagnostic. Export one pool handle, compare adapter LUIDs, query external image support, call `texture_from_d3d11_shared_handle`, wrap it, create every slice's two plane views, then destroy it without presenting. Linux covers descriptor construction and decision tables. Windows CI covers compilation and handle cleanup. Actual import needs a Vulkan plus D3D11 GPU runner; generic Windows CI is not sufficient.
5. Prove synchronization outside production. Implement the smallest pinned wgpu patch for initial tracker state, raw acquire/release barriers, and one candidate submit hook. Test keyed mutex first because it matches the documented NT-handle resource. Test the D3D11-fence/timeline-semaphore variant separately and keep it disabled unless its resource-sharing contract is confirmed. Add finite timeout and forced-failure tests. Windows CI can compile and run mocked state machines; cross-API conformance needs hardware.
6. Connect imported plane views to `YuvConverter`, run the first-frame canary, retain leases through `on_submitted_work_done`, and switch to readback on every recoverable error. Keep direct mode opt-in. Linux tests cover crop/fit/color metadata, epoch races, budget exhaustion, and fallback decisions. Windows CI covers the complete code shape. Hardware tests cover pixels and synchronization.
7. Run an attended rig qualification before changing the default. Test the venue's shipping NVIDIA driver with the real Vulkan backend, all deployed H.264/HEVC NV12 profiles and resolutions, 50 fps content, three output threads, pause/step/seek/loop/rapid cue changes, EOF, device teardown, and injected sync timeouts. Compare canary frames, watch for luma/chroma tearing and old slices, confirm bounded GPU memory/handle counts, measure decode headroom and starvation against readback, and soak for at least one show-length run. If serialized keyed access loses timing headroom or the driver rejects array import, stop the direct path and evaluate the GPU-copy ring.

## Risks and failure containment

| Risk | Observable failure | Detection and containment |
| --- | --- | --- |
| Driver rejects decoder, shared flags, NV12 arrays, or exact Vulkan import | Pool, handle, image, memory bind, or view creation error | Probe before engagement; reopen the established readback path once and record the API/HRESULT/VkResult. |
| D3D11 and Vulkan adapters differ | Import failure, corruption, or device loss | Compare LUIDs before export and require the exact external-image query. Never attempt cross-adapter sharing in this issue. |
| Missing acquire, release, wait, or wrong image layout | Stale/torn luma or chroma, hangs, validation errors, device loss | No production enable without one complete protocol; finite keyed/fence waits; raw acquire/release barriers in the conversion submit; canary and attended soak. |
| Array layer or allocated extent is wrong | Correct luma with wrong chroma, adjacent frame, green edge, or out-of-range view | Build descriptors from `GetDesc`, validate `data[1]` and padded extent separately, and compare visible luma/chroma through both conversion paths in the canary. |
| Decoder still reads a reference surface while Vulkan owns it | Intermittent corruption that depends on GOP structure | Start serialized. Do not allow the decoder to advance across a handoff until completion unless a later design proves per-surface synchronization. |
| Too many application-held surfaces | Decoder stalls or reports static pool exhausted | One lease budget controls pool enlargement, channel capacity, peek, and in-flight records; fall back on budget/setup rejection rather than growing at runtime. |
| Epoch teardown drops resources too early | Use-after-free, old cue frame, device loss | Submitted leases ignore epoch disposal until GPU completion; pool identities include epoch and are retired after all records. |
| NT handles, COM references, or imported allocations leak | Growing handle count and GPU memory across cue changes | One export/import per pool, close every NT handle deterministically, count live pools/leases, and include rapid cue cycling in qualification. |
| Driver accepts the path but renders plausible wrong pixels | Silent show corruption | First-frame dual-path canary, optional cue-boundary checks, visible path status, kill switch, driver-version logging, and attended validation. Some silent corruption remains undetectable automatically. |
| The wgpu fork diverges from 29.0.4 | Upgrade and safety maintenance cost | Keep the patch limited to initial state and next-submit synchronization, add focused upstream tests, and remove it when an upstream release has equivalent APIs. |

The largest risk is synchronization, not handle import. A successful `CreateSharedHandle`, `vkAllocateMemory`, or `create_texture_from_hal` does not prove that decoder writes, codec reference reads, Vulkan sampling, and pool recycling are ordered. Any implementation that treats import success as the runtime gate is incomplete.
