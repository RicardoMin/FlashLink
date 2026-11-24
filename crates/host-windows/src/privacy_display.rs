use super::privacy_display_plan::{
    candidate_sort_key, fallback_resolutions, format_call_log, format_display_error,
    format_indexed_path_log, normalize_rotation, normalize_scaling, PathLogInfo, PlanStrategy,
    MODE_IDX_INVALID,
};
use std::{
    fs::OpenOptions,
    io::{self, Write},
    mem::size_of,
    path::Path,
    ptr,
    time::{SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::Graphics::Gdi::DISPLAYCONFIG_PATH_ACTIVE;
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, Display::*},
    Foundation::{ERROR_INSUFFICIENT_BUFFER, LUID},
};

const DISPLAY_LOG: &str = "privacy-display.log";

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

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn append_display_log(dir: &Path, line: &str) {
    if let Ok(mut file) = OpenOptions::new()
        .append(true)
        .create(true)
        .open(dir.join(DISPLAY_LOG))
    {
        let _ = writeln!(file, "{line}");
    }
}

fn path_log_info(
    path: &DISPLAYCONFIG_PATH_INFO,
    modes: &[DISPLAYCONFIG_MODE_INFO],
) -> PathLogInfo {
    let source_mode_idx = unsafe { path.sourceInfo.Anonymous.modeInfoIdx };
    let target_mode_idx = unsafe { path.targetInfo.Anonymous.modeInfoIdx };
    let source_rect = modes
        .get(source_mode_idx as usize)
        .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
        .map(|m| {
            let s = unsafe { m.Anonymous.sourceMode };
            (s.position.x, s.position.y, s.width, s.height)
        });
    PathLogInfo {
        adapter_high: path.targetInfo.adapterId.HighPart,
        adapter_low: path.targetInfo.adapterId.LowPart,
        source_id: path.sourceInfo.id,
        target_id: path.targetInfo.id,
        path_flags: path.flags,
        source_mode_idx,
        target_mode_idx,
        source_rect,
        rotation: path.targetInfo.rotation,
        scaling: path.targetInfo.scaling,
        refresh_num: path.targetInfo.refreshRate.Numerator,
        refresh_den: path.targetInfo.refreshRate.Denominator,
    }
}

fn log_topology_call(
    log_dir: &Path,
    phase: &str,
    strategy: &str,
    flags: u32,
    status: i32,
    paths: &[DISPLAYCONFIG_PATH_INFO],
    modes: &[DISPLAYCONFIG_MODE_INFO],
) {
    let secs = now_secs();
    let single = (paths.len() == 1).then(|| path_log_info(&paths[0], modes));
    append_display_log(
        log_dir,
        &format_call_log(
            secs,
            phase,
            strategy,
            flags,
            status,
            paths.len(),
            modes.len(),
            single.as_ref(),
        ),
    );
    if paths.len() > 1 {
        let path_phase = format!("{phase}_path");
        for (idx, path) in paths.iter().enumerate() {
            let info = path_log_info(path, modes);
            append_display_log(
                log_dir,
                &format_indexed_path_log(secs, &path_phase, idx, &info),
            );
        }
    }
}

struct PlanAttempt {
    topology: Topology,
    strategy: PlanStrategy,
    config_flags: u32,
}

impl PlanAttempt {
    fn validate(&self, log_dir: &Path) -> std::result::Result<(), i32> {
        self.topology.apply_config(
            Some(log_dir),
            "validate",
            self.strategy.as_str(),
            self.config_flags | SDC_VALIDATE,
        )
    }

    fn apply(&self, log_dir: &Path) -> std::result::Result<(), i32> {
        self.topology.apply_config(
            Some(log_dir),
            "apply",
            self.strategy.as_str(),
            self.config_flags | SDC_APPLY,
        )
    }
}

pub struct VirtualPlan {
    active: PlanAttempt,
    fallbacks: Vec<PlanAttempt>,
}

