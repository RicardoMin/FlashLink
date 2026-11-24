use std::{ffi::c_void, sync::mpsc, thread, time::Duration};
use windows_sys::Win32::{
    System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED},
    UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
};

extern "C" {
    fn flashlink_snapshot_icons() -> *mut c_void;
    fn flashlink_position_icons(snapshot: *const c_void, restoring: bool);
    fn flashlink_free_icons(snapshot: *mut c_void);
}

struct Snapshot {
    snapshot: *mut c_void,
    dpi: *mut c_void,
    initialized: bool,
}

impl Snapshot {
    pub fn capture() -> Self {
        unsafe {
            let dpi = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            let initialized = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
            let snapshot = if initialized { flashlink_snapshot_icons() } else { std::ptr::null_mut() };
            Self { snapshot, dpi, initialized }
        }
    }

    fn align(&self, restoring: bool) {
        unsafe { flashlink_position_icons(self.snapshot, restoring) }
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe {
            flashlink_free_icons(self.snapshot);
            if self.initialized {
                CoUninitialize();
            }
            if !self.dpi.is_null() {
                SetThreadDpiAwarenessContext(self.dpi);
            }
        }
    }
}

pub struct DesktopIcons {
    requests: mpsc::Sender<Option<mpsc::Sender<()>>>,
}

impl DesktopIcons {
    pub fn capture() -> Result<Self, String> {
        let (requests, work) = mpsc::channel::<Option<mpsc::Sender<()>>>();
        let (ready, captured) = mpsc::sync_channel(0);
        thread::spawn(move || {
            // Explorer can stop answering briefly during a hotplug. Keep its
            // COM apartment off the thread responsible for the stream lease.
            let snapshot = Snapshot::capture();
            if ready.send(!snapshot.snapshot.is_null()).is_err() { return; }
            for reply in work {
                snapshot.align(reply.is_some());
                if let Some(done) = reply { let _ = done.send(()); }
            }
        });
        match captured.recv_timeout(Duration::from_secs(3)) {
            Ok(true) => Ok(Self { requests }),
            _ => Err("无法在切换前读取 Windows 桌面图标位置，请稍后重试。".into()),
        }
    }

    pub fn align(&self) {
        let _ = self.requests.send(None);
    }

    pub fn finish(&self) {
        let (reply, done) = mpsc::channel();
        if self.requests.send(Some(reply)).is_ok() {
            let _ = done.recv_timeout(Duration::from_secs(3));
        }
        thread::sleep(Duration::from_millis(600));
        let (reply2, done2) = mpsc::channel();
        if self.requests.send(Some(reply2)).is_ok() {
            let _ = done2.recv_timeout(Duration::from_secs(3));
        }
    }
}
