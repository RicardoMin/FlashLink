//! Pure, platform-independent helpers for selecting virtual-only Windows display
//! plans, normalizing CCD path fields, and formatting privacy display diagnostics.

pub const MODE_IDX_INVALID: u32 = u32::MAX;
const ROTATION_IDENTITY: i32 = 1;
const ROTATION_MAX: i32 = 4;
const SCALING_PREFERRED: i32 = 128;
pub const DPI_SCALE_VALUES: &[u32] = &[100, 125, 150, 175, 200, 225, 250, 300, 350, 400, 450, 500];

pub fn dpi_scale_percent(min_scale_rel: i32, cur_scale_rel: i32) -> Option<u32> {
    let rec_idx = -min_scale_rel;
    let cur_idx = rec_idx.checked_add(cur_scale_rel)?;
    usize::try_from(cur_idx)
        .ok()
        .and_then(|idx| DPI_SCALE_VALUES.get(idx).copied())
}

pub fn relative_dpi_for_percent(
    min_scale_rel: i32,
    max_scale_rel: i32,
    target_percent: u32,
) -> Option<i32> {
    let target_idx = DPI_SCALE_VALUES
        .iter()
        .position(|&value| value == target_percent)? as i32;
    let rec_idx = -min_scale_rel;
    Some((target_idx - rec_idx).clamp(min_scale_rel, max_scale_rel))
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStrategy {
    SuppliedFullModes,
    SuppliedSourceOnly,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyPersistence {
    SaveToDatabase,
    Transient,
}

impl PlanStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SuppliedFullModes => "supplied_full_modes",
            Self::SuppliedSourceOnly => "supplied_source_only",
        }
    }
}

pub fn candidate_sort_key(active: bool, target_id: u32, source_id: u32) -> (bool, u32, u32) {
    (!active, target_id, source_id)
}

/// Choose one candidate index per target so that no two selected paths on the
/// same adapter share a source ID (which Windows treats as clone or rejects).
pub fn select_distinct_source_paths(
    candidates_per_target: &[Vec<(i32, u32, u32)>],
) -> Option<Vec<usize>> {
    fn backtrack(
        idx: usize,
        candidates_per_target: &[Vec<(i32, u32, u32)>],
        used_sources: &mut Vec<(i32, u32, u32)>,
        chosen: &mut Vec<usize>,
    ) -> bool {
        if idx == candidates_per_target.len() {
            return true;
        }
        for (choice_idx, &source_key) in candidates_per_target[idx].iter().enumerate() {
            if used_sources.contains(&source_key) {
                continue;
            }
            used_sources.push(source_key);
            chosen.push(choice_idx);
            if backtrack(idx + 1, candidates_per_target, used_sources, chosen) {
                return true;
            }
            chosen.pop();
            used_sources.pop();
        }
        false
    }

    let mut used = Vec::with_capacity(candidates_per_target.len());
    let mut chosen = Vec::with_capacity(candidates_per_target.len());
    backtrack(0, candidates_per_target, &mut used, &mut chosen).then_some(chosen)
}

/// Windows requires at least one active source mode at (0, 0). When restoring a
/// physical subset while the former primary monitor is still asleep, shift the
/// subset so its first entry sits at (0, 0).
pub fn normalize_origin_positions(positions: &mut [(i32, i32)]) {
    if positions.is_empty() || positions.iter().any(|&(x, y)| x == 0 && y == 0) {
        return;
    }
    let (dx, dy) = positions[0];
    for (x, y) in positions.iter_mut() {
        *x -= dx;
        *y -= dy;
    }
}

pub fn normalize_rotation(raw: i32) -> i32 {
    if (ROTATION_IDENTITY..=ROTATION_MAX).contains(&raw) {
        raw
    } else {
        ROTATION_IDENTITY
    }
}

