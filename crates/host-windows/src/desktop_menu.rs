use super::wide;
use crate::management::PairedDevice;
use std::ptr;
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub const TRAY_MESSAGE: u32 = WM_APP + 41;
pub const OPEN: usize = 2101;
pub const DEVICES: usize = 2102;
pub const EXIT: usize = 2103;
pub const REPORT: usize = 2104;
pub const PRIVACY: usize = 2105;
pub const RESTORE: usize = 2107;
pub const UPDATE: usize = 2108;

fn add(menu: HMENU, id: usize, title: &str) {
    unsafe { AppendMenuW(menu, MF_STRING, id, wide(title).as_ptr()) };
}

pub fn attach_menu(window: HWND) {
    unsafe {
        let bar = CreateMenu();
        let actions = CreatePopupMenu();
        add(actions, PRIVACY, "防窥模式");
        add(actions, RESTORE, "本机恢复显示…");
        add(actions, DEVICES, "管理已授权设备…");
        add(actions, REPORT, "导出测试报告…");
        add(actions, UPDATE, "检查更新…");
        add(actions, EXIT, "退出主机");
        AppendMenuW(bar, MF_POPUP, actions as usize, wide("主机(&H)").as_ptr());
        SetMenu(window, bar);
    }
}

pub fn set_privacy(window: HWND, label: &str, enabled: bool) {
    unsafe {
        let menu = GetSubMenu(GetMenu(window), 0);
        let flags = MF_BYCOMMAND | MF_STRING | if enabled { MF_ENABLED } else { MF_GRAYED };
        ModifyMenuW(menu, PRIVACY as u32, flags, PRIVACY, wide(label).as_ptr());
        DrawMenuBar(window);
    }
}

pub struct Tray {
    data: NOTIFYICONDATAW,
    pub available: bool,
    pub shell_restarted: u32,
}

impl Tray {
    pub fn install(window: HWND) -> Self {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: window,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: TRAY_MESSAGE,
            hIcon: unsafe { LoadIconW(ptr::null_mut(), IDI_APPLICATION) },
            ..Default::default()
        };
        let tip = wide("FlashLink · 局域网主机");
        data.szTip[..tip.len()].copy_from_slice(&tip);
        Self {
            available: unsafe { Shell_NotifyIconW(NIM_ADD, &data) != 0 },
            shell_restarted: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
            data,
        }
    }

    pub fn restore_icon(&mut self) {
        self.available = unsafe { Shell_NotifyIconW(NIM_ADD, &self.data) != 0 };
        if !self.available {
            show(self.data.hWnd);
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &self.data) };
    }
}

pub fn show(window: HWND) {
    unsafe {
        ShowWindow(window, SW_RESTORE);
        SetForegroundWindow(window);
    }
}

fn popup(window: HWND, menu: HMENU) -> usize {
    unsafe {
        let mut position = POINT::default();
        GetCursorPos(&mut position);
        SetForegroundWindow(window);
        let selected = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            position.x,
            position.y,
            0,
            window,
            ptr::null(),
        );
        PostMessageW(window, WM_NULL, 0, 0);
        DestroyMenu(menu);
        selected as usize
    }
}

pub fn tray_action(window: HWND) -> usize {
    let menu = unsafe { CreatePopupMenu() };
    add(menu, OPEN, "打开 FlashLink");
    add(menu, DEVICES, "管理已授权设备…");
    add(menu, REPORT, "导出测试报告…");
    add(menu, UPDATE, "检查更新…");
    add(menu, EXIT, "退出主机");
    popup(window, menu)
}

pub fn select_device(window: HWND, devices: &[PairedDevice]) -> Option<String> {
    let menu = unsafe { CreatePopupMenu() };
    for (index, device) in devices.iter().enumerate() {
        let name: String = device
            .name
            .chars()
            .filter(|ch| !ch.is_control())
            .take(64)
            .collect();
        let status = if device.enabled { "" } else { " · 已禁用" };
        let label = format!(
            "{}  ·  {}{status}",
            name.replace('&', "&&"),
            device.uuid.chars().take(8).collect::<String>()
        );
        add(menu, index + 1, &label);
    }
    let device = devices.get(popup(window, menu).checked_sub(1)?)?;
    let question = format!(
        "移除设备「{}」？这会结束它当前的连接，下次需要在电脑上重新授权。",
        device.name
    );
    let answer = unsafe {
        MessageBoxW(
            window,
            wide(&question).as_ptr(),
            wide("移除已授权设备").as_ptr(),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
        )
    };
    (answer == IDYES).then(|| device.uuid.clone())
}
