use super::privacy_display_plan::{
    candidate_sort_key, dpi_scale_percent, format_available_target_log, format_call_log,
    format_display_error, format_indexed_path_log, normalize_origin_positions, normalize_rotation,
    normalize_scaling, relative_dpi_for_percent, select_distinct_source_paths, ApplyPersistence,
    AvailableTargetLogInfo, PathLogInfo, PlanStrategy, MODE_IDX_INVALID,
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

    fn apply(&self, log_dir: &Path, persistence: ApplyPersistence) -> std::result::Result<(), i32> {
        let save = match persistence {
            ApplyPersistence::SaveToDatabase => SDC_SAVE_TO_DATABASE,
            ApplyPersistence::Transient => 0,
        };
        self.topology.apply_config(
            Some(log_dir),
            "apply",
            self.strategy.as_str(),
            self.config_flags | SDC_APPLY | save,
        )
    }
}

pub struct VirtualPlan {
    active: PlanAttempt,
    fallbacks: Vec<PlanAttempt>,
    primary_dpi_percent: Option<u32>,
}

impl VirtualPlan {
    pub fn activate(&mut self, log_dir: &Path) -> Result<()> {
        // Persist the virtual-only topology for the initial connected monitor set (Set A)
        // while privacy is active. When a disabled external HDMI/DP monitor enters standby,
        // drops HPD, and later re-asserts HPD, Windows reloads Set A's database entry instead
        // of re-enabling the external monitor and restarting its standby/HPD oscillation loop.
        // Restore always writes the original layout back with SDC_SAVE_TO_DATABASE.
        self.activate_with_persistence(log_dir, ApplyPersistence::SaveToDatabase)?;
        self.sync_dpi(log_dir);
        Ok(())
    }

    pub fn sync_dpi(&self, log_dir: &Path) {
        let Some(target_percent) = self.primary_dpi_percent else {
            return;
        };
        let src = &self.active.topology.paths[0].sourceInfo;
        apply_source_dpi_percent(log_dir, src.adapterId, src.id, target_percent);
    }

