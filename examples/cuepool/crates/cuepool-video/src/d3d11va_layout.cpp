#include <cstddef>
#include <libavutil/hwcontext_d3d11va.h>

#if defined(_WIN64)
static_assert(sizeof(AVD3D11VADeviceContext) == 56);
static_assert(offsetof(AVD3D11VADeviceContext, device) == 0);
static_assert(offsetof(AVD3D11VADeviceContext, device_context) == 8);
static_assert(offsetof(AVD3D11VADeviceContext, video_device) == 16);
static_assert(offsetof(AVD3D11VADeviceContext, video_context) == 24);
static_assert(offsetof(AVD3D11VADeviceContext, lock) == 32);
static_assert(offsetof(AVD3D11VADeviceContext, unlock) == 40);
static_assert(offsetof(AVD3D11VADeviceContext, lock_ctx) == 48);

static_assert(sizeof(AVD3D11VAFramesContext) == 24);
static_assert(offsetof(AVD3D11VAFramesContext, texture) == 0);
static_assert(offsetof(AVD3D11VAFramesContext, BindFlags) == 8);
static_assert(offsetof(AVD3D11VAFramesContext, MiscFlags) == 12);
static_assert(offsetof(AVD3D11VAFramesContext, texture_infos) == 16);
#else
static_assert(sizeof(AVD3D11VADeviceContext) == 28);
static_assert(sizeof(AVD3D11VAFramesContext) == 16);
#endif
