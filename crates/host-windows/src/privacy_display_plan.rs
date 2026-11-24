//! Pure, platform-independent helpers for selecting virtual-only Windows display
//! plans, normalizing CCD path fields, and formatting privacy display diagnostics.

pub const MODE_IDX_INVALID: u32 = u32::MAX;
const ROTATION_IDENTITY: i32 = 1;
const ROTATION_MAX: i32 = 4;
const SCALING_PREFERRED: i32 = 128;
const DEFAULT_WIDTH: u32 = 1920;
const DEFAULT_HEIGHT: u32 = 1080;
const SECONDARY_FALLBACK_WIDTH: u32 = 1366;
const SECONDARY_FALLBACK_HEIGHT: u32 = 768;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanStrategy {
    SuppliedFullModes,
    SuppliedSourceOnly,
    TopologySupplied,
    SuppliedNoModes,
}

impl PlanStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SuppliedFullModes => "supplied_full_modes",
            Self::SuppliedSourceOnly => "supplied_source_only",
            Self::TopologySupplied => "topology_supplied",
            Self::SuppliedNoModes => "supplied_no_modes",
        }
    }
}

pub fn candidate_sort_key(active: bool, target_id: u32, source_id: u32) -> (bool, u32, u32) {
    (!active, target_id, source_id)
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

pub fn fallback_resolutions(primary: Option<(u32, u32)>) -> Vec<(u32, u32)> {
    let mut out = Vec::with_capacity(3);
    if let Some((w, h)) = primary {
        if (800..=7680).contains(&w) && (600..=4320).contains(&h) {
            out.push((w, h));
        }
    }
    for candidate in [
        (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        (SECONDARY_FALLBACK_WIDTH, SECONDARY_FALLBACK_HEIGHT),
    ] {
        if !out.contains(&candidate) {
            out.push(candidate);
        }
    }
    out
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
    fn builds_deduplicated_fallback_resolution_order() {
        assert_eq!(
            fallback_resolutions(Some((2560, 1440))),
            vec![(2560, 1440), (1920, 1080), (1366, 768)]
        );
        assert_eq!(
            fallback_resolutions(Some((1920, 1080))),
            vec![(1920, 1080), (1366, 768)]
        );
        assert_eq!(
            fallback_resolutions(Some((320, 240))),
            vec![(1920, 1080), (1366, 768)]
        );
        assert_eq!(fallback_resolutions(None), vec![(1920, 1080), (1366, 768)]);
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
    }
}
