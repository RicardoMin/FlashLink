//! The guard runs outside Sunshine's kill-on-close job. Its stdin is a private, inherited pipe;
//! closing the host drops the pipe, even when the GUI is killed before doing normal cleanup.
use super::{
    privacy_display::{self, Target, Topology},
    privacy_time::{recovery_step, RecoveryStep, State, Timer},
};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufRead, Write},
    mem::size_of,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    ptr,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::WAIT_OBJECT_0,
    System::{
        RemoteDesktop::*,
        Shutdown::LockWorkStation,
        Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        },
    },
};

pub const ENGINE_EXIT: u32 = 0xF104;
pub const MARKER: &str = "privacy-restore.pending";
const MANUAL: &str = "privacy-local-restore.request";

#[derive(serde::Serialize, serde::Deserialize)]
struct Journal {
    transaction: String,
    targets: Vec<Target>,
}
impl Journal {
    fn read(dir: &Path) -> Result<Self, String> {
        serde_json::from_slice(&fs::read(dir.join(MARKER)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
    fn save(&self, dir: &Path) -> Result<(), String> {
        let temp = dir.join(format!("privacy-{}.tmp", self.transaction));
        let mut file = File::create_new(&temp).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temp, dir.join(MARKER)).map_err(|e| e.to_string())
    }
    fn restored(&self) -> bool {
        Topology::active()
            .and_then(|s| s.targets())
            .is_ok_and(|s| s == self.targets)
    }
    fn clear(&self, dir: &Path) -> Result<(), String> {
        fs::remove_file(dir.join(MARKER)).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(dir.join(MANUAL));
        record(dir, "restored");
        Ok(())
    }
}

pub fn record(dir: &Path, event: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .append(true)
        .create(true)
        .open(dir.join("privacy-events.log"))
    {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let _ = writeln!(file, "{secs} {event}");
    }
}
pub fn locked() -> Option<bool> {
    unsafe {
        let mut session = 0;
        if ProcessIdToSessionId(std::process::id(), &mut session) == 0 {
            return None;
        }
        let (mut buffer, mut len) = (ptr::null_mut(), 0);
        if WTSQuerySessionInformationW(
            ptr::null_mut(),
            session,
            WTSSessionInfoEx,
            &mut buffer,
            &mut len,
        ) == 0
        {
            return None;
        }
        let result = if len as usize >= size_of::<WTSINFOEXW>() && !buffer.is_null() {
            let info = &*buffer.cast::<WTSINFOEXW>();
            if info.Level == 1 {
                let details = &info.Data.WTSInfoExLevel1;
                match (details.SessionId == session, details.SessionFlags as u32) {
                    (true, WTS_SESSIONSTATE_LOCK) => Some(true),
                    (true, WTS_SESSIONSTATE_UNLOCK) if details.SessionState == WTSActive => {
                        Some(false)
                    }
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        };
        WTSFreeMemory(buffer.cast());
        result
    }
}
fn restore(dir: &Path, journal: &Journal, original: Option<&Topology>) -> Result<(), String> {
    record(dir, "locking");
    let mut saw_lock = false;
    let mut reported = false;
    let mut logged_manual = false;
    loop {
        let state = locked();
        let manual = state == Some(false)
            && fs::read_to_string(dir.join(MANUAL)).is_ok_and(|s| s == journal.transaction);
        let step = recovery_step(state, saw_lock, manual);
        if step == RecoveryStep::Restore {
            if !saw_lock && !manual {
                record(dir, "locked");
                saw_lock = true;
            }
            // A changed cable or monitor may make the exact in-memory snapshot inapplicable.
            // Validate what Windows actually did. An explicit local recovery can accept the
            // current saved physical outputs after a cable change.
            let log_dir = (!reported || (manual && !logged_manual)).then_some(dir);
            if manual {
                logged_manual = true;
            }
            let _ = original.map(|topo| topo.restore(log_dir));
            if !journal.restored() {
                let _ = privacy_display::restore_database(log_dir);
            }
            if journal.restored()
                || (manual && Topology::active().is_ok_and(|s| s.has_physical_output()))
            {
                if reported {
                    privacy_display::log_restore_recovered(dir, manual);
                }
                return journal.clear(dir);
            }
            if !reported {
                record(dir, "restore_pending");
                reported = true;
            }
        } else if step == RecoveryStep::Lock {
            unsafe {
                LockWorkStation();
            }
        }
        // Some GPUs reject SetDisplayConfig on Winlogon's secure desktop. A subsequent user
        // unlock is respected; wait for a local restore action or the next independent lock.
        thread::sleep(Duration::from_secs(1));
    }
}
fn protect_engine(pid: u32) -> Result<OwnedHandle, String> {
    let raw = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
    if raw.is_null() {
        return Err(format!("无法监控串流进程：{}", io::Error::last_os_error()));
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
}
fn stop_engine(engine: &OwnedHandle) {
    unsafe {
        TerminateProcess(engine.as_raw_handle(), ENGINE_EXIT);
    }
}
fn report(s: &str) {
    let mut out = io::stdout().lock();
    let _ = writeln!(out, "{s}");
    let _ = out.flush();
}

pub fn run(dir: &Path, engine: Option<u32>) -> Result<(), String> {
    let guard_lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("privacy-guard.lock"))
        .map_err(|e| e.to_string())?;
    guard_lock
        .try_lock()
        .map_err(|_| "隐私恢复进程已在运行".to_string())?;
    let marker = dir.join(MARKER);
    if marker.try_exists().map_err(|e| e.to_string())? {
        let journal = Journal::read(dir)?;
        if journal.restored() {
            return journal.clear(dir);
        }
        return restore(dir, &journal, None);
    }
    let Some(pid) = engine else {
        return Ok(());
    };
    if locked() != Some(false) {
        return Err("请在当前已解锁的 Windows 桌面开启隐私。".into());
    }
    let engine = protect_engine(pid)?;
    let original = Topology::active()?;
    if !original.has_physical_output() {
        return Err("当前没有可保护的物理显示器。".into());
    }
    let mut id = [0u8; 16];
    getrandom::fill(&mut id).map_err(|e| e.to_string())?;
    let journal = Journal {
        transaction: id.iter().map(|b| format!("{b:02x}")).collect(),
        targets: original.targets()?,
    };
    let mut plan = Topology::virtual_only_plan(dir, &original)?;
    let (tx, rx) = mpsc::sync_channel(4);
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(s) if s == "beat" => {
                    if tx.try_send(Instant::now()).is_err() {
                        break;
                    }
                }
                _ => break,
            }
        }
    });
    // Never touch displays if the parent lost its paired stream while preparing this plan.
    let mut latest = rx
        .recv_timeout(Duration::from_secs(4))
        .map_err(|_| "串流已中断，请重新连接。".to_string())?;
    loop {
        match rx.try_recv() {
            Ok(t) => latest = t,
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => return Err("主机已退出".into()),
        }
    }
    if latest.elapsed() >= super::privacy_time::STALE {
        return Err("连接中断，请重新连接后开启隐私。".into());
    }
    journal.save(dir)?;
    if let Err(e) = plan.activate(dir) {
        // SetDisplayConfig can fail after a topology change. Only unlock recovery bookkeeping
        // without locking when we can positively verify that our original paths never changed.
        if journal.restored() {
            journal.clear(dir)?;
            return Err(e);
        }
        stop_engine(&engine);
        restore(dir, &journal, Some(&original))?;
        return Err(e);
    }
    let mut timer = Timer::new(Instant::now());
    let mut last = State::Playing;
    if plan.still_private() {
        record(dir, "private");
        report("PRIVATE");
        loop {
            let received = rx.recv_timeout(Duration::from_millis(250));
            if matches!(received, Err(mpsc::RecvTimeoutError::Disconnected)) {
                break;
            }
            let now = Instant::now();
            if !plan.still_private()
                || locked() != Some(false)
                || unsafe { WaitForSingleObject(engine.as_raw_handle(), 0) } == WAIT_OBJECT_0
            {
                break;
            }
            let state = timer.tick(
                now,
                received.is_ok_and(|ts| now.duration_since(ts) < Duration::from_secs(1)),
            );
            if state == State::Locking {
                break;
            }
            if state != last {
                record(
                    dir,
                    if state == State::Playing {
                        "resumed"
                    } else {
                        "lost"
                    },
                );
                report(if state == State::Playing {
                    "PRIVATE"
                } else {
                    "GRACE"
                });
                last = state;
            }
        }
    } else {
        plan.log_active_mismatch(dir);
    }
    stop_engine(&engine);
    report("LOCKING");
    restore(dir, &journal, Some(&original))?;
    report("RESTORED");
    Ok(())
}

