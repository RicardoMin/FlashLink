use super::privacy_display_plan::{
    candidate_sort_key, format_call_log, format_display_error,
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

    fn matches(&self, now: &Topology) -> bool {
        let [path] = now.paths.as_slice() else { return false };
        let wanted = &self.active.topology.paths[0].targetInfo;
        let actual = &path.targetInfo;
        actual.adapterId.HighPart == wanted.adapterId.HighPart
            && actual.adapterId.LowPart == wanted.adapterId.LowPart
            && actual.id == wanted.id
            && now.primary_resolution() == self.active.topology.primary_resolution()
    }

    pub fn still_private(&self, log_dir: &Path) -> bool {
        let actual = Topology::active();
        if actual.as_ref().is_ok_and(|now| self.matches(now)) {
            return true;
        }
        let secs = now_secs();
        let expected = path_log_info(&self.active.topology.paths[0], &self.active.topology.modes);
        append_display_log(
            log_dir,
            &format_indexed_path_log(secs, "verify_expected", 0, &expected),
        );
        match actual {
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
        false
    }

    pub fn reapply_if_known_displays(&mut self, dir: &Path, original: &Topology, known: &[Target]) -> Result<bool> {
        let mut now = Topology::active()?;
        // Windows may have finished the transition since the watchdog sampled
        // the mismatch. Another mode set here would restart desktop capture.
        if self.matches(&now) {
            return Ok(false);
        }
        now.paths.retain(|p| !is_mttvdd(p.targetInfo.adapterId));
        if !now.paths.is_empty() {
            let targets = now.targets()?;
            if !targets.iter().all(|t| known.iter().any(|o| o.identity == t.identity)) {
                return Err("显示器身份已变化".into());
            }
        }
        if self.active.apply(dir).is_err() {
            // Hotplug can invalidate source and target IDs. Rebuild from current
            // available paths, always using the pre-privacy desktop as the template.
            let mut next = Topology::virtual_only_plan(dir, original)?;
            next.activate(dir)?;
            *self = next;
        }
        Ok(true)
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

    fn primary_path(&self) -> Option<(&DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_MODE)> {
        self.paths.iter().find_map(|path| {
            let idx = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let mode = self.modes.get(idx)?;
            if mode.infoType != DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE { return None; }
            let source = unsafe { mode.Anonymous.sourceMode };
            (source.position.x == 0 && source.position.y == 0 && source.width > 0 && source.height > 0)
                .then_some((path, source))
        })
    }

    fn primary_resolution(&self) -> Option<(u32, u32)> {
        self.primary_path().map(|(_, source)| (source.width, source.height))
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

    pub fn physical_desktop_sizes(&self) -> Vec<(u32, u32)> {
        self.paths.iter()
            .filter(|path| !is_mttvdd(path.targetInfo.adapterId))
            .filter_map(|path| path_log_info(path, &self.modes).source_rect)
            .map(|(_, _, width, height)| (width, height))
            .collect()
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

        let (primary, desktop) = original.primary_path().ok_or("无法读取当前主屏的桌面分辨率。")?;
        let mut attempts = Vec::new();
        for raw in candidates {
            build_attempts_for_candidate(raw, &all.modes, primary, desktop, &mut attempts);
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
        if last_code == 5 {
            return Err(format_display_error("validate", last_code));
        }
        Err(format!("无法为虚拟屏校验原主屏的 {}×{} 分辨率。{}。", desktop.width, desktop.height, format_display_error("validate", last_code)))
    }
}

fn build_attempts_for_candidate(
    raw: DISPLAYCONFIG_PATH_INFO,
    all_modes: &[DISPLAYCONFIG_MODE_INFO],
    primary: &DISPLAYCONFIG_PATH_INFO,
    desktop: DISPLAYCONFIG_SOURCE_MODE,
    out: &mut Vec<PlanAttempt>,
) {
    let mut mode = DISPLAYCONFIG_MODE_INFO::default();
    mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
    mode.id = raw.sourceInfo.id;
    mode.adapterId = raw.sourceInfo.adapterId;
    mode.Anonymous.sourceMode = DISPLAYCONFIG_SOURCE_MODE {
        width: desktop.width,
        height: desktop.height,
        pixelFormat: DISPLAYCONFIG_PIXELFORMAT_32BPP,
        position: Default::default(),
    };
    // A portrait desktop may be available as a rotated landscape timing or as a
    // native portrait timing. Both must expose the same pixel grid to Explorer.
    let rotation = normalize_rotation(primary.targetInfo.rotation);
    let mut rotations = vec![rotation];
    if rotation != DISPLAYCONFIG_ROTATION_IDENTITY {
        rotations.push(DISPLAYCONFIG_ROTATION_IDENTITY);
    }
    for rotation in rotations {
        let mut path = normalized_path_without_target_mode(raw, 0);
        path.targetInfo.rotation = rotation;
        let rate = primary.targetInfo.refreshRate;
        if rate.Numerator != 0 && rate.Denominator != 0 {
            let mut preferred = path;
            preferred.targetInfo.refreshRate = rate;
            out.push(PlanAttempt {
                topology: Topology { paths: vec![preferred], modes: vec![mode] },
                strategy: PlanStrategy::SuppliedSourceOnly,
                config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG,
            });
        }
        out.push(PlanAttempt {
            topology: Topology { paths: vec![path], modes: vec![mode] },
            strategy: PlanStrategy::SuppliedSourceOnly,
            config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG,
        });
        let target_idx = unsafe { raw.targetInfo.Anonymous.modeInfoIdx } as usize;
        if let Some(target) = all_modes.get(target_idx)
            .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET) {
            path.targetInfo.Anonymous.modeInfoIdx = 1;
            out.push(PlanAttempt {
                topology: Topology { paths: vec![path], modes: vec![mode, *target] },
                strategy: PlanStrategy::SuppliedFullModes,
                config_flags: SDC_USE_SUPPLIED_DISPLAY_CONFIG,
            });
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_virtual_candidate_preserves_the_original_primary_desktop() {
        // Secondary paths precede the primary, as can happen on docked laptops.
        for (width, height, rotation, multi) in [
            (1920, 1080, 1, false), (3840, 2160, 1, true),
            (1600, 2560, 2, true), (5120, 1440, 1, false),
        ] {
            let mut desktop = DISPLAYCONFIG_SOURCE_MODE::default();
            desktop.width = width;
            desktop.height = height;
            let mut primary = DISPLAYCONFIG_PATH_INFO::default();
            primary.targetInfo.rotation = rotation;
            primary.targetInfo.refreshRate = DISPLAYCONFIG_RATIONAL { Numerator: 144, Denominator: 1 };
            let mut mode = DISPLAYCONFIG_MODE_INFO::default();
            mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
            mode.Anonymous.sourceMode = desktop;
            let mut original = Topology { paths: Vec::new(), modes: Vec::new() };
            if multi {
                let mut other = desktop;
                other.width = 1920;
                other.height = 1200;
                other.position.x = -1920;
                let mut secondary_mode = mode;
                secondary_mode.Anonymous.sourceMode = other;
                original.paths.push(DISPLAYCONFIG_PATH_INFO::default());
                original.modes.push(secondary_mode);
            }
            primary.sourceInfo.Anonymous.modeInfoIdx = original.modes.len() as u32;
            original.paths.push(primary);
            original.modes.push(mode);
            let (p, src) = original.primary_path().unwrap();
            let mut candidates = Vec::new();
            build_attempts_for_candidate(DISPLAYCONFIG_PATH_INFO::default(), &[], p, src, &mut candidates);
            assert_eq!(candidates[0].topology.paths[0].targetInfo.rotation, rotation);
            assert!(candidates.iter().all(|c| c.topology.primary_resolution() == Some((width, height))));
            assert!(candidates.iter().any(|c| c.topology.paths[0].targetInfo.refreshRate.Numerator == 0));
        }
    }

    #[test]
    #[ignore = "interactive hardware exercise; powers physical displays down for 45 seconds"]
    fn exercise_virtual_display_with_hotplug_and_restore() {
        assert_eq!(std::env::var("FLASHLINK_INTERACTIVE_TEST").as_deref(), Ok("yes"));
        let dir = std::env::temp_dir();
        let original = Topology::active().unwrap();
        let targets = original.targets().unwrap();
        let icons = crate::desktop_icons::DesktopIcons::capture().unwrap();
        let mut plan = Topology::virtual_only_plan(&dir, &original).unwrap();
        let result = (|| -> Result<()> {
            plan.activate(&dir)?;
            let start = std::time::Instant::now();
            let mut repair = crate::privacy_time::DisplayRepair::default();
            let mut align_at = Some(start + std::time::Duration::from_millis(900));
            while start.elapsed().as_secs() < 45 {
                if !plan.still_private(&dir) {
                    if !repair.can_repair(std::time::Instant::now()) {
                        return Err("virtual desktop did not recover".into());
                    }
                    if plan.reapply_if_known_displays(&dir, &original, &targets)? {
                        repair.reapplied(std::time::Instant::now());
                    }
                    align_at = Some(std::time::Instant::now() + std::time::Duration::from_millis(900));
                    println!("reapplied at {:.2}s", start.elapsed().as_secs_f64());
                } else {
                    repair.verified();
                    if align_at.is_some_and(|t| std::time::Instant::now() >= t) {
                        icons.align();
                        align_at = None;
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
            Ok(())
        })();
        let mut restored = false;
        for _ in 0..24 {
            if original.restore(Some(&dir)).is_err() { let _ = restore_database(Some(&dir)); }
            if Topology::active().and_then(|s| s.targets()).is_ok_and(|s| s == targets) {
                restored = true;
                icons.finish();
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(350));
        }
        assert!(restored, "original display layout not yet restored");
        result.unwrap();
    }

    #[test]
    #[ignore = "requires an interactive Windows desktop and an installed MttVDD supporting the primary resolution"]
    fn validate_primary_resolution_on_local_virtual_display() {
        let original = Topology::active().unwrap();
        let primary = original.primary_resolution().unwrap();
        let plan = Topology::virtual_only_plan(&std::env::temp_dir(), &original).unwrap();
        // virtual_only_plan uses SDC_VALIDATE. Running this test must not switch displays.
        let selected = plan.active.topology.primary_resolution();
        let rate = plan.active.topology.paths[0].targetInfo.refreshRate;
        println!("Primary desktop: {primary:?}, validated virtual desktop: {selected:?} refresh={}/{}",rate.Numerator,rate.Denominator);
        assert_eq!(selected, Some(primary));
    }
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

pub fn log_repair_error(dir: &Path, reason: &str) {
    append_display_log(dir, &format!("{} phase=repair_failed detail={reason}", now_secs()));
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
