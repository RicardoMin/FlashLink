//! Minimal, strictly scoped Windows service (`FlashLinkVddHelper`) that enables
//! FlashLink's managed `Root\MttVDD` virtual display only while a privacy guard
//! holds an active named-pipe lease, and automatically disables it on release,
//! guard crash/disconnect, or system boot.
//!
//! Security boundary:
//! - Installed into admin-protected `Program Files\FlashLink Virtual Display 25.7.23`.
//! - Accepts zero paths, device IDs, or commands beyond the 4 fixed protocol verbs.
//! - Refuses to touch `Root\MttVDD` unless `VDDPATH` points to FlashLink's own directory.
//! - Pipe DACL restricts access to LocalSystem, Administrators, and Interactive Users,
//!   and rejects all remote clients (`PIPE_REJECT_REMOTE_CLIENTS`).

use super::{
    bundle, privacy_driver,
    privacy_vdd_protocol::{
        apply_service_update, format_request, format_response, is_helper_current, parse_request,
        parse_response, service_command_line, LeaseAction, LeaseTracker, Request, Response,
        StopOutcome, HELPER_EXE_NAME, HELPER_PROTOCOL_VERSION, MAX_LINE_BYTES, PIPE_NAME, PIPE_SDDL,
        SERVICE_DESCRIPTION, SERVICE_DISPLAY_NAME, SERVICE_NAME,
    },
};
use std::{
    ffi::c_void,
    io,
    mem::size_of,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    ptr,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    Foundation::{
        LocalFree, ERROR_BROKEN_PIPE, ERROR_CALL_NOT_IMPLEMENTED, ERROR_FILE_NOT_FOUND,
        ERROR_IO_PENDING, ERROR_NO_MORE_ITEMS, ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED,
        ERROR_SERVICE_ALREADY_RUNNING, ERROR_SERVICE_CANNOT_ACCEPT_CTRL,
        ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_NOT_ACTIVE, HANDLE, INVALID_HANDLE_VALUE,
        NO_ERROR, WAIT_OBJECT_0,
    },
    Security::{
        Authorization::{ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1},
        SECURITY_ATTRIBUTES,
    },
    Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE,
        FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_WRITE_DATA, OPEN_EXISTING,
        PIPE_ACCESS_DUPLEX, SYNCHRONIZE,
    },
    System::{
        Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, WaitNamedPipeW,
            PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
        },
        Services::*,
        Threading::{CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects},
        IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
    },
};

const CONFIGFLAG_DISABLED: u32 = 0x0000_0001;
const SERVICE_ALL_ACCESS_MASK: u32 = 0x000F_01FF;
const SC_MANAGER_ALL_ACCESS_MASK: u32 = 0x000F_003F;
const CLIENT_OPEN_ACCESS: u32 =
    FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE;

static STATUS_HANDLE: AtomicUsize = AtomicUsize::new(0);
static STOP_EVENT_RAW: AtomicUsize = AtomicUsize::new(0);
static STOPPING: AtomicBool = AtomicBool::new(false);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                CloseServiceHandle(self.0);
            }
        }
    }
}

fn is_dev_disabled(set: HDEVINFO, dev: &SP_DEVINFO_DATA) -> bool {
    let mut flags: u32 = 0;
    let mut size: u32 = 0;
    let ok = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            set,
            dev,
            SPDRP_CONFIGFLAGS,
            ptr::null_mut(),
            (&mut flags as *mut u32).cast(),
            size_of::<u32>() as u32,
            &mut size,
        )
    };
    ok != 0 && (flags & CONFIGFLAG_DISABLED != 0)
}

