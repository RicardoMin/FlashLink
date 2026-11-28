#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(any(windows, test))]
mod audio;
#[cfg(any(windows, test))]
mod profile;
#[cfg(windows)]
mod windows_job;

#[cfg(windows)]
mod bundle;
#[cfg(windows)]
mod desktop_icons;
#[cfg(windows)]
mod gui;
#[cfg(windows)]
mod host_update;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod lan;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod lease;
#[cfg(any(windows, test))]
mod management;
#[cfg(windows)]
mod privacy;
#[cfg(windows)]
mod privacy_display;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod privacy_display_plan;
#[cfg(windows)]
mod privacy_driver;
#[cfg(any(windows, test))]
mod privacy_time;
#[cfg(any(windows, test))]
#[cfg_attr(not(windows), allow(dead_code))]
mod privacy_vdd_protocol;
#[cfg(windows)]
mod privacy_vdd_service;
#[cfg(windows)]
mod service;
use std::ffi::OsString;
#[cfg(any(test, windows))]
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;
#[cfg(windows)]
use std::{
    env,
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket},
    time::Duration,
};
const HELP: &str = "FlashLink.exe [--sunshine PATH] [--state-dir DIR] [--port PORT]

打开 Windows 主机窗口，允许同一局域网内的手机或平板连接这台电脑。
--sunshine PATH  指定 sunshine.exe
--state-dir DIR  指定 FlashLink 配对和日志目录
--port PORT      首选串流端口（默认 47989）";
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Debug, Default)]
struct Options {
    sunshine: Option<PathBuf>,
    state_dir: Option<PathBuf>,
    port: Option<u16>,
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let mut options = Options::default();
    while let Some(key) = args.next() {
        let key = key.to_str().ok_or("invalid argument")?;
        match key {
            "-h" | "--help" => return Ok(None),
            "--sunshine" | "--state-dir" | "--port" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("missing value for {key}"))?;
                if value.is_empty() {
                    return Err(format!("empty value for {key}"));
                }
                match key {
                    "--sunshine" => options.sunshine = Some(value.into()),
                    "--state-dir" => options.state_dir = Some(value.into()),
                    _ => {
                        let port = value
                            .to_str()
                            .and_then(|s| s.parse::<u16>().ok())
                            .filter(|port| (1032..=65514).contains(port))
                            .ok_or("--port must be between 1032 and 65514")?;
                        options.port = Some(port);
                    }
                }
            }
            _ => return Err(format!("unknown argument {key}; try --help")),
        }
    }
    Ok(Some(options))
}

#[cfg(windows)]
fn check_ports(base: u16) -> Result<(), String> {
    let message = |protocol: &str, port, e: io::Error| {
        format!(
            "cannot bind {protocol} port {port}: {e}. An existing Sunshine service may already be running. Use that existing host directly, or select a free base port."
        )
    };
    for port in [base - 8, base - 5, base, base + 1, base + 21] {
        TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).map_err(|e| message("TCP", port, e))?;
    }
    for port in [base + 9, base + 10, base + 11] {
        UdpSocket::bind((Ipv4Addr::UNSPECIFIED, port)).map_err(|e| message("UDP", port, e))?;
    }
    Ok(())
}

#[cfg(windows)]
fn local_connection(port: u16) -> io::Result<TcpStream> {
    TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        Duration::from_millis(250),
    )
}

