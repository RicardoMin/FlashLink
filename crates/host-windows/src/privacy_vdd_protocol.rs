//! Pure, platform-independent protocol and lease state machine for the scoped
//! `FlashLinkVddHelper` service that dynamically enables `Root\MttVDD` only
//! during active privacy sessions.

use super::privacy_display_plan::ApplyPersistence;

pub const SERVICE_NAME: &str = "FlashLinkVddHelper";
pub const SERVICE_DISPLAY_NAME: &str = "FlashLink Virtual Display Helper";
pub const SERVICE_DESCRIPTION: &str =
    "仅在 FlashLink 隐私模式期间动态启用和停用虚拟显示器（Root\\MttVDD）。";
pub const PIPE_NAME: &str = r"\\.\pipe\FlashLinkVddHelper";
pub const HELPER_EXE_NAME: &str = "FlashLinkVddHelper.exe";
#[cfg(test)]
pub const PIPE_RIGHT_READ_DATA: u32 = 0x0000_0001;
#[cfg(test)]
pub const PIPE_RIGHT_WRITE_DATA: u32 = 0x0000_0002;
#[cfg(test)]
pub const PIPE_RIGHT_CREATE_INSTANCE: u32 = 0x0000_0004;
#[cfg(test)]
pub const PIPE_RIGHT_READ_ATTRIBUTES: u32 = 0x0000_0080;
#[cfg(test)]
pub const PIPE_RIGHT_SYNCHRONIZE: u32 = 0x0010_0000;
/// Least-privilege client access mask for synchronous duplex pipe I/O + WaitNamedPipeW.
/// Explicitly omits FILE_CREATE_PIPE_INSTANCE (0x0000_0004) so non-elevated users cannot
/// create competing server instances on the same pipe name.
#[cfg(test)]
pub const CLIENT_PIPE_ACCESS: u32 = PIPE_RIGHT_READ_DATA
    | PIPE_RIGHT_WRITE_DATA
    | PIPE_RIGHT_READ_ATTRIBUTES
    | PIPE_RIGHT_SYNCHRONIZE;
/// Allow LocalSystem (SY) and Built-in Administrators (BA) full pipe access, and
/// Interactive Users (IU) only the specific client I/O mask (`0x00100083`), excluding
/// `FILE_CREATE_PIPE_INSTANCE`.
pub const PIPE_SDDL: &str = "D:(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100083;;;IU)";
pub const MAX_LINE_BYTES: usize = 128;
/// Protocol/ACL revision reported by `STATUS`. Legacy PR test builds returned
/// unversioned `"READY"` (treated as version 1) with a wider `GW` pipe ACL;
/// bumping to version 2 forces `prepare()` to upgrade existing helper services.
pub const HELPER_PROTOCOL_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Status,
    Acquire,
    Release,
    Disable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    Ready(u32),
    Unmanaged,
    Ok,
    Busy,
    Err(String),
}

pub fn parse_request(line: &str) -> Option<Request> {
    match line.trim() {
        "STATUS" => Some(Request::Status),
        "ACQUIRE" => Some(Request::Acquire),
        "RELEASE" => Some(Request::Release),
        "DISABLE" => Some(Request::Disable),
        _ => None,
    }
}

pub fn format_request(req: Request) -> &'static str {
    match req {
        Request::Status => "STATUS\n",
        Request::Acquire => "ACQUIRE\n",
        Request::Release => "RELEASE\n",
        Request::Disable => "DISABLE\n",
    }
}

pub fn parse_response(line: &str) -> Option<Response> {
    let trimmed = line.trim();
    match trimmed {
        "READY" => Some(Response::Ready(1)),
        "UNMANAGED" => Some(Response::Unmanaged),
        "OK" => Some(Response::Ok),
        "BUSY" => Some(Response::Busy),
        _ => {
            if let Some(version) = trimmed.strip_prefix("READY ") {
                let parsed = version.parse::<u32>().ok().filter(|&v| v > 0)?;
                return Some(Response::Ready(parsed));
            }
            trimmed
                .strip_prefix("ERR ")
                .map(|msg| Response::Err(msg.to_string()))
        }
    }
}

pub fn is_helper_current(resp: Option<&Response>) -> bool {
    matches!(resp, Some(Response::Ready(version)) if *version >= HELPER_PROTOCOL_VERSION)
}

