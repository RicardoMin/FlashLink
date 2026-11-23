//! FlashLink's single local player seat. A foreground player is distinct from a
//! decoded stream: the Moonlight activity also handles pairing and app selection.
use std::net::IpAddr;
use std::sync::Mutex;

const INVALID: i64 = -1;
const BUSY: i64 = -2;
const UNAVAILABLE: i64 = -3;

#[repr(i32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Launching = 1,
    Player = 2,
}

struct Attempt {
    id: i64,
    host: String,
    phase: Phase,
}

#[derive(Default)]
struct Client {
    last_id: i64,
    active: Option<Attempt>,
}

impl Client {
    fn begin(&mut self, host: &str) -> i64 {
        if self.active.is_some() {
            return BUSY;
        }
        let Some(host) = normalize_host(host) else {
            return INVALID;
        };
        let Some(id) = self.last_id.checked_add(1) else {
            return UNAVAILABLE;
        };
        self.last_id = id;
        self.active = Some(Attempt {
            id,
            host,
            phase: Phase::Launching,
        });
        id
    }

    fn update(&mut self, id: i64, event: i32) -> bool {
        let Some(active) = self.active.as_mut().filter(|active| active.id == id) else {
            return false;
        };
        match event {
            1 if active.phase == Phase::Launching => active.phase = Phase::Player,
            2 | 3 => self.active = None,
            _ => return false,
        }
        true
    }
}

fn normalize_host(input: &str) -> Option<String> {
    let value = input.trim();
    if let Some(address) = simple_host(value) {
        return Some(address);
    }
    let (host, port) = value.rsplit_once(':')?;
    if !port.bytes().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let port = port.parse::<u16>().ok()?;
    if !(1029..=65514).contains(&port) {
        return None;
    }
    let address = simple_host(host)?;
    if host.starts_with('[') {
        return Some(format!("[{address}]:{port}"));
    }
    if host.contains(':') {
        return None;
    }
    Some(format!("{address}:{port}"))
}

fn simple_host(value: &str) -> Option<String> {
    if let Some(v6) = value.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        return v6
            .parse::<std::net::Ipv6Addr>()
            .ok()
            .map(|ip| ip.to_string());
    }
    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    let dns = value.strip_suffix('.').unwrap_or(value);
    if dns.is_empty() || dns.len() > 253 || dns.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    let valid = dns.split('.').all(|label| {
        !label.is_empty()
            && label.len() <= 63
            && !label.starts_with('-')
            && !label.ends_with('-')
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    });
    valid.then(|| dns.to_ascii_lowercase())
}

static CLIENT: Mutex<Client> = Mutex::new(Client {
    last_id: 0,
    active: None,
});

#[no_mangle]
pub extern "C" fn fl_client_abi_version() -> u32 {
    1
}

/// # Safety
/// `host` must be readable for `length` bytes during this call.
#[no_mangle]
pub unsafe extern "C" fn fl_client_begin(host: *const u8, length: usize) -> i64 {
    if host.is_null() || length == 0 || length > 1024 {
        return INVALID;
    }
    let bytes = unsafe { std::slice::from_raw_parts(host, length) };
    let Ok(value) = std::str::from_utf8(bytes) else {
        return INVALID;
    };
    CLIENT
        .lock()
        .map_or(UNAVAILABLE, |mut client| client.begin(value))
}

/// # Safety
/// `out` must be writable for `capacity` bytes during this call.
#[no_mangle]
pub unsafe extern "C" fn fl_client_target(id: i64, out: *mut u8, capacity: usize) -> i32 {
    if out.is_null() {
        return 0;
    }
    let Ok(client) = CLIENT.lock() else { return 0 };
    let Some(active) = client.active.as_ref().filter(|active| active.id == id) else {
        return 0;
    };
    let bytes = active.host.as_bytes();
    if bytes.len() > capacity {
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
    bytes.len() as i32
}

#[no_mangle]
pub extern "C" fn fl_client_phase() -> i32 {
    CLIENT.lock().map_or(UNAVAILABLE as i32, |c| {
        c.active.as_ref().map_or(0, |a| a.phase as i32)
    })
}

#[no_mangle]
pub extern "C" fn fl_client_event(id: i64, event: i32) -> i32 {
    CLIENT
        .lock()
        .map_or(0, |mut c| i32::from(c.update(id, event)))
}

// On activity recreation the original MethodChannel callback is lost. Android
// still delivers the player result with the attempt ID saved by the old activity.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_com_limelight_FlashLinkActivity_releaseRecoveredAttempt(
    _env: *mut std::ffi::c_void,
    _class: *mut std::ffi::c_void,
    id: i64,
) {
    fl_client_event(id, 2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_addresses_not_urls_or_shell_arguments() {
        for (input, expected) in [
            ("  Game-PC.LOCAL. ", "game-pc.local"),
            ("192.168.1.4", "192.168.1.4"),
            ("[fd01:0::f]", "fd01::f"),
            ("::1", "::1"),
            ("PC.local:49000", "pc.local:49000"),
            ("[fd00:0::1]:49000", "[fd00::1]:49000"),
        ] {
            assert_eq!(normalize_host(input).as_deref(), Some(expected));
        }
        for bad in [
            "",
            "-flag",
            "https://pc",
            "pc/remote",
            "pc:65535",
            "pc:",
            "pc:-1",
            "x..local",
            "[pc]",
            "example\0.local",
            "999.1.1.1",
            "xy z",
        ] {
            assert_eq!(normalize_host(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn local_seat_rejects_duplicate_launches_and_late_callbacks() {
        let mut state = Client::default();
        let first = state.begin("pc");
        assert_eq!(state.begin("other"), BUSY);
        assert!(state.update(first, 1));
        assert!(!state.update(first, 1));
        assert!(state.update(first, 2));
        let second = state.begin("second");
        assert!(second > first);
        assert!(!state.update(first, 3));
        assert_eq!(state.active.as_ref().unwrap().host, "second");
        assert!(state.update(second, 3));
        assert!(state.active.is_none());
    }
}