    fn activate_with_persistence(
        &mut self,
        log_dir: &Path,
        persistence: ApplyPersistence,
    ) -> Result<()> {
        let mut last_phase = "apply";
        let mut last_code = match self.active.apply(log_dir, persistence) {
            Ok(()) => return Ok(()),
            Err(code) => code,
        };
        if persistence == ApplyPersistence::SaveToDatabase
            && last_code != 5
            && self.active.apply(log_dir, ApplyPersistence::Transient).is_ok()
        {
            return Ok(());
        }
        let remaining = std::mem::take(&mut self.fallbacks);
        for candidate in remaining {
            match candidate.validate(log_dir) {
                Ok(()) => match candidate.apply(log_dir, persistence) {
                    Ok(()) => {
                        self.active = candidate;
                        return Ok(());
                    }
                    Err(code) => {
                        if persistence == ApplyPersistence::SaveToDatabase
                            && code != 5
                            && candidate.apply(log_dir, ApplyPersistence::Transient).is_ok()
                        {
                            self.active = candidate;
                            return Ok(());
                        }
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
        log_available_targets(log_dir, secs);
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
        // Reapply transiently (without SDC_SAVE_TO_DATABASE): a sleeping external monitor
        // drops HPD into a reduced monitor set (Set B), and we must never overwrite Set B's
        // database entry so unplugging the external monitor later still uses the laptop panel.
        if self.active.apply(dir, ApplyPersistence::Transient).is_err() {
            // Hotplug can invalidate source and target IDs. Rebuild from current
            // available paths, always using the pre-privacy desktop as the template.
            let mut next = Topology::virtual_only_plan(dir, original)?;
            next.activate_with_persistence(dir, ApplyPersistence::Transient)?;
            *self = next;
        }
        self.sync_dpi(dir);
        Ok(true)
    }
}

const DISPLAYCONFIG_DEVICE_INFO_GET_DPI_SCALE: i32 = -3;
const DISPLAYCONFIG_DEVICE_INFO_SET_DPI_SCALE: i32 = -4;

#[repr(C)]
struct DisplayConfigSourceDpiScaleGet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    min_scale_rel: i32,
    cur_scale_rel: i32,
    max_scale_rel: i32,
}

#[repr(C)]
struct DisplayConfigSourceDpiScaleSet {
    header: DISPLAYCONFIG_DEVICE_INFO_HEADER,
    scale_rel: i32,
}

fn query_source_dpi(adapter: LUID, source_id: u32) -> Option<(i32, i32, i32)> {
    let mut info = DisplayConfigSourceDpiScaleGet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_DPI_SCALE,
            size: size_of::<DisplayConfigSourceDpiScaleGet>() as u32,
            adapterId: adapter,
            id: source_id,
        },
        min_scale_rel: 0,
        cur_scale_rel: 0,
        max_scale_rel: 0,
    };
    let status = unsafe { DisplayConfigGetDeviceInfo(&mut info.header) };
    (status == 0).then_some((info.min_scale_rel, info.cur_scale_rel, info.max_scale_rel))
}

fn source_dpi_percent(adapter: LUID, source_id: u32) -> Option<u32> {
    let (min_rel, cur_rel, _) = query_source_dpi(adapter, source_id)?;
    dpi_scale_percent(min_rel, cur_rel)
}

fn apply_source_dpi_percent(log_dir: &Path, adapter: LUID, source_id: u32, target_percent: u32) {
    let Some((min_rel, cur_rel, max_rel)) = query_source_dpi(adapter, source_id) else {
        append_display_log(
            log_dir,
            &format!(
                "{} phase=dpi_sync status=query_failed target_pct={target_percent}",
                now_secs()
            ),
        );
        return;
    };
    let cur_pct = dpi_scale_percent(min_rel, cur_rel).unwrap_or(0);
    let Some(desired_rel) = relative_dpi_for_percent(min_rel, max_rel, target_percent) else {
        return;
    };
    if desired_rel == cur_rel {
        append_display_log(
            log_dir,
            &format!(
                "{} phase=dpi_sync status=unchanged cur_pct={cur_pct} target_pct={target_percent}",
                now_secs()
            ),
        );
        return;
    }
    let mut set = DisplayConfigSourceDpiScaleSet {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_SET_DPI_SCALE,
            size: size_of::<DisplayConfigSourceDpiScaleSet>() as u32,
            adapterId: adapter,
            id: source_id,
        },
        scale_rel: desired_rel,
    };
    let status = unsafe { DisplayConfigSetDeviceInfo(&mut set.header) };
    append_display_log(
        log_dir,
        &format!(
            "{} phase=dpi_sync status={status} cur_pct={cur_pct} target_pct={target_percent} rel={desired_rel}",
            now_secs()
        ),
    );
}

