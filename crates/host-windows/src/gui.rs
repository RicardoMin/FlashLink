use super::{
    service::{self, Action, Event},
    Options,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::os::windows::process::CommandExt;
use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    ptr,
    sync::mpsc,
    thread,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        CreateFontW, DeleteObject, Ellipse, FillRect, GetStockObject, GetSysColor,
        GetSysColorBrush, RoundRect, SelectObject, SetBkColor, SetDCBrushColor, SetTextColor,
        COLOR_WINDOW, COLOR_WINDOWTEXT, DC_BRUSH, DEFAULT_CHARSET, DEFAULT_QUALITY, FW_NORMAL,
        FW_SEMIBOLD, HDC, NULL_PEN,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

const RETRY: usize = 1001;
#[path = "desktop_menu.rs"]
mod desktop_menu;
const FIREWALL: usize = 1002;
const LOG: usize = 1003;
const LICENSES: usize = 1006;
const YES: usize = 1004;
const NO: usize = 1005;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
enum Input {
    RestoreTray,
    Button(usize),
    Close(HWND),
}
thread_local! {static INPUT:RefCell<Vec<Input>>=const{RefCell::new(Vec::new())};}
thread_local! {static SHELL_RESTARTED:Cell<u32>=const{Cell::new(0)};}
unsafe extern "system" fn wndproc(w: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg != 0 && SHELL_RESTARTED.with(Cell::get) == msg {
        INPUT.with(|q| q.borrow_mut().push(Input::RestoreTray));
        return 0;
    }
    match msg {
        WM_ERASEBKGND if standard_palette() => {
            paint_window(w, wp as HDC);
            1
        }
        WM_CTLCOLORSTATIC if standard_palette() => {
            let id = GetDlgCtrlID(lp as HWND);
            let (background, foreground) = match id {
                RAIL_TITLE => (RAIL, INK),
                RAIL_LABEL => (RAIL, MUTED),
                RAIL_SELECTED => (SELECTION, INK),
                PAGE_TITLE => (PAPER, INK),
                PAGE_SUBTITLE => (PAPER, MUTED),
                PANEL_TITLE => (WHITE, INK),
                PANEL_TEXT => (WHITE, MUTED),
                CODE_LABEL => (SELECTION, BLUE),
                _ => return DefWindowProcW(w, msg, wp, lp),
            };
            let dc = wp as HDC;
            SetBkColor(dc, background);
            SetTextColor(dc, foreground);
            SetDCBrushColor(dc, background);
            GetStockObject(DC_BRUSH) as LRESULT
        }
        desktop_menu::TRAY_MESSAGE => {
            let id = match lp as u32 {
                WM_LBUTTONUP | WM_LBUTTONDBLCLK | WM_KEYUP => desktop_menu::OPEN,
                WM_RBUTTONUP | WM_CONTEXTMENU => desktop_menu::tray_action(w),
                _ => 0,
            };
            if id != 0 {
                INPUT.with(|q| q.borrow_mut().push(Input::Button(id)));
            }
            0
        }
        WM_CLOSE => {
            INPUT.with(|q| q.borrow_mut().push(Input::Close(w)));
            0
        }
        WM_COMMAND => {
            // Control notifications share WM_COMMAND with actual button clicks.
            if lp == 0 || (wp >> 16) == BN_CLICKED as usize {
                INPUT.with(|q| q.borrow_mut().push(Input::Button(wp & 0xffff)));
            }
            0
        }
        _ => unsafe { DefWindowProcW(w, msg, wp, lp) },
    }
}
// Labels remain native STATIC controls, so UI Automation and selection state can be read normally.
const RAIL_TITLE: i32 = 3010;
const RAIL_LABEL: i32 = 3011;
const RAIL_SELECTED: i32 = 3012;
const PAGE_TITLE: i32 = 3013;
const PAGE_SUBTITLE: i32 = 3014;
const PANEL_TITLE: i32 = 3015;
const PANEL_TEXT: i32 = 3016;
const CODE_LABEL: i32 = 3017;
const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
const PAPER: u32 = rgb(247, 249, 251);
const RAIL: u32 = rgb(237, 242, 246);
const WHITE: u32 = rgb(255, 255, 255);
const BLUE: u32 = rgb(40, 111, 229);
const SELECTION: u32 = rgb(224, 235, 255);
const INK: u32 = rgb(23, 29, 36);
const MUTED: u32 = rgb(75, 89, 103);

fn standard_palette() -> bool {
    // Keep the user's system colors in custom and dark/high-contrast color schemes.
    unsafe { GetSysColor(COLOR_WINDOW) == WHITE && GetSysColor(COLOR_WINDOWTEXT) == 0 }
}

unsafe fn tile(dc: HDC, color: u32, rect: (i32, i32, i32, i32), radius: i32) {
    SetDCBrushColor(dc, color);
    let (left, top, right, bottom) = rect;
    RoundRect(dc, left, top, right, bottom, radius, radius);
}

unsafe fn paint_window(window: HWND, dc: HDC) {
    let mut bounds = RECT::default();
    GetClientRect(window, &mut bounds);
    let brush = GetStockObject(DC_BRUSH);
    let old_brush = SelectObject(dc, brush);
    let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
    SetDCBrushColor(dc, PAPER);
    FillRect(dc, &bounds, brush);
    if GetWindow(window, GW_OWNER).is_null() {
        tile(dc, RAIL, (0, 0, 180, bounds.bottom), 0);
        tile(dc, SELECTION, (8, 115, 172, 158), 12);
        tile(dc, BLUE, (8, 125, 11, 148), 3);
        // A monitor glyph marks the local computer in the device sidebar.
        tile(dc, BLUE, (20, 127, 43, 143), 4);
        tile(dc, WHITE, (22, 129, 41, 140), 2);
        tile(dc, BLUE, (27, 144, 36, 146), 2);
        tile(dc, WHITE, (198, 96, bounds.right - 18, 288), 16);
        tile(dc, SELECTION, (214, 110, 247, 143), 10);
        SetDCBrushColor(dc, BLUE);
        Ellipse(dc, 225, 121, 236, 132);
        tile(dc, WHITE, (198, 300, bounds.right - 18, 455), 16);
    } else {
        tile(dc, WHITE, (14, 14, bounds.right - 14, 232), 14);
        tile(dc, SELECTION, (28, 162, bounds.right - 28, 219), 12);
    }
    SelectObject(dc, old_pen);
    SelectObject(dc, old_brush);
}

pub fn notice(s: &str) {
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            wide(s).as_ptr(),
            wide("FlashLink").as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
}
fn window_handle(title: &str, parent: HWND, width: i32, height: i32) -> HWND {
    let style = if parent.is_null() {
        WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX
    } else {
        WS_CAPTION | WS_SYSMENU | WS_POPUP
    };
    unsafe {
        CreateWindowExW(
            0,
            wide("FlashLinkHostWindow").as_ptr(),
            wide(title).as_ptr(),
            style | WS_CLIPCHILDREN,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            width,
            height,
            parent,
            ptr::null_mut(),
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        )
    }
}
struct Font(windows_sys::Win32::Graphics::Gdi::HFONT);
impl Font {
    fn new(size: i32, bold: bool) -> Self {
        Self(unsafe {
            CreateFontW(
                -size,
                0,
                0,
                0,
                if bold { FW_SEMIBOLD } else { FW_NORMAL } as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET as u32,
                0,
                0,
                DEFAULT_QUALITY as u32,
                0,
                wide("Microsoft YaHei UI").as_ptr(),
            )
        })
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}
fn child(
    parent: HWND,
    class: &str,
    title: &str,
    id: usize,
    rect: (i32, i32, i32, i32),
    font: &Font,
) -> HWND {
    let (x, y, w, h) = rect;
    let style = WS_CHILD | WS_VISIBLE | if class == "BUTTON" { WS_TABSTOP } else { 0 };
    unsafe {
        let handle = CreateWindowExW(
            0,
            wide(class).as_ptr(),
            wide(title).as_ptr(),
            style,
            x,
            y,
            w,
            h,
            parent,
            id as _,
            GetModuleHandleW(ptr::null()),
            ptr::null(),
        );
        SendMessageW(handle, WM_SETFONT, font.0 as WPARAM, 1);
        handle
    }
}
fn set_text(w: HWND, s: &str) {
    unsafe {
        SetWindowTextW(w, wide(s).as_ptr());
    }
}
fn visible(w: HWND, v: bool) {
    unsafe {
        ShowWindow(w, if v { SW_SHOW } else { SW_HIDE });
    }
}
fn enable(w: HWND, v: bool) {
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow(w, v.into());
    }
}
fn start(options: Options, events: mpsc::Sender<Event>) -> mpsc::Sender<Action> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        if let Err(error) = service::serve(&options, &events, &rx) {
            let _ = events.send(Event::Failed(error));
        }
    });
    tx
}

