use super::{
    service::{self, Action, Event},
    Options,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::os::windows::process::CommandExt;
use std::{
    cell::RefCell,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    ptr,
    sync::mpsc,
    thread,
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{
        CreateFontW, DeleteObject, GetSysColorBrush, COLOR_WINDOW, DEFAULT_CHARSET,
        DEFAULT_QUALITY, FW_NORMAL, FW_SEMIBOLD,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::*,
};

const RETRY: usize = 1001;
const FIREWALL: usize = 1002;
const LOG: usize = 1003;
const LICENSES: usize = 1006;
const YES: usize = 1004;
const NO: usize = 1005;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
enum Input {
    Button(usize),
    Close(HWND),
}
thread_local! {static INPUT:RefCell<Vec<Input>>=const{RefCell::new(Vec::new())};}
unsafe extern "system" fn wndproc(w: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CLOSE => {
            INPUT.with(|q| q.borrow_mut().push(Input::Close(w)));
            0
        }
        WM_COMMAND => {
            INPUT.with(|q| q.borrow_mut().push(Input::Button(wp & 0xffff)));
            0
        }
        _ => unsafe { DefWindowProcW(w, msg, wp, lp) },
    }
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
            style,
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
    let main = window_handle("FlashLink · Windows 主机", ptr::null_mut(), 625, 395);
    if main.is_null() {
        return Err(format!(
            "无法打开 Windows 窗口：{}",
            io::Error::last_os_error()
        ));
    }
    let normal = Font::new(16, false);
    let heading = Font::new(29, true);
    let title = Font::new(20, true);
    child(main, "STATIC", "FlashLink", 0, (24, 15, 555, 42), &heading);
    let phase = child(
        main,
        "STATIC",
        "正在启动串流引擎…",
        0,
        (24, 74, 565, 31),
        &title,
    );
    let details = child(
        main,
        "STATIC",
        "请保持桌面解锁，并让手机或平板连接同一局域网。",
        0,
        (24, 116, 560, 99),
        &normal,
    );
    let note = child(
        main,
        "STATIC",
        "远程试玩期间，这台电脑的物理屏幕仍会显示画面。",
        0,
        (24, 218, 563, 48),
        &normal,
    );
    let firewall = child(
        main,
        "BUTTON",
        "允许专用网络连接",
        FIREWALL,
        (24, 287, 167, 38),
        &normal,
    );
    let retry = child(
        main,
        "BUTTON",
        "重新启动",
        RETRY,
        (205, 287, 110, 38),
        &normal,
    );
    child(
        main,
        "BUTTON",
        "查看日志",
        LOG,
        (461, 287, 110, 38),
        &normal,
    );
    child(
        main,
        "BUTTON",
        "开源许可",
        LICENSES,
        (335, 287, 110, 38),
        &normal,
    );
    visible(retry, false);
    enable(firewall, false);
    let consent = window_handle("FlashLink · 连接请求", main, 485, 292);
    if consent.is_null() {
        return Err("无法创建配对窗口".into());
    }
    let device = child(consent, "STATIC", "", 0, (20, 14, 430, 35), &title);
    let identity = child(consent, "STATIC", "", 0, (20, 56, 430, 46), &normal);
    let code = child(consent, "STATIC", "", 0, (20, 110, 430, 44), &heading);
    child(consent, "BUTTON", "拒绝", NO, (195, 175, 100, 39), &normal);
    child(
        consent,
        "BUTTON",
        "允许连接",
        YES,
        (309, 175, 120, 39),
        &normal,
    );
    visible(main, true);
    let (event_tx, event_rx) = mpsc::channel();
    let mut commands = start(options.clone(), event_tx.clone());
    let mut pending = None;
    let mut connection = None;
    let mut firewall_active = false;
    let (fw_tx, fw_rx) = mpsc::channel::<String>();
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
                Event::Online { base, exe, address } => {
                    set_text(phase, "已就绪，等待手机或平板连接");
                    set_text(details,&format!("电脑地址   {address}:{base}\r\n在手机或平板的“附近的电脑”中选择这台电脑。首次连接时，两端会显示同一个确认码。"));
                    connection = Some((exe, base));
                    enable(firewall, true);
                }
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
        let inputs = INPUT.with(|q| std::mem::take(&mut *q.borrow_mut()));
        for input in inputs {
            match input {
                Input::Close(w) if w == main => break 'events,
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
    unsafe {
        DestroyWindow(consent);
        DestroyWindow(main);
    }
    Ok(())
}

fn firewall_setup(engine: &Path, base: u16) -> Result<String, String> {
    let gui = std::env::current_exe().map_err(|e| e.to_string())?;
    // Pass paths as quoted PowerShell literals (apostrophes doubled). Only the firewall
    // helper elevates. The GUI and desktop streaming engine retain the user's token.
    let literal = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "''"));
    let script = include_str!("firewall.ps1")
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
        Some(0) => "专用网络规则已就绪。在手机或平板刷新“附近的电脑”。",
        Some(2) => {
            "规则已保存。当前没有专用网络连接，请在可信 Wi-Fi 的 Windows 网络属性中选择“专用”。"
        }
        Some(5) => "未获得管理员授权。已保留原防火墙设置。",
        Some(3) => "Windows 存在显式阻止规则，请在 Windows 安全中心核对此应用的专用网络权限。",
        _ => "无法添加规则。请检查 Windows Defender 防火墙或网络管理员策略。",
    }
    .into())
}
