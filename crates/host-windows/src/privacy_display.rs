use std::{io, mem::size_of, ptr};
use windows_sys::Win32::Graphics::Gdi::DISPLAYCONFIG_PATH_ACTIVE;
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, Display::*},
    Foundation::{ERROR_INSUFFICIENT_BUFFER, LUID},
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct Target {
    identity: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    rotation: i32,
}

type Result<T> = std::result::Result<T, String>;

pub struct Topology {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}
impl Topology {
    fn query(flags: u32) -> Result<Self> {
        // Hotplug may change the required buffer sizes between the two calls.
        for _ in 0..4 {
            let (mut n, mut m) = (0, 0);
            let e = unsafe { GetDisplayConfigBufferSizes(flags, &mut n, &mut m) };
            if e != 0 {
                return Err(format!("读取显示数量失败 ({e})"));
            }
            if n == 0 || n > 8192 || m > 32768 {
                return Err("显示路径数量异常".into());
            }
            let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); n as usize];
            let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); m as usize];
            let e = unsafe {
                QueryDisplayConfig(
                    flags,
                    &mut n,
                    paths.as_mut_ptr(),
                    &mut m,
                    modes.as_mut_ptr(),
                    ptr::null_mut(),
                )
            };
            if e == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if e != 0 {
                return Err(format!("无法查询显示配置 ({e})"));
            }
            paths.truncate(n as usize);
            modes.truncate(m as usize);
            return Ok(Self { paths, modes });
        }
        Err("显示器正在重新连接，请稍后重试".into())
    }
    pub fn active() -> Result<Self> {
        Self::query(QDC_ONLY_ACTIVE_PATHS)
    }
    fn set(&self, operation: u32) -> Result<()> {
        let modes = if self.modes.is_empty() {
            ptr::null()
        } else {
            self.modes.as_ptr()
        };
        let e = unsafe {
            SetDisplayConfig(
                self.paths.len() as _,
                self.paths.as_ptr(),
                self.modes.len() as _,
                modes,
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | operation,
            )
        };
        if e != 0 {
            Err(format!("Windows 拒绝显示配置 ({e})"))
        } else {
            Ok(())
        }
    }
    pub fn targets(&self) -> Result<Vec<Target>> {
        use sha2::{Digest, Sha256};
        let mut displays = Vec::new();
        for path in &self.paths {
            let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            name.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as _;
            name.header.adapterId = path.targetInfo.adapterId;
            name.header.id = path.targetInfo.id;
            let e = unsafe { DisplayConfigGetDeviceInfo(&mut name.header) };
            let count = name
                .monitorDevicePath
                .iter()
                .position(|v| *v == 0)
                .unwrap_or(name.monitorDevicePath.len());
            if e != 0 || count == 0 {
                return Err("无法识别原显示器，未开启隐私。".into());
            }
            let raw = String::from_utf16_lossy(&name.monitorDevicePath[..count]).to_lowercase();
            let idx = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let mode = self
                .modes
                .get(idx)
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                .ok_or("无法记录原显示模式")?;
            let source = unsafe { mode.Anonymous.sourceMode };
            displays.push(Target {
                identity: format!("{:x}", Sha256::digest(raw.as_bytes())),
                x: source.position.x,
                y: source.position.y,
                width: source.width,
                height: source.height,
                rotation: path.targetInfo.rotation,
            });
        }
        displays.sort();
        if displays.is_empty() {
            return Err("缺少活动显示器".into());
        }
        Ok(displays)
    }
    pub fn has_physical_output(&self) -> bool {
        self.paths
            .iter()
            .any(|p| !is_mttvdd(p.targetInfo.adapterId))
    }
    pub fn restore(&self) -> Result<()> {
        self.set(SDC_APPLY)
    }
    pub fn virtual_only_plan() -> Result<Self> {
        let all = Self::query(QDC_ALL_PATHS)?;
        let mut candidates: Vec<_> = all
            .paths
            .into_iter()
            .filter(|p| p.targetInfo.targetAvailable != 0 && is_mttvdd(p.targetInfo.adapterId))
            .collect();
        candidates.sort_by_key(|p| {
            (
                p.flags & DISPLAYCONFIG_PATH_ACTIVE == 0,
                p.targetInfo.id,
                p.sourceInfo.id,
            )
        });
        let mut path = candidates
            .first()
            .copied()
            .ok_or("未找到已启用的虚拟显示器。请先在电脑上点“安装虚拟屏”，安装后重新连接桌面。")?;
        path.flags = DISPLAYCONFIG_PATH_ACTIVE;
        path.sourceInfo.Anonymous.modeInfoIdx = u32::MAX;
        path.targetInfo.Anonymous.modeInfoIdx = u32::MAX;
        path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL::default();
        let target = Self {
            paths: vec![path],
            modes: vec![],
        };
        target.set(SDC_VALIDATE | SDC_ALLOW_CHANGES)?;
        Ok(target)
    }
    pub fn activate(&self) -> Result<()> {
        self.set(SDC_APPLY | SDC_ALLOW_CHANGES)
    }
    pub fn still_private(&self) -> bool {
        let target = &self.paths[0].targetInfo;
        Self::active().is_ok_and(|now| {
            now.paths.len() == 1 && {
                let p = &now.paths[0].targetInfo;
                p.id == target.id
                    && p.adapterId.HighPart == target.adapterId.HighPart
                    && p.adapterId.LowPart == target.adapterId.LowPart
            }
        })
    }
}