impl VirtualPlan {
    pub fn activate(&mut self, log_dir: &Path) -> Result<()> {
        let mut last_phase = "apply";
        let mut last_code = match self.active.apply(log_dir) {
            Ok(()) => return Ok(()),
            Err(code) => code,
        };
        let remaining = std::mem::take(&mut self.fallbacks);
        for candidate in remaining {
            match candidate.validate(log_dir) {
                Ok(()) => match candidate.apply(log_dir) {
                    Ok(()) => {
                        self.active = candidate;
                        return Ok(());
                    }
                    Err(code) => {
                        last_phase = "apply";
                        last_code = code;
                    }
                },
                Err(code) => {
                    last_phase = "validate";
                    last_code = code;
                }
            }
        }
        Err(format_display_error(last_phase, last_code))
    }

    pub fn still_private(&self) -> bool {
        let target = &self.active.topology.paths[0].targetInfo;
        Topology::active().is_ok_and(|now| {
            now.paths.len() == 1 && {
                let p = &now.paths[0].targetInfo;
                p.id == target.id
                    && p.adapterId.HighPart == target.adapterId.HighPart
                    && p.adapterId.LowPart == target.adapterId.LowPart
            }
        })
    }

    pub fn log_active_mismatch(&self, log_dir: &Path) {
        let secs = now_secs();
        let expected = path_log_info(&self.active.topology.paths[0], &self.active.topology.modes);
        append_display_log(
            log_dir,
            &format_indexed_path_log(secs, "verify_expected", 0, &expected),
        );
        match Topology::active() {
            Ok(now) => {
                for (idx, path) in now.paths.iter().enumerate() {
                    let info = path_log_info(path, &now.modes);
                    append_display_log(
                        log_dir,
                        &format_indexed_path_log(secs, "verify_actual", idx, &info),
                    );
                }
            }
            Err(_) => {
                append_display_log(
                    log_dir,
                    &format!("{secs} phase=verify_actual status=query_failed"),
                );
            }
        }
    }
}

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

    fn apply_config(
        &self,
        log_dir: Option<&Path>,
        phase: &str,
        strategy: &str,
        flags: u32,
    ) -> std::result::Result<(), i32> {
        let modes = if self.modes.is_empty() {
            ptr::null()
        } else {
            self.modes.as_ptr()
        };
        let paths = if self.paths.is_empty() {
            ptr::null()
        } else {
            self.paths.as_ptr()
        };
        let e = unsafe {
            SetDisplayConfig(
                self.paths.len() as _,
                paths,
                self.modes.len() as _,
                modes,
                flags,
            )
        };
        if let Some(dir) = log_dir {
            log_topology_call(dir, phase, strategy, flags, e, &self.paths, &self.modes);
        }
        if e == 0 {
            Ok(())
        } else {
            Err(e)
        }
    }

    fn primary_resolution(&self) -> Option<(u32, u32)> {
        let mut fallback = None;
        for path in &self.paths {
            let idx = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let Some(mode) = self
                .modes
                .get(idx)
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
            else {
                continue;
            };
            let source = unsafe { mode.Anonymous.sourceMode };
            if source.position.x == 0 && source.position.y == 0 {
                return Some((source.width, source.height));
            }
            if fallback.is_none() {
                fallback = Some((source.width, source.height));
            }
        }
        fallback
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

    pub fn restore(&self, log_dir: Option<&Path>) -> Result<()> {
        if self
            .apply_config(
                log_dir,
                "restore_exact",
                "supplied_exact",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY,
            )
            .is_ok()
        {
            return Ok(());
        }
        if self
            .apply_config(
                log_dir,
                "restore_allow_changes",
                "supplied_allow_changes",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_ALLOW_CHANGES,
            )
            .is_ok()
        {
            return Ok(());
        }

        // Physical outputs (especially DisplayPort) may renegotiate target timings after
        // being disabled. Retain the exact multi-monitor source layout while letting Windows
        // negotiate fresh target modes.
        let mut source_paths = self.paths.clone();
        let mut source_modes = Vec::new();
        let mut index_map: Vec<(usize, u32)> = Vec::new();
        for path in &mut source_paths {
            let old_idx = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let new_idx =
                if let Some(&(_, mapped)) = index_map.iter().find(|&&(orig, _)| orig == old_idx) {
                    mapped
                } else if let Some(mode) = self
                    .modes
                    .get(old_idx)
                    .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                {
                    let mapped = source_modes.len() as u32;
                    source_modes.push(*mode);
                    index_map.push((old_idx, mapped));
                    mapped
                } else {
                    MODE_IDX_INVALID
                };
            path.sourceInfo.Anonymous.modeInfoIdx = new_idx;
            path.targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
            path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL::default();
            path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED;
        }
        let source_only = Self {
            paths: source_paths,
            modes: source_modes,
        };
        source_only
            .apply_config(
                log_dir,
                "restore_source_modes",
                "supplied_source_only",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_ALLOW_CHANGES,
            )
            .map_err(|code| format_display_error("restore", code))
    }

    pub fn virtual_only_plan(log_dir: &Path, original: &Self) -> Result<VirtualPlan> {
        let all = Self::query(QDC_ALL_PATHS)?;
        let mut candidates: Vec<_> = all
            .paths
            .iter()
            .copied()
            .filter(|p| p.targetInfo.targetAvailable != 0 && is_mttvdd(p.targetInfo.adapterId))
            .collect();
        candidates.sort_by_key(|p| {
            candidate_sort_key(
                p.flags & DISPLAYCONFIG_PATH_ACTIVE != 0,
                p.targetInfo.id,
                p.sourceInfo.id,
            )
        });
        append_display_log(
            log_dir,
            &format!(
                "{} phase=plan_start original_paths={} all_paths={} all_modes={} mttvdd_candidates={}",
                now_secs(),
                original.paths.len(),
                all.paths.len(),
                all.modes.len(),
                candidates.len(),
            ),
        );
        if candidates.is_empty() {
            return Err(
                "未找到已启用的虚拟显示器。请先在电脑上点“安装虚拟屏”，安装后重新连接桌面。"
                    .into(),
            );
        }

        let resolutions = fallback_resolutions(original.primary_resolution());
        let mut attempts = Vec::new();
        for raw in &candidates {
            build_attempts_for_candidate(*raw, &all.modes, &resolutions, &mut attempts);
        }

        let mut iter = attempts.into_iter();
        let mut last_code = 0;
        while let Some(attempt) = iter.next() {
            match attempt.validate(log_dir) {
                Ok(()) => {
                    return Ok(VirtualPlan {
                        active: attempt,
                        fallbacks: iter.collect(),
                    });
                }
                Err(code) => last_code = code,
            }
        }
        Err(format_display_error("validate", last_code))
    }
}