pub fn format_response(resp: &Response) -> String {
    match resp {
        Response::Ready(version) => format!("READY {version}\n"),
        Response::Unmanaged => "UNMANAGED\n".to_string(),
        Response::Ok => "OK\n".to_string(),
        Response::Busy => "BUSY\n".to_string(),
        Response::Err(msg) => {
            let clean: String = msg
                .chars()
                .map(|c| if c == '\r' || c == '\n' { ' ' } else { c })
                .take(96)
                .collect();
            format!("ERR {clean}\n")
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseAction {
    Enable,
    Disable,
    Noop,
}

/// Refcounted lease state machine for `Root\MttVDD`.
/// `Root\MttVDD` is enabled on the `0 -> 1` transition and disabled on `1 -> 0`.
#[derive(Default, Debug)]
pub struct LeaseTracker {
    active_leases: usize,
}

impl LeaseTracker {
    pub fn acquire(&mut self) -> LeaseAction {
        self.active_leases += 1;
        if self.active_leases == 1 {
            LeaseAction::Enable
        } else {
            LeaseAction::Noop
        }
    }

    pub fn release(&mut self) -> LeaseAction {
        if self.active_leases == 0 {
            return LeaseAction::Noop;
        }
        self.active_leases -= 1;
        if self.active_leases == 0 {
            LeaseAction::Disable
        } else {
            LeaseAction::Noop
        }
    }

    pub fn can_force_disable(&self) -> bool {
        self.active_leases == 0
    }

    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active_leases
    }
}

/// When `Root\MttVDD` is dynamically attached only during privacy mode (`dynamic_vdd == true`),
/// reduced monitor sets containing MttVDD (such as `{Internal, MttVDD}` during external monitor
/// standby HPD drops) never occur outside privacy mode. Persisting the virtual-only layout for
/// those reduced privacy sets prevents repeated Set-B bounces without risking a black laptop
/// screen when the external monitor is unplugged outside privacy mode.
///
/// When `MttVDD` is permanently attached (`dynamic_vdd == false`), reduced-set reapplies MUST
/// remain strictly transient so unplugging the external monitor still loads the laptop panel.
pub fn reapply_persistence(dynamic_vdd: bool) -> ApplyPersistence {
    if dynamic_vdd {
        ApplyPersistence::SaveToDatabase
    } else {
        ApplyPersistence::Transient
    }
}

pub fn service_command_line(exe_path: &str) -> String {
    format!("\"{exe_path}\" --vdd-service")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    Stopped,
    NotActive,
}

/// Safely orchestrate replacing an existing helper service binary:
/// 1. When updating an existing service, confirm the service has reached `SERVICE_STOPPED`
///    (or was already inactive) before touching the executable; any stop-wait timeout or
///    status-query error is returned immediately without attempting replacement.
/// 2. If binary replacement fails after stopping an active service, attempt to restart the
///    previous service binary before returning the replacement error.
pub fn apply_service_update<E>(
    has_existing: bool,
    stop_existing: impl FnOnce() -> Result<StopOutcome, E>,
    replace_binary: impl FnOnce() -> Result<(), E>,
    restart_existing: impl FnOnce(),
) -> Result<(), E> {
    let stopped_active = if has_existing {
        stop_existing()? == StopOutcome::Stopped
    } else {
        false
    };
    if let Err(err) = replace_binary() {
        if stopped_active {
            restart_existing();
        }
        return Err(err);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_update_aborts_without_replacing_binary_when_stop_times_out() {
        let mut replaced = false;
        let mut restarted = false;
        let result = apply_service_update(
            true,
            || Err("stop timed out"),
            || {
                replaced = true;
                Ok(())
            },
            || {
                restarted = true;
            },
        );
        assert_eq!(result, Err("stop timed out"));
        assert!(!replaced);
        assert!(!restarted);
    }

    #[test]
    fn service_update_restarts_previous_service_when_replace_fails_after_stop() {
        let mut replaced = false;
        let mut restarted = false;
        let result = apply_service_update(
            true,
            || Ok(StopOutcome::Stopped),
            || {
                replaced = true;
                Err("access denied")
            },
            || {
                restarted = true;
            },
        );
        assert_eq!(result, Err("access denied"));
        assert!(replaced);
        assert!(restarted);
    }

    #[test]
    fn service_update_does_not_restart_when_previous_service_was_not_active() {
        let mut restarted = false;
        let result = apply_service_update(
            true,
            || Ok(StopOutcome::NotActive),
            || Err("disk full"),
            || {
                restarted = true;
            },
        );
        assert_eq!(result, Err("disk full"));
        assert!(!restarted);

        let mut fresh_restarted = false;
        let fresh = apply_service_update(
            false,
            || unreachable!("fresh install must not stop a service"),
            || Err("disk full"),
            || {
                fresh_restarted = true;
            },
        );
        assert_eq!(fresh, Err("disk full"));
        assert!(!fresh_restarted);
    }

    #[test]
    fn round_trips_requests_and_responses() {
        for req in [
            Request::Status,
            Request::Acquire,
            Request::Release,
            Request::Disable,
        ] {
            assert_eq!(parse_request(format_request(req)), Some(req));
        }
        assert_eq!(parse_request("EXEC calc.exe\n"), None);
        assert_eq!(parse_request(""), None);

        for resp in [
            Response::Ready(HELPER_PROTOCOL_VERSION),
            Response::Unmanaged,
            Response::Ok,
            Response::Busy,
            Response::Err("device busy".into()),
        ] {
            let wire = format_response(&resp);
            assert!(wire.ends_with('\n'));
            assert_eq!(parse_response(&wire), Some(resp));
        }
        let sanitized = format_response(&Response::Err("line1\r\nline2".into()));
        assert_eq!(sanitized, "ERR line1  line2\n");
    }

    #[test]
    fn legacy_unversioned_ready_response_triggers_helper_upgrade() {
        let legacy = parse_response("READY\n");
        assert_eq!(legacy, Some(Response::Ready(1)));
        assert!(!is_helper_current(legacy.as_ref()));

        let current = parse_response(&format!("READY {HELPER_PROTOCOL_VERSION}\n"));
        assert_eq!(current, Some(Response::Ready(HELPER_PROTOCOL_VERSION)));
        assert!(is_helper_current(current.as_ref()));

        assert_eq!(parse_response("READY 0\n"), None);
        assert_eq!(parse_response("READY abc\n"), None);
        assert!(!is_helper_current(Some(&Response::Unmanaged)));
        assert!(!is_helper_current(None));
    }

    #[test]
    fn lease_tracker_enables_on_first_acquire_and_disables_on_last_release() {
        let mut tracker = LeaseTracker::default();
        assert!(tracker.can_force_disable());
        assert_eq!(tracker.acquire(), LeaseAction::Enable);
        assert!(!tracker.can_force_disable());
        assert_eq!(tracker.active_count(), 1);
        assert_eq!(tracker.acquire(), LeaseAction::Noop);
        assert_eq!(tracker.active_count(), 2);
        assert_eq!(tracker.release(), LeaseAction::Noop);
        assert!(!tracker.can_force_disable());
        assert_eq!(tracker.release(), LeaseAction::Disable);
        assert!(tracker.can_force_disable());
        assert_eq!(tracker.release(), LeaseAction::Noop);
    }

    #[test]
    fn persists_reduced_set_reapply_only_when_vdd_is_dynamically_managed() {
        assert_eq!(reapply_persistence(true), ApplyPersistence::SaveToDatabase);
        assert_eq!(reapply_persistence(false), ApplyPersistence::Transient);
    }

    #[test]
    fn pipe_sddl_restricts_interactive_users_to_client_access_without_create_instance() {
        assert_eq!(CLIENT_PIPE_ACCESS, 0x0010_0083);
        assert_eq!(CLIENT_PIPE_ACCESS & PIPE_RIGHT_CREATE_INSTANCE, 0);
        let expected_ace = format!("(A;;0x{CLIENT_PIPE_ACCESS:08x};;;IU)");
        assert!(PIPE_SDDL.contains(&expected_ace));
        assert!(!PIPE_SDDL.contains("GW;;;IU)"));
        assert!(!PIPE_SDDL.contains("GA;;;IU)"));
    }

    #[test]
    fn quotes_service_binary_path_with_spaces() {
        assert_eq!(
            service_command_line(
                r"C:\Program Files\FlashLink Virtual Display 25.7.23\FlashLinkVddHelper.exe"
            ),
            "\"C:\\Program Files\\FlashLink Virtual Display 25.7.23\\FlashLinkVddHelper.exe\" --vdd-service"
        );
    }
}
