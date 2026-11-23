use std::time::{Duration, Instant};

pub const STALE: Duration = Duration::from_secs(5);
pub const GRACE: Duration = Duration::from_secs(15);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Playing,
    Reconnecting,
    Locking,
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
        if self.state == State::Locking || age >= STALE + GRACE {
            self.state = State::Locking;
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
pub enum RecoveryStep {
    Lock,
    Restore,
    Wait,
}
/// Unlocking after a GPU refused restoration on Winlogon's desktop gives the local owner
/// control again. The explicit local restore action is scoped to the current transaction.
pub fn recovery_step(locked: Option<bool>, saw_lock: bool, local_consent: bool) -> RecoveryStep {
    if locked == Some(true) || (locked == Some(false) && local_consent) {
        RecoveryStep::Restore
    } else if saw_lock {
        RecoveryStep::Wait
    } else {
        RecoveryStep::Lock
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
    fn service_resumes_after_recovery_and_owner_unlock_in_either_order() {
        assert_eq!(desktop_state(true, Some(false)), Desktop::Recovering);
        assert_eq!(desktop_state(true, Some(true)), Desktop::Recovering);
        assert_eq!(desktop_state(false, Some(true)), Desktop::Locked);
        assert_eq!(desktop_state(false, None), Desktop::Unknown);
        assert_eq!(desktop_state(false, Some(false)), Desktop::Ready);
    }
    #[test]
    fn automatic_restore_requires_confirmed_lock_and_respects_a_later_local_unlock() {
        assert_eq!(recovery_step(None, false, false), RecoveryStep::Lock);
        assert_eq!(recovery_step(Some(false), false, false), RecoveryStep::Lock);
        assert_eq!(
            recovery_step(Some(true), false, false),
            RecoveryStep::Restore
        );
        assert_eq!(recovery_step(Some(false), true, false), RecoveryStep::Wait);
        assert_eq!(recovery_step(None, true, true), RecoveryStep::Wait);
        assert_eq!(
            recovery_step(Some(false), true, true),
            RecoveryStep::Restore
        );
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
        assert_eq!(lease.tick(now + (STALE + GRACE) * 2, true), State::Locking);
        assert_eq!(lease.tick(now + (STALE + GRACE) * 2, true), State::Locking);
    }
}
