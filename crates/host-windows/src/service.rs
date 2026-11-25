use super::{
    check_ports, check_recovery_files, default_executable,
    lan::Nearby,
    management::{Api, Credentials},
    privacy_driver::Readiness,
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
    sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError},
    time::{Duration, Instant},
};

pub enum Event {
    Paused(Desktop),
    Privacy(PrivacyControl, String),
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
    SetPrivacy(bool),
    ListDevices,
    Revoke(String),
    Answer(String, bool),
    Stop,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PrivacyControl {
    Enable,
    Disable,
    Busy,
}

// The setup worker can wait for Windows consent while discovery and pairing continue.
// Its result survives lock/unlock; an intent to enter privacy belongs to one engine session.
enum PrivacySetup {
    New,
    Checking(Receiver<Result<Readiness, String>>),
    Checked(Result<Readiness, String>),
}

impl PrivacySetup {
    fn start(&mut self) {
        if matches!(
            self,
            Self::Checking(_) | Self::Checked(Ok(Readiness::RestartRequired))
        ) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(super::privacy_driver::prepare(std::ptr::null_mut()));
        });
        *self = Self::Checking(rx);
    }

    fn poll(&mut self, request: &mut Option<[u8; 32]>, ui: &Sender<Event>) -> Option<[u8; 32]> {
        let Self::Checking(rx) = self else {
            return None;
        };
        let outcome = match rx.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) => Err("准备过程意外中断。".into()),
        };
        *self = Self::Checked(outcome);
        let owner = request.take();
        if owner.is_some() && matches!(self, Self::Checked(Ok(Readiness::Ready))) {
            return owner;
        }
        self.show(ui);
        None
    }

    fn show(&self, ui: &Sender<Event>) {
        use PrivacyControl::{Busy, Enable};
        let (control, message) = match self {
            Self::New | Self::Checking(_) => (
                Busy,
                "正在准备内置防窥功能。若 Windows 请求授权，请在电脑上确认。".into(),
            ),
            Self::Checked(Ok(Readiness::Ready)) => (
                Enable,
                "防窥模式已就绪，当前为普通模式。连接远程桌面后即可开启。".into(),
            ),
            Self::Checked(Ok(Readiness::RestartRequired)) => (
                Busy,
                "Windows 要求本次安装重启一次后生效，后续日常开关无需重启。当前仍可使用普通串流。"
                    .into(),
            ),
            Self::Checked(Err(e)) => (Enable, format!("点击“开启防窥模式”可重试。\r\n{e}")),
        };
        let _ = ui.send(Event::Privacy(control, message));
    }
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
    let mut setup = PrivacySetup::New;
    loop {
        if !wait_for_desktop(&dir, ui, commands)? {
            return Ok(());
        }
        let _ = ui.send(Event::Status("正在启动并验证串流引擎…".into()));
        match run_engine(options, &engine, &dir, ui, commands, &mut setup)? {
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
            super::privacy::reconcile_deferred_restore(dir);
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
    setup: &mut PrivacySetup,
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
    let profile = Profile::prepare(dir, base)
        .map_err(|e| format!("无法打开主机配置。请确认 FlashLink 只运行了一次。\r\n{e}"))?;
    if matches!(setup, PrivacySetup::New) {
        setup.start();
        setup.show(ui);
    }
    let cert = dir.join("flashlink-cert.pem");
    let key = dir.join("flashlink-key.pem");
    let creds = Credentials::create(&profile.auth)?;
    let name = env::var("COMPUTERNAME")
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "FlashLink PC".into());
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
    let nearby = Nearby::start(base, &name, &cert, &key).map_err(|e| {
        format!("无法开启附近发现或安全配对。请关闭另一个 FlashLink 后重试。\r\n{e}")
    })?;
    let _ = ui.send(Event::Online {
        base,
        exe: engine.to_owned(),
        address: lan_address().unwrap_or_else(|| "等待局域网连接".into()),
    });
    setup.show(ui);
    let mut active: Option<(String, super::lan::Approval, Instant)> = None;
    let mut audio = crate::audio::Playback::new(&nearby, ui);
    let mut next_poll = Instant::now();
    let mut next_deferred = Instant::now();
    let mut failures = 0;
    let mut guard: Option<super::privacy::Guard> = None;
    let mut guard_outcome = None;
    let mut requested_owner: Option<[u8; 32]> = None;
    loop {
        if let Some(owner) = setup.poll(&mut requested_owner, ui) {
            let result =
                privacy_owner(dir, &nearby, active.is_some()).and_then(|(current_owner, beat)| {
                    if current_owner != owner {
                        return Err("连接设备已变更。请重新开启防窥模式。".into());
                    }
                    super::privacy::Guard::start(dir, child.id(), owner, beat)
                });
            let event = match result {
                Ok(g) => {
                    guard = Some(g);
                    Event::Privacy(PrivacyControl::Busy, "正在切换到防窥模式…".into())
                }
                Err(e) => Event::Privacy(PrivacyControl::Enable, e),
            };
            let _ = ui.send(event);
        }
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
                let (control, text) = match message.as_str() {
                    "PRIVATE" => (
                        PrivacyControl::Disable,
                        "防窥模式已开启，只有远程设备显示桌面。",
                    ),
                    "GRACE" => (PrivacyControl::Disable, "连接中断，正在等待原设备重新连接…"),
                    "DISPLAY_UNSTABLE" => (
                        PrivacyControl::Busy,
                        "显示配置不稳定，正在退出防窥模式并恢复实体屏。",
                    ),
                    "RECOVERING" => (PrivacyControl::Busy, "串流已结束，正在恢复实体屏。"),
                    "RESTORED" => (PrivacyControl::Busy, "已恢复原显示布局。"),
                    msg => {
                        guard_outcome = Some(
                            match msg {
                                "VISIBLE" => "已关闭防窥模式，原显示布局已恢复，串流继续。",
                                "VISIBLE_FALLBACK" => {
                                    "已自动关闭防窥模式，实体屏已恢复，串流继续。"
                                }
                                msg => msg.strip_prefix("ERROR ").unwrap_or(msg),
                            }
                            .to_owned(),
                        );
                        continue;
                    }
                };
                let _ = ui.send(Event::Privacy(control, text.into()));
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
                let _ = ui.send(Event::Privacy(
                    PrivacyControl::Enable,
                    guard_outcome
                        .take()
                        .unwrap_or_else(|| "防窥会话已结束。".into()),
                ));
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
            Ok(Action::SetPrivacy(false)) => {
                if let Some(g) = guard.as_mut() {
                    let _ = ui.send(Event::Privacy(
                        PrivacyControl::Busy,
                        "正在恢复实体屏…".into(),
                    ));
                    if super::lease::sole_owner(dir).ok() == Some(g.owner) {
                        g.end_by_user(dir);
                    } else {
                        g.finish(dir, "guard_owner_unavailable");
                    }
                }
            }
            Ok(Action::SetPrivacy(true)) if guard.is_none() && requested_owner.is_none() => {
                if matches!(setup, PrivacySetup::Checked(Ok(Readiness::RestartRequired))) {
                    setup.show(ui);
                } else if super::privacy::locked() != Some(false) {
                    let _ = ui.send(Event::Privacy(
                        PrivacyControl::Enable,
                        "请先在本机解锁 Windows 后开启防窥模式。".into(),
                    ));
                } else {
                    let owner =
                        privacy_owner(dir, &nearby, active.is_some()).map(|(owner, _)| owner);
                    match (&*setup, owner) {
                        (PrivacySetup::Checked(Ok(Readiness::Ready)), Err(e)) => {
                            let _ = ui.send(Event::Privacy(PrivacyControl::Enable, e));
                        }
                        (_, owner) => {
                            requested_owner = owner.ok();
                            setup.start();
                            setup.show(ui);
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
        audio.poll();
        if let Some(owner) = requested_owner {
            if nearby
                .heartbeat(&owner)
                .is_none_or(|beat| beat.elapsed() >= super::privacy_time::STALE)
            {
                requested_owner = None;
                let _ = ui.send(Event::Privacy(
                    PrivacyControl::Busy,
                    "串流已中断，已取消本次开启。内置组件准备完成后可重新连接。".into(),
                ));
            }
        }
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
        } else if Instant::now() >= next_deferred && super::privacy::reconcile_deferred_restore(dir)
        {
            next_deferred = Instant::now() + Duration::from_millis(1500);
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

fn privacy_owner(
    dir: &Path,
    nearby: &Nearby,
    pairing: bool,
) -> Result<([u8; 32], Instant), String> {
    if super::privacy::locked() != Some(false) {
        return Err("请在当前已解锁的 Windows 桌面开启防窥模式。".into());
    }
    if pairing {
        return Err("请先处理待授权请求。".into());
    }
    if super::privacy::reconcile_deferred_restore(dir) {
        return Err("正在恢复原显示器布局，请稍后再开启防窥模式。".into());
    }
    let owner = super::lease::sole_owner(dir)?;
    let beat = nearby
        .heartbeat(&owner)
        .filter(|ts| ts.elapsed() < super::privacy_time::STALE)
        .ok_or("请先用已配对的安卓 FlashLink 打开桌面，再开启防窥模式。")?;
    Ok((owner, beat))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preparation_never_activates_without_a_current_request() {
        let (ui, events) = mpsc::channel();
        let (done, task) = mpsc::channel();
        let mut setup = PrivacySetup::Checking(task);
        // A second check while Windows is prompting must use the first task.
        setup.start();
        let mut old_request = Some([1; 32]);
        assert_eq!(setup.poll(&mut old_request, &ui), None);
        assert!(old_request.is_some());
        // Both startup and a new engine after lock/unlock have no pending enable request.
        done.send(Ok(Readiness::Ready)).unwrap();
        assert_eq!(setup.poll(&mut None, &ui), None);
        assert!(matches!(
            events.try_recv(),
            Ok(Event::Privacy(PrivacyControl::Enable, _))
        ));
        assert_eq!(setup.poll(&mut old_request, &ui), None);
    }

    #[test]
    fn consent_and_restart_results_release_the_pending_request() {
        let (ui, events) = mpsc::channel();
        let owner = [2; 32];
        for (outcome, can_retry) in [
            (Err("授权已取消".into()), true),
            (Ok(Readiness::RestartRequired), false),
        ] {
            let (done, task) = mpsc::channel();
            let mut setup = PrivacySetup::Checking(task);
            let mut request = Some(owner);
            done.send(outcome).unwrap();
            assert_eq!(setup.poll(&mut request, &ui), None);
            assert_eq!(request, None);
            let Event::Privacy(control, text) = events.recv().unwrap() else {
                panic!("missing privacy state")
            };
            assert_eq!(control == PrivacyControl::Enable, can_retry);
            assert!(text.contains(if can_retry { "重试" } else { "重启" }));
            if !can_retry {
                assert!(text.contains("无需重启"));
            }
        }
        let (done, task) = mpsc::channel();
        let mut retry = PrivacySetup::Checking(task);
        let mut request = Some(owner);
        done.send(Ok(Readiness::Ready)).unwrap();
        assert_eq!(retry.poll(&mut request, &ui), Some(owner));
        assert_eq!(retry.poll(&mut request, &ui), None);
    }
}