/// Enable or disable FlashLink's managed `Root\MttVDD` display device(s).
/// Returns `Ok(true)` if at least one managed `Root\MttVDD` device was found and
/// brought into the requested state; `Ok(false)` if FlashLink does not manage
/// MttVDD on this host or no `Root\MttVDD` device is installed.
pub fn set_mttvdd_enabled(enable: bool) -> io::Result<bool> {
    if !privacy_driver::uses_flashlink_directory() {
        return Ok(false);
    }
    unsafe {
        let set = DeviceSet(SetupDiGetClassDevsW(
            &GUID_DEVCLASS_DISPLAY,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT,
        ));
        if set.0 == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut found = false;
        for i in 0.. {
            let mut dev = SP_DEVINFO_DATA {
                cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInfo(set.0, i, &mut dev) == 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                    break;
                }
                return Err(err);
            }
            let mut ids = [0u16; 2048];
            let mut size = 0;
            if SetupDiGetDeviceRegistryPropertyW(
                set.0,
                &dev,
                SPDRP_HARDWAREID,
                ptr::null_mut(),
                ids.as_mut_ptr().cast(),
                size_of_val(&ids) as u32,
                &mut size,
            ) == 0
            {
                continue;
            }
            let matches = ids[..(size as usize / 2).min(ids.len())]
                .split(|&c| c == 0)
                .any(|part| String::from_utf16_lossy(part).eq_ignore_ascii_case(r"Root\MttVDD"));
            if !matches {
                continue;
            }
            found = true;
            let disabled = is_dev_disabled(set.0, &dev);
            if enable && !disabled || !enable && disabled {
                continue;
            }
            let params = SP_PROPCHANGE_PARAMS {
                ClassInstallHeader: SP_CLASSINSTALL_HEADER {
                    cbSize: size_of::<SP_CLASSINSTALL_HEADER>() as u32,
                    InstallFunction: DIF_PROPERTYCHANGE,
                },
                StateChange: if enable { DICS_ENABLE } else { DICS_DISABLE },
                Scope: DICS_FLAG_GLOBAL,
                HwProfile: 0,
            };
            if SetupDiSetClassInstallParamsW(
                set.0,
                &dev,
                &params.ClassInstallHeader,
                size_of::<SP_PROPCHANGE_PARAMS>() as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set.0, &dev) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(found)
    }
}

fn report_service_status(state: u32, exit_code: u32, wait_hint: u32) {
    let raw = STATUS_HANDLE.load(Ordering::Acquire);
    if raw == 0 {
        return;
    }
    let controls = if state == SERVICE_RUNNING {
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_SHUTDOWN
    } else {
        0
    };
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: state,
        dwControlsAccepted: controls,
        dwWin32ExitCode: exit_code,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    unsafe {
        SetServiceStatus(raw as SERVICE_STATUS_HANDLE, &status);
    }
}

unsafe extern "system" fn service_ctrl_handler(
    control: u32,
    _event_type: u32,
    _event_data: *mut c_void,
    _context: *mut c_void,
) -> u32 {
    match control {
        SERVICE_CONTROL_STOP | SERVICE_CONTROL_SHUTDOWN => {
            STOPPING.store(true, Ordering::Release);
            report_service_status(SERVICE_STOP_PENDING, NO_ERROR, 5000);
            let ev = STOP_EVENT_RAW.load(Ordering::Acquire);
            if ev != 0 {
                SetEvent(ev as HANDLE);
            }
            NO_ERROR
        }
        SERVICE_CONTROL_INTERROGATE => NO_ERROR,
        _ => ERROR_CALL_NOT_IMPLEMENTED,
    }
}

fn create_pipe_instance(first: bool) -> io::Result<OwnedHandle> {
    create_pipe_instance_named(PIPE_NAME, PIPE_SDDL, first)
}