fn build_attempts_for_candidate(
    raw: DISPLAYCONFIG_PATH_INFO,
    all_modes: &[DISPLAYCONFIG_MODE_INFO],
    resolutions: &[(u32, u32)],
    out: &mut Vec<PlanAttempt>,
) {
    let src_idx = unsafe { raw.sourceInfo.Anonymous.modeInfoIdx } as usize;
    let tgt_idx = unsafe { raw.targetInfo.Anonymous.modeInfoIdx } as usize;
    let existing_source = all_modes
        .get(src_idx)
        .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
        .copied();
    let existing_target = all_modes
        .get(tgt_idx)
        .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET)
        .copied();

    // Strategy A: Keep existing valid source + target modes from QDC_ALL_PATHS when the virtual
    // display is already active, normalizing the single-monitor primary origin to (0, 0).
    if let (Some(mut source_mode), Some(target_mode)) = (existing_source, existing_target) {
        source_mode.id = raw.sourceInfo.id;
        source_mode.adapterId = raw.sourceInfo.adapterId;
        let mut sm = unsafe { source_mode.Anonymous.sourceMode };
        sm.position.x = 0;
        sm.position.y = 0;
        source_mode.Anonymous.sourceMode = sm;

        let mut path = raw;
        path.flags = DISPLAYCONFIG_PATH_ACTIVE;
        path.sourceInfo.Anonymous.modeInfoIdx = 0;
        path.targetInfo.Anonymous.modeInfoIdx = 1;
        path.targetInfo.rotation = normalize_rotation(path.targetInfo.rotation);
        path.targetInfo.scaling = normalize_scaling(path.targetInfo.scaling);

        out.push(PlanAttempt {
            topology: Topology {
                paths: vec![path],
                modes: vec![source_mode, target_mode],
            },
            strategy: PlanStrategy::SuppliedFullModes,
            config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        });
    }

    // Strategy B: Supply a valid source mode at (0, 0) and let Windows resolve the target mode
    // via SDC_ALLOW_CHANGES. Covers inactive virtual displays and drivers rejecting reused target
    // timings.
    let mut existing_res = None;
    if let Some(mut source_mode) = existing_source {
        source_mode.id = raw.sourceInfo.id;
        source_mode.adapterId = raw.sourceInfo.adapterId;
        let mut sm = unsafe { source_mode.Anonymous.sourceMode };
        existing_res = Some((sm.width, sm.height));
        sm.position.x = 0;
        sm.position.y = 0;
        source_mode.Anonymous.sourceMode = sm;

        out.push(PlanAttempt {
            topology: Topology {
                paths: vec![normalized_path_without_target_mode(raw, 0)],
                modes: vec![source_mode],
            },
            strategy: PlanStrategy::SuppliedSourceOnly,
            config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        });
    }
    for &(width, height) in resolutions {
        if existing_res == Some((width, height)) {
            continue;
        }
        let mut source_mode = DISPLAYCONFIG_MODE_INFO::default();
        source_mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
        source_mode.id = raw.sourceInfo.id;
        source_mode.adapterId = raw.sourceInfo.adapterId;
        let mut sm = DISPLAYCONFIG_SOURCE_MODE::default();
        sm.width = width;
        sm.height = height;
        sm.pixelFormat = DISPLAYCONFIG_PIXELFORMAT_32BPP;
        sm.position.x = 0;
        sm.position.y = 0;
        source_mode.Anonymous.sourceMode = sm;

        out.push(PlanAttempt {
            topology: Topology {
                paths: vec![normalized_path_without_target_mode(raw, 0)],
                modes: vec![source_mode],
            },
            strategy: PlanStrategy::SuppliedSourceOnly,
            config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
        });
    }

    // Strategy C: Topology-only lookup in Windows' persistence database.
    out.push(PlanAttempt {
        topology: Topology {
            paths: vec![normalized_path_without_target_mode(raw, MODE_IDX_INVALID)],
            modes: Vec::new(),
        },
        strategy: PlanStrategy::TopologySupplied,
        config_flags: SDC_TOPOLOGY_SUPPLIED | SDC_ALLOW_PATH_ORDER_CHANGES,
    });

    // Strategy D: Legacy supplied-path-without-modes fallback, with normalized target enums.
    out.push(PlanAttempt {
        topology: Topology {
            paths: vec![normalized_path_without_target_mode(raw, MODE_IDX_INVALID)],
            modes: Vec::new(),
        },
        strategy: PlanStrategy::SuppliedNoModes,
        config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES,
    });
}

