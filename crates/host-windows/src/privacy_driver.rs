//! Install only the four pinned files from the upstream signed amd64 MttVDD release. The
//! elevated entry point accepts no paths, commands or driver packages from the caller.
use std::{
    fs,
    io::{self, Write},
    mem::size_of,
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::*,
    Foundation::{ERROR_FILE_NOT_FOUND, ERROR_NO_MORE_ITEMS, HWND},
    System::{Com::CoTaskMemFree, Registry::*},
    UI::{Shell::*, WindowsAndMessaging::SW_SHOWNORMAL},
};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
pub fn launch(window: HWND) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let path: Vec<_> = exe.as_os_str().encode_wide().chain([0]).collect();
    let result = unsafe {
        ShellExecuteW(
            window,
            wide("runas").as_ptr(),
            path.as_ptr(),
            wide("--install-privacy-display").as_ptr(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize <= 32 {
        Err("Windows 未启动组件安装，请检查是否已同意系统授权。".into())
    } else {
        Ok(())
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
fn installed() -> io::Result<bool> {
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
        for i in 0.. {
            let mut dev = node();
            if SetupDiEnumDeviceInfo(set.0, i, &mut dev) == 0 {
                let e = io::Error::last_os_error();
                return if e.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                    Ok(false)
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
            ) != 0
                && ids[..size as usize / 2]
                    .split(|v| *v == 0)
                    .any(|s| String::from_utf16_lossy(s).eq_ignore_ascii_case(r"Root\MttVDD"))
            {
                return Ok(true);
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
        let path: Vec<_> = inf.as_os_str().encode_wide().chain([0]).collect();
        let mut reboot = 0;
        // Let normal Windows ranking, catalog verification and policy decide installation.
        if UpdateDriverForPlugAndPlayDevicesW(
            ptr::null_mut(),
            hardware.as_ptr(),
            path.as_ptr(),
            0,
            &mut reboot,
        ) == 0
        {
            let error = io::Error::last_os_error();
            SetupDiCallClassInstaller(DIF_REMOVE, set.0, &dev);
            return Err(error);
        }
        Ok(reboot != 0)
    }
}
const DEFAULT_SETTINGS: &str = include_str!("../../../packaging/windows/mttvdd/vdd_settings.xml");

// Only the installer writes the protected settings file. A new monitor can be
// added by running the same installer again, without replacing the signed driver.
fn settings_with_modes(settings: &str, modes: &[(u32, u32)]) -> io::Result<String> {
    let (prefix, suffix) = settings.split_once("</resolutions>")
        .ok_or_else(|| io::Error::other("虚拟屏配置缺少 resolutions 节点"))?;
    let mut result = prefix.to_owned();
    let mut known = std::collections::BTreeSet::new();
    for block in prefix.split("<resolution>").skip(1) {
        let number = |start, end| block.split_once(start)?.1.split_once(end)?.0.trim().parse::<u32>().ok();
        if let (Some(w), Some(h)) = (number("<width>", "</width>"), number("<height>", "</height>")) {
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
        RegGetValueW(HKEY_LOCAL_MACHINE, wide(r"SOFTWARE\MikeTheTech\VirtualDisplayDriver").as_ptr(),
            wide("VDDPATH").as_ptr(), RRF_RT_REG_SZ, ptr::null_mut(), value.as_mut_ptr().cast(), &mut bytes)
    };
    let len = value.iter().position(|&c| c == 0).unwrap_or(value.len());
    status == 0 && PathBuf::from(std::ffi::OsString::from_wide(&value[..len])) == dir
}

fn update_modes(dir: &Path) -> io::Result<bool> {
    let path = dir.join("vdd_settings.xml");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    let sizes = super::privacy_display::Topology::active().map_err(io::Error::other)?.physical_desktop_sizes();
    let settings = settings_with_modes(existing.as_deref().unwrap_or(DEFAULT_SETTINGS), &sizes)?;
    if existing.as_ref() == Some(&settings) {
        return Ok(false);
    }
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).map_err(|e| io::Error::other(e.to_string()))?;
    let temporary = dir.join(format!("settings-{:x}.tmp", u64::from_ne_bytes(nonce)));
    let mut file = fs::File::create_new(&temporary)?;
    let write = file.write_all(settings.as_bytes()).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = write.and_then(|_| fs::rename(&temporary, &path)) {
        let _ = fs::remove_file(temporary);
        return Err(e);
    }
    Ok(true)
}

pub fn install() -> io::Result<String> {
    let dir = program_files()?.join("FlashLink Virtual Display 25.7.23");
    if installed()? && !uses_directory(&dir) {
        return Ok(
            "已存在虚拟屏组件。如果没有显示输出，请在设备管理器中启用 Virtual Display Driver。"
                .into(),
        );
    }
    match fs::create_dir(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("setup.lock"))?;
    lock.try_lock()?;
    if installed()? {
        if uses_directory(&dir) && update_modes(&dir)? {
            return Ok("已加入当前实体屏的桌面分辨率。重启 Windows 后新模式生效。".into());
        }
        return Ok("虚拟屏已安装。".into());
    }
    const FILES: &[(&str, &[u8])] = &[
        (
            "MttVDD.inf",
            include_bytes!("../../../packaging/windows/mttvdd/MttVDD.inf"),
        ),
        (
            "MttVDD.dll",
            include_bytes!("../../../packaging/windows/mttvdd/MttVDD.dll"),
        ),
        (
            "mttvdd.cat",
            include_bytes!("../../../packaging/windows/mttvdd/mttvdd.cat"),
        ),
        (
            "LICENSE",
            include_bytes!("../../../packaging/windows/mttvdd/LICENSE"),
        ),
    ];
    for (name, data) in FILES {
        let path = dir.join(name);
        match fs::File::create_new(&path) {
            Ok(mut file) => {
                file.write_all(data)?;
                file.sync_all()?;
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && fs::read(&path)? == *data => {}
            Err(e) => return Err(e),
        }
    }
    update_modes(&dir)?;
    configure_directory(&dir)?;
    if register(&dir.join("MttVDD.inf"))? {
        Ok("虚拟屏组件已安装。Windows 要求重启电脑后使用。".into())
    } else {
        Ok("虚拟屏组件已安装。请从安卓打开桌面，再从主机菜单开启隐私。".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
