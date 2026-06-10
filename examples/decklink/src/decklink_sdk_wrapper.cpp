// Minimal DeckLink SDK wrapper for Windows — captures video input via COM.
//
// Uses the Blackmagic DeckLink API directly through generated COM headers.
// This avoids the Linux-only decklink-c wrapper entirely.

#include <windows.h>
#include <combaseapi.h>
#include <objbase.h>
#include <unknwn.h>
#include <cstdint>
#include <cstring>
#include <mutex>
#include <vector>
#include <atomic>

#include "DeckLinkAPI.h"

// GUIDs we need (from DeckLinkAPI_i.c)
const CLSID CLSID_CDeckLinkIterator =
{ 0xBA6C6F44, 0x6DA5, 0x4DCE, { 0x94, 0xAA, 0xEE, 0x2D, 0x13, 0x72, 0xA6, 0x76 } };

const IID IID_IDeckLinkIterator =
{ 0x50FB36CD, 0x3063, 0x4B73, { 0xBD, 0xBB, 0x95, 0x80, 0x87, 0xF2, 0xD8, 0xBA } };

const IID IID_IDeckLinkInput =
{ 0x4095DB82, 0xE294, 0x4B8C, { 0xAA, 0xA8, 0x3B, 0x9E, 0x80, 0xC4, 0x93, 0x36 } };

const IID IID_IDeckLinkInputCallback =
{ 0x3A94F075, 0xC37D, 0x4BA8, { 0xBC, 0xC0, 0x1D, 0x77, 0x8C, 0x8F, 0x88, 0x1B } };

const IID IID_IDeckLinkVideoBuffer =
{ 0x81F03D70, 0xDE13, 0x4B17, { 0x87, 0x3A, 0xC8, 0xAC, 0x96, 0x89, 0xC6, 0x82 } };

struct FrameBuffer {
    std::mutex mutex;
    std::vector<uint8_t> data;
    long width = 0;
    long height = 0;
    long row_bytes = 0;
    std::atomic<bool> has_new_frame{false};
};

static FrameBuffer g_frame_buffer;
static IDeckLinkInput* g_input = nullptr;
static IDeckLinkIterator* g_iterator = nullptr;

class InputCallback : public IDeckLinkInputCallback {
public:
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, LPVOID* ppv) override {
        if (ppv == nullptr) return E_INVALIDARG;
        if (iid == __uuidof(IUnknown) || iid == IID_IDeckLinkInputCallback) {
            *ppv = this;
            AddRef();
            return S_OK;
        }
        *ppv = nullptr;
        return E_NOINTERFACE;
    }

    ULONG STDMETHODCALLTYPE AddRef() override { return ++ref_count_; }

    ULONG STDMETHODCALLTYPE Release() override {
        ULONG count = --ref_count_;
        if (count == 0) delete this;
        return count;
    }

    HRESULT STDMETHODCALLTYPE VideoInputFormatChanged(
        BMDVideoInputFormatChangedEvents /*notificationEvents*/,
        IDeckLinkDisplayMode* /*newDisplayMode*/,
        BMDDetectedVideoInputFormatFlags /*detectedSignalFlags*/) override {
        return S_OK;
    }

    HRESULT STDMETHODCALLTYPE VideoInputFrameArrived(
        IDeckLinkVideoInputFrame* videoFrame,
        IDeckLinkAudioInputPacket* /*audioPacket*/) override {
        if (!videoFrame) return S_OK;

        IDeckLinkVideoBuffer* buffer = nullptr;
        HRESULT hr = videoFrame->QueryInterface(IID_IDeckLinkVideoBuffer, reinterpret_cast<void**>(&buffer));
        if (hr != S_OK || !buffer) return S_OK;

        hr = buffer->StartAccess(bmdBufferAccessRead);
        if (hr != S_OK) {
            buffer->Release();
            return S_OK;
        }

        void* frame_bytes = nullptr;
        hr = buffer->GetBytes(&frame_bytes);
        if (hr != S_OK || !frame_bytes) {
            buffer->EndAccess(bmdBufferAccessRead);
            buffer->Release();
            return S_OK;
        }

        long width = videoFrame->GetWidth();
        long height = videoFrame->GetHeight();
        long row_bytes = videoFrame->GetRowBytes();

        size_t frame_size = static_cast<size_t>(row_bytes) * static_cast<size_t>(height);

        std::lock_guard<std::mutex> lock(g_frame_buffer.mutex);
        g_frame_buffer.data.resize(frame_size);
        std::memcpy(g_frame_buffer.data.data(), frame_bytes, frame_size);
        g_frame_buffer.width = width;
        g_frame_buffer.height = height;
        g_frame_buffer.row_bytes = row_bytes;
        g_frame_buffer.has_new_frame.store(true);

        buffer->EndAccess(bmdBufferAccessRead);
        buffer->Release();

        return S_OK;
    }