fn create_pipe_instance_named(pipe_name: &str, sddl: &str, first: bool) -> io::Result<OwnedHandle> {
    let sddl = wide(sddl);
    let mut sd: *mut c_void = ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            ptr::null_mut(),
        )
    };
    if ok == 0 || sd.is_null() {
        return Err(io::Error::last_os_error());
    }
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let pipe_mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
    let name = wide(pipe_name);
    let handle =
        unsafe { CreateNamedPipeW(name.as_ptr(), open_mode, pipe_mode, 8, 512, 512, 1000, &sa) };
    let err = io::Error::last_os_error();
    unsafe {
        LocalFree(sd);
    }
    if handle == INVALID_HANDLE_VALUE {
        return Err(err);
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

fn create_event(manual_reset: bool) -> io::Result<OwnedHandle> {
    let h = unsafe { CreateEventW(ptr::null(), i32::from(manual_reset), 0, ptr::null()) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

fn wait_overlapped(
    pipe: HANDLE,
    ov: &OVERLAPPED,
    io_event: HANDLE,
    stop_event: HANDLE,
) -> io::Result<u32> {
    let handles = [io_event, stop_event];
    let waited = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, u32::MAX) };
    if waited == WAIT_OBJECT_0 {
        let mut transferred = 0u32;
        if unsafe { GetOverlappedResult(pipe, ov, &mut transferred, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(transferred)
    } else {
        unsafe {
            CancelIoEx(pipe, ov);
        }
        let mut transferred = 0u32;
        let _ = unsafe { GetOverlappedResult(pipe, ov, &mut transferred, 1) };
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "service stopping",
        ))
    }
}

fn read_line_overlapped(pipe: HANDLE, stop_event: HANDLE) -> io::Result<Option<String>> {
    let io_event = create_event(true)?;
    let mut line = Vec::with_capacity(32);
    let mut byte = [0u8; 1];
    loop {
        unsafe {
            ResetEvent(io_event.as_raw_handle());
        }
        let mut ov = OVERLAPPED::default();
        ov.hEvent = io_event.as_raw_handle();
        let mut read = 0u32;
        let ok = unsafe { ReadFile(pipe, byte.as_mut_ptr(), 1, &mut read, &mut ov) };
        let got = if ok != 0 {
            read
        } else {
            let err = io::Error::last_os_error();
            match err.raw_os_error().map(|c| c as u32) {
                Some(ERROR_IO_PENDING) => {
                    match wait_overlapped(pipe, &ov, io_event.as_raw_handle(), stop_event) {
                        Ok(n) => n,
                        Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => {
                            return Ok(None);
                        }
                        Err(e) => return Err(e),
                    }
                }
                Some(ERROR_BROKEN_PIPE) => return Ok(None),
                _ => return Err(err),
            }
        };
        if got == 0 {
            return Ok(None);
        }
        if byte[0] == b'\n' {
            return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
        }
        if line.len() < MAX_LINE_BYTES {
            line.push(byte[0]);
        }
    }
}

fn write_all_overlapped(pipe: HANDLE, data: &[u8], stop_event: HANDLE) -> io::Result<()> {
    let io_event = create_event(true)?;
    let mut offset = 0;
    while offset < data.len() {
        unsafe {
            ResetEvent(io_event.as_raw_handle());
        }
        let mut ov = OVERLAPPED::default();
        ov.hEvent = io_event.as_raw_handle();
        let mut written = 0u32;
        let chunk = &data[offset..];
        let ok = unsafe {
            WriteFile(
                pipe,
                chunk.as_ptr(),
                chunk.len() as u32,
                &mut written,
                &mut ov,
            )
        };
        let n = if ok != 0 {
            written
        } else {
            let err = io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
                wait_overlapped(pipe, &ov, io_event.as_raw_handle(), stop_event)?
            } else {
                return Err(err);
            }
        };
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "pipe write zero"));
        }
        offset += n as usize;
    }
    unsafe {
        FlushFileBuffers(pipe);
    }
    Ok(())
}

struct ServerLeaseGuard {
    tracker: Arc<Mutex<LeaseTracker>>,
    active: bool,
}

impl ServerLeaseGuard {
    fn release_now(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        self.active = false;
        let mut guard = self.tracker.lock().unwrap_or_else(|e| e.into_inner());
        if guard.release() == LeaseAction::Disable {
            set_mttvdd_enabled(false)?;
        }
        Ok(())
    }
}

impl Drop for ServerLeaseGuard {
    fn drop(&mut self) {
        let _ = self.release_now();
    }
}

