// Explorer stores item positions in folder-view coordinates, whose origin
// moves when a monitor to the left of the primary is disconnected.
#include <windows.h>
#include <shlobj.h>
#include <exdisp.h>
#include <servprov.h>
#include <wrl/client.h>
#include <map>
#include <memory>
#include <vector>

using Microsoft::WRL::ComPtr;
struct Positions {
    RECT primary;
    std::map<std::vector<BYTE>, POINT> items;
};

struct Desktop {
    ComPtr<IFolderView> folder;
    HWND list = nullptr;
    RECT primary{};

    bool open() {
        ComPtr<IShellWindows> windows;
        if (FAILED(CoCreateInstance(CLSID_ShellWindows, nullptr, CLSCTX_LOCAL_SERVER,
                                    IID_PPV_ARGS(&windows)))) return false;
        VARIANT location{}, empty{};
        location.vt = VT_I4;
        location.lVal = CSIDL_DESKTOP;
        long ignored = 0;
        ComPtr<IDispatch> desktop;
        if (windows->FindWindowSW(&location, &empty, SWC_DESKTOP, &ignored,
                                  SWFO_NEEDDISPATCH, &desktop) != S_OK) return false;
        ComPtr<IServiceProvider> services;
        ComPtr<IShellBrowser> browser;
        ComPtr<IShellView> view;
        HWND host = nullptr;
        if (FAILED(desktop.As(&services)) ||
            FAILED(services->QueryService(SID_STopLevelBrowser, IID_PPV_ARGS(&browser))) ||
            FAILED(browser->QueryActiveShellView(&view)) || FAILED(view.As(&folder)) ||
            FAILED(view->GetWindow(&host))) return false;
        list = FindWindowExW(host, nullptr, L"SysListView32", nullptr);
        MONITORINFO monitor{sizeof(MONITORINFO)};
        if (!list || !GetMonitorInfoW(MonitorFromPoint({0, 0}, MONITOR_DEFAULTTOPRIMARY),
                                      &monitor)) return false;
        primary = monitor.rcMonitor;
        return true;
    }
};

// PIDLs identify items after Explorer reflows the desktop.
extern "C" void* flashlink_snapshot_icons() noexcept {
    try {
        Desktop d;
        if (!d.open()) return nullptr;
        int count = 0;
        if (FAILED(d.folder->ItemCount(SVGIO_ALLVIEW, &count))) return nullptr;
        std::unique_ptr<Positions> result(new Positions);
        result->primary = d.primary;
        for (int i = 0; i < count; ++i) {
            PITEMID_CHILD item = nullptr;
            POINT point{};
            if (FAILED(d.folder->Item(i, &item)) || !item) return nullptr;
            std::unique_ptr<ITEMIDLIST, decltype(&CoTaskMemFree)> cleanup(item, CoTaskMemFree);
            if (FAILED(d.folder->GetItemPosition(item, &point)) ||
                !ClientToScreen(d.list, &point)) return nullptr;
            auto bytes = reinterpret_cast<const BYTE*>(item);
            result->items.emplace(std::vector<BYTE>(bytes, bytes + ILGetSize(item)), point);
        }
        return result.release();
    } catch (...) { return nullptr; }
}

extern "C" void flashlink_position_icons(const void* snapshot, bool restoring) noexcept {
    try {
        if (!snapshot) return;
        Desktop d;
        if (!d.open() || d.folder->GetAutoArrange() == S_OK) return;
        std::vector<PCUITEMID_CHILD> items;
        std::vector<POINT> positions;
        const auto& saved = *static_cast<const Positions*>(snapshot);
        if (!EqualRect(&saved.primary, &d.primary)) return;
        for (const auto& entry : saved.items) {
            auto item = reinterpret_cast<PCUITEMID_CHILD>(entry.first.data());
            POINT target = entry.second, current{};
            if (!restoring && !PtInRect(&saved.primary, target)) continue;
            if (!MonitorFromPoint(target, MONITOR_DEFAULTTONULL) || !ScreenToClient(d.list, &target) ||
                FAILED(d.folder->GetItemPosition(item, &current)) ||
                (current.x == target.x && current.y == target.y)) continue;
            items.push_back(item);
            positions.push_back(target);
        }
        if (!items.empty()) {
            d.folder->SelectAndPositionItems(static_cast<UINT>(items.size()), items.data(),
                                             positions.data(), SVSI_POSITIONITEM | SVSI_NOTAKEFOCUS);
        }
    } catch (...) {}
}

extern "C" void flashlink_free_icons(void* snapshot) noexcept {
    delete static_cast<Positions*>(snapshot);
}
