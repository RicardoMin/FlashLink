// Explorer stores item positions in folder-view coordinates, whose origin
// moves when a monitor to the left of the primary is disconnected.
// Also snapshots visible top-level application window placements so transient
// HPD bounces through a smaller secondary monitor do not leave windows shrunk
// or stranded on the wrong monitor.
#include <windows.h>
#include <shlobj.h>
#include <exdisp.h>
#include <servprov.h>
#include <wrl/client.h>
#include <map>
#include <memory>
#include <vector>

using Microsoft::WRL::ComPtr;

struct SavedWindow {
    HWND hwnd;
    RECT rect;
    bool maximized;
    bool on_primary;
};

struct Positions {
    RECT primary;
    std::map<std::vector<BYTE>, POINT> items;
    std::vector<SavedWindow> windows;
};

static bool is_shell_window(HWND hwnd) {
    wchar_t cls[64]{};
    int len = GetClassNameW(hwnd, cls, 64);
    if (len <= 0) return true;
    return wcscmp(cls, L"Progman") == 0 ||
           wcscmp(cls, L"WorkerW") == 0 ||
           wcscmp(cls, L"Shell_TrayWnd") == 0 ||
           wcscmp(cls, L"Shell_SecondaryTrayWnd") == 0 ||
           wcscmp(cls, L"Windows.UI.Core.CoreWindow") == 0;
}

static BOOL CALLBACK capture_window_proc(HWND hwnd, LPARAM lparam) {
    auto* pos = reinterpret_cast<Positions*>(lparam);
    if (!IsWindowVisible(hwnd) || IsIconic(hwnd)) return TRUE;
    LONG_PTR ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    if ((ex & WS_EX_TOOLWINDOW) != 0) return TRUE;
    if (GetWindow(hwnd, GW_OWNER) != nullptr) return TRUE;
    if (is_shell_window(hwnd)) return TRUE;
    RECT r{};
    if (!GetWindowRect(hwnd, &r)) return TRUE;
    if (r.right - r.left < 64 || r.bottom - r.top < 64) return TRUE;
    POINT center{ (r.left + r.right) / 2, (r.top + r.bottom) / 2 };
    bool on_primary = PtInRect(&pos->primary, center) != FALSE;
    pos->windows.push_back(SavedWindow{ hwnd, r, IsZoomed(hwnd) != FALSE, on_primary });
    return TRUE;
}

static void restore_windows(const Positions& saved, bool restoring) {
    for (const auto& w : saved.windows) {
        if (!IsWindow(w.hwnd) || !IsWindowVisible(w.hwnd) || IsIconic(w.hwnd)) continue;
        // During privacy mode, restore primary-monitor windows that may have been shrunk
        // or moved by a transient bounce through a smaller secondary monitor.
        // On exit, move former secondary-monitor windows back to their original monitor
        // (leaving primary windows where the user worked during privacy).
        if (!restoring && !w.on_primary) continue;
        if (restoring && w.on_primary) continue;
        POINT center{ (w.rect.left + w.rect.right) / 2, (w.rect.top + w.rect.bottom) / 2 };
        if (!MonitorFromPoint(center, MONITOR_DEFAULTTONULL)) continue;
        if (w.maximized) {
            if (IsZoomed(w.hwnd)) {
                RECT cur{};
                if (GetWindowRect(w.hwnd, &cur)) {
                    POINT cur_center{ (cur.left + cur.right) / 2, (cur.top + cur.bottom) / 2 };
                    if (MonitorFromPoint(cur_center, MONITOR_DEFAULTTONULL) ==
                        MonitorFromPoint(center, MONITOR_DEFAULTTONULL)) {
                        continue;
                    }
                }
                ShowWindowAsync(w.hwnd, SW_RESTORE);
            }
            SetWindowPos(w.hwnd, nullptr, w.rect.left, w.rect.top,
                         w.rect.right - w.rect.left, w.rect.bottom - w.rect.top,
                         SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
            ShowWindowAsync(w.hwnd, SW_SHOWMAXIMIZED);
        } else {
            RECT cur{};
            if (GetWindowRect(w.hwnd, &cur) && EqualRect(&cur, &w.rect)) continue;
            SetWindowPos(w.hwnd, nullptr, w.rect.left, w.rect.top,
                         w.rect.right - w.rect.left, w.rect.bottom - w.rect.top,
                         SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
        }
    }
}

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
        EnumWindows(capture_window_proc, reinterpret_cast<LPARAM>(result.get()));
        return result.release();
    } catch (...) { return nullptr; }
}

extern "C" void flashlink_position_icons(const void* snapshot, bool restoring) noexcept {
    try {
        if (!snapshot) return;
        const auto& saved = *static_cast<const Positions*>(snapshot);
        Desktop d;
        if (d.open() && EqualRect(&saved.primary, &d.primary)) {
            restore_windows(saved, restoring);
            if (d.folder->GetAutoArrange() != S_OK) {
                std::vector<PCUITEMID_CHILD> items;
                std::vector<POINT> positions;
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
            }
        }
    } catch (...) {}
}

extern "C" void flashlink_free_icons(void* snapshot) noexcept {
    delete static_cast<Positions*>(snapshot);
}
