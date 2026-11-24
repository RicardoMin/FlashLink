use super::{
    check_ports, check_recovery_files, default_executable,
    lan::Nearby,
    management::{Api, Credentials},
    privacy_time::{desktop_state, Desktop},
    profile::Profile,
    ready,
    windows_job::KillOnClose,
    Options,
};
use std::{
    env, fs,
    net::UdpSocket,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc::{Receiver, RecvTimeoutError, Sender},
    time::{Duration, Instant},
};

pub enum Event {
    Paused(Desktop),
    Privacy(String),
    Devices(Result<Vec<super::management::PairedDevice>, String>),
    Status(String),
    Online {
        base: u16,
        exe: PathBuf,
        address: String,
    },
    Ask {
        id: String,
        name: String,
        address: String,
        code: String,
    },
    Dismiss,
    Failed(String),
}
pub enum Action {
    TogglePrivacy,
    ListDevices,
    Revoke(String),
    Answer(String, bool),
    Stop,
}

pub fn profile_dir(options: &Options) -> Result<PathBuf, String> {
    options
        .state_dir
        .clone()
        .or_else(|| {
            env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("FlashLink").join("DemoHost"))
        })
        .ok_or_else(|| "找不到当前 Windows 用户目录".into())
}

pub fn serve(
    options: &Options,
    ui: &Sender<Event>,
    commands: &Receiver<Action>,
) -> Result<(), String> {
    let engine = if let Some(path) = &options.sunshine {
        path.clone()
    } else {
        let _ = ui.send(Event::Status("正在准备内置串流引擎…".into()));
        super::bundle::prepare()
            .map_err(|e| format!("准备离线引擎失败：{e}"))?
            .or_else(default_executable)
            .ok_or("未找到 Sunshine。请使用完整的 FlashLink Windows 应用或解压完整便携包。")?
    };

    let engine = std::path::absolute(engine).map_err(|e| e.to_string())?;
    if !engine.is_file() {
        return Err(format!("找不到串流引擎 {}", engine.display()));
    }
    let dir = std::path::absolute(profile_dir(options)?).map_err(|e| e.to_string())?;
    loop {
        if !wait_for_desktop(&dir, ui, commands)? {
            return Ok(());
        }
        let _ = ui.send(Event::Status("正在启动并验证串流引擎…".into()));
        match run_engine(options, &engine, &dir, ui, commands)? {
            Exit::Stop => return Ok(()),
            Exit::DesktopChanged => {}
        }
    }
}

enum Exit {
    Stop,
    DesktopChanged,
}

fn wait_for_desktop(
    dir: &Path,
    ui: &Sender<Event>,
    commands: &Receiver<Action>,
) -> Result<bool, String> {
    let mut last = None;
    let mut recovery: Option<Child> = None;
    loop {
        let pending = dir
            .join(super::privacy::MARKER)
            .try_exists()
            .map_err(|e| e.to_string())?;
        let state = desktop_state(pending, super::privacy::locked());
        if state == Desktop::Ready {
            return Ok(true);
        }
        if last != Some(state) {
            let _ = ui.send(Event::Paused(state));
            last = Some(state);
        }
        if pending {
            if let Some(child) = recovery.as_mut() {
                if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                    if !status.success() && dir.join(super::privacy::MARKER).exists() {
                        return Err("隐私恢复进程意外结束，请在本机检查显示后重新启动。".into());
                    }
                    recovery = None;
                }
            } else {
                recovery = super::privacy::recover_if_abandoned(dir)?;
            }
        }
        if matches!(
            commands.recv_timeout(Duration::from_millis(600)),
            Ok(Action::Stop) | Err(RecvTimeoutError::Disconnected)
        ) {
            return Ok(false);
        }
    }
}

/// Finish tearing down the old instance before reusing its ports and profile on unlock.
fn pause_engine(child: &mut Child) -> Result<Exit, String> {
    if child.try_wait().map_err(|e| e.to_string())?.is_none() {
        child.kill().map_err(|e| e.to_string())?;
        child.wait().map_err(|e| e.to_string())?;
    }
    Ok(Exit::DesktopChanged)
}