pub fn window(options: Options) -> Result<(), String> {
    // Windows scales this logical layout per display; leave bitmap scaling to the OS on high DPI.
    let name = wide("FlashLinkHostWindow");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: unsafe { GetModuleHandleW(ptr::null()) },
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
        lpszClassName: name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&wc) } == 0 {
        return Err(io::Error::last_os_error().to_string());
    }
    let main = window_handle("FlashLink · Windows 主机", ptr::null_mut(), 758, 536);
    if main.is_null() {
        return Err(format!(
            "无法打开 Windows 窗口：{}",
            io::Error::last_os_error()
        ));
    }
    let normal = Font::new(16, false);
    let small = Font::new(14, false);
    let heading = Font::new(28, true);
    let title = Font::new(20, true);
    let name = std::env::var("COMPUTERNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "FlashLink PC".into());
    child(
        main,
        "STATIC",
        "FlashLink",
        RAIL_TITLE as _,
        (18, 24, 150, 37),
        &heading,
    );
    child(
        main,
        "STATIC",
        "我的设备",
        RAIL_LABEL as _,
        (18, 85, 140, 21),
        &small,
    );
    child(
        main,
        "STATIC",
        "这台电脑",
        RAIL_SELECTED as _,
        (53, 125, 108, 23),
        &normal,
    );
    for (id, label, y) in [
        (desktop_menu::DEVICES, "管理已授权设备", 182),
        (desktop_menu::REPORT, "导出测试报告", 227),
        (LOG, "查看日志", 272),
        (LICENSES, "开源许可", 317),
    ] {
        child(main, "BUTTON", label, id, (16, y, 146, 34), &normal);
    }
    child(
        main,
        "STATIC",
        &name,
        PAGE_TITLE as _,
        (204, 17, 497, 40),
        &heading,
    );
    child(
        main,
        "STATIC",
        "Windows  ·  局域网被控端",
        PAGE_SUBTITLE as _,
        (205, 60, 488, 24),
        &small,
    );
    let phase = child(
        main,
        "STATIC",
        "正在启动串流引擎…",
        PANEL_TITLE as _,
        (257, 112, 443, 45),
        &title,
    );
    let details = child(
        main,
        "STATIC",
        "请保持桌面解锁，并让手机或平板连接同一局域网。",
        PANEL_TEXT as _,
        (217, 158, 485, 74),
        &normal,
    );
    let firewall = child(
        main,
        "BUTTON",
        "允许专用网络连接",
        FIREWALL,
        (216, 239, 156, 34),
        &normal,
    );
    let retry = child(
        main,
        "BUTTON",
        "重新启动",
        RETRY,
        (382, 239, 108, 34),
        &normal,
    );
    child(
        main,
        "STATIC",
        "隐私与显示",
        PANEL_TITLE as _,
        (217, 316, 486, 31),
        &title,
    );
    let note = child(
        main,
        "STATIC",
        "普通模式会在本机显示桌面。首次开启隐私前，请先安装虚拟屏。",
        PANEL_TEXT as _,
        (217, 353, 486, 47),
        &normal,
    );
    child(
        main,
        "BUTTON",
        "切换隐私模式",
        desktop_menu::PRIVACY,
        (216, 406, 156, 34),
        &normal,
    );
    child(
        main,
        "BUTTON",
        "安装虚拟屏",
        desktop_menu::VDD,
        (382, 406, 122, 34),
        &normal,
    );
    visible(retry, false);
    enable(firewall, false);
    let consent = window_handle("FlashLink · 连接请求", main, 515, 336);
    if consent.is_null() {
        return Err("无法创建配对窗口".into());
    }
    child(
        consent,
        "STATIC",
        "首次设备授权",
        PANEL_TEXT as _,
        (29, 26, 440, 20),
        &small,
    );
    let device = child(
        consent,
        "STATIC",
        "",
        PANEL_TITLE as _,
        (28, 54, 445, 43),
        &title,
    );
    let identity = child(
        consent,
        "STATIC",
        "",
        PANEL_TEXT as _,
        (29, 105, 446, 49),
        &normal,
    );
    child(
        consent,
        "STATIC",
        "确认码",
        CODE_LABEL as _,
        (42, 183, 51, 23),
        &small,
    );
    let code = child(
        consent,
        "STATIC",
        "",
        CODE_LABEL as _,
        (104, 169, 353, 43),
        &heading,
    );
    child(consent, "BUTTON", "拒绝", NO, (255, 247, 93, 36), &normal);
    child(
        consent,
        "BUTTON",
        "允许连接",
        YES,
        (359, 247, 117, 36),
        &normal,
    );
    desktop_menu::attach_menu(main);
    let mut tray = desktop_menu::Tray::install(main);
    SHELL_RESTARTED.with(|id| id.set(tray.shell_restarted));
    visible(main, true);
    let (event_tx, event_rx) = mpsc::channel();
    let mut commands = start(options.clone(), event_tx.clone());
    let mut pending = None;
    let mut connection = None;
    let mut firewall_active = false;
    let (fw_tx, fw_rx) = mpsc::channel::<String>();
    let (report_tx, report_rx) = mpsc::channel::<Result<String, String>>();
    let mut exporting = false;
    unsafe {
        SetTimer(main, 1, 150, None);
    }
    let mut msg = MSG::default();
    'events: while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {
        if unsafe { IsDialogMessageW(if pending.is_some() { consent } else { main }, &msg) } == 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        for event in event_rx.try_iter() {
            match event {
                Event::Devices(Ok(devices)) if !devices.is_empty() => {
                    desktop_menu::show(main);
                    if let Some(id) = desktop_menu::select_device(main, &devices) {
                        let _ = commands.send(Action::Revoke(id));
                    }
                }
                Event::Devices(Ok(_)) => notice("当前没有已授权的安卓设备。"),
                Event::Devices(Err(error)) => notice(&error),
                Event::Paused(state) => {
                    use crate::privacy_time::Desktop;
                    connection = None;
                    pending = None;
                    enable(firewall, false);
                    visible(consent, false);
                    visible(retry, false);
                    let (heading, explanation)=match state {
                        Desktop::Recovering=>("正在恢复显示","恢复原显示配置后，主机会在桌面可用时自动上线。如果 Windows 暂时拒绝恢复，可在本机解锁后通过“主机 → 本机恢复显示…”确认。"),
                        Desktop::Locked=>("等待本机解锁","Windows 已锁定。请在电脑上解锁，手机和平板随后即可重新发现并直连。"),
                        Desktop::Unknown=>("正在确认 Windows 桌面状态","确认当前用户的桌面已解锁后，主机会自动恢复连接。"),
                        Desktop::Ready=>continue,
                    };
                    set_text(phase, heading);
                    set_text(details, explanation);
                    set_text(note, "附近发现和设备授权已暂停，原有配对记录保留。");
                }
                Event::Online { base, exe, address } => {
                    set_text(
                        note,
                        "普通模式会在本机显示桌面。可从“主机”菜单安装虚拟屏并开启隐私。",
                    );
                    set_text(phase, "已就绪，等待手机或平板连接");
                    set_text(details,&format!("电脑地址   {address}:{base}\r\n在手机或平板的“我的设备”中选择这台电脑。首次连接时，两端会显示同一个确认码。"));
                    connection = Some((exe, base));
                    enable(firewall, true);
                }
                Event::Privacy(text) => set_text(note, &text),
                Event::Status(text) => set_text(phase, &text),
                Event::Ask {
                    id,
                    name,
                    address,
                    code: token,
                } => {
                    pending = Some(id);
                    set_text(device, &format!("{} 请求连接", name));
                    set_text(
                        identity,
                        &format!("设备地址  {address}\r\n请核对手机或平板上显示的确认码。"),
                    );
                    set_text(code, &token);
                    visible(main, true);
                    visible(consent, true);
                    unsafe {
                        ShowWindow(main, SW_RESTORE);
                        SetForegroundWindow(consent);
                        FlashWindow(consent, 1);
                    }
                }
                Event::Dismiss => {
                    pending = None;
                    visible(consent, false);
                }
                Event::Failed(text) => {
                    connection = None;
                    enable(firewall, false);
                    pending = None;
                    visible(consent, false);
                    set_text(phase, "主机未就绪");
                    set_text(details, &text);
                    visible(retry, true);
                }
            }
        }
        for message in fw_rx.try_iter() {
            firewall_active = false;
            enable(firewall, connection.is_some());
            set_text(note, &message);
        }
        for result in report_rx.try_iter() {
            exporting = false;
            match result {
                Ok(path) => notice(&format!("测试报告已保存到文稿文件夹：\r\n{path}")),
                Err(error) => notice(&format!("无法导出测试报告：{error}")),
            }
        }
        let inputs = INPUT.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for input in inputs {
            match input {
                Input::RestoreTray => tray.restore_icon(),
                Input::Close(w) if w == main && tray.available => {
                    if pending.is_some() {
                        desktop_menu::show(consent);
                    } else {
                        visible(main, false);
                    }
                }
                Input::Close(w) if w == main => break 'events,
                Input::Button(desktop_menu::EXIT) => break 'events,
                Input::Button(desktop_menu::REPORT) if !exporting => {
                    if let Ok(dir) = service::profile_dir(&options) {
                        exporting = true;
                        let tx = report_tx.clone();
                        thread::spawn(move || {
                            let _ = tx.send(export_report(&dir));
                        });
                    }
                }
                Input::Button(desktop_menu::PRIVACY) => {
                    if connection.is_some() {
                        let _ = commands.send(Action::TogglePrivacy);
                    } else {
                        notice("请先等待主机就绪。");
                    }
                }
                Input::Button(desktop_menu::RESTORE) => {
                    if connection.is_some() {
                        notice("请先结束隐私模式。串流服务停止后可在本机恢复显示。");
                    } else if unsafe {
                        MessageBoxW(
                            main,
                            wide("继续会将 Windows 桌面显示在实体屏幕上。确认你正在本机恢复显示？")
                                .as_ptr(),
                            wide("本机恢复显示").as_ptr(),
                            MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION,
                        )
                    } == IDYES
                    {
                        if let Err(e) = service::profile_dir(&options)
                            .and_then(|d| super::privacy::request_local_restore(&d))
                        {
                            notice(&e);
                        }
                    }
                }
                Input::Button(desktop_menu::VDD) => {
                    if let Err(e) = super::privacy_driver::launch(main) {
                        notice(&e);
                    }
                }
                Input::Button(desktop_menu::OPEN) => {
                    desktop_menu::show(if pending.is_some() { consent } else { main })
                }
                Input::Button(desktop_menu::DEVICES) => {
                    if connection.is_some() && pending.is_none() {
                        let _ = commands.send(Action::ListDevices);
                    } else {
                        notice("请先等待主机就绪并处理当前的连接请求。");
                    }
                }
                Input::Close(_) | Input::Button(NO) => {
                    if let Some(id) = pending.take() {
                        let _ = commands.send(Action::Answer(id, false));
                        visible(consent, false);
                    }
                }
                Input::Button(YES) => {
                    if let Some(id) = pending.take() {
                        let _ = commands.send(Action::Answer(id, true));
                        visible(consent, false);
                        set_text(phase, "正在确认配对…");
                    }
                }
                Input::Button(RETRY) => {
                    visible(retry, false);
                    enable(firewall, false);
                    connection = None;
                    set_text(phase, "正在启动串流引擎…");
                    set_text(details, "正在检查端口与本机身份。");
                    commands = start(options.clone(), event_tx.clone());
                }
                Input::Button(FIREWALL) if !firewall_active => {
                    if let Some((engine, base)) = connection.clone() {
                        firewall_active = true;
                        enable(firewall, false);
                        let tx = fw_tx.clone();
                        thread::spawn(move || {
                            let status = firewall_setup(&engine, base)
                                .unwrap_or_else(|e| format!("专用网络设置失败：{e}"));
                            let _ = tx.send(status);
                        });
                    }
                }
                Input::Button(LICENSES) => {
                    let local = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.join("OpenSourceNotices.txt")));
                    let portable = std::env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|d| d.join("NOTICE.txt")));
                    if let Some(file) = [local, portable]
                        .into_iter()
                        .flatten()
                        .find(|p| p.is_file())
                    {
                        let _ = std::process::Command::new("notepad.exe").arg(file).spawn();
                    } else {
                        notice("FlashLink 自研代码使用 MIT 许可，Sunshine 使用 GPLv3。详见项目发行包内的开源许可。");
                    }
                }
                Input::Button(LOG) => {
                    if let Ok(dir) = service::profile_dir(&options) {
                        let log = dir.join("flashlink-sunshine.log");
                        if log.is_file() {
                            let _ = std::process::Command::new("notepad.exe").arg(log).spawn();
                        } else {
                            notice("本次启动尚未生成 Sunshine 日志。请查看窗口内的错误信息。");
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let _ = commands.send(Action::Stop);
    drop(tray);
    unsafe {
        DestroyWindow(consent);
        DestroyWindow(main);
    }
    Ok(())
}

fn export_report(directory: &Path) -> Result<String, String> {
    let script = include_str!("../assets/Export-FlashLinkReport.ps1");
    let command = format!(
        "& {{ {script}\n }} -StateDir $env:FLASHLINK_REPORT_STATE -HostExecutable $env:FLASHLINK_REPORT_EXE"
    );
    let root = std::env::var_os("SystemRoot").ok_or("找不到 Windows 系统目录")?;
    let result = std::process::Command::new(
        PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
    )
    .args(["-NoProfile", "-NonInteractive", "-Command", &command])
    .env("FLASHLINK_REPORT_STATE", directory)
    .env(
        "FLASHLINK_REPORT_EXE",
        std::env::current_exe().map_err(|e| e.to_string())?,
    )
    .creation_flags(0x08000000)
    .output()
    .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err("请检查文稿文件夹写入权限及 PowerShell 的执行状态。".into());
    }
    Ok(String::from_utf8_lossy(&result.stdout).trim().to_owned())
}

fn firewall_setup(engine: &Path, base: u16) -> Result<String, String> {
    let gui = std::env::current_exe().map_err(|e| e.to_string())?;
    // Pass paths as quoted PowerShell literals (apostrophes doubled). Only the firewall
    // helper elevates. The GUI and desktop streaming engine retain the user's token.
    let literal = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
    let script = include_str!("../assets/firewall.ps1")
        .replace("__GUI__", &literal(&gui))
        .replace("__ENGINE__", &literal(engine))
        .replace("__BASE__", &base.to_string());
    let encoded = STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    let shell = PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let launch=format!("try {{ $p=Start-Process -FilePath {} -Verb RunAs -ArgumentList '-NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -EncodedCommand {}' -Wait -PassThru -ErrorAction Stop; exit $p.ExitCode }} catch {{ exit 5 }}",literal(&shell),encoded);
    let result = std::process::Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-Command", &launch])
        .creation_flags(0x08000000)
        .status()
        .map_err(|e| e.to_string())?;
    Ok(match result.code() {
        Some(0) => "专用网络规则已就绪。在手机或平板刷新“我的设备”。",
        Some(2) => {
            "规则已保存。当前没有专用网络连接，请在可信 Wi-Fi 的 Windows 网络属性中选择“专用”。"
        }
        Some(5) => "未获得管理员授权。已保留原防火墙设置。",
        Some(3) => "Windows 存在显式阻止规则，请在 Windows 安全中心核对此应用的专用网络权限。",
        _ => "无法添加规则。请检查 Windows Defender 防火墙或网络管理员策略。",
    }
    .into())
}
