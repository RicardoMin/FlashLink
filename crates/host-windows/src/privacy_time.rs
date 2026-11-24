use std::time::{Duration, Instant};

pub const STALE: Duration = Duration::from_secs(5);
pub const GRACE: Duration = Duration::from_secs(15);
// Bound a continuous loss of the virtual-only desktop. A successful mode-set
// call alone does not establish recovery: the active paths must be read back.
#[derive(Default)]
pub struct DisplayRepair {
    first_mismatch: Option<Instant>,
    writes: std::collections::VecDeque<Instant>,
}

impl DisplayRepair {
    pub fn can_repair(&mut self, now: Instant) -> bool {
        self.writes.retain(|t| now.saturating_duration_since(*t) < Duration::from_secs(30));
        self.writes.len() < 4
            && now.duration_since(*self.first_mismatch.get_or_insert(now)) < Duration::from_secs(5)
    }

    pub fn reapplied(&mut self, now: Instant) {
        self.writes.push_back(now);
    }

    pub fn verified(&mut self) {
        self.first_mismatch = None;
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Playing,
    Reconnecting,
    Expired,
}

pub struct Timer {
    last: Instant,
    state: State,
}
impl Timer {
    pub fn new(last: Instant) -> Self {
        Self {
            last,
            state: State::Playing,
        }
    }
    pub fn tick(&mut self, now: Instant, heartbeat: bool) -> State {
        // Expiration is irreversible, including a heartbeat arriving exactly at the deadline.
        let age = now.saturating_duration_since(self.last);
        if self.state == State::Expired || age >= STALE + GRACE {
            self.state = State::Expired;
        } else if heartbeat {
            self.last = now;
            self.state = State::Playing;
        } else if age >= STALE {
            self.state = State::Reconnecting;
        }
        self.state
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Desktop {
    Ready,
    Recovering,
    Locked,
    Unknown,
}
pub fn desktop_state(pending: bool, locked: Option<bool>) -> Desktop {
    if pending {
        return Desktop::Recovering;
    }
    match locked {
        Some(false) => Desktop::Ready,
        Some(true) => Desktop::Locked,
        None => Desktop::Unknown,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_mode_sets_end_privacy_even_if_windows_temporarily_recovers() {
        let start = Instant::now();
        let mut repair = DisplayRepair::default();
        for seconds in [0, 2, 13, 26] {
            let now = start + Duration::from_secs(seconds);
            assert!(repair.can_repair(now));
            repair.reapplied(now);
            repair.verified();
        }
        assert!(!repair.can_repair(start + Duration::from_secs(29)));
        assert!(repair.can_repair(start + Duration::from_secs(30)));
    }

    #[test]
    fn recovered_samples_reset_the_continuous_mismatch_deadline() {
        let start = Instant::now();
        let mut repair = DisplayRepair::default();
        for seconds in [0, 2, 4, 6] {
            assert!(repair.can_repair(start + Duration::from_secs(seconds)));
            repair.verified();
        }
        // Successful writes without an observed recovery must still time out.
        for seconds in 7..12 {
            assert!(repair.can_repair(start + Duration::from_secs(seconds)));
        }
        assert!(!repair.can_repair(start + Duration::from_secs(12)));
    }

    #[test]
    fn service_resumes_after_recovery_and_owner_unlock_in_either_order() {
        assert_eq!(desktop_state(true, Some(false)), Desktop::Recovering);
        assert_eq!(desktop_state(true, Some(true)), Desktop::Recovering);
        assert_eq!(desktop_state(false, Some(true)), Desktop::Locked);
        assert_eq!(desktop_state(false, None), Desktop::Unknown);
        assert_eq!(desktop_state(false, Some(false)), Desktop::Ready);
    }
    #[test]
    fn privacy_lease_restores_only_before_deadline() {
        let now = Instant::now();
        let mut lease = Timer::new(now);
        assert_eq!(lease.tick(now + STALE, false), State::Reconnecting);
        assert_eq!(
            lease.tick(now + STALE + GRACE - Duration::from_millis(1), true),
            State::Playing
        );
        assert_eq!(lease.tick(now + (STALE + GRACE) * 2, true), State::Expired);
        assert_eq!(lease.tick(now + (STALE + GRACE) * 2, true), State::Expired);
    }
}