fn handle_client(
    pipe: OwnedHandle,
    tracker: Arc<Mutex<LeaseTracker>>,
    stop_event: Arc<OwnedHandle>,
) {
    let raw = pipe.as_raw_handle();
    let stop_raw = stop_event.as_raw_handle();
    if let Ok(Some(line)) = read_line_overlapped(raw, stop_raw) {
        match parse_request(&line) {
            Some(Request::Status) => {
                let resp = if privacy_driver::uses_flashlink_directory()
                    && privacy_driver::is_mttvdd_present().unwrap_or(false)
                {
                    Response::Ready(HELPER_PROTOCOL_VERSION)
                } else {
                    Response::Unmanaged
                };
                let _ = write_all_overlapped(raw, format_response(&resp).as_bytes(), stop_raw);
            }
            Some(Request::Disable) => {
                let resp = {
                    let guard = tracker.lock().unwrap_or_else(|e| e.into_inner());
                    if !privacy_driver::uses_flashlink_directory() {
                        Response::Unmanaged
                    } else if !guard.can_force_disable() {
                        Response::Busy
                    } else {
                        match set_mttvdd_enabled(false) {
                            Ok(_) => Response::Ok,
                            Err(e) => Response::Err(e.to_string()),
                        }
                    }
                };
                let _ = write_all_overlapped(raw, format_response(&resp).as_bytes(), stop_raw);
            }
            Some(Request::Acquire) => {
                let acquired = {
                    let mut guard = tracker.lock().unwrap_or_else(|e| e.into_inner());
                    if !privacy_driver::uses_flashlink_directory() {
                        Err(Response::Unmanaged)
                    } else {
                        match guard.acquire() {
                            LeaseAction::Enable => match set_mttvdd_enabled(true) {
                                Ok(true) => Ok(()),
                                Ok(false) => {
                                    guard.release();
                                    Err(Response::Unmanaged)
                                }
                                Err(e) => {
                                    guard.release();
                                    Err(Response::Err(e.to_string()))
                                }
                            },
                            LeaseAction::Noop => Ok(()),
                            LeaseAction::Disable => unreachable!(),
                        }
                    }
                };
                match acquired {
                    Ok(()) => {
                        let mut lease_guard = ServerLeaseGuard {
                            tracker: Arc::clone(&tracker),
                            active: true,
                        };
                        if write_all_overlapped(
                            raw,
                            format_response(&Response::Ok).as_bytes(),
                            stop_raw,
                        )
                        .is_ok()
                        {
                            // Hold the lease until explicit RELEASE, client disconnect/crash, or service stop.
                            while !STOPPING.load(Ordering::Acquire) {
                                match read_line_overlapped(raw, stop_raw) {
                                    Ok(Some(next))
                                        if parse_request(&next) == Some(Request::Release) =>
                                    {
                                        let resp = match lease_guard.release_now() {
                                            Ok(()) => Response::Ok,
                                            Err(e) => Response::Err(e.to_string()),
                                        };
                                        let _ = write_all_overlapped(
                                            raw,
                                            format_response(&resp).as_bytes(),
                                            stop_raw,
                                        );
                                        break;
                                    }
                                    Ok(Some(_)) => {
                                        let _ = write_all_overlapped(
                                            raw,
                                            format_response(&Response::Err("bad command".into()))
                                                .as_bytes(),
                                            stop_raw,
                                        );
                                    }
                                    Ok(None) | Err(_) => break,
                                }
                            }
                        }
                    }
                    Err(resp) => {
                        let _ =
                            write_all_overlapped(raw, format_response(&resp).as_bytes(), stop_raw);
                    }
                }
            }
            Some(Request::Release) | None => {
                let _ = write_all_overlapped(
                    raw,
                    format_response(&Response::Err("bad command".into())).as_bytes(),
                    stop_raw,
                );
            }
        }
    }
    unsafe {
        DisconnectNamedPipe(raw);
    }
}

unsafe extern "system" fn service_main(_argc: u32, _argv: *mut *mut u16) {
    STOPPING.store(false, Ordering::Release);
    let name = wide(SERVICE_NAME);
    let handle =
        RegisterServiceCtrlHandlerExW(name.as_ptr(), Some(service_ctrl_handler), ptr::null());
    if handle.is_null() {
        return;
    }
    STATUS_HANDLE.store(handle as usize, Ordering::Release);
    report_service_status(SERVICE_START_PENDING, NO_ERROR, 5000);

    let Ok(stop_event) = create_event(true) else {
        report_service_status(SERVICE_STOPPED, 1, 0);
        return;
    };
    let stop_event = Arc::new(stop_event);
    STOP_EVENT_RAW.store(stop_event.as_raw_handle() as usize, Ordering::Release);

    // Boot/startup invariant: if FlashLink manages MttVDD, keep it disabled whenever
    // no privacy session holds a lease so a prior power loss or crash never leaves a
    // black screen at Windows logon.
    let _ = set_mttvdd_enabled(false);

    let tracker = Arc::new(Mutex::new(LeaseTracker::default()));
    let first_pipe = match create_pipe_instance(true) {
        Ok(p) => p,
        Err(e) => {
            STOP_EVENT_RAW.store(0, Ordering::Release);
            report_service_status(SERVICE_STOPPED, e.raw_os_error().unwrap_or(1) as u32, 0);
            return;
        }
    };
    report_service_status(SERVICE_RUNNING, NO_ERROR, 0);

    let mut next_pipe = Some(first_pipe);
    if let Ok(connect_ev) = create_event(true) {
        while !STOPPING.load(Ordering::Acquire) {
            let pipe = match next_pipe.take() {
                Some(p) => p,
                None => match create_pipe_instance(false) {
                    Ok(p) => p,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                },
            };
            ResetEvent(connect_ev.as_raw_handle());
            let mut ov = OVERLAPPED::default();
            ov.hEvent = connect_ev.as_raw_handle();
            let connected = if ConnectNamedPipe(pipe.as_raw_handle(), &mut ov) != 0 {
                true
            } else {
                match io::Error::last_os_error().raw_os_error().map(|c| c as u32) {
                    Some(ERROR_PIPE_CONNECTED) => true,
                    Some(ERROR_IO_PENDING) => wait_overlapped(
                        pipe.as_raw_handle(),
                        &ov,
                        connect_ev.as_raw_handle(),
                        stop_event.as_raw_handle(),
                    )
                    .is_ok(),
                    _ => false,
                }
            };
            if connected && !STOPPING.load(Ordering::Acquire) {
                let t = Arc::clone(&tracker);
                let stop_ev = Arc::clone(&stop_event);
                thread::spawn(move || handle_client(pipe, t, stop_ev));
            }
        }
    }

    // Ensure MttVDD is disabled when the service shuts down cleanly.
    let _lock = tracker.lock().unwrap_or_else(|e| e.into_inner());
    let _ = set_mttvdd_enabled(false);
    STOP_EVENT_RAW.store(0, Ordering::Release);
    report_service_status(SERVICE_STOPPED, NO_ERROR, 0);
}

