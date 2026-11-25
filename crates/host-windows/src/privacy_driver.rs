//! Install only the four pinned files from the upstream signed amd64 MttVDD release. The
//! elevated entry point accepts no paths, commands or driver packages from the caller.
use std::{
    fs,
    io::{self, Write},
    mem::size_of,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    ptr,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    Foundation::{
        ERROR_CANCELLED, ERROR_FILE_NOT_FOUND, ERROR_GEN_FAILURE, ERROR_NO_MORE_ITEMS,
        ERROR_PATH_NOT_FOUND, ERROR_SUCCESS_REBOOT_REQUIRED, HWND, WAIT_OBJECT_0,
    },
    System::{
        Com::{
            CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_APARTMENTTHREADED,
            COINIT_DISABLE_OLE1DDE,
        },
        Registry::*,
        Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE},
    },
    UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    RestartRequired,
}

pub const SETUP_ARGUMENT: &str = "--prepare-privacy-display";

fn launch(window: HWND) -> io::Result<Readiness> {
    let exe = std::env::current_exe()?;
    let path: Vec<_> = exe.as_os_str().encode_wide().chain([0]).collect();
    let verb = wide("runas");
    let parameter = wide(SETUP_ARGUMENT);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as _,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        hwnd: window,
        lpVerb: verb.as_ptr(),
        lpFile: path.as_ptr(),
        lpParameters: parameter.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };
    unsafe {
        let hr = CoInitializeEx(
            ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as _,
        );
        if hr < 0 {
            return Err(io::Error::other(format!(
                "无法初始化 Windows 授权窗口 (0x{:08X})",
                hr as u32
            )));
        }
        let result = ShellExecuteExW(&mut info);
        let error = io::Error::last_os_error();
        CoUninitialize();
        if result == 0 {
            return Err(error);
        }
        if info.hProcess.is_null() {
            return Err(io::Error::other("无法获取 Windows 初始化进程"));
        }
        let process = OwnedHandle::from_raw_handle(info.hProcess);
        let mut code = 0;
        if WaitForSingleObject(process.as_raw_handle(), INFINITE) != WAIT_OBJECT_0
            || GetExitCodeProcess(process.as_raw_handle(), &mut code) == 0
        {
            return Err(io::Error::last_os_error());
        }
        match code {
            0 => Ok(Readiness::Ready),
            ERROR_SUCCESS_REBOOT_REQUIRED => Ok(Readiness::RestartRequired),
            error => Err(io::Error::from_raw_os_error(error as i32)),
        }
    }
}

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
fn node() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as _,
        ..Default::default()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Device {
    Missing,
    Starting,
    Running,
    RestartRequired,
    Unconfigured,
    Problem(u32),
}

fn device_status(flags: u32, problem: u32) -> Device {
    if flags & DN_HAS_PROBLEM != 0 {
        return match problem {
            CM_PROB_NEED_RESTART => Device::RestartRequired,
            CM_PROB_NOT_CONFIGURED | CM_PROB_FAILED_INSTALL => Device::Unconfigured,
            code => Device::Problem(code),
        };
    }
    if flags & DN_STARTED != 0 {
        Device::Running
    } else {
        Device::Starting
    }
}

fn device() -> io::Result<Device> {
    unsafe {
        // An interrupted first setup can leave a root node without a display class.
        let set = DeviceSet(SetupDiGetClassDevsW(
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_ALLCLASSES,
        ));
        if set.0 == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut found = Device::Missing;
        for i in 0.. {
            let mut dev = node();
            if SetupDiEnumDeviceInfo(set.0, i, &mut dev) == 0 {
                let e = io::Error::last_os_error();
                return if e.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                    Ok(found)
                } else {
                    Err(e)
                };
            }
            let mut ids = [0u16; 2048];
            let mut size = 0;
            if SetupDiGetDeviceRegistryPropertyW(
                set.0,
                &dev,
                SPDRP_HARDWAREID,
                ptr::null_mut(),
                ids.as_mut_ptr().cast(),
                size_of_val(&ids) as _,
                &mut size,
            ) == 0
            {
                continue;
            }
            let matches_id = |expected: &str| {
                ids[..size as usize / 2]
                    .split(|v| *v == 0)
                    .any(|s| String::from_utf16_lossy(s).eq_ignore_ascii_case(expected))
            };
            let root_driver = matches_id(r"Root\MttVDD");
            // Both IDs occur in the bundled INF. Software-enumerated instances can
            // serve privacy too, but FlashLink only registers or repairs its root device.
            if root_driver || matches_id("MttVDD") {
                let (mut flags, mut problem) = (0, 0);
                let result = CM_Get_DevNode_Status(&mut flags, &mut problem, dev.DevInst, 0);
                if result == CR_NO_SUCH_DEVNODE {
                    continue;
                }
                if result != CR_SUCCESS {
                    return Err(io::Error::other(format!("读取显示设备状态失败 ({result})")));
                }
                found = device_status(flags, problem);
                if found == Device::Unconfigured && !root_driver {
                    found = Device::Problem(problem);
                }
                if found == Device::Running {
                    return Ok(found);
                }
            }
        }
        unreachable!()
    }
}

