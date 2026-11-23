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
pub fn install() -> io::Result<String> {
    if installed()? {
        return Ok(
            "已存在虚拟屏组件。如果没有显示输出，请在设备管理器中启用 Virtual Display Driver。"
                .into(),
        );
    }
    let dir = program_files()?.join("FlashLink Virtual Display 25.7.23");
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
            "vdd_settings.xml",
            include_bytes!("../../../packaging/windows/mttvdd/vdd_settings.xml"),
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
    configure_directory(&dir)?;
    if register(&dir.join("MttVDD.inf"))? {
        Ok("虚拟屏组件已安装。Windows 要求重启电脑后使用。".into())
    } else {
        Ok("虚拟屏组件已安装。请从安卓打开桌面，再从主机菜单开启隐私。".into())
    }
}