fn normalized_path_without_target_mode(
    mut path: DISPLAYCONFIG_PATH_INFO,
    source_mode_idx: u32,
) -> DISPLAYCONFIG_PATH_INFO {
    path.flags = DISPLAYCONFIG_PATH_ACTIVE;
    path.sourceInfo.Anonymous.modeInfoIdx = source_mode_idx;
    path.targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
    path.targetInfo.rotation = normalize_rotation(path.targetInfo.rotation);
    path.targetInfo.scaling = normalize_scaling(path.targetInfo.scaling);
    path.targetInfo.scanLineOrdering = DISPLAYCONFIG_SCANLINE_ORDERING_UNSPECIFIED;
    path.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL::default();
    path
}

pub fn restore_database(log_dir: Option<&Path>) -> Result<()> {
    // Our temporary configuration deliberately omitted SDC_SAVE_TO_DATABASE. The saved mode is
    // the fallback when a guard was interrupted before it could reapply its in-memory snapshot.
    let flags = SDC_APPLY | SDC_USE_DATABASE_CURRENT;
    let e = unsafe { SetDisplayConfig(0, ptr::null(), 0, ptr::null(), flags) };
    if let Some(dir) = log_dir {
        log_topology_call(dir, "restore_database", "database_current", flags, e, &[], &[]);
    }
    if e == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(e).to_string())
    }
}

pub fn log_restore_recovered(dir: &Path, manual: bool) {
    append_display_log(
        dir,
        &format!(
            "{} phase=restore_recovered manual={}",
            now_secs(),
            if manual { 1 } else { 0 }
        ),
    );
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