pub fn restore_database() -> Result<()> {
    // Our temporary configuration deliberately omitted SDC_SAVE_TO_DATABASE. The saved mode is
    // the fallback when a guard was interrupted before it could reapply its in-memory snapshot.
    let e = unsafe {
        SetDisplayConfig(
            0,
            ptr::null(),
            0,
            ptr::null(),
            SDC_APPLY | SDC_USE_DATABASE_CURRENT,
        )
    };
    if e == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(e).to_string())
    }
}
struct Devices(HDEVINFO);
impl Drop for Devices {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}
fn is_mttvdd(adapter: LUID) -> bool {
    // INDIRECT_WIRED also describes actual USB monitors. Identify the PnP hardware ID behind
    // the CCD adapter interface; neither monitor names nor the connector type identify a VDD.
    let mut name = DISPLAYCONFIG_ADAPTER_NAME::default();
    name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADAPTER_NAME;
    name.header.size = size_of::<DISPLAYCONFIG_ADAPTER_NAME>() as _;
    name.header.adapterId = adapter;
    unsafe {
        if DisplayConfigGetDeviceInfo(&mut name.header) != 0 {
            return false;
        }
        let h = SetupDiCreateDeviceInfoList(ptr::null(), ptr::null_mut());
        if h == -1 {
            return false;
        }
        let set = Devices(h);
        let mut interface = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as _,
            ..Default::default()
        };
        if SetupDiOpenDeviceInterfaceW(set.0, name.adapterDevicePath.as_ptr(), 0, &mut interface)
            == 0
        {
            return false;
        }
        let mut node = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as _,
            ..Default::default()
        };
        let mut bytes = 0;
        // As documented, this size query fills DeviceInfoData, even though detail is not requested.
        SetupDiGetDeviceInterfaceDetailW(
            set.0,
            &interface,
            ptr::null_mut(),
            0,
            &mut bytes,
            &mut node,
        );
        let mut ids = [0u16; 1024];
        if SetupDiGetDeviceRegistryPropertyW(
            set.0,
            &node,
            SPDRP_HARDWAREID,
            ptr::null_mut(),
            ids.as_mut_ptr().cast(),
            size_of_val(&ids) as _,
            &mut bytes,
        ) == 0
        {
            return false;
        }
        ids[..(bytes as usize / 2).min(ids.len())]
            .split(|c| *c == 0)
            .any(|part| {
                let id = String::from_utf16_lossy(part);
                id.eq_ignore_ascii_case(r"Root\MttVDD") || id.eq_ignore_ascii_case("MttVDD")
            })
    }
}