pub fn run_service() -> io::Result<()> {
    let mut name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: name.as_mut_ptr(),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW {
            lpServiceName: ptr::null_mut(),
            lpServiceProc: None,
        },
    ];
    if unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wait_service_state(svc: SC_HANDLE, desired: u32, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut status = SERVICE_STATUS::default();
        if unsafe { QueryServiceStatus(svc, &mut status) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if status.dwCurrentState == desired {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "等待虚拟屏辅助服务状态切换超时",
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn stop_service_and_wait(svc: SC_HANDLE, timeout: Duration) -> io::Result<StopOutcome> {
    let mut status = SERVICE_STATUS::default();
    if unsafe { ControlService(svc, SERVICE_CONTROL_STOP, &mut status) } != 0 {
        wait_service_state(svc, SERVICE_STOPPED, timeout)?;
        return Ok(StopOutcome::Stopped);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error().map(|c| c as u32) {
        Some(ERROR_SERVICE_NOT_ACTIVE) => Ok(StopOutcome::NotActive),
        Some(ERROR_SERVICE_CANNOT_ACCEPT_CTRL) => {
            let mut current = SERVICE_STATUS::default();
            if unsafe { QueryServiceStatus(svc, &mut current) } == 0 {
                return Err(io::Error::last_os_error());
            }
            if current.dwCurrentState == SERVICE_STOPPED {
                return Ok(StopOutcome::NotActive);
            }
            if current.dwCurrentState == SERVICE_STOP_PENDING {
                wait_service_state(svc, SERVICE_STOPPED, timeout)?;
                return Ok(StopOutcome::Stopped);
            }
            Err(err)
        }
        _ => Err(err),
    }
}

fn start_service_and_wait(svc: SC_HANDLE, timeout: Duration) -> io::Result<()> {
    if unsafe { StartServiceW(svc, 0, ptr::null()) } == 0 {
        let err = io::Error::last_os_error();
        if err.raw_os_error() != Some(ERROR_SERVICE_ALREADY_RUNNING as i32) {
            return Err(err);
        }
    }
    wait_service_state(svc, SERVICE_RUNNING, timeout)
}

/// Elevated installer entry point: deploy `FlashLinkVddHelper.exe` into the
/// admin-protected `vdd_dir`, register/update the `FlashLinkVddHelper` service
/// (`SERVICE_AUTO_START`), and start it so `Root\MttVDD` stays disabled outside
/// privacy mode.
pub fn install_or_update(vdd_dir: &Path) -> io::Result<()> {
    let scm = ServiceHandle(unsafe {
        OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_ALL_ACCESS_MASK)
    });
    if scm.0.is_null() {
        return Err(io::Error::last_os_error());
    }
    let svc_name = wide(SERVICE_NAME);
    let existing = unsafe { OpenServiceW(scm.0, svc_name.as_ptr(), SERVICE_ALL_ACCESS_MASK) };
    let existing = if existing.is_null() {
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_SERVICE_DOES_NOT_EXIST as i32) {
            None
        } else {
            return Err(err);
        }
    } else {
        Some(ServiceHandle(existing))
    };

    let helper_exe = vdd_dir.join(HELPER_EXE_NAME);
    let existing_raw = existing.as_ref().map(|svc| svc.0);
    apply_service_update(
        existing_raw.is_some(),
        || {
            let svc = existing_raw.expect("existing service handle");
            stop_service_and_wait(svc, Duration::from_secs(10))
        },
        || bundle::write_pure_executable(&helper_exe),
        || {
            if let Some(svc) = existing_raw {
                let _ = start_service_and_wait(svc, Duration::from_secs(10));
            }
        },
    )?;

    let cmd = service_command_line(&helper_exe.to_string_lossy());
    let cmd_wide = wide(&cmd);
    let display_wide = wide(SERVICE_DISPLAY_NAME);
    let svc = if let Some(svc) = existing {
        if unsafe {
            ChangeServiceConfigW(
                svc.0,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                cmd_wide.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                display_wide.as_ptr(),
            )
        } == 0
        {
            let err = io::Error::last_os_error();
            let _ = start_service_and_wait(svc.0, Duration::from_secs(10));
            return Err(err);
        }
        svc
    } else {
        let created = unsafe {
            CreateServiceW(
                scm.0,
                svc_name.as_ptr(),
                display_wide.as_ptr(),
                SERVICE_ALL_ACCESS_MASK,
                SERVICE_WIN32_OWN_PROCESS,
                SERVICE_AUTO_START,
                SERVICE_ERROR_NORMAL,
                cmd_wide.as_ptr(),
                ptr::null(),
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
            )
        };
        if created.is_null() {
            return Err(io::Error::last_os_error());
        }
        ServiceHandle(created)
    };

    let mut desc_wide = wide(SERVICE_DESCRIPTION);
    let desc = SERVICE_DESCRIPTIONW {
        lpDescription: desc_wide.as_mut_ptr(),
    };
    let _ = unsafe {
        ChangeServiceConfig2W(
            svc.0,
            SERVICE_CONFIG_DESCRIPTION,
            (&desc as *const SERVICE_DESCRIPTIONW).cast(),
        )
    };

    start_service_and_wait(svc.0, Duration::from_secs(10))
}

fn open_client_pipe() -> io::Result<Option<OwnedHandle>> {
    open_client_pipe_named(PIPE_NAME)
}

fn open_client_pipe_named(pipe_name: &str) -> io::Result<Option<OwnedHandle>> {
    let name = wide(pipe_name);
    for _ in 0..4 {
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                CLIENT_OPEN_ACCESS,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            return Ok(Some(unsafe { OwnedHandle::from_raw_handle(handle) }));
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error().map(|c| c as u32) {
            Some(ERROR_FILE_NOT_FOUND) => return Ok(None),
            Some(ERROR_PIPE_BUSY) => unsafe {
                WaitNamedPipeW(name.as_ptr(), 1000);
            },
            _ => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "连接虚拟屏辅助服务超时",
    ))
}

fn write_sync(handle: HANDLE, data: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        let mut written = 0u32;
        let chunk = &data[offset..];
        if unsafe {
            WriteFile(
                handle,
                chunk.as_ptr(),
                chunk.len() as u32,
                &mut written,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if written == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "pipe write zero"));
        }
        offset += written as usize;
    }
    Ok(())
}

