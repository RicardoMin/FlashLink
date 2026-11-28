//! Checks run off the window thread. The service authorizes the final stop after rechecking streams.
use base64::{engine::general_purpose::STANDARD, Engine};
use flashlink_update::{
    windows_release, Asset, FEED_URL, MAX_FEED_BYTES, WINDOWS_BUILD, WINDOWS_VERSION,
};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, SystemTime},
};
use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::*};

const NO_WINDOW: u32 = 0x08000000;
const INSTALL_SCRIPT: &str = include_str!("../assets/install-update.ps1");

fn system_dir() -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        env::var_os("SystemRoot").ok_or_else(|| io::Error::other("找不到 Windows 系统目录"))?,
    )
    .join("System32"))
}

fn updates_dir() -> io::Result<PathBuf> {
    let dir = PathBuf::from(
        env::var_os("LOCALAPPDATA").ok_or_else(|| io::Error::other("找不到本机用户目录"))?,
    )
    .join("FlashLink")
    .join("Updates");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn check_due() -> bool {
    let Ok(dir) = updates_dir() else { return false };
    let stamp = dir.join("last-check");
    let recent = fs::metadata(&stamp)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|time| SystemTime::now().duration_since(time).ok())
        .is_some_and(|elapsed| elapsed < Duration::from_secs(86400));
    !recent && fs::write(stamp, []).is_ok()
}

fn fetch(url: &str, max: u64, output: &mut impl Write) -> io::Result<u64> {
    let mut child = Command::new(system_dir()?.join("curl.exe"))
        .args([
            "--disable",
            "--silent",
            "--fail",
            "--location",
            "--max-redirs",
            "6",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "10",
            "--max-time",
            if max <= MAX_FEED_BYTES as u64 {
                "25"
            } else {
                "900"
            },
            "--",
            url,
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .creation_flags(NO_WINDOW)
        .spawn()?;
    let mut reader = child.stdout.take().unwrap().take(max + 1);
    let transferred = io::copy(&mut reader, output);
    drop(reader);
    if !matches!(&transferred, Ok(n) if *n <= max) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(transferred
            .err()
            .unwrap_or_else(|| io::Error::other("文件超过清单声明的长度")));
    }
    if !child.wait()?.success() {
        return Err(io::Error::other(
            "连接 GitHub Releases 失败，请检查网络后重试。",
        ));
    }
    transferred
}

struct Offer {
    version: String,
    notes: String,
    asset: Asset,
    single: bool,
}

fn check() -> Result<Option<Offer>, String> {
    let mut bytes = Vec::new();
    fetch(FEED_URL, MAX_FEED_BYTES as u64, &mut bytes).map_err(|e| e.to_string())?;
    let release = windows_release(&bytes)?;
    if release.build <= WINDOWS_BUILD {
        return Ok(None);
    }
    let single = crate::bundle::single_file().map_err(|e| e.to_string())?;
    if !single
        && !env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|dir| dir.join("Sunshine/sunshine.exe")))
            .is_some_and(|p| p.is_file())
    {
        return Err("当前程序来自开发目录，请从官方发行页下载 Windows 正式包。".into());
    }
    Ok(Some(Offer {
        version: release.version,
        notes: release.notes,
        asset: if single { release.exe } else { release.zip },
        single,
    }))
}

struct Package {
    file: PathBuf,
    offer: Offer,
    handed_off: bool,
}
impl Drop for Package {
    fn drop(&mut self) {
        if !self.handed_off {
            let _ = fs::remove_file(&self.file);
        }
    }
}

fn download(offer: Offer) -> io::Result<Package> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    let path = updates_dir()?.join(format!("package-{:032x}.tmp", u128::from_le_bytes(random)));
    let mut file = fs::File::create_new(&path)?;
    let package = Package {
        file: path,
        offer,
        handed_off: false,
    };
    let asset = &package.offer.asset;
    let count = fetch(&asset.url, asset.size, &mut file)?;
    drop(file);
    let mut hasher = Sha256::new();
    io::copy(&mut fs::File::open(&package.file)?, &mut hasher)?;
    if count != asset.size || format!("{:x}", hasher.finalize()) != asset.sha256 {
        return Err(io::Error::other(
            "下载不完整或 SHA-256 校验失败，已取消更新。",
        ));
    }
    Ok(package)
}