pub fn normalize_scaling(raw: i32) -> i32 {
    if (1..=5).contains(&raw) || raw == SCALING_PREFERRED {
        raw
    } else {
        SCALING_PREFERRED
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathLogInfo {
    pub adapter_high: i32,
    pub adapter_low: u32,
    pub source_id: u32,
    pub target_id: u32,
    pub path_flags: u32,
    pub source_mode_idx: u32,
    pub target_mode_idx: u32,
    pub source_rect: Option<(i32, i32, u32, u32)>,
    pub rotation: i32,
    pub scaling: i32,
    pub refresh_num: u32,
    pub refresh_den: u32,
}

fn format_mode_idx(idx: u32) -> String {
    if idx == MODE_IDX_INVALID {
        "invalid".into()
    } else {
        idx.to_string()
    }
}

pub fn format_path_fields(p: &PathLogInfo) -> String {
    let rect = match p.source_rect {
        Some((x, y, w, h)) => format!("{x},{y},{w}x{h}"),
        None => "none".into(),
    };
    format!(
        "adapter={}:0x{:08x} src={} tgt={} path_flags=0x{:08x} src_mode_idx={} tgt_mode_idx={} src_rect={} rot={} scale={} rr={}/{}",
        p.adapter_high,
        p.adapter_low,
        p.source_id,
        p.target_id,
        p.path_flags,
        format_mode_idx(p.source_mode_idx),
        format_mode_idx(p.target_mode_idx),
        rect,
        p.rotation,
        p.scaling,
        p.refresh_num,
        p.refresh_den,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn format_call_log(
    secs: u64,
    phase: &str,
    strategy: &str,
    flags: u32,
    status: i32,
    path_count: usize,
    mode_count: usize,
    single_path: Option<&PathLogInfo>,
) -> String {
    let base = format!(
        "{secs} phase={phase} strategy={strategy} flags=0x{flags:08x} status={status} paths={path_count} modes={mode_count}"
    );
    match single_path {
        Some(path) => format!("{base} {}", format_path_fields(path)),
        None => base,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AvailableTargetLogInfo {
    pub adapter_high: i32,
    pub adapter_low: u32,
    pub target_id: u32,
    pub output_technology: i32,
    pub target_available: bool,
    pub active: bool,
    pub status_flags: u32,
}

pub fn format_available_target_log(
    secs: u64,
    phase: &str,
    index: usize,
    t: &AvailableTargetLogInfo,
) -> String {
    format!(
        "{secs} phase={phase} index={index} adapter={}:0x{:08x} tgt={} tech={} avail={} active={} status_flags=0x{:08x}",
        t.adapter_high,
        t.adapter_low,
        t.target_id,
        t.output_technology,
        if t.target_available { 1 } else { 0 },
        if t.active { 1 } else { 0 },
        t.status_flags,
    )
}

pub fn format_indexed_path_log(
    secs: u64,
    phase: &str,
    index: usize,
    path: &PathLogInfo,
) -> String {
    format!(
        "{secs} phase={phase} index={index} {}",
        format_path_fields(path)
    )
}

pub fn format_display_error(phase: &str, code: i32) -> String {
    let label = match phase {
        "validate" => "校验",
        "apply" => "应用",
        "restore" | "restore_exact" | "restore_allow_changes" | "restore_source_modes" => "恢复",
        _ => phase,
    };
    format!("Windows 拒绝显示配置（{label}阶段，错误码 {code}，详见 privacy-display.log）")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sorts_active_virtual_candidates_before_inactive_ones() {
        let mut items = vec![
            (false, 0, 1),
            (true, 1, 0),
            (false, 0, 0),
            (true, 0, 2),
            (true, 0, 0),
        ];
        items.sort_by_key(|&(active, tgt, src)| candidate_sort_key(active, tgt, src));
        assert_eq!(
            items,
            vec![
                (true, 0, 0),
                (true, 0, 2),
                (true, 1, 0),
                (false, 0, 0),
                (false, 0, 1),
            ]
        );
    }

    #[test]
    fn normalizes_invalid_inactive_rotation_and_scaling() {
        assert_eq!(normalize_rotation(0), ROTATION_IDENTITY);
        assert_eq!(normalize_rotation(-1), ROTATION_IDENTITY);
        assert_eq!(normalize_rotation(5), ROTATION_IDENTITY);
        for valid in 1..=4 {
            assert_eq!(normalize_rotation(valid), valid);
        }

        assert_eq!(normalize_scaling(0), SCALING_PREFERRED);
        assert_eq!(normalize_scaling(99), SCALING_PREFERRED);
        for valid in [1, 2, 3, 4, 5, 128] {
            assert_eq!(normalize_scaling(valid), valid);
        }
    }

    #[test]
    fn formats_structured_call_and_path_logs() {
        let path = PathLogInfo {
            adapter_high: 0,
            adapter_low: 0x12a34,
            source_id: 0,
            target_id: 1,
            path_flags: 1,
            source_mode_idx: 0,
            target_mode_idx: MODE_IDX_INVALID,
            source_rect: Some((0, 0, 1920, 1080)),
            rotation: 1,
            scaling: 128,
            refresh_num: 60,
            refresh_den: 1,
        };
        let line = format_call_log(
            1700000000,
            "validate",
            PlanStrategy::SuppliedSourceOnly.as_str(),
            0x0000_0460,
            87,
            1,
            1,
            Some(&path),
        );
        assert_eq!(
            line,
            "1700000000 phase=validate strategy=supplied_source_only flags=0x00000460 status=87 paths=1 modes=1 adapter=0:0x00012a34 src=0 tgt=1 path_flags=0x00000001 src_mode_idx=0 tgt_mode_idx=invalid src_rect=0,0,1920x1080 rot=1 scale=128 rr=60/1"
        );

        let multi = format_indexed_path_log(1700000001, "restore_path", 1, &path);
        assert!(multi.starts_with("1700000001 phase=restore_path index=1 adapter=0:0x00012a34 "));
        assert!(format_display_error("validate", 87).contains("校验阶段，错误码 87"));
        assert!(format_display_error("apply", 31).contains("应用阶段，错误码 31"));

        let avail = AvailableTargetLogInfo {
            adapter_high: 0,
            adapter_low: 0x11df5,
            target_id: 4355,
            output_technology: 5,
            target_available: true,
            active: false,
            status_flags: 1,
        };
        assert_eq!(
            format_available_target_log(1700000002, "verify_available", 1, &avail),
            "1700000002 phase=verify_available index=1 adapter=0:0x00011df5 tgt=4355 tech=5 avail=1 active=0 status_flags=0x00000001"
        );
    }

    #[test]
    fn assigns_distinct_sources_per_adapter_when_restoring_targets() {
        // Two targets on the same adapter where the first target can only use source 1
        // or prefers source 0 that conflicts with the second target's only source.
        let candidates = vec![
            vec![(0, 0x11df5, 0), (0, 0x11df5, 1)],
            vec![(0, 0x11df5, 0)],
            vec![(0, 0x5f4139e9, 0)],
        ];
        assert_eq!(select_distinct_source_paths(&candidates), Some(vec![1, 0, 0]));

        let impossible = vec![vec![(0, 0x11df5, 0)], vec![(0, 0x11df5, 0)]];
        assert_eq!(select_distinct_source_paths(&impossible), None);
    }

    #[test]
    fn normalizes_origin_when_primary_monitor_is_asleep() {
        let mut subset = vec![(-2560, 0)];
        normalize_origin_positions(&mut subset);
        assert_eq!(subset, vec![(0, 0)]);

        let mut dual = vec![(-2560, 0), (0, 0)];
        normalize_origin_positions(&mut dual);
        assert_eq!(dual, vec![(-2560, 0), (0, 0)]);
    }

    #[test]
    fn maps_windows_relative_dpi_scales_across_different_recommended_bases() {
        // Primary P2711V: recommended 150% (min=-2), current 175% (cur=+1).
        assert_eq!(dpi_scale_percent(-2, 1), Some(175));
        assert_eq!(dpi_scale_percent(-2, 0), Some(150));
        // Virtual display with 150% recommended (min=-2, max=6) -> +1 selects 175%.
        assert_eq!(relative_dpi_for_percent(-2, 6, 175), Some(1));
        // Virtual display with 100% recommended (min=0, max=4) -> +3 selects 175%.
        assert_eq!(relative_dpi_for_percent(0, 4, 175), Some(3));
        // Clamped when target exceeds adapter max.
        assert_eq!(relative_dpi_for_percent(0, 2, 175), Some(2));
    }
}