pub struct Guard {
    pub owner: [u8; 32],
    pub last: Instant,
    pub messages: mpsc::Receiver<String>,
    child: Child,
    reader: Option<thread::JoinHandle<()>>,
    pipe: Option<ChildStdin>,
}
impl Guard {
    pub fn start(dir: &Path, pid: u32, owner: [u8; 32], beat: Instant) -> Result<Self, String> {
        let mut child = Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--privacy-guard")
            .arg(dir)
            .arg(pid.to_string())
            .creation_flags(0x08000000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut pipe = child.stdin.take().unwrap();
        writeln!(pipe, "beat").map_err(|e| e.to_string())?;
        let out = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        let reader = thread::spawn(move || {
            for s in io::BufReader::new(out).lines().map_while(Result::ok) {
                if tx.send(s).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            owner,
            last: beat,
            messages,
            reader: Some(reader),
            child,
            pipe: Some(pipe),
        })
    }
    pub fn beat(&mut self, ts: Instant) {
        if ts > self.last && ts.elapsed() < Duration::from_secs(2) {
            if let Some(pipe) = self.pipe.as_mut() {
                let _ = writeln!(pipe, "beat");
            }
            self.last = ts;
        }
    }
    pub fn finish(&mut self) {
        self.pipe.take();
    }
    pub fn exited(&mut self) -> bool {
        if self.child.try_wait().is_ok_and(|v| v.is_some()) {
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            true
        } else {
            false
        }
    }
}
/// Existing guards own the same lock across the display change and all restoration retries.
/// If that guard died, start one replacement and let the supervisor observe its exit status.
pub fn recover_if_abandoned(dir: &Path) -> Result<Option<Child>, String> {
    if !dir.join(MARKER).try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("privacy-guard.lock"))
        .map_err(|e| e.to_string())?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => return Ok(None),
        Err(fs::TryLockError::Error(e)) => return Err(e.to_string()),
    }
    if !dir.join(MARKER).try_exists().map_err(|e| e.to_string())? {
        return Ok(None);
    }
    let journal = Journal::read(dir).map_err(|e| format!("无法读取隐私恢复记录：{e}"))?;
    if journal.restored() {
        journal.clear(dir)?;
        return Ok(None);
    }
    drop(lock);
    Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
        .arg("--privacy-recover")
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x08000000)
        .spawn()
        .map(Some)
        .map_err(|e| e.to_string())
}

pub fn request_local_restore(dir: &Path) -> Result<(), String> {
    if locked() != Some(false) {
        return Err("请先在电脑本机解锁。".into());
    }
    let journal = Journal::read(dir).map_err(|_| "没有待恢复的隐私会话。".to_string())?;
    fs::write(dir.join(MANUAL), journal.transaction).map_err(|e| e.to_string())?;
    // The service supervisor owns recovery startup. A running guard sees this journal token
    // on its next poll; no parallel process should contend for its transaction.
    Ok(())
}