fn run_engine(
    options: &Options,
    engine: &Path,
    dir: &Path,
    ui: &Sender<Event>,
    commands: &Receiver<Action>,
) -> Result<Exit, String> {
    let preferred = options.port.unwrap_or(47989);
    let base = if options.port.is_some() {
        check_ports(preferred)?;
        preferred
    } else {
        [47989, 49000, 49100, 49200]
            .into_iter()
            .find(|p| check_ports(*p).is_ok())
            .ok_or("本机多个串流端口均已占用；请关闭重复启动的 FlashLink 或指定可用端口。")?
    };
    let engine_dir = engine.parent().ok_or("引擎目录无效")?;
    let program_data = env::var_os("ProgramData").map(PathBuf::from);
    check_recovery_files(engine_dir, program_data.as_deref())?;
    let mut profile = Profile::prepare(dir, base)
        .map_err(|e| format!("无法打开主机配置。请确认 FlashLink 只运行了一次。\r\n{e}"))?;
    let cert = dir.join("flashlink-cert.pem");
    let key = dir.join("flashlink-key.pem");
    let auth = dir.join("flashlink-gui-auth.json");
    let creds = Credentials::create(&auth)?;
    let name = env::var("COMPUTERNAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "FlashLink PC".into());
    profile
        .overrides
        .retain(|v| !v.starts_with("credentials_file="));
    profile
        .overrides
        .push(format!("credentials_file={}", auth.display()));
    let job = KillOnClose::new().map_err(|e| format!("创建进程管理器失败：{e}"))?;
    profile.remember_stream_log();
    let mut child = Command::new(engine)
        .current_dir(engine_dir)
        .arg(&profile.config)
        .args(&profile.overrides)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map_err(|e| format!("启动 Sunshine 失败：{e}"))?;
    if let Err(e) = job.attach(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("无法管理串流进程：{e}"));
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    let api = loop {
        if super::privacy::locked() != Some(false) {
            return pause_engine(&mut child);
        }
        if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!(
                "Sunshine 提前退出（{exit}）。\r\n{}",
                log_note(&profile.log)
            ));
        }
        if matches!(
            commands.recv_timeout(Duration::from_millis(210)),
            Ok(Action::Stop) | Err(RecvTimeoutError::Disconnected)
        ) {
            return Ok(Exit::Stop);
        }
        if ready(base).unwrap_or(false) {
            // Authenticate against the private TLS certificate and freshly generated manager
            // credentials before advertising the host or calling it ready.
            let api = Api::new(base, &cert, creds)?;
            api.pending().map_err(|e| {
                format!(
                    "引擎已监听，但本机管理验证失败：{e}\r\n{}",
                    log_note(&profile.log)
                )
            })?;
            break api;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "60 秒内未能连接本机 Sunshine。\r\n{}",
                log_note(&profile.log)
            ));
        }
    };
    let mut nearby = Nearby::start(base, &name, &cert, &key).map_err(|e| {
        format!("无法开启附近发现或安全配对。请关闭另一个 FlashLink 后重试。\r\n{e}")
    })?;
    let _ = ui.send(Event::Online {
        base,
        exe: engine.to_owned(),
        address: lan_address().unwrap_or_else(|| "等待局域网连接".into()),
    });
    let mut active: Option<(String, super::lan::Approval, Instant)> = None;
    let mut next_poll = Instant::now();
    let mut failures = 0;
    let mut guard: Option<super::privacy::Guard> = None;
    loop {
        if let Some(g) = guard.as_mut() {
            if let Some(beat) = nearby.heartbeat(&g.owner) {
                g.beat(beat);
            }
            let exited = g.exited();
            let mut ending = false;
            for message in g.messages.try_iter() {
                if matches!(message.as_str(), "RECOVERING" | "RESTORED") {
                    ending = true;
                }
                let text = match message.as_str() {
                    "PRIVATE" => "隐私已开启，只有虚拟屏幕处于活动状态。",
                    "VISIBLE" => "已退出隐私，原显示布局已恢复，串流继续。",
                    "GRACE" => "连接中断，正在等待原设备重新连接…",
                    "DISPLAY_UNSTABLE" => "显示配置不稳定，正在退出隐私并恢复实体屏。",
                    "VISIBLE_FALLBACK" => "已自动退出隐私，实体屏已恢复，串流继续。",
                    "RECOVERING" => "串流已结束，正在恢复实体屏。",
                    "RESTORED" => "已恢复原显示布局。",
                    msg => msg.strip_prefix("ERROR ").unwrap_or(msg),
                };
                let _ = ui.send(Event::Privacy(text.into()));
            }
            let pending = dir
                .join(super::privacy::MARKER)
                .try_exists()
                .map_err(|e| e.to_string())?;
            if ending
                || (pending && (exited || child.try_wait().map_err(|e| e.to_string())?.is_some()))
            {
                let _ = ui.send(Event::Paused(Desktop::Recovering));
                return pause_engine(&mut child);
            }
            if exited {
                guard = None;
            }
        }
        if let Some(exit) = child.try_wait().map_err(|e| e.to_string())? {
            // The guard marks its own process exit. This also covers the race where the child
            // exits after try_wait above but before RECOVERING arrives on the guard's stdout pipe.
            if exit.code() == Some(super::privacy::ENGINE_EXIT as i32)
                || (guard.is_some() && dir.join(super::privacy::MARKER).exists())
            {
                let _ = ui.send(Event::Paused(Desktop::Recovering));
                return Ok(Exit::DesktopChanged);
            }
            return Err(format!(
                "串流引擎已退出（{exit}）。\r\n{}",
                log_note(&profile.log)
            ));
        }
        match commands.recv_timeout(Duration::from_millis(150)) {
            Ok(Action::TogglePrivacy) => {
                if let Some(g) = guard.as_mut() {
                    if super::lease::sole_owner(dir).ok() == Some(g.owner) {
                        g.end_by_user(dir);
                    } else {
                        g.finish(dir, "guard_owner_unavailable");
                    }
                } else {
                    let result = (|| {
                        if active.is_some() {
                            return Err("请先处理待授权请求。".into());
                        }
                        let owner = super::lease::sole_owner(dir)?;
                        let beat = nearby
                            .heartbeat(&owner)
                            .filter(|ts| ts.elapsed() < super::privacy_time::STALE)
                            .ok_or("请先用已配对的最新版安卓 FlashLink 打开桌面，再开启隐私。")?;
                        super::privacy::Guard::start(dir, child.id(), owner, beat)
                    })();
                    match result {
                        Ok(g) => {
                            guard = Some(g);
                            let _ = ui.send(Event::Privacy("正在检查虚拟屏并切换显示…".into()));
                        }
                        Err(e) => {
                            let _ = ui.send(Event::Privacy(e));
                        }
                    }
                }
            }
            Ok(Action::ListDevices) => {
                let _ = ui.send(Event::Devices(api.devices()));
            }
            Ok(Action::Revoke(uuid)) => {
                let status = match api.revoke(&uuid) {
                    Ok(()) => "已移除设备，下次连接需要重新授权。".into(),
                    Err(error) => format!("移除设备时出错：{error}"),
                };
                let _ = ui.send(Event::Status(status));
            }
            Ok(Action::Stop) | Err(RecvTimeoutError::Disconnected) => return Ok(Exit::Stop),
            Ok(Action::Answer(id, allow))
                if active
                    .as_ref()
                    .is_some_and(|(current, _, _)| current == &id) =>
            {
                let (_, approval, deadline) = active.take().unwrap();
                let _ = ui.send(Event::Dismiss);
                let valid = Instant::now() < deadline;
                let result = api.respond(
                    &id,
                    (allow && valid).then_some((approval.pin.as_str(), approval.name.as_str())),
                );
                let status = match result {
                    Ok(true) if allow && valid => "设备已配对，可以开始串流。".into(),
                    Ok(_) if !allow || !valid => "连接请求已拒绝或过期。".into(),
                    Ok(false) => "配对未完成，请在手机或平板上重新连接。".into(),
                    Err(e) => format!("本机配对接口失败：{e}"),
                    _ => unreachable!(),
                };
                let _ = ui.send(Event::Status(status));
            }
            _ => {}
        }
        if Instant::now() < next_poll {
            continue;
        }
        next_poll = Instant::now() + Duration::from_millis(650);
        if super::privacy::locked() != Some(false) {
            if let Some(g) = guard.as_mut() {
                g.finish(dir, "guard_desktop_unavailable");
            }
            return pause_engine(&mut child);
        }
        if let Some(g) = guard.as_mut() {
            if super::lease::sole_owner(dir).ok() != Some(g.owner) {
                g.finish(dir, "guard_owner_unavailable");
            }
        }
        let pending = match api.pending() {
            Ok(p) => {
                failures = 0;
                p
            }
            Err(e) => {
                failures += 1;
                if failures >= 3 {
                    if guard.is_some() {
                        super::privacy::record(dir, "guard_management_unavailable");
                    }
                    return Err(format!("无法与本机引擎通信：{e}。请重新开始。"));
                }
                continue;
            }
        };
        if let Some((id, _, deadline)) = &active {
            let present = pending.iter().any(|p| p.id == *id);
            if !present || Instant::now() >= *deadline {
                if present {
                    let _ = api.respond(id, None);
                }
                active = None;
                let _ = ui.send(Event::Dismiss);
                let _ = ui.send(Event::Status("请求已结束，等待手机或平板连接。".into()));
            }
        }
        if active.is_none() {
            for p in pending {
                if guard.is_some() {
                    let _ = api.respond(&p.id, None);
                    continue;
                }
                if let Some(req) = nearby.take_request(p.address, &p.name) {
                    let _ = ui.send(Event::Ask {
                        id: p.id.clone(),
                        name: req.name.clone(),
                        address: p.address.to_string(),
                        code: req.code.clone(),
                    });
                    active = Some((p.id, req, Instant::now() + Duration::from_secs(45)));
                    break;
                }
            }
        }
    }
}

fn log_note(path: &std::path::Path) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut details = String::new();
    if let Ok(mut file) = fs::File::open(path) {
        let len = file.metadata().map(|s| s.len()).unwrap_or(0);
        let _ = file.seek(SeekFrom::Start(len.saturating_sub(8192)));
        let mut bytes = Vec::new();
        let _ = file.take(8192).read_to_end(&mut bytes);
        if let Some(line) = String::from_utf8_lossy(&bytes)
            .lines()
            .rev()
            .find(|line| line.contains("Fatal:") || line.contains("Error:"))
        {
            details = line.chars().take(230).collect();
        }
    }
    format!("{details}\r\n日志：{}", path.display())
}

fn lan_address() -> Option<String> {
    // Connecting a datagram socket selects the default route without sending a packet.
    let socket = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("192.0.2.1", 9)).ok()?;
    Some(socket.local_addr().ok()?.ip().to_string())
}