fn log_available_targets(log_dir: &Path, secs: u64) {
    let Ok(all) = Topology::query(QDC_ALL_PATHS) else {
        append_display_log(
            log_dir,
            &format!("{secs} phase=verify_available status=query_failed"),
        );
        return;
    };
    let mut entries: Vec<AvailableTargetLogInfo> = Vec::new();
    for path in &all.paths {
        let active = path.flags & DISPLAYCONFIG_PATH_ACTIVE != 0;
        let available = path.targetInfo.targetAvailable != 0;
        if !available && !active {
            continue;
        }
        let key = (
            path.targetInfo.adapterId.HighPart,
            path.targetInfo.adapterId.LowPart,
            path.targetInfo.id,
        );
        let info = AvailableTargetLogInfo {
            adapter_high: key.0,
            adapter_low: key.1,
            target_id: key.2,
            output_technology: path.targetInfo.outputTechnology,
            target_available: available,
            active,
            status_flags: path.targetInfo.statusFlags,
        };
        if let Some(existing) = entries.iter_mut().find(|e| {
            (e.adapter_high, e.adapter_low, e.target_id) == key
        }) {
            if info.active && !existing.active {
                *existing = info;
            }
        } else {
            entries.push(info);
        }
    }
    entries.sort_by_key(|e| (e.adapter_high, e.adapter_low, e.target_id));
    for (idx, entry) in entries.iter().enumerate() {
        append_display_log(
            log_dir,
            &format_available_target_log(secs, "verify_available", idx, entry),
        );
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
        let mut displays = Vec::new();
        for path in &self.paths {
            let identity = target_identity(path)
                .ok_or_else(|| "无法识别原显示器，未开启隐私。".to_string())?;
            let idx = unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
            let mode = self
                .modes
                .get(idx)
                .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
                .ok_or("无法记录原显示模式")?;
            let source = unsafe { mode.Anonymous.sourceMode };
            displays.push(Target {
                identity,
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

    pub fn path_count(&self) -> usize {
        self.paths.len()
    }

    pub fn matches_primary_only(&self, now: &Topology) -> bool {
        let [actual] = now.paths.as_slice() else {
            return false;
        };
        let Some((wanted, wanted_src)) = self.primary_path() else {
            return false;
        };
        actual.targetInfo.adapterId.HighPart == wanted.targetInfo.adapterId.HighPart
            && actual.targetInfo.adapterId.LowPart == wanted.targetInfo.adapterId.LowPart
            && actual.targetInfo.id == wanted.targetInfo.id
            && now.primary_resolution() == Some((wanted_src.width, wanted_src.height))
    }

    /// Activate only the pre-privacy primary monitor transiently (without saving to the CCD
    /// database). Used when exiting privacy on multi-monitor setups so Sunshine drops MttVDD
    /// (or any transient secondary display) and locks onto the primary display before secondary
    /// monitors are re-attached.
    pub fn restore_primary_only(&self, log_dir: Option<&Path>) -> Result<()> {
        let (primary, source) = self
            .primary_path()
            .ok_or_else(|| "无法定位原主屏配置".to_string())?;
        let mut full_path = *primary;
        let mut full_modes = Vec::new();
        let src_old = unsafe { primary.sourceInfo.Anonymous.modeInfoIdx } as usize;
        if let Some(m) = self
            .modes
            .get(src_old)
            .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
        {
            full_path.sourceInfo.Anonymous.modeInfoIdx = full_modes.len() as u32;
            full_modes.push(*m);
        } else {
            full_path.sourceInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
        }
        let tgt_old = unsafe { primary.targetInfo.Anonymous.modeInfoIdx } as usize;
        if let Some(m) = self
            .modes
            .get(tgt_old)
            .filter(|m| m.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_TARGET)
        {
            full_path.targetInfo.Anonymous.modeInfoIdx = full_modes.len() as u32;
            full_modes.push(*m);
        } else {
            full_path.targetInfo.Anonymous.modeInfoIdx = MODE_IDX_INVALID;
        }
        let full = Self {
            paths: vec![full_path],
            modes: full_modes,
        };
        if full
            .apply_config(
                log_dir,
                "restore_primary_exact",
                "primary_only_exact",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY,
            )
            .is_ok()
        {
            return Ok(());
        }
        if full
            .apply_config(
                log_dir,
                "restore_primary_allow_changes",
                "primary_only_allow_changes",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_ALLOW_CHANGES,
            )
            .is_ok()
        {
            return Ok(());
        }
        let mut source_path = normalized_path_without_target_mode(*primary, 0);
        source_path.targetInfo.rotation = normalize_rotation(primary.targetInfo.rotation);
        let mut mode = DISPLAYCONFIG_MODE_INFO::default();
        mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
        mode.id = source_path.sourceInfo.id;
        mode.adapterId = source_path.sourceInfo.adapterId;
        mode.Anonymous.sourceMode = source;
        let source_only = Self {
            paths: vec![source_path],
            modes: vec![mode],
        };
        source_only
            .apply_config(
                log_dir,
                "restore_primary_source_modes",
                "primary_only_source",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_ALLOW_CHANGES,
            )
            .map_err(|code| format_display_error("restore", code))
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
                SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_SAVE_TO_DATABASE,
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
                SDC_USE_SUPPLIED_DISPLAY_CONFIG
                    | SDC_APPLY
                    | SDC_ALLOW_CHANGES
                    | SDC_SAVE_TO_DATABASE,
            )
            .is_ok()
        {
            return Ok(());
        }

        // Physical outputs (especially DisplayPort/HDMI) may renegotiate target timings after
        // being disabled. Retain the exact multi-monitor source layout while letting Windows
        // negotiate fresh target modes, and write the restored layout back to the CCD database.
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
        if source_only
            .apply_config(
                log_dir,
                "restore_source_modes",
                "supplied_source_only",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG
                    | SDC_APPLY
                    | SDC_ALLOW_CHANGES
                    | SDC_SAVE_TO_DATABASE,
            )
            .is_ok()
        {
            return Ok(());
        }
        source_only
            .apply_config(
                log_dir,
                "restore_source_modes_force_enum",
                "supplied_source_only",
                SDC_USE_SUPPLIED_DISPLAY_CONFIG
                    | SDC_APPLY
                    | SDC_ALLOW_CHANGES
                    | SDC_SAVE_TO_DATABASE
                    | SDC_FORCE_MODE_ENUMERATION,
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
        let primary_dpi_percent =
            source_dpi_percent(primary.sourceInfo.adapterId, primary.sourceInfo.id);
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
                        primary_dpi_percent,
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
        for _ in 0..30 {
            if original.restore(Some(&dir)).is_err()
                && restore_targets(Some(&dir), &targets).is_err()
                && !Topology::active().is_ok_and(|s| s.has_physical_output())
            {
                let _ = restore_database(Some(&dir));
            }
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

fn target_identity(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    use sha2::{Digest, Sha256};
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
        return None;
    }
    let raw = String::from_utf16_lossy(&name.monitorDevicePath[..count]).to_lowercase();
    Some(format!("{:x}", Sha256::digest(raw.as_bytes())))
}

fn apply_target_layout(
    log_dir: Option<&Path>,
    phase: &str,
    entries: &[(&Target, Vec<DISPLAYCONFIG_PATH_INFO>)],
    persistence: ApplyPersistence,
    normalize_origin: bool,
) -> Result<()> {
    let candidate_keys: Vec<Vec<(i32, u32, u32)>> = entries
        .iter()
        .map(|(_, paths)| {
            paths
                .iter()
                .map(|p| {
                    (
                        p.sourceInfo.adapterId.HighPart,
                        p.sourceInfo.adapterId.LowPart,
                        p.sourceInfo.id,
                    )
                })
                .collect()
        })
        .collect();
    let chosen = select_distinct_source_paths(&candidate_keys)
        .ok_or_else(|| "无法为恢复的显示器分配独立源路径".to_string())?;
    let mut positions: Vec<(i32, i32)> = entries.iter().map(|(t, _)| (t.x, t.y)).collect();
    if normalize_origin {
        normalize_origin_positions(&mut positions);
    }
    let mut paths = Vec::with_capacity(entries.len());
    let mut modes = Vec::with_capacity(entries.len());
    for (idx, ((target, candidates), &choice)) in entries.iter().zip(&chosen).enumerate() {
        let mut path = normalized_path_without_target_mode(candidates[choice], idx as u32);
        path.targetInfo.rotation = normalize_rotation(target.rotation);
        let mut mode = DISPLAYCONFIG_MODE_INFO::default();
        mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
        mode.id = path.sourceInfo.id;
        mode.adapterId = path.sourceInfo.adapterId;
        mode.Anonymous.sourceMode = DISPLAYCONFIG_SOURCE_MODE {
            width: target.width,
            height: target.height,
            pixelFormat: DISPLAYCONFIG_PIXELFORMAT_32BPP,
            position: windows_sys::Win32::Foundation::POINTL {
                x: positions[idx].0,
                y: positions[idx].1,
            },
        };
        paths.push(path);
        modes.push(mode);
    }
    let save = match persistence {
        ApplyPersistence::SaveToDatabase => SDC_SAVE_TO_DATABASE,
        ApplyPersistence::Transient => 0,
    };
    let topo = Topology { paths, modes };
    topo.apply_config(
        log_dir,
        phase,
        "targets_source_only",
        SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_APPLY | SDC_ALLOW_CHANGES | save,
    )
    .map_err(|code| format_display_error("restore", code))
}

/// Rebuild and apply the pre-privacy display layout directly from persisted target identities
/// (`QDC_ALL_PATHS`), independent of adapter LUIDs or the CCD database.
///
/// Returns `Ok(true)` when all original targets were available and restored (saving back to the
/// CCD database). When an external monitor is still in standby/HPD-low, wakes any available
/// physical subset transiently (without modifying Set B's database) and returns `Ok(false)`.
pub fn restore_targets(log_dir: Option<&Path>, wanted: &[Target]) -> Result<bool> {
    if wanted.is_empty() {
        return Err("缺少待恢复的显示器记录".into());
    }
    let all = Topology::query(QDC_ALL_PATHS)?;
    let mut available_by_identity: std::collections::HashMap<String, Vec<DISPLAYCONFIG_PATH_INFO>> =
        std::collections::HashMap::new();
    for path in all.paths {
        if path.targetInfo.targetAvailable == 0 {
            continue;
        }
        if let Some(id) = target_identity(&path) {
            available_by_identity.entry(id).or_default().push(path);
        }
    }
    for paths in available_by_identity.values_mut() {
        paths.sort_by_key(|p| {
            candidate_sort_key(
                p.flags & DISPLAYCONFIG_PATH_ACTIVE != 0,
                p.targetInfo.id,
                p.sourceInfo.id,
            )
        });
    }

    let mut matched: Vec<(&Target, Vec<DISPLAYCONFIG_PATH_INFO>)> = Vec::new();
    for target in wanted {
        if let Some(paths) = available_by_identity.get(&target.identity) {
            matched.push((target, paths.clone()));
        }
    }
    if matched.len() == wanted.len() {
        apply_target_layout(
            log_dir,
            "restore_targets_full",
            &matched,
            ApplyPersistence::SaveToDatabase,
            false,
        )?;
        return Ok(true);
    }

    // External monitor may still be asleep (HPD low). If no physical monitor is active yet,
    // wake the available physical subset transiently to bring the GPU and sleeping monitor back
    // without polluting Set B's CCD database entry.
    if Topology::active().is_ok_and(|s| s.has_physical_output()) {
        return Ok(false);
    }
    let physical: Vec<(&Target, Vec<DISPLAYCONFIG_PATH_INFO>)> = matched
        .into_iter()
        .filter_map(|(t, paths)| {
            let phys: Vec<_> = paths
                .into_iter()
                .filter(|p| !is_mttvdd(p.targetInfo.adapterId))
                .collect();
            (!phys.is_empty()).then_some((t, phys))
        })
        .collect();
    if !physical.is_empty() {
        apply_target_layout(
            log_dir,
            "restore_targets_wake_physical",
            &physical,
            ApplyPersistence::Transient,
            true,
        )?;
    }
    Ok(false)
}

pub fn restore_database(log_dir: Option<&Path>) -> Result<()> {
    // Fallback when neither the in-memory topology nor persisted target identities could be
    // reapplied (e.g. a sleeping external monitor left Windows on an untouched reduced set).
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
