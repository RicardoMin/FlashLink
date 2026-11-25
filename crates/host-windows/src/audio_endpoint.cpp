// Use the same default render role as Sunshine's Windows WASAPI loopback capture.
#include <windows.h>
#include <mmdeviceapi.h>
#include <endpointvolume.h>
#include <wrl/client.h>
#include <cwchar>

using Microsoft::WRL::ComPtr;

namespace {
struct AudioApartment {
    HRESULT initialized = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    ~AudioApartment() {
        if (SUCCEEDED(initialized)) CoUninitialize();
    }
    HRESULT devices(ComPtr<IMMDeviceEnumerator>& result) const {
        if (FAILED(initialized) && initialized != RPC_E_CHANGED_MODE) return initialized;
        return CoCreateInstance(__uuidof(MMDeviceEnumerator), nullptr, CLSCTX_ALL,
                                IID_PPV_ARGS(result.GetAddressOf()));
    }
    HRESULT volume(const wchar_t* id, ComPtr<IAudioEndpointVolume>& result) const {
        ComPtr<IMMDeviceEnumerator> devices_;
        HRESULT hr = devices(devices_);
        if (FAILED(hr)) return hr;
        ComPtr<IMMDevice> device;
        hr = devices_->GetDevice(id, device.GetAddressOf());
        if (FAILED(hr)) return hr;
        return device->Activate(__uuidof(IAudioEndpointVolume), CLSCTX_ALL, nullptr,
                                reinterpret_cast<void**>(result.GetAddressOf()));
    }
};
}  // namespace

extern "C" HRESULT flashlink_audio_default(wchar_t** id, size_t* length) {
    AudioApartment apartment;
    ComPtr<IMMDeviceEnumerator> devices;
    HRESULT hr = apartment.devices(devices);
    if (FAILED(hr)) return hr;
    ComPtr<IMMDevice> device;
    hr = devices->GetDefaultAudioEndpoint(eRender, eConsole, device.GetAddressOf());
    if (FAILED(hr)) return hr;
    hr = device->GetId(id);
    if (SUCCEEDED(hr)) *length = std::wcslen(*id) + 1;
    return hr;
}

extern "C" HRESULT flashlink_audio_get_mute(const wchar_t* id, BOOL* muted) {
    AudioApartment apartment;
    ComPtr<IAudioEndpointVolume> volume;
    HRESULT hr = apartment.volume(id, volume);
    if (FAILED(hr)) return hr;
    return volume->GetMute(muted);
}

extern "C" HRESULT flashlink_audio_set(const wchar_t* id, int percent, BOOL muted) {
    AudioApartment apartment;
    ComPtr<IAudioEndpointVolume> volume;
    HRESULT hr = apartment.volume(id, volume);
    if (FAILED(hr)) return hr;
    // Set mute before changing level when ending muted. Never expose the previous
    // slider level for a moment when resuming audible playback.
    if (muted) {
        hr = volume->SetMute(TRUE, nullptr);
        if (FAILED(hr)) return hr;
    }
    if (percent >= 0) {
        hr = volume->SetMasterVolumeLevelScalar(static_cast<float>(percent) / 100.0f, nullptr);
        if (FAILED(hr)) return hr;
    }
    return muted ? S_OK : volume->SetMute(FALSE, nullptr);
}