private:
    std::atomic<ULONG> ref_count_{1};
};

extern "C" {

int decklink_init(int device_index) {
    HRESULT hr = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    if (FAILED(hr) && hr != RPC_E_CHANGED_MODE) return -1;

    hr = CoCreateInstance(
        CLSID_CDeckLinkIterator,
        nullptr,
        CLSCTX_ALL,
        IID_IDeckLinkIterator,
        reinterpret_cast<void**>(&g_iterator));

    if (FAILED(hr) || !g_iterator) return -2;

    IDeckLink* device = nullptr;
    for (int i = 0; i <= device_index; ++i) {
        if (device) { device->Release(); device = nullptr; }
        hr = g_iterator->Next(&device);
        if (hr != S_OK) break;
    }

    if (!device) {
        if (g_iterator) { g_iterator->Release(); g_iterator = nullptr; }
        return -3;
    }

    hr = device->QueryInterface(IID_IDeckLinkInput, reinterpret_cast<void**>(&g_input));
    device->Release();

    if (FAILED(hr) || !g_input) {
        if (g_iterator) { g_iterator->Release(); g_iterator = nullptr; }
        return -4;
    }

    IDeckLinkDisplayModeIterator* mode_iter = nullptr;
    hr = g_input->GetDisplayModeIterator(&mode_iter);
    if (FAILED(hr) || !mode_iter) {
        g_input->Release(); g_input = nullptr;
        g_iterator->Release(); g_iterator = nullptr;
        return -5;
    }

    IDeckLinkDisplayMode* mode = nullptr;
    hr = mode_iter->Next(&mode);
    mode_iter->Release();

    if (hr != S_OK || !mode) {
        g_input->Release(); g_input = nullptr;
        g_iterator->Release(); g_iterator = nullptr;
        return -6;
    }

    BMDDisplayMode mode_id = mode->GetDisplayMode();
    mode->Release();

    hr = g_input->EnableVideoInput(mode_id, bmdFormat8BitBGRA, bmdVideoInputFlagDefault);
    if (FAILED(hr)) {
        g_input->Release(); g_input = nullptr;
        g_iterator->Release(); g_iterator = nullptr;
        return -7;
    }

    InputCallback* callback = new InputCallback();
    hr = g_input->SetCallback(callback);
    if (FAILED(hr)) {
        delete callback;
        g_input->Release(); g_input = nullptr;
        g_iterator->Release(); g_iterator = nullptr;
        return -8;
    }
    callback->Release();

    hr = g_input->StartStreams();
    if (FAILED(hr)) {
        g_input->Release(); g_input = nullptr;
        g_iterator->Release(); g_iterator = nullptr;
        return -9;
    }

    return 0;
}

int decklink_get_frame(int* width, int* height, int* row_bytes, unsigned char** data) {
    std::lock_guard<std::mutex> lock(g_frame_buffer.mutex);
    if (!g_frame_buffer.has_new_frame.load()) return 0;

    size_t size = g_frame_buffer.data.size();
    *data = static_cast<unsigned char*>(std::malloc(size));
    if (!*data) return -1;

    std::memcpy(*data, g_frame_buffer.data.data(), size);
    *width = static_cast<int>(g_frame_buffer.width);
    *height = static_cast<int>(g_frame_buffer.height);
    *row_bytes = static_cast<int>(g_frame_buffer.row_bytes);
    g_frame_buffer.has_new_frame.store(false);
    return 1;
}

void decklink_shutdown() {
    if (g_input) {
        g_input->StopStreams();
        g_input->Release();
        g_input = nullptr;
    }
    if (g_iterator) {
        g_iterator->Release();
        g_iterator = nullptr;
    }
    CoUninitialize();
}

} // extern "C"
