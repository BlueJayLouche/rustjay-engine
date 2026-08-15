// Compile-time ABI guard for the AVD3D12VA structs that d3d12_zero_copy.rs
// reinterprets with #[repr(C)] mirrors. If a future FFmpeg SDK reorders or
// resizes these, the build breaks here instead of corrupting memory at run
// time.
//
// These mirrors match the FFmpeg 8.0 / 8.1.0 ABI. FFmpeg reworked the D3D12VA
// structs mid-series (8.1.2+: texture arrays, subresource indices, extra
// flags) without a libavutil major bump, so a build against an n8.1-latest or
// master SDK fails here BY DESIGN — adopting the new ABI also means adopting
// the array-texture resource model in d3d12_zero_copy.rs, not just resizing
// the mirrors. Build against the 8.0-ABI SDK (FFMPEG_DIR on the rigs, or the
// pinned SDK in release-apps.yml); a runtime version check in configure_pool
// declines the direct path if the DLLs ever diverge from the build.
#include <cstddef>
#include <libavutil/hwcontext_d3d12va.h>

#ifdef _WIN64
static_assert(sizeof(AVD3D12VADeviceContext) == 40, "AVD3D12VADeviceContext size");
static_assert(offsetof(AVD3D12VADeviceContext, device) == 0, "device offset");
static_assert(offsetof(AVD3D12VADeviceContext, video_device) == 8, "video_device offset");
static_assert(offsetof(AVD3D12VADeviceContext, lock) == 16, "lock offset");
static_assert(offsetof(AVD3D12VADeviceContext, unlock) == 24, "unlock offset");
static_assert(offsetof(AVD3D12VADeviceContext, lock_ctx) == 32, "lock_ctx offset");

static_assert(sizeof(AVD3D12VASyncContext) == 24, "AVD3D12VASyncContext size");
static_assert(offsetof(AVD3D12VASyncContext, fence) == 0, "fence offset");
static_assert(offsetof(AVD3D12VASyncContext, event) == 8, "event offset");
static_assert(offsetof(AVD3D12VASyncContext, fence_value) == 16, "fence_value offset");

static_assert(sizeof(AVD3D12VAFrame) == 32, "AVD3D12VAFrame size");
static_assert(offsetof(AVD3D12VAFrame, texture) == 0, "texture offset");
static_assert(offsetof(AVD3D12VAFrame, sync_ctx) == 8, "sync_ctx offset");

static_assert(sizeof(AVD3D12VAFramesContext) == 8, "AVD3D12VAFramesContext size");
static_assert(offsetof(AVD3D12VAFramesContext, format) == 0, "format offset");
static_assert(offsetof(AVD3D12VAFramesContext, flags) == 4, "flags offset");
#else
static_assert(sizeof(AVD3D12VADeviceContext) == 20, "AVD3D12VADeviceContext size (32-bit)");
static_assert(sizeof(AVD3D12VAFramesContext) == 8, "AVD3D12VAFramesContext size (32-bit)");
#endif
