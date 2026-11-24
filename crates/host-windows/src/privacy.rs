//! The guard runs outside Sunshine's kill-on-close job. Its stdin is a private, inherited pipe;
//! closing the host drops the pipe, even when the GUI is killed before doing normal cleanup.
use super::{
    privacy_display::{self, Target, Topology},
    privacy_time::{State, Timer},
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
        Threading::{
            OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
            PROCESS_TERMINATE,
        },
    },
};

pub const ENGINE_EXIT: u32 = 0xF104;
pub const MARKER: &str = "privacy-restore.pending";
pub const DEFERRED_MARKER: &str = "privacy-database-restore.pending";
const MANUAL: &str = "privacy-local-restore.request";

#[derive(Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Journal {
    transaction: String,
    targets: Vec<Target>,
}
impl Journal {
    fn read_from(dir: &Path, name: &str) -> Result<Self, String> {
        serde_json::from_slice(&fs::read(dir.join(name)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
    fn read(dir: &Path) -> Result<Self, String> {
        Self::read_from(dir, MARKER)
    }
    fn save_at(&self, dir: &Path, name: &str) -> Result<(), String> {
        let temp = dir.join(format!("privacy-{}-{name}.tmp", self.transaction));
        let _ = fs::remove_file(&temp);
        let mut file = File::create_new(&temp).map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        let dest = dir.join(name);
        if dest.exists() {
            fs::remove_file(&dest).map_err(|e| e.to_string())?;
        }
        fs::rename(&temp, &dest).map_err(|e| e.to_string())
    }
    fn save(&self, dir: &Path) -> Result<(), String> {
        self.save_at(dir, MARKER)
    }
    fn restored(&self) -> bool {
        Topology::active()
            .and_then(|s| s.targets())
            .is_ok_and(|s| s == self.targets)
    }
    fn clear(&self, dir: &Path) -> Result<(), String> {
        fs::remove_file(dir.join(MARKER)).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(dir.join(MANUAL));
        if Self::read_from(dir, DEFERRED_MARKER).is_ok_and(|d| d.targets == self.targets) {
            let _ = fs::remove_file(dir.join(DEFERRED_MARKER));
        }
        record(dir, "restored");
        Ok(())
    }
    fn defer_database_restore(&self, dir: &Path) -> Result<(), String> {
        // Persist the deferred Set-A target layout BEFORE clearing MARKER so a crash
        // between file operations never loses the record needed to clean DB[Set A]
        // when the unplugged external monitor is reconnected later.
        self.save_at(dir, DEFERRED_MARKER)?;
        fs::remove_file(dir.join(MARKER)).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(dir.join(MANUAL));
        record(dir, "restored_deferred");
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
    let mut reported = false;
    loop {
        let manual = locked() == Some(false)
            && fs::read_to_string(dir.join(MANUAL)).is_ok_and(|s| s == journal.transaction);
        let log_dir = (!reported).then_some(dir);
        if !journal.restored() {
            if let Some(topo) = original {
                let _ = topo.restore(log_dir);
            }
            if !journal.restored() {
                let _ = privacy_display::restore_targets(log_dir, &journal.targets);
            }
            if !journal.restored() && !Topology::active().is_ok_and(|s| s.has_physical_output()) {
                let _ = privacy_display::restore_database(log_dir);
            }
        }
        if journal.restored() {
            if reported { privacy_display::log_restore_recovered(dir, manual); }
            return journal.clear(dir);
        }
        if manual && Topology::active().is_ok_and(|s| s.has_physical_output()) {
            if reported { privacy_display::log_restore_recovered(dir, true); }
            return journal.defer_database_restore(dir);
        }
        if !reported {
            record(dir, "restore_pending");
            reported = true;
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn restore_while_connected(
    dir: &Path, journal: &Journal, original: &Topology, icons: &crate::desktop_icons::DesktopIcons,
) -> Result<bool, String> {
    let deadline = Instant::now() + Duration::from_secs(14);
    // On multi-monitor hosts, hand off through primary-only first so Sunshine drops MttVDD
    // (or any transient secondary screen during HDMI wake-up) and locks onto the original
    // primary display before secondary displays are re-attached.
    if original.path_count() > 1 {
        let handoff_deadline = Instant::now() + Duration::from_secs(9);
        let mut primary_stable_since: Option<Instant> = None;
        let mut next_write = Instant::now();
        while Instant::now() < handoff_deadline && locked() == Some(false) {
            if Topology::active().is_ok_and(|now| original.matches_primary_only(&now)) {
                let since = *primary_stable_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(1800) {
                    break;
                }
            } else {
                primary_stable_since = None;
                if Instant::now() >= next_write {
                    if original.restore_primary_only(Some(dir)).is_ok() {
                        // Allow HDMI/DP link training to complete without spamming mode sets.
                        next_write = Instant::now() + Duration::from_millis(1500);
                    } else {
                        let _ = privacy_display::restore_targets(Some(dir), &journal.targets);
                        next_write = Instant::now() + Duration::from_millis(750);
                    }
                }
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    let mut next_write = Instant::now();
    while Instant::now() < deadline && locked() == Some(false) {
        if journal.restored() {
            thread::sleep(Duration::from_millis(500));
            icons.finish();
            journal.clear(dir)?;
            return Ok(true);
        }
        if Instant::now() >= next_write {
            if original.restore(Some(dir)).is_ok() {
                next_write = Instant::now() + Duration::from_millis(1500);
            } else {
                let _ = privacy_display::restore_targets(Some(dir), &journal.targets);
                if !journal.restored() && !Topology::active().is_ok_and(|s| s.has_physical_output())
                {
                    let _ = privacy_display::restore_database(Some(dir));
                }
                next_write = Instant::now() + Duration::from_millis(750);
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Ok(false)
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

enum Control {
    Beat(Instant),
    End,
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
    let icons = crate::desktop_icons::DesktopIcons::capture()?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            match line {
                Ok(s) if s == "beat" => {
                    if tx.send(Control::Beat(Instant::now())).is_err() {
                        break;
                    }
                }
                Ok(s) if s == "end" => {
                    let _ = tx.send(Control::End);
                    break;
                }
                _ => break,
            }
        }
    });
    // Never touch displays if the parent lost its paired stream while preparing this plan.
    let Control::Beat(mut latest) = rx
        .recv_timeout(Duration::from_secs(4))
        .map_err(|_| "串流已中断，请重新连接。".to_string())? else {
            return Err("已取消切换".into());
        };
    loop {
        match rx.try_recv() {
            Ok(Control::Beat(t)) => latest = t,
            Ok(Control::End) => return Err("已取消切换".into()),
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => return Err("主机已退出".into()),
        }
    }
    if latest.elapsed() >= super::privacy_time::STALE {
        return Err("连接中断，请重新连接后开启隐私。".into());
    }
    journal.save(dir)?;
    if let Err(e) = plan.activate(dir) {
        // A failed mode set may already have changed the active paths.
        if journal.restored() {
            journal.clear(dir)?;
            return Err(e);
        }
        if !restore_while_connected(dir, &journal, &original, &icons)? {
            stop_engine(&engine);
            restore(dir, &journal, Some(&original))?;
        }
        return Err(e);
    }
    let started = Instant::now();
    let mut timer = Timer::new(started);
    let mut last = State::Playing;
    let mut repair = super::privacy_time::DisplayRepair::default();
    let mut align_passes = [
        Some(started + Duration::from_millis(900)),
        Some(started + Duration::from_millis(2200)),
    ];
    let mut display_failed = true;
    if plan.still_private(dir) {
        display_failed = false;
        icons.align();
        record(dir, "private");
        report("PRIVATE");
        loop {
            let poll = if started.elapsed() < Duration::from_secs(35) {
                Duration::from_millis(100)
            } else {
                Duration::from_millis(250)
            };
            let received = rx.recv_timeout(poll);
            if matches!(received, Err(mpsc::RecvTimeoutError::Disconnected)) {
                record(dir, "guard_pipe_closed");
                break;
            }
            let now = Instant::now();
            if locked() != Some(false) {
                record(dir, "guard_desktop_unavailable");
                break;
            }
            if unsafe { WaitForSingleObject(engine.as_raw_handle(), 0) } == WAIT_OBJECT_0 {
                record(dir, "guard_engine_exited");
                break;
            }
            if matches!(received, Ok(Control::End)) {
                if latest.elapsed() < super::privacy_time::STALE {
                    if restore_while_connected(dir, &journal, &original, &icons)? {
                        report("VISIBLE");
                        return Ok(());
                    }
                }
                break;
            }
            if let Ok(Control::Beat(ts)) = received {
                latest = ts;
            }
            let state = timer.tick(
                now,
                matches!(received, Ok(Control::Beat(ts)) if now.duration_since(ts) < Duration::from_secs(1)),
            );
            if !plan.still_private(dir) {
                if state != State::Playing {
                    record(dir, "guard_display_without_lease");
                    break;
                }
                if !repair.can_repair(now) {
                    display_failed = true;
                    record(dir, "guard_display_timeout");
                    break;
                }
                match plan.reapply_if_known_displays(dir, &original, &journal.targets) {
                    Ok(true) => {
                        repair.reapplied(Instant::now());
                        record(dir, "display_reapplied");
                    }
                    Ok(false) => repair.verified(),
                    Err(reason) => {
                        display_failed = true;
                        privacy_display::log_repair_error(dir, &reason);
                        record(dir, "guard_display_changed");
                        break;
                    }
                }
                let t = Instant::now();
                align_passes = [
                    Some(t + Duration::from_millis(900)),
                    Some(t + Duration::from_millis(2200)),
                ];
            } else {
                repair.verified();
                for slot in &mut align_passes {
                    if slot.is_some_and(|after| now >= after) {
                        icons.align();
                        *slot = None;
                    }
                }
            }
            if state == State::Expired {
                record(dir, "guard_lease_expired");
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
        record(dir, "guard_display_changed");
    }
    if display_failed && latest.elapsed() < super::privacy_time::STALE {
        report("DISPLAY_UNSTABLE");
        if restore_while_connected(dir, &journal, &original, &icons)? {
            report("VISIBLE_FALLBACK");
            return Ok(());
        }
    }
    stop_engine(&engine);
    report("RECOVERING");
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
    pub fn finish(&mut self, dir: &Path, reason: &str) {
        if self.pipe.is_some() {
            record(dir, reason);
            self.pipe.take();
        }
    }
    pub fn end_by_user(&mut self, dir: &Path) {
        if let Some(mut pipe) = self.pipe.take() {
            record(dir, "guard_user_end");
            let _ = writeln!(pipe, "end");
        }
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

/// If a previous manual restore completed on a reduced monitor set (e.g. external monitor
/// unplugged), its full pre-privacy layout is retained in `DEFERRED_MARKER` because Windows
/// still stores virtual-only for the original monitor set (Set A). Once all original targets
/// become available again on an unlocked desktop, automatically restore and persist the original
/// layout back to the CCD database and clear the deferred marker.
///
/// Returns `true` when a `SetDisplayConfig` write was attempted so the caller can pace retries.
pub fn reconcile_deferred_restore(dir: &Path) -> bool {
    if !dir.join(DEFERRED_MARKER).try_exists().unwrap_or(false) {
        return false;
    }
    if dir.join(MARKER).try_exists().unwrap_or(false) || locked() != Some(false) {
        return false;
    }
    let Ok(deferred) = Journal::read_from(dir, DEFERRED_MARKER) else {
        return false;
    };
    if !privacy_display::all_targets_available(&deferred.targets) {
        return false;
    }
    if deferred.restored() {
        let _ = fs::remove_file(dir.join(DEFERRED_MARKER));
        record(dir, "deferred_database_restored");
        return false;
    }
    let _ = privacy_display::restore_targets(Some(dir), &deferred.targets);
    if deferred.restored() {
        let _ = fs::remove_file(dir.join(DEFERRED_MARKER));
        record(dir, "deferred_database_restored");
    }
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_restore_defers_original_set_until_reconnected_and_clears_active_marker() {
        let dir = std::env::temp_dir().join(format!(
            "flashlink-deferred-restore-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let legacy_json = br#"{"transaction":"abc123","targets":[{"identity":"a","x":-2560,"y":0,"width":2560,"height":1600,"rotation":1},{"identity":"b","x":0,"y":0,"width":3840,"height":2160,"rotation":1}]}"#;
        let journal: Journal = serde_json::from_slice(legacy_json).unwrap();
        journal.save(&dir).unwrap();
        fs::write(dir.join(MANUAL), &journal.transaction).unwrap();
        assert!(dir.join(MARKER).exists());

        journal.defer_database_restore(&dir).unwrap();
        assert!(!dir.join(MARKER).exists());
        assert!(!dir.join(MANUAL).exists());
        assert!(dir.join(DEFERRED_MARKER).exists());
        assert_eq!(Journal::read_from(&dir, DEFERRED_MARKER).unwrap(), journal);

        // A later full restore of the same target set also clears the deferred marker.
        journal.save(&dir).unwrap();
        journal.clear(&dir).unwrap();
        assert!(!dir.join(MARKER).exists());
        assert!(!dir.join(DEFERRED_MARKER).exists());

        let events = fs::read_to_string(dir.join("privacy-events.log")).unwrap();
        assert!(events.contains("restored_deferred"));
        assert!(events.contains("restored\n") || events.ends_with("restored\r\n") || events.ends_with("restored"));
        let _ = fs::remove_dir_all(&dir);
    }
}