#[cfg(windows)]
fn ready(base: u16) -> io::Result<bool> {
    let mut stream = local_connection(base)?;
    stream.set_read_timeout(Some(Duration::from_millis(300)))?;
    stream.set_write_timeout(Some(Duration::from_millis(300)))?;
    stream
        .write_all(b"GET /serverinfo HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut body = String::new();
    stream.take(32 * 1024).read_to_string(&mut body)?;
    Ok(
        body.contains(&format!("<hostname>{}</hostname>", profile::ENGINE_NAME))
            && local_connection(base + 1).is_ok(),
    )
}

#[cfg(windows)]
fn default_executable() -> Option<PathBuf> {
    let bundled = env::current_exe()
        .ok()?
        .parent()?
        .join("Sunshine")
        .join("sunshine.exe");
    if bundled.is_file() {
        return Some(bundled);
    }
    ["ProgramW6432", "ProgramFiles"]
        .iter()
        .filter_map(env::var_os)
        .map(|p| PathBuf::from(p).join("Sunshine").join("sunshine.exe"))
        .find(|p| p.is_file())
}

#[cfg(any(windows, test))]
fn check_recovery_files(engine_dir: &Path, program_data: Option<&Path>) -> Result<(), String> {
    // Sunshine calls its recovery code on startup even when display changes and NVIDIA's
    // global OpenGL setting are disabled. These paths do not honor the profile overrides.
    let display = engine_dir.join("config").join("display_device.state");
    let gpu = program_data.map(|dir| dir.join("Sunshine").join("nvprefs_undo.json"));
    for path in std::iter::once(display).chain(gpu) {
        if path
            .try_exists()
            .map_err(|e| format!("cannot check {}: {e}", path.display()))?
        {
            return Err(format!(
                "Sunshine already has recovery data at {}. Use that Sunshine instance to finish its session or restore its settings before starting FlashLink.",
                path.display()
            ));
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    #[cfg(windows)]
    {
        let mut args = env::args_os().skip(1);
        let op = args.next();
        if op.as_deref() == Some(std::ffi::OsStr::new("--vdd-service")) {
            return match privacy_vdd_service::run_service() {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            };
        }
        if op.as_deref() == Some(std::ffi::OsStr::new(privacy_driver::SETUP_ARGUMENT)) {
            if args.next().is_some() {
                return ExitCode::FAILURE;
            }
            // ExitCode::from(u8) cannot carry full Windows setup errors or restart-required (3010).
            std::process::exit(privacy_driver::setup_exit_code());
        }
        if op.as_deref() == Some(std::ffi::OsStr::new("--privacy-guard"))
            || op.as_deref() == Some(std::ffi::OsStr::new("--privacy-recover"))
        {
            let Some(dir) = args.next() else {
                return ExitCode::FAILURE;
            };
            let pid = if op.as_deref() == Some(std::ffi::OsStr::new("--privacy-guard")) {
                match args.next().and_then(|p| p.to_str()?.parse().ok()) {
                    Some(pid) => Some(pid),
                    None => return ExitCode::FAILURE,
                }
            } else {
                None
            };
            return match privacy::run(Path::new(&dir), pid) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    let _ = writeln!(io::stdout(), "ERROR {e}");
                    ExitCode::FAILURE
                }
            };
        }
    }
    let options = match parse_args(std::env::args_os().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            #[cfg(windows)]
            gui::notice(HELP);
            #[cfg(not(windows))]
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            #[cfg(windows)]
            gui::notice(&error);
            #[cfg(not(windows))]
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    #[cfg(windows)]
    match host_update::recover_if_interrupted().and_then(|recovering| {
        if recovering {
            Ok(true)
        } else {
            bundle::relaunch_installed()
        }
    }) {
        Ok(true) => return ExitCode::SUCCESS,
        Ok(false) => {}
        Err(error) => {
            gui::notice(&format!("准备 FlashLink 失败：{error}"));
            return ExitCode::FAILURE;
        }
    }
    #[cfg(windows)]
    if let Err(error) = gui::window(options) {
        gui::notice(&error);
        return ExitCode::FAILURE;
    }
    #[cfg(not(windows))]
    {
        let _ = options;
        eprintln!("请在 Windows 10/11 x64 电脑上启动被控端。");
        ExitCode::FAILURE
    }
    #[cfg(windows)]
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_upstream_recovery_files_to_the_instance_that_created_them() {
        let root = std::env::temp_dir().join(format!("flashlink-recovery-{}", std::process::id()));
        std::fs::create_dir_all(root.join("config")).unwrap();
        std::fs::create_dir_all(root.join("Sunshine")).unwrap();
        assert!(check_recovery_files(&root, Some(&root)).is_ok());
        for file in ["config/display_device.state", "Sunshine/nvprefs_undo.json"] {
            let path = root.join(file);
            std::fs::write(&path, b"original settings").unwrap();
            assert!(check_recovery_files(&root, Some(&root)).is_err());
            assert_eq!(std::fs::read(&path).unwrap(), b"original settings");
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn validates_every_port_offset_and_preserves_paths_with_spaces() {
        let args = [
            "--sunshine",
            "C:/Program Files/Sunshine/sunshine.exe",
            "--port",
            "65514",
        ];
        let opts = parse_args(args.map(OsString::from)).unwrap().unwrap();
        assert_eq!(opts.port, Some(65514));
        assert_eq!(opts.sunshine, Some(PathBuf::from(args[1])));
        for invalid in ["0", "1028", "65515", "x"] {
            assert!(parse_args(["--port", invalid].map(OsString::from)).is_err());
        }
    }
}