fn read_line_sync(handle: HANDLE) -> io::Result<String> {
    let mut line = Vec::with_capacity(32);
    let mut byte = [0u8; 1];
    loop {
        let mut read = 0u32;
        if unsafe { ReadFile(handle, byte.as_mut_ptr(), 1, &mut read, ptr::null_mut()) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed"));
        }
        if byte[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        if line.len() < MAX_LINE_BYTES {
            line.push(byte[0]);
        }
    }
}

/// Active lease on FlashLink's dynamically managed `Root\MttVDD` device.
/// While this handle remains open, `Root\MttVDD` stays enabled. Calling `release()`
/// or dropping it (including process crash/termination) disables `Root\MttVDD`.
pub struct VddLease {
    pipe: Option<OwnedHandle>,
}

impl VddLease {
    /// Attempt to acquire a dynamic MttVDD lease from `FlashLinkVddHelper`.
    /// Returns `Ok(None)` when the helper service is not installed or does not
    /// manage MttVDD on this machine (allowing graceful fallback to legacy static MttVDD).
    pub fn try_acquire() -> Result<Option<Self>, String> {
        let Some(pipe) = open_client_pipe().map_err(|e| format!("连接虚拟屏辅助服务失败：{e}"))?
        else {
            return Ok(None);
        };
        write_sync(
            pipe.as_raw_handle(),
            format_request(Request::Acquire).as_bytes(),
        )
        .map_err(|e| format!("请求启用虚拟屏失败：{e}"))?;
        let line = read_line_sync(pipe.as_raw_handle())
            .map_err(|e| format!("等待虚拟屏启用结果失败：{e}"))?;
        match parse_response(&line) {
            Some(Response::Ok) => Ok(Some(Self { pipe: Some(pipe) })),
            Some(Response::Unmanaged) => Ok(None),
            Some(Response::Err(msg)) => Err(format!("动态启用虚拟屏失败：{msg}")),
            _ => Err("虚拟屏辅助服务返回了无效响应".into()),
        }
    }

    /// Explicitly release the lease and wait for `FlashLinkVddHelper` to finish
    /// disabling `Root\MttVDD` before restoring physical displays.
    pub fn release(mut self) -> io::Result<()> {
        self.release_inner()
    }

    fn release_inner(&mut self) -> io::Result<()> {
        let Some(pipe) = self.pipe.take() else {
            return Ok(());
        };
        let raw = pipe.as_raw_handle();
        write_sync(raw, format_request(Request::Release).as_bytes())?;
        let line = read_line_sync(raw)?;
        match parse_response(&line) {
            Some(Response::Ok) => Ok(()),
            Some(Response::Err(msg)) => Err(io::Error::other(msg)),
            _ => Err(io::Error::other("unexpected release response")),
        }
    }
}

impl Drop for VddLease {
    fn drop(&mut self) {
        let _ = self.release_inner();
    }
}

/// Query whether `FlashLinkVddHelper` is running at the current protocol/ACL
/// revision and actively managing FlashLink's `Root\MttVDD` installation.
pub fn is_ready() -> bool {
    is_ready_named(PIPE_NAME)
}

fn is_ready_named(pipe_name: &str) -> bool {
    let Ok(Some(pipe)) = open_client_pipe_named(pipe_name) else {
        return false;
    };
    let raw = pipe.as_raw_handle();
    if write_sync(raw, format_request(Request::Status).as_bytes()).is_err() {
        return false;
    }
    let resp = read_line_sync(raw)
        .ok()
        .and_then(|line| parse_response(&line));
    is_helper_current(resp.as_ref())
}

/// Best-effort recovery hook: if `FlashLinkVddHelper` is running and no active
/// lease is held, ensure `Root\MttVDD` is disabled.
pub fn ensure_disabled_if_managed() {
    if let Ok(Some(pipe)) = open_client_pipe() {
        let raw = pipe.as_raw_handle();
        if write_sync(raw, format_request(Request::Disable).as_bytes()).is_ok() {
            let _ = read_line_sync(raw);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::{
        Foundation::{GENERIC_ALL, GENERIC_WRITE},
        Security::{
            AclSizeInformation, CreateWellKnownSid, EqualSid, GetAce, GetAclInformation,
            GetSecurityDescriptorDacl, WinInteractiveSid, ACCESS_ALLOWED_ACE, ACL,
            ACL_SIZE_INFORMATION,
        },
        Storage::FileSystem::{FILE_CREATE_PIPE_INSTANCE, FILE_WRITE_ATTRIBUTES},
    };

    #[test]
    fn parsed_pipe_dacl_denies_create_pipe_instance_to_interactive_users() {
        assert_eq!(
            CLIENT_OPEN_ACCESS,
            crate::privacy_vdd_protocol::CLIENT_PIPE_ACCESS
        );
        assert_eq!(CLIENT_OPEN_ACCESS & FILE_CREATE_PIPE_INSTANCE, 0);

        let sddl = wide(PIPE_SDDL);
        let mut sd: *mut c_void = ptr::null_mut();
        assert_ne!(
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut sd,
                    ptr::null_mut(),
                )
            },
            0
        );
        let mut present = 0;
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut defaulted = 0;
        assert_ne!(
            unsafe { GetSecurityDescriptorDacl(sd, &mut present, &mut dacl, &mut defaulted) },
            0
        );
        assert_ne!(present, 0);
        assert!(!dacl.is_null());

        let mut info = ACL_SIZE_INFORMATION::default();
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );

        let mut iu_sid = [0u8; 68];
        let mut iu_sid_len = iu_sid.len() as u32;
        assert_ne!(
            unsafe {
                CreateWellKnownSid(
                    WinInteractiveSid,
                    ptr::null_mut(),
                    iu_sid.as_mut_ptr().cast(),
                    &mut iu_sid_len,
                )
            },
            0
        );

        let mut found_iu = false;
        for idx in 0..info.AceCount {
            let mut ace_ptr: *mut c_void = ptr::null_mut();
            assert_ne!(unsafe { GetAce(dacl, idx, &mut ace_ptr) }, 0);
            let ace = unsafe { &*(ace_ptr as *const ACCESS_ALLOWED_ACE) };
            let sid_ptr = (&ace.SidStart as *const u32).cast::<c_void>().cast_mut();
            if unsafe { EqualSid(sid_ptr, iu_sid.as_mut_ptr().cast()) } != 0 {
                found_iu = true;
                assert_eq!(ace.Mask, CLIENT_OPEN_ACCESS);
                assert_eq!(ace.Mask & FILE_CREATE_PIPE_INSTANCE, 0);
                assert_eq!(ace.Mask & FILE_WRITE_ATTRIBUTES, 0);
                assert_eq!(ace.Mask & GENERIC_WRITE, 0);
                assert_eq!(ace.Mask & GENERIC_ALL, 0);
            }
        }
        unsafe {
            LocalFree(sd);
        }
        assert!(found_iu, "PIPE_SDDL must contain an Interactive Users ACE");
    }

    fn serve_one_status_reply(
        pipe_name: &str,
        sddl: &str,
        reply: String,
    ) -> thread::JoinHandle<()> {
        let server = create_pipe_instance_named(pipe_name, sddl, true).unwrap();
        let stop_event = create_event(true).unwrap();
        thread::spawn(move || {
            let raw = server.as_raw_handle();
            let stop_raw = stop_event.as_raw_handle();
            let connect_ev = create_event(true).unwrap();
            let mut ov = OVERLAPPED::default();
            ov.hEvent = connect_ev.as_raw_handle();
            let connected = unsafe { ConnectNamedPipe(raw, &mut ov) } != 0
                || match io::Error::last_os_error().raw_os_error().map(|c| c as u32) {
                    Some(ERROR_PIPE_CONNECTED) => true,
                    Some(ERROR_IO_PENDING) => {
                        wait_overlapped(raw, &ov, connect_ev.as_raw_handle(), stop_raw).is_ok()
                    }
                    _ => false,
                };
            assert!(connected);
            let req = read_line_overlapped(raw, stop_raw).unwrap().unwrap();
            assert_eq!(parse_request(&req), Some(Request::Status));
            write_all_overlapped(raw, reply.as_bytes(), stop_raw).unwrap();
            unsafe {
                DisconnectNamedPipe(raw);
            }
        })
    }

    #[test]
    fn client_pipe_access_supports_duplex_round_trip() {
        let pipe_name = format!(r"\\.\pipe\FlashLinkVddHelperAclTest-{}", std::process::id());
        let expected_wire = format_response(&Response::Ready(HELPER_PROTOCOL_VERSION));
        let handle = serve_one_status_reply(&pipe_name, PIPE_SDDL, expected_wire);

        let client = open_client_pipe_named(&pipe_name).unwrap().unwrap();
        write_sync(
            client.as_raw_handle(),
            format_request(Request::Status).as_bytes(),
        )
        .unwrap();
        let resp = read_line_sync(client.as_raw_handle()).unwrap();
        assert_eq!(
            parse_response(&resp),
            Some(Response::Ready(HELPER_PROTOCOL_VERSION))
        );
        handle.join().unwrap();
    }

    #[test]
    fn is_ready_rejects_legacy_unversioned_helper_and_accepts_current_helper() {
        let pid = std::process::id();
        let legacy_sddl = "D:(A;;GRGW;;;SY)(A;;GRGW;;;BA)(A;;GRGW;;;IU)";
        let legacy_pipe = format!(r"\\.\pipe\FlashLinkVddHelperLegacyTest-{pid}");
        let legacy_server = serve_one_status_reply(&legacy_pipe, legacy_sddl, "READY\n".into());
        assert!(!is_ready_named(&legacy_pipe));
        legacy_server.join().unwrap();

        let current_pipe = format!(r"\\.\pipe\FlashLinkVddHelperCurrentTest-{pid}");
        let current_wire = format_response(&Response::Ready(HELPER_PROTOCOL_VERSION));
        let current_server = serve_one_status_reply(&current_pipe, PIPE_SDDL, current_wire);
        assert!(is_ready_named(&current_pipe));
        current_server.join().unwrap();
    }
}