enum Task {
    Checked(Result<Option<Offer>, String>),
    Downloaded(Result<Package, String>),
}

#[derive(PartialEq, Eq)]
enum Activity {
    Idle,
    Checking,
    Downloading,
}

pub struct Client {
    tx: mpsc::Sender<Task>,
    rx: mpsc::Receiver<Task>,
    activity: Activity,
    manual: bool,
    offer: Option<Offer>,
    package: Option<Package>,
    stop_pending: bool,
}

fn message(window: HWND, body: &str, style: MESSAGEBOX_STYLE) -> i32 {
    let text: Vec<_> = body.encode_utf16().chain([0]).collect();
    let title: Vec<_> = "FlashLink 更新".encode_utf16().chain([0]).collect();
    unsafe { MessageBoxW(window, text.as_ptr(), title.as_ptr(), style) }
}

impl Client {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let mut client = Self {
            tx,
            rx,
            activity: Activity::Idle,
            manual: false,
            offer: None,
            package: None,
            stop_pending: false,
        };
        if check_due() {
            client.check(false);
        }
        client
    }
    pub fn check(&mut self, manual: bool) {
        self.manual |= manual;
        if self.activity != Activity::Idle || self.offer.is_some() || self.package.is_some() {
            return;
        }
        self.activity = Activity::Checking;
        let tx = self.tx.clone();
        thread::spawn(move || {
            let _ = tx.send(Task::Checked(check()));
        });
    }

    pub fn status(&self) -> &'static str {
        if self.package.is_some() {
            "已下载，等待主机空闲"
        } else if self.offer.is_some() {
            "有新版本可用"
        } else {
            match self.activity {
                Activity::Idle => "",
                Activity::Checking => "正在检查更新…",
                Activity::Downloading => "正在下载更新…",
            }
        }
    }

    pub fn cancel_handoff(&mut self) {
        self.package = None;
        self.stop_pending = false;
    }
    pub fn needs_idle(&self) -> bool {
        self.offer.is_some() || self.package.is_some() || self.activity == Activity::Downloading
    }
    pub fn retry_handoff(&mut self) {
        self.stop_pending = false;
    }

    /// Return true once per idle transition to request an authoritative stop from the worker.
    pub fn poll(&mut self, window: HWND, idle: bool) -> bool {
        for task in self.rx.try_iter() {
            self.activity = Activity::Idle;
            match task {
                Task::Checked(Ok(offer)) => {
                    self.offer = offer;
                    if self.offer.is_none() && self.manual {
                        message(
                            window,
                            &format!("当前 Windows 版本 {WINDOWS_VERSION} 已是最新版。"),
                            MB_OK,
                        );
                    }
                }
                Task::Checked(Err(e)) if self.manual => {
                    message(window, &format!("检查更新失败：{e}"), MB_OK);
                }
                Task::Downloaded(Err(e)) => {
                    message(window, &format!("更新未完成：{e}"), MB_OK);
                }
                Task::Downloaded(Ok(package)) => {
                    self.package = Some(package);
                }
                _ => {}
            }
            if self.offer.is_none() {
                self.manual = false;
            }
        }
        if self.manual || (idle && unsafe { IsWindowVisible(window) != 0 && IsIconic(window) == 0 })
        {
            if let Some(offer) = self.offer.take() {
                self.manual = false;
                let body = format!("FlashLink {WINDOWS_VERSION} → {}\r\n\r\n{}\r\n\r\n现在下载并在主机空闲时重启更新？", offer.version, offer.notes);
                if message(window, &body, MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION) == IDYES {
                    self.activity = Activity::Downloading;
                    let tx = self.tx.clone();
                    thread::spawn(move || {
                        let _ =
                            tx.send(Task::Downloaded(download(offer).map_err(|e| e.to_string())));
                    });
                }
            }
        }
        if !idle {
            self.stop_pending = false;
        }
        if self.package.is_some() && idle && !self.stop_pending {
            self.stop_pending = true;
            return true;
        }
        false
    }

    pub fn launch(&mut self) -> io::Result<()> {
        let mut package = self
            .package
            .take()
            .ok_or_else(|| io::Error::other("更新包不可用"))?;
        self.stop_pending = false;
        installer(&env::current_exe()?)?
            .env("FLASHLINK_UPDATE_RECOVER", "0")
            .env("FLASHLINK_UPDATE_FILE", &package.file)
            .env("FLASHLINK_UPDATE_HASH", &package.offer.asset.sha256)
            .env(
                "FLASHLINK_UPDATE_SIZE",
                package.offer.asset.size.to_string(),
            )
            .env(
                "FLASHLINK_UPDATE_SINGLE",
                if package.offer.single { "1" } else { "0" },
            )
            .spawn()?;
        package.handed_off = true;
        Ok(())
    }
}