fn program_files() -> io::Result<PathBuf> {
    unsafe {
        let mut path = ptr::null_mut();
        if SHGetKnownFolderPath(&FOLDERID_ProgramFiles, 0, ptr::null_mut(), &mut path) < 0
            || path.is_null()
        {
            return Err(io::Error::other("无法定位 Program Files"));
        }
        let mut n = 0;
        while *path.add(n) != 0 {
            n += 1;
        }
        let owned = std::ffi::OsString::from_wide(std::slice::from_raw_parts(path, n));
        CoTaskMemFree(path.cast());
        Ok(owned.into())
    }
}
fn configure_directory(dir: &Path) -> io::Result<()> {
    let name = wide(r"SOFTWARE\MikeTheTech\VirtualDisplayDriver");
    let value = wide("VDDPATH");
    unsafe {
        let r = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            name.as_ptr(),
            value.as_ptr(),
            RRF_RT_ANY,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
        );
        if r == 0 {
            return Ok(());
        } // Preserve an existing installation's settings.
        if r != ERROR_FILE_NOT_FOUND {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
        let path: Vec<_> = dir.as_os_str().encode_wide().chain([0]).collect();
        let r = RegSetKeyValueW(
            HKEY_LOCAL_MACHINE,
            name.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            path.as_ptr().cast(),
            (path.len() * 2) as _,
        );
        if r != 0 {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
    }
    Ok(())
}
fn register(inf: &Path) -> io::Result<bool> {
    unsafe {
        let set = DeviceSet(SetupDiCreateDeviceInfoList(
            &GUID_DEVCLASS_DISPLAY,
            ptr::null_mut(),
        ));
        if set.0 == -1 {
            return Err(io::Error::last_os_error());
        }
        let mut dev = node();
        let hardware = wide("Root\\MttVDD\0");
        if SetupDiCreateDeviceInfoW(
            set.0,
            wide("Display").as_ptr(),
            &GUID_DEVCLASS_DISPLAY,
            ptr::null(),
            ptr::null_mut(),
            DICD_GENERATE_ID,
            &mut dev,
        ) == 0
            || SetupDiSetDeviceRegistryPropertyW(
                set.0,
                &mut dev,
                SPDRP_HARDWAREID,
                hardware.as_ptr().cast(),
                (hardware.len() * 2) as _,
            ) == 0
            || SetupDiCallClassInstaller(DIF_REGISTERDEVICE, set.0, &dev) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let result = bind(inf);
        if result.is_err() {
            SetupDiCallClassInstaller(DIF_REMOVE, set.0, &dev);
        }
        result
    }
}

fn bind(inf: &Path) -> io::Result<bool> {
    let path: Vec<_> = inf.as_os_str().encode_wide().chain([0]).collect();
    let mut reboot = 0;
    // Let normal Windows ranking, catalog verification and policy decide installation.
    if unsafe {
        UpdateDriverForPlugAndPlayDevicesW(
            ptr::null_mut(),
            wide(r"Root\MttVDD").as_ptr(),
            path.as_ptr(),
            0,
            &mut reboot,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(reboot != 0)
}

const DEFAULT_SETTINGS: &str = include_str!("../assets/mttvdd/vdd_settings.xml");

// Only the elevated helper writes the protected settings file.
fn settings_with_modes(settings: &str, modes: &[(u32, u32)]) -> io::Result<String> {
    let (prefix, suffix) = settings
        .split_once("</resolutions>")
        .ok_or_else(|| io::Error::other("虚拟屏配置缺少 resolutions 节点"))?;
    let mut result = prefix.to_owned();
    let mut known = std::collections::BTreeSet::new();
    for block in prefix.split("<resolution>").skip(1) {
        let number = |start, end| {
            block
                .split_once(start)?
                .1
                .split_once(end)?
                .0
                .trim()
                .parse::<u32>()
                .ok()
        };
        if let (Some(w), Some(h)) = (
            number("<width>", "</width>"),
            number("<height>", "</height>"),
        ) {
            known.insert((w, h));
        }
    }
    for &(width, height) in modes {
        if width > 0 && height > 0 && known.insert((width, height)) {
            result.push_str(&format!("<resolution><width>{width}</width><height>{height}</height><refresh_rate>60</refresh_rate></resolution>\n    "));
        }
    }
    result.push_str("</resolutions>");
    result.push_str(suffix);
    Ok(result)
}

fn uses_directory(dir: &Path) -> bool {
    let mut value = [0u16; 1024];
    let mut bytes = size_of_val(&value) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            wide(r"SOFTWARE\MikeTheTech\VirtualDisplayDriver").as_ptr(),
            wide("VDDPATH").as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            value.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    let len = value.iter().position(|&c| c == 0).unwrap_or(value.len());
    status == 0 && PathBuf::from(std::ffi::OsString::from_wide(&value[..len])) == dir
}

fn pending_settings(dir: &Path) -> io::Result<Option<String>> {
    let existing = match fs::read_to_string(dir.join("vdd_settings.xml")) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let sizes = super::privacy_display::Topology::active()
        .map_err(io::Error::other)?
        .physical_desktop_sizes();
    let settings = settings_with_modes(existing.as_deref().unwrap_or(DEFAULT_SETTINGS), &sizes)?;
    Ok((existing.as_ref() != Some(&settings)).then_some(settings))
}

// A killed helper must not leave a half-written INF/DLL or settings file for the next run.
fn write_component(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    let temporary = path.with_file_name(format!("setup-{:x}.tmp", u64::from_ne_bytes(nonce)));
    let mut file = fs::File::create_new(&temporary)?;
    let write = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = write.and_then(|_| fs::rename(&temporary, path)) {
        let _ = fs::remove_file(temporary);
        return Err(e);
    }
    Ok(())
}

// Settings edits do not always put the PnP node into CM_PROB_NEED_RESTART. A volatile
// machine key keeps that result across host relaunches and expires when Windows reboots.
const RESTART_KEY: &str = r"SOFTWARE\MikeTheTech\VirtualDisplayDriver\FlashLinkPendingRestart";
fn restart_pending() -> io::Result<bool> {
    let mut key = ptr::null_mut();
    unsafe {
        match RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(RESTART_KEY).as_ptr(),
            0,
            KEY_QUERY_VALUE,
            &mut key,
        ) {
            0 => {
                RegCloseKey(key);
                Ok(true)
            }
            ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => Ok(false),
            code => Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
}

fn mark_restart() -> io::Result<Readiness> {
    let mut key = ptr::null_mut();
    unsafe {
        let code = RegCreateKeyExW(
            HKEY_LOCAL_MACHINE,
            wide(RESTART_KEY).as_ptr(),
            0,
            ptr::null(),
            REG_OPTION_VOLATILE,
            KEY_QUERY_VALUE,
            ptr::null(),
            &mut key,
            ptr::null_mut(),
        );
        if code != 0 {
            return Err(io::Error::from_raw_os_error(code as i32));
        }
        RegCloseKey(key);
    }
    Ok(Readiness::RestartRequired)
}

fn usable(state: Device) -> io::Result<Option<Readiness>> {
    match state {
        Device::Running
            if super::privacy_display::Topology::privacy_target_available()
                .map_err(io::Error::other)? =>
        {
            Ok(Some(Readiness::Ready))
        }
        Device::Running | Device::Starting | Device::Missing => Ok(None),
        Device::RestartRequired => Ok(Some(Readiness::RestartRequired)),
        Device::Problem(CM_PROB_DISABLED) => Err(io::Error::other(
            "Windows 已禁用防窥显示设备。请在设备管理器启用 Virtual Display Driver 后重试。",
        )),
        Device::Problem(code) => Err(io::Error::other(format!(
            "Windows 报告防窥显示设备异常（设备管理器代码 {code}）。"
        ))),
        Device::Unconfigured => Err(io::Error::other(
            "Windows 尚未配置已有的 Virtual Display Driver。请检查设备管理器后重试。",
        )),
    }
}

// PnP registration may finish before the indirect monitor is available to SetDisplayConfig.
fn await_output(mut state: Device) -> io::Result<Readiness> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(ready) = usable(state)? {
            return Ok(ready);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "Windows 尚未提供可用的防窥显示输出，请稍后重试。",
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
        state = device()?;
    }
}

fn component_dir() -> io::Result<PathBuf> {
    Ok(program_files()?.join("FlashLink Virtual Display 25.7.23"))
}

pub fn prepare(window: HWND) -> Result<Readiness, String> {
    let result = (|| -> io::Result<Readiness> {
        if restart_pending()? {
            return Ok(Readiness::RestartRequired);
        }
        let dir = component_dir()?;
        let state = device()?;
        let setup = match state {
            Device::Missing => true,
            Device::Unconfigured => uses_directory(&dir),
            Device::Running if uses_directory(&dir) => pending_settings(&dir)?.is_some(),
            _ => false,
        };
        if !setup {
            return await_output(state);
        }
        if launch(window)? == Readiness::RestartRequired {
            return Ok(Readiness::RestartRequired);
        }
        await_output(device()?)
    })();
    result.map_err(|e| {
        if e.raw_os_error() == Some(ERROR_CANCELLED as i32) {
            "Windows 管理员授权已取消。".into()
        } else {
            format!("防窥模式准备失败：{e}")
        }
    })
}

// This elevated entry point only accepts a fixed operation and only writes bundled bytes.
fn provision() -> io::Result<Readiness> {
    let dir = component_dir()?;
    fs::create_dir_all(&dir)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("setup.lock"))?;
    lock.lock()?;
    if restart_pending()? {
        return Ok(Readiness::RestartRequired);
    }
    let state = device()?;
    match state {
        Device::Missing => {}
        Device::Unconfigured if uses_directory(&dir) => {}
        Device::Running if uses_directory(&dir) => {
            if let Some(settings) = pending_settings(&dir)? {
                write_component(&dir.join("vdd_settings.xml"), settings.as_bytes())?;
                return mark_restart();
            }
            return Ok(Readiness::Ready);
        }
        _ => return await_output(state),
    }
    const FILES: &[(&str, &[u8])] = &[
        ("MttVDD.inf", include_bytes!("../assets/mttvdd/MttVDD.inf")),
        ("MttVDD.dll", include_bytes!("../assets/mttvdd/MttVDD.dll")),
        ("mttvdd.cat", include_bytes!("../assets/mttvdd/mttvdd.cat")),
        ("LICENSE", include_bytes!("../assets/mttvdd/LICENSE")),
    ];
    for (name, data) in FILES {
        let path = dir.join(name);
        if fs::read(&path).as_deref().ok() != Some(*data) {
            write_component(&path, data)?;
        }
    }
    if let Some(settings) = pending_settings(&dir)? {
        write_component(&dir.join("vdd_settings.xml"), settings.as_bytes())?;
    }
    configure_directory(&dir)?;
    let inf = dir.join("MttVDD.inf");
    let reboot = if state == Device::Unconfigured {
        bind(&inf)?
    } else {
        register(&inf)?
    };
    if reboot {
        mark_restart()
    } else {
        Ok(Readiness::Ready)
    }
}

pub fn setup_exit_code() -> i32 {
    match provision() {
        Ok(Readiness::Ready) => 0,
        Ok(Readiness::RestartRequired) => ERROR_SUCCESS_REBOOT_REQUIRED as i32,
        Err(e) => e.raw_os_error().unwrap_or(ERROR_GEN_FAILURE as i32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_present_device_must_be_running_and_free_of_pnp_problems() {
        assert_eq!(device_status(0, 0), Device::Starting);
        assert_eq!(device_status(DN_STARTED, CM_PROB_DISABLED), Device::Running);
        for (code, expected) in [
            (CM_PROB_DISABLED, Device::Problem(CM_PROB_DISABLED)),
            (CM_PROB_NEED_RESTART, Device::RestartRequired),
            (CM_PROB_FAILED_INSTALL, Device::Unconfigured),
        ] {
            assert_eq!(device_status(DN_STARTED | DN_HAS_PROBLEM, code), expected);
        }
    }

    #[test]
    fn installer_adds_each_connected_desktop_size_once_and_preserves_existing_modes() {
        let sizes = [(3840, 2160), (1600, 2560), (3440, 1440), (1600, 2560)];
        let updated = settings_with_modes(DEFAULT_SETTINGS, &sizes).unwrap();
        assert_eq!(updated.matches("<width>1600</width>").count(), 1);
        assert_eq!(updated.matches("<width>3840</width>").count(), 1);
        assert!(updated.contains("<width>3440</width><height>1440</height>"));
        assert_eq!(settings_with_modes(&updated, &sizes).unwrap(), updated);
    }
}