fn installer(target: &Path) -> io::Result<Command> {
    let encoded = STANDARD.encode(
        INSTALL_SCRIPT
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    );
    let args = env::args_os()
        .skip(1)
        .map(|a| quote_arg(&a.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ");
    let mut command = Command::new(system_dir()?.join("WindowsPowerShell/v1.0/powershell.exe"));
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-EncodedCommand",
            &encoded,
        ])
        .env_remove("PSModulePath")
        .env_remove("FLASHLINK_UPDATE_BOOT")
        .env("FLASHLINK_UPDATE_PARENT", std::process::id().to_string())
        .env("FLASHLINK_UPDATE_ARGS", args)
        .env("FLASHLINK_UPDATE_TARGET", target)
        .creation_flags(NO_WINDOW);
    Ok(command)
}

pub fn recover_if_interrupted() -> io::Result<bool> {
    let exe = env::current_exe()?;
    if !exe
        .with_file_name(".flashlink-update.pending")
        .try_exists()?
        || env::var_os("FLASHLINK_UPDATE_BOOT").is_some()
    {
        return Ok(false);
    }
    installer(&exe)?
        .env("FLASHLINK_UPDATE_RECOVER", "1")
        .spawn()?;
    Ok(true)
}

// CommandLineToArgvW quoting, including backslashes before a quote or the closing delimiter.
fn quote_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for c in arg.chars() {
        if c == '\\' {
            slashes += 1;
            continue;
        }
        let quote = c == '"';
        out.extend(std::iter::repeat_n(
            '\\',
            if quote { slashes * 2 + 1 } else { slashes },
        ));
        out.push(c);
        slashes = 0;
    }
    out.extend(std::iter::repeat_n('\\', slashes * 2));
    out.push('"');
    out
}

/// The install helper waits for this event before discarding its backups.
pub fn acknowledge_startup() {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE},
    };
    let Some(token) = env::var("FLASHLINK_UPDATE_BOOT").ok() else {
        return;
    };
    if token.len() != 32 || !token.bytes().all(|c| c.is_ascii_hexdigit()) {
        return;
    }
    let name: Vec<_> = format!("Local\\FlashLinkUpdate-{token}")
        .encode_utf16()
        .chain([0])
        .collect();
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !event.is_null() {
            SetEvent(event);
            CloseHandle(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_preserves_windows_quoting_and_helper_fits_process_command_line() {
        assert_eq!(quote_arg(r"C:\state folder\"), "\"C:\\state folder\\\\\"");
        assert_eq!(quote_arg("a\"b"), "\"a\\\"b\"");
        assert!(INSTALL_SCRIPT.encode_utf16().count().div_ceil(3) * 8 + 256 < 32767);
    }
}
